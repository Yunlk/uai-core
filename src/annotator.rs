//! ucontent 的 x-annotator-auth-token 生成。
//!
//! ## 这是什么（读之前先接受这个事实）
//!
//! ucontent 的进度、答案等受保护接口不认 SSO 下发的门户 JWT。它们要的是
//! x-annotator-auth-token，而这个 token 是前端在浏览器里自己签发的：
//! ucontent 的公开 JS bundle 里直接写着签发逻辑和密钥：
//!
//! // general-timing-sdk.js（v12）里的 generateJWTToken(openId)
//! _generateJwtToken(open_id) {
//!   const payload = {
//!     open_id, name: "", email: "", administrator: false,
//!     exp: Date.now() + 31536e6,          // 约 1 年
//!     iss: "c4f772063dcfa98e9c50",
//!     aud: "edx.unipus.cn",
//!   };
//!   return jwt.sign(payload, "a824b379f126b8b7aa5e33dee83fb0a05aa7462c"); // HS256
//! }
//!
//! 上面两个常量逐字抄自 general-timing-sdk.js 的 generateJWTToken，
//! 不是猜的。取证方式：门户首页 https://uai.unipus.cn/home 的 HTML 直接
//! 引用该 SDK（ucontent.cdn.unipus.cn/uai/sdk/general-timing/v12/…），
//! 无需登录即可下载。
//!
//! 也就是说这是客户端自铸的 HS256 token，密钥随 JS 公开下发，
//! 不是服务端按账号签发的凭据。这里照抄同样算法，只为复现前端行为。
//!
//! ## 这不是伪造身份
//!
//! - 下面的 iss/密钥是前端 bundle 里的公开常量，不是用户密码或服务端密钥；
//! - 真正标识账号的是 open_id，它只能由已登录会话取得（/api/account/user/info
//!   的 ssoId），本模块不提供编造 open_id 的途径；
//! - mint 对空 open_id 直接报错，杜绝「匿名铸一个万能令牌」。
//!
//! ## 实测结论
//!
//! - 只带这个 token、不带任何 cookie 即可访问进度接口；
//! - 进度接口的 courseId 必须是 instanceId（course-v2: 实例 ID），
//!   传 course-v1: 资源 ID 会返回 code:6 没有权限；
//! - 目录和内容接口不校验鉴权，两种 ID 都接受。

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// 前端 bundle 里硬编码的签发者标识（公开常量）。
///
/// 来源：general-timing-sdk.js v12 的 generateJWTToken。
const ISSUER: &str = "c4f772063dcfa98e9c50";

/// 前端 bundle 里硬编码的 HS256 密钥（公开常量，随 JS 下发）。
///
/// 来源同上。它与 [ISSUER] 是同一个 generateJWTToken 的两半，
/// 必须成对改：只换其中一个会签出服务端不认的 token。
const SIGNING_SECRET: &[u8] = b"a824b379f126b8b7aa5e33dee83fb0a05aa7462c";

/// 前端 bundle 里硬编码的受众（公开常量）。
const AUDIENCE: &str = "edx.unipus.cn";

/// token 有效期：前端写的是 Date.now() + 31536e6，约一年。
const LIFETIME_MS: u64 = 31_536_000_000;

/// 用 open_id 铸造 ucontent 的 x-annotator-auth-token。
///
/// open_id 必须来自已登录会话（/api/account/user/info 返回的 ssoId），
/// 不能凭空编造——本函数会拒绝空值。
pub fn mint(open_id: &str) -> Result<String> {
    if open_id.trim().is_empty() {
        return Err(Error::invalid("缺少 open_id，无法生成 ucontent 访问令牌"));
    }

    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::invalid(format!("系统时间异常：{error}")))?
        .as_millis() as u64
        + LIFETIME_MS;

    sign(open_id, exp)
}

/// 用给定的 exp 签发 token。
///
/// 单独拆出来是为了能用固定向量做回归测试：mint 取当前时间，
/// 每次调用结果都不同，没法逐字断言。
fn sign(open_id: &str, exp: u64) -> Result<String> {
    // 字段顺序与前端一致，便于和浏览器行为逐字对照。
    let header = base64_url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = base64_url(build_payload(open_id, exp).as_bytes());
    let signing_input = format!("{header}.{payload}");

    let mut mac = Hmac::<Sha256>::new_from_slice(SIGNING_SECRET)
        .map_err(|error| Error::invalid(format!("初始化签名失败：{error}")))?;
    mac.update(signing_input.as_bytes());
    let signature = base64_url(&mac.finalize().into_bytes());

    Ok(format!("{signing_input}.{signature}"))
}

