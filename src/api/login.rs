//! SSO 登录：密码登录与二维码登录。
//!
//! ## 链路
//!
//! ```text
//! 密码：POST /sso/0.1/sso/cip/login  {service, username, password}
//!         → rs.serviceTicket
//! 二维码：POST /sso/7.0/sso/code      {deviceId, service}
//!         → rs.content（二维码内容）+ 轮询 UMCS socket
//!         → ticket
//! 两者：GET /sso/serviceTicket/validate?ticket=&service=
//!         → rs.jwt          ← 门户 JWT
//! ```
//!
//! ⚠️ **成功码是字符串 `"0"`**，与 ucontent 的 `0`、门户的 `1` 都不同。
//!
//! ## 两个加密要点
//!
//! 用户名与密码走 AES-128-CBC + PKCS7，输出**大写十六进制**，
//! 见 [`crate::crypto::encrypt_login_field`]。大小写写错会被当成密码错误。
//!
//! ## 密码不落盘
//!
//! 本模块不写日志、不存密码。调用方负责不要把密码传给日志。

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::crypto::encrypt_login_field;
use crate::endpoints::{
    PORTAL_SERVICE, SSO_ADD_COOKIE, SSO_BASE, SSO_PASSWORD_LOGIN, SSO_QR_CODE, SSO_TICKET_VALIDATE,
};
use crate::error::{Error, Result};

/// 登录成功后的凭据。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginTicket {
    /// 门户 JWT。
    pub jwt: String,
}

/// 响应里的业务码。**是字符串**，可能是 `"0"` 也可能是 `0`。
fn response_code(body: &Value) -> String {
    match body.get("code") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// SSO 是否成功（`code == "0"`）。
pub fn succeeded(body: &Value) -> bool {
    response_code(body) == "0"
}

/// 从响应里取一段可读的失败原因。
fn login_error_message(body: &Value, code: String) -> String {
    let message = crate::transport::body_message(body, "登录失败");
    if code.is_empty() {
        return message;
    }
    format!("{message}（code={code}）")
}

/// 密码登录，返回 serviceTicket。
///
/// ⚠️ 调用方必须用**带 cookie 罐**的客户端，后续 `validate` 依赖它。
pub fn password_login(client: &Client, username: &str, password: &str) -> Result<String> {
    if username.trim().is_empty() {
        return Err(Error::invalid("请输入用户名、手机号或邮箱"));
    }
    if password.is_empty() {
        return Err(Error::invalid("请输入密码"));
    }

    let body = client
        .post(format!("{SSO_BASE}{SSO_PASSWORD_LOGIN}"))
        .json(&json!({
            "service": PORTAL_SERVICE,
            "username": encrypt_login_field(username)?,
            "password": encrypt_login_field(password)?,
        }))
        .send()
        .map_err(|error| Error::network(format!("登录请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析登录响应失败：{error}")))?;

    let code = response_code(&body);
    if code != "0" {
        return Err(Error::api(login_error_message(&body, code)));
    }

    body.get("rs")
        .and_then(|value| value.get("serviceTicket"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::parse("登录响应缺少 serviceTicket"))
}

/// 申请二维码，返回 `(二维码内容, deviceId)`。
///
/// 调用方拿到 `content` 后自行渲染二维码，并用 `deviceId` 轮询 UMCS socket。
pub fn request_qr_code(client: &Client, device_id: &str) -> Result<String> {
    let body = client
        .post(format!("{SSO_BASE}{SSO_QR_CODE}"))
        .json(&json!({
            "deviceId": device_id,
            "service": PORTAL_SERVICE,
        }))
        .send()
        .map_err(|error| Error::network(format!("二维码请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析二维码响应失败：{error}")))?;

    let code = response_code(&body);
    if code != "0" {
        return Err(Error::api(login_error_message(&body, code)));
    }

    body.get("rs")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::parse("二维码响应缺少 content"))
}

/// 用 serviceTicket 换门户 JWT。
pub fn validate_ticket(client: &Client, ticket: &str) -> Result<LoginTicket> {
    let body = client
        .get(format!("{SSO_BASE}{SSO_TICKET_VALIDATE}"))
        .query(&[("ticket", ticket), ("service", PORTAL_SERVICE)])
        .send()
        .map_err(|error| Error::network(format!("校验 serviceTicket 失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析 serviceTicket 响应失败：{error}")))?;

    let code = response_code(&body);
    if code != "0" {
        return Err(Error::api(login_error_message(&body, code)));
    }

    let jwt = body
        .get("rs")
        .and_then(|value| value.get("jwt"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::parse("serviceTicket 响应缺少 jwt"))?;

    Ok(LoginTicket {
        jwt: jwt.to_owned(),
    })
}

/// 二维码登录成功后补充 cookie。
///
/// 部分接口依赖 cookie 而不只是 JWT，故在拿到 ticket 后调用一次。
pub fn add_cookie(client: &Client, ticket: &str) -> Result<()> {
    client
        .get(format!("{SSO_BASE}{SSO_ADD_COOKIE}"))
        .query(&[("ticket", ticket), ("service", PORTAL_SERVICE)])
        .send()
        .map_err(|error| Error::network(format!("补充登录 cookie 失败：{error}")))?;
    Ok(())
}

/// 端到端密码登录：换 JWT（不含门户用户信息校验）。
pub fn login_with_password(client: &Client, username: &str, password: &str) -> Result<LoginTicket> {
    let ticket = password_login(client, username, password)?;
    validate_ticket(client, &ticket)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ SSO 的成功码是**字符串 `"0"`**，不是数字 0，也不是门户的 1。
    #[test]
    fn success_code_is_string_zero() {
        assert!(succeeded(&serde_json::json!({"code": "0"})));
        assert!(succeeded(&serde_json::json!({"code": 0})), "数字形式也接受");
        assert!(!succeeded(&serde_json::json!({"code": "1"})));
        assert!(
            !succeeded(&serde_json::json!({"code": 1})),
            "1 是门户的成功码"
        );
    }

    /// 失败消息要带 code，便于排查。
    #[test]
    fn error_message_includes_code() {
        let body = serde_json::json!({"code": "1001", "message": "密码错误"});
        let message = login_error_message(&body, response_code(&body));
        assert!(message.contains("密码错误"), "{message}");
        assert!(message.contains("1001"), "{message}");
    }

    /// 缺少 code 字段时不能 panic。
    #[test]
    fn missing_code_is_not_success() {
        assert!(!succeeded(&serde_json::json!({})));
        assert!(!succeeded(&Value::Null));
    }

    /// 空用户名/密码要给出可读错误，且**不发请求**。
    #[test]
    fn empty_credentials_are_rejected_before_network() {
        let client = crate::transport::build_client().unwrap();
        let error = password_login(&client, "  ", "x").unwrap_err();
        assert!(error.message().contains("用户名"), "{error}");
        let error = password_login(&client, "user", "").unwrap_err();
        assert!(error.message().contains("密码"), "{error}");
    }
}
