//! 教材激活标记：`POST /api/product/course/courseActiveFlag`。
//!
//! **只读，不激活。** body `{resourceId}`。
//!
//! `resourceId` 是数字时发**数字**，否则发字符串——服务端对类型敏感，
//! 发错会得到空结果而不是错误。
//!
//! ## 为什么要查这个
//!
//! 用户要求「只刷已激活的教材」：未激活的教材刷了也不计入班级计分点，
//! 白跑还可能触发限流。

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::endpoints::{PORTAL_ACTIVE_FLAG, portal_url};
use crate::error::{Error, Result};
use crate::transport::{body_message, portal_ok};

/// 在嵌套结构里找布尔字段。
fn find_named_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    match value {
        Value::Object(map) => {
            for key in keys {
                match map.get(*key) {
                    Some(Value::Bool(flag)) => return Some(*flag),
                    Some(Value::Number(number)) => {
                        if let Some(code) = number.as_i64() {
                            // 与书架一致：0 表示已激活。
                            return Some(code == 0);
                        }
                    }
                    Some(Value::String(text)) => match text.trim() {
                        "0" => return Some(true),
                        "1" => return Some(false),
                        "true" => return Some(true),
                        "false" => return Some(false),
                        _ => {}
                    },
                    _ => {}
                }
            }
            map.values().find_map(|child| find_named_bool(child, keys))
        }
        Value::Array(items) => items.iter().find_map(|child| find_named_bool(child, keys)),
        _ => None,
    }
}

/// 查询单本教材是否已激活。
///
/// 返回 `Ok(None)` 表示服务端**没给明确答案**（既不 true 也不 false），
/// 与「明确未激活」不同——调用方要区分这两者。
pub fn fetch(client: &Client, portal_token: &str, resource_id: &str) -> Result<Option<bool>> {
    if resource_id.trim().is_empty() {
        return Err(Error::invalid("查询激活状态需要 resourceId"));
    }

    // 数字形式的 ID 要发数字，否则服务端返回空结果。
    let resource_value = resource_id
        .parse::<u64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(resource_id.to_owned()));

    let body = client
        .post(portal_url(PORTAL_ACTIVE_FLAG))
        .header("Authorization", portal_token)
        .json(&json!({ "resourceId": resource_value }))
        .send()
        .map_err(|error| Error::network(format!("查询教材激活状态失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析教材激活状态失败：{error}")))?;

    if !portal_ok(&body) {
        return Err(Error::api(body_message(&body, "教材激活状态接口返回失败")));
    }

    Ok(find_named_bool(
        &body,
        &[
            "active",
            "isActive",
            "activated",
            "isActivated",
            "courseActiveFlag",
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_bool_by_name() {
        assert_eq!(
            find_named_bool(&json!({"data": {"isActive": true}}), &["isActive"]),
            Some(true)
        );
    }

    /// 数字 0 表示已激活（与书架字段同一约定）。
    #[test]
    fn zero_number_means_active() {
        assert_eq!(
            find_named_bool(&json!({"courseActiveFlag": 0}), &["courseActiveFlag"]),
            Some(true)
        );
        assert_eq!(
            find_named_bool(&json!({"courseActiveFlag": 1}), &["courseActiveFlag"]),
            Some(false)
        );
    }

    /// 字符串形式也接受。
    #[test]
    fn string_forms_are_accepted() {
        assert_eq!(
            find_named_bool(&json!({"active": "0"}), &["active"]),
            Some(true)
        );
        assert_eq!(
            find_named_bool(&json!({"active": "false"}), &["active"]),
            Some(false)
        );
    }

    /// 没有明确答案时返回 None，不能猜成 false。
    #[test]
    fn ambiguous_response_yields_none() {
        assert_eq!(find_named_bool(&json!({"data": {}}), &["isActive"]), None);
        assert_eq!(
            find_named_bool(&json!({"active": "未知"}), &["active"]),
            None
        );
    }

    /// 空 resourceId 要本地报错，不发请求。
    #[test]
    fn empty_resource_id_is_rejected() {
        let client = crate::transport::build_client().unwrap();
        let error = fetch(&client, "token", "  ").unwrap_err();
        assert!(error.message().contains("resourceId"), "{error}");
    }
}
