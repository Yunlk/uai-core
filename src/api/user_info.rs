//! 当前用户信息：`GET /api/account/user/info`。
//!
//! ## ★ 唯一正确的 `open_id` 来源是 `ssoId`
//!
//! 响应里字段很多，**必须取 `ssoId`**：
//!
//! ```text
//! ✅ ssoId      → ucontent 的 open_id
//! ❌ appUserId  → 门户自己的用户 ID，拿它调进度接口 401
//! ```
//!
//! 这个错误曾经很隐蔽：目录/内容接口**不校验鉴权**，传错令牌也返回 `code:0`，
//! 所以只有打进度接口时才暴露。而 401 时服务端返回的是 **HTML 重定向页**，
//! `.json()` 失败后只报 `error decoding response body`，真因完全看不见。
//!
//! ⚠️ **门户的成功码是 `1`**（不是 ucontent 的 `0`）。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{PORTAL_USER_INFO, portal_url};
use crate::error::{Error, Result};
use crate::session::Session;
use crate::transport::{body_message, portal_ok};

/// 从任意深度的对象里找出第一个非空字符串字段。
fn find_first_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    return Some(text.to_owned());
                }
            }
            map.values()
                .find_map(|child| find_first_string(child, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_first_string(child, keys)),
        _ => None,
    }
}

/// 同 [`find_first_string`]，但**数字也接受**（转成十进制文本）。
///
/// `school` 在响应里是数字（如 `1000`），而 `appUserId` 是字符串——两者都要能取出来。
/// 布尔与 null 不算数，避免把 `"flag": true` 当成 ID。
fn find_first_scalar_text(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                match map.get(*key) {
                    Some(Value::String(text)) if !text.trim().is_empty() => {
                        return Some(text.clone());
                    }
                    Some(Value::Number(number)) => return Some(number.to_string()),
                    _ => {}
                }
            }
            map.values()
                .find_map(|child| find_first_scalar_text(child, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_first_scalar_text(child, keys)),
        _ => None,
    }
}

/// 读取当前用户信息，构造 [`Session`]。
///
/// 只填 `portal_token` / `open_id` / `display_name`；
/// 书架等数据由各自端点负责。
pub fn fetch(client: &Client, portal_token: &str) -> Result<Session> {
    let body = client
        .get(portal_url(PORTAL_USER_INFO))
        .header("Authorization", portal_token)
        .send()
        .map_err(|error| Error::network(format!("验证门户登录态失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析用户信息失败：{error}")))?;

    if !portal_ok(&body) {
        return Err(Error::api(body_message(&body, "门户用户信息校验失败")));
    }

    let display_name = find_first_string(
        &body,
        &["name", "nickName", "username", "userName", "phone"],
    )
    .unwrap_or_else(|| "已登录用户".to_owned());

    // ★ 只有 ssoId 能让 ucontent 的进度接口返回 200。
    let open_id = find_first_string(&body, &["ssoId", "sso_id"]).unwrap_or_default();

    // 学校号是 `/api/aca/*` 的 `u-school` 头，缺了会被服务端当 `6011 feature denied`。
    let school = find_first_scalar_text(&body, &["school"]).unwrap_or_default();

    // 只作标识用，**绝不能**当 open_id。
    let app_user_id =
        find_first_scalar_text(&body, &["appUserId", "app_user_id"]).unwrap_or_default();

    Ok(Session {
        portal_token: portal_token.to_owned(),
        open_id,
        display_name,
        school,
        app_user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ★ 必须取 `ssoId`，不是 `appUserId`。
    #[test]
    fn open_id_comes_from_sso_id() {
        let body = json!({
            "code": 1,
            "data": {
                "appUserId": 12345,
                "ssoId": "00000000000000000000000000000000",
                "name": "某同学",
            }
        });
        assert_eq!(
            find_first_string(&body, &["ssoId", "sso_id"]).unwrap(),
            "00000000000000000000000000000000"
        );
    }

    /// 嵌套任意深度都能找到。
    #[test]
    fn finds_field_at_any_depth() {
        let body = json!({"data": {"user": {"profile": {"ssoId": "deep"}}}});
        assert_eq!(find_first_string(&body, &["ssoId"]).unwrap(), "deep");
    }

    /// 数组里也能找到。
    #[test]
    fn finds_field_inside_array() {
        let body = json!({"data": [{"x": 1}, {"ssoId": "in-array"}]});
        assert_eq!(find_first_string(&body, &["ssoId"]).unwrap(), "in-array");
    }

    /// 空串要跳过，继续找下一个候选键。
    #[test]
    fn skips_empty_strings() {
        let body = json!({"name": "  ", "nickName": "有效"});
        assert_eq!(
            find_first_string(&body, &["name", "nickName"]).unwrap(),
            "有效"
        );
    }

    /// 找不到时返回 None，不 panic。
    #[test]
    fn returns_none_when_absent() {
        assert!(find_first_string(&json!({"a": 1}), &["ssoId"]).is_none());
    }

    /// ★ `school` 在响应里是**数字**，必须能取出来 —— 它是 `/api/aca/*` 的 `u-school`。
    #[test]
    fn school_is_read_from_a_number() {
        let body = json!({"code": 1, "value": {"userInfo": {"ssoId": "abc", "school": 1000}}});
        assert_eq!(find_first_scalar_text(&body, &["school"]).unwrap(), "1000");
    }

    /// `appUserId` 是字符串，且**不能**被当成 `open_id`。
    #[test]
    fn app_user_id_is_separate_from_open_id() {
        let body = json!({
            "value": {"userInfo": {
                "ssoId": "00000000000000000000000000000000",
                "appUserId": "1581876226543616901",
                "school": 1000,
            }}
        });
        assert_eq!(
            find_first_string(&body, &["ssoId"]).unwrap(),
            "00000000000000000000000000000000"
        );
        assert_eq!(
            find_first_scalar_text(&body, &["appUserId"]).unwrap(),
            "1581876226543616901"
        );
    }

    /// 布尔值不能被当成 ID 文本。
    #[test]
    fn booleans_are_not_scalar_text() {
        assert!(find_first_scalar_text(&json!({"school": true}), &["school"]).is_none());
        assert!(find_first_scalar_text(&json!({"school": null}), &["school"]).is_none());
    }
}