/// 按前端字段顺序拼 payload。name/email 前端固定填空串。
fn build_payload(open_id: &str, exp: u64) -> String {
    format!(
        r#"{{"open_id":"{}","name":"","email":"","administrator":false,"exp":{},"iss":"{}","aud":"{}"}}"#,
        escape_json(open_id),
        exp,
        ISSUER,
        AUDIENCE
    )
}

/// 最小 JSON 字符串转义，防止 open_id 里出现引号破坏结构。
fn escape_json(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(ch),
        }
    }
    output
}

/// JWT 使用的 base64url（无填充）。
fn base64_url(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        output.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        output.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        // 无填充：不足的字节不输出。
        if chunk.len() > 1 {
            output.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(TABLE[(triple & 0x3F) as usize] as char);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空 open_id 必须被拒绝——不能铸出匿名令牌。
    #[test]
    fn empty_open_id_is_rejected() {
        assert!(mint("").is_err());
        assert!(mint("   ").is_err());
    }

    /// 正常 open_id 产出三段式 JWT。
    #[test]
    fn mints_three_part_jwt() {
        let token = mint("00000000000000000000000000000000").unwrap();
        assert_eq!(token.split('.').count(), 3, "{token}");
    }

    /// ★ payload 必须与前端 generateJWTToken 逐字一致。
    ///
    /// 这个断言同时钉死三件事：字段顺序、iss 常量、aud 常量。
    /// HS256 的签名对 payload 字节敏感，顺序一变 token 就变、服务端就可能不认。
    #[test]
    fn payload_matches_frontend_literal() {
        let payload = build_payload("open", 1_700_000_000_000);
        assert_eq!(
            payload,
            r#"{"open_id":"open","name":"","email":"","administrator":false,"exp":1700000000000,"iss":"c4f772063dcfa98e9c50","aud":"edx.unipus.cn"}"#
        );
    }

    /// ★★ 固定向量回归：签名密钥 + iss + 字段顺序必须与前端一致。
    ///
    /// 向量按 general-timing-sdk.js v12 的 generateJWTToken 算法算出：
    /// sign("open", 1700000000000)。改密钥、换 iss、调整字段顺序，
    /// 都会让这条断言失败——这正是它存在的意义。
    #[test]
    fn signature_matches_frontend_vector() {
        let token = sign("open", 1_700_000_000_000).unwrap();
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
             eyJvcGVuX2lkIjoib3BlbiIsIm5hbWUiOiIiLCJlbWFpbCI6IiIsImFkbWluaXN0cmF0b3IiOmZhbHNlLCJleHAiOjE3MDAwMDAwMDAwMDAsImlzcyI6ImM0Zjc3MjA2M2RjZmE5OGU5YzUwIiwiYXVkIjoiZWR4LnVuaXB1cy5jbiJ9.\
             SFrix4-QsiofDBjZBiumL9JULcxch53FDvhqSYWeSro"
        );
    }

    /// 同一 open_id 在毫秒内重复铸造应得到同一 token（exp 相同）。
    /// 不断言严格相等，只断言头部一致，避免跨毫秒抖动导致偶发失败。
    #[test]
    fn header_is_stable() {
        let token = mint("abc").unwrap();
        let header = token.split('.').next().unwrap();
        // {"alg":"HS256","typ":"JWT"}
        assert_eq!(header, "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
    }

    /// base64url 必须无填充、无 + /。
    #[test]
    fn base64_url_has_no_padding_or_plus() {
        let encoded = base64_url(b"any carnal pleasure");
        assert!(!encoded.contains('='), "{encoded}");
        assert!(
            !encoded.contains('+') && !encoded.contains('/'),
            "{encoded}"
        );
    }

    /// 引号必须被转义，否则 payload 结构会被破坏。
    #[test]
    fn payload_escapes_quotes() {
        let payload = build_payload(r#"a"b"#, 123);
        assert!(payload.contains(r#"a\"b"#), "{payload}");
        assert!(serde_json::from_str::<serde_json::Value>(&payload).is_ok());
    }
}
