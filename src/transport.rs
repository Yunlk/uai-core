//! HTTP 传输层：客户端构造、鉴权头、限流退避、错误呈现。
//!
//! ## 为什么单独一层
//!
//! 旧工程里「发一个带鉴权头的 GET」这段代码在 `content.rs`、`submit.rs`、
//! `portal.rs` 里各写了一遍，限流退避写了两遍。分叉的后果很实在：
//! 有一次一个 example 用了门户 JWT 打进度接口，拿到 401 的 **HTML 重定向页**，
//! 解析出来「全部未完成」，整个实验结论作废。
//!
//! 这里所有请求都必须经过 [`Transport`]，鉴权头与错误处理不可能再分叉。
//!
//! ## 两个实测坑
//!
//! 1. **401 返回的是 HTML 重定向页**，`.json()` 失败后只报
//!    `error decoding response body`，把真因掩盖掉。所以这里在 JSON 解析失败时
//!    会**附带响应正文的一小段**，让 401/登录页一眼可见。
//! 2. **限流码是 `600001`/`600002`**，退避基数 20 秒 × 第几次，最多 4 次。

use reqwest::blocking::{Client, Response};
use serde_json::Value;
use std::time::Duration;

use crate::endpoints::{
    AUTH_HEADER, HOST_APP_ID, HOST_APP_ID_HEADER, HOST_OPENID_HEADER, HOST_PLATFORM_HEADER,
    HOST_PLATFORM_PC, HOST_SCHOOL_HEADER,
};
use crate::error::{Error, Result};

/// 常规请求超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 可取消路径的超时。
///
/// 阻塞式 reqwest **无法中途 abort**，缩短超时是唯一手段：
/// 它把「点停止后还要干等 30 秒」压到 8 秒以内。
pub const CANCEL_TIMEOUT: Duration = Duration::from_secs(8);

/// 触发限流时的最大重试次数。
pub const MAX_RATE_LIMIT_RETRIES: u32 = 4;

/// 限流退避基数（乘以第几次尝试）。
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(20);

/// 缺省 User-Agent。
pub const USER_AGENT: &str = "uai-core/0.1";

/// 构造普通 HTTP 客户端（无 cookie 罐）。
pub fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| Error::network(format!("创建网络客户端失败：{error}")))
}

/// 构造带 cookie 罐的客户端，供 SSO 的 `add/cookie` 链路复用。
pub fn build_cookie_client() -> Result<Client> {
    Client::builder()
        .cookie_store(true)
        .timeout(DEFAULT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| Error::network(format!("创建网络客户端失败：{error}")))
}

/// 鉴权方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth<'a> {
    /// 不带鉴权头。
    None,
    /// 门户 SSO JWT，走 `Authorization`。
    Portal(&'a str),
    /// ucontent 自铸 token，走 `x-annotator-auth-token`。
    Annotator(&'a str),
    /// 门户 JWT，但*伪装*成 annotator 头。
    ///
    /// 目录/内容接口不校验鉴权，所以这个组合能用——旧工程靠它在登录响应
    /// 没带 `ssoId` 时仍能读目录。**不要**拿它去打进度/答案（会 401）。
    Lenient(&'a str),
}

/// 请求失败时的可读摘要。
///
/// 把响应正文前若干字符附上：401 时正文是 HTML 登录页，
/// `error decoding response body` 这句话本身毫无信息量。
fn readable_error(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return format!("HTTP {status}（响应为空）");
    }
    let head: String = trimmed.chars().take(200).collect();
    let looks_html = trimmed.starts_with('<') || trimmed.contains("<html");
    if looks_html {
        format!("HTTP {status}：返回了 HTML 页面而非 JSON，多半是鉴权失败被重定向到登录页")
    } else if trimmed.chars().count() > 200 {
        format!("HTTP {status}：{head}…")
    } else {
        format!("HTTP {status}：{trimmed}")
    }
}

/// 门户「微应用宿主」身份。
///
/// ## 为什么需要它
///
/// `/api/aca/*`（考核方案、综合成绩）**只带 `Authorization` 会返回
/// `{"code":6011,"msg":"feature denied"}`**——服务端认不出请求来自哪个应用。
///
/// 门户是 qiankun 微应用架构，宿主 `cloud-pc` 在每个请求上并进这四个头
/// （宿主模块 `49405` 的 `getExtraHeaders()`）：
///
/// ```text
/// u-app-id: 116        ← 宿主常量 $m
/// u-platform: 2        ← 枚举 OD { MOBILE=1, PC=2, PAD=3 }
/// u-school: 1000       ← 用户所在学校
/// u-openid: <ssoId>
/// Authorization: <门户 JWT>
/// ```
///
/// 实测：删掉 `u-app-id` 立刻退回 `6011`；带上就正常返回。所以这不是「可选优化」，
/// 是**打通考核接口的必要条件**。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostIdentity {
    /// `u-app-id`。默认 [`HOST_APP_ID`]。
    pub app_id: String,
    /// `u-platform`。默认 [`HOST_PLATFORM_PC`]。
    pub platform: String,
    /// `u-school`。来自用户信息的 `school`。
    pub school: String,
    /// `u-openid`。来自用户信息的 `ssoId`。
    pub open_id: String,
}

impl HostIdentity {
    /// 用学校号与 `open_id` 构造，`app_id`/`platform` 取实测默认值。
    pub fn new(school: impl Into<String>, open_id: impl Into<String>) -> Self {
        Self {
            app_id: HOST_APP_ID.to_owned(),
            platform: HOST_PLATFORM_PC.to_owned(),
            school: school.into(),
            open_id: open_id.into(),
        }
    }

    /// 覆盖 `u-app-id`。
    pub fn with_app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    /// 覆盖 `u-platform`。
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = platform.into();
        self
    }

    /// 四个头是否都齐了。缺任何一个都会被服务端当成 `6011`。
    pub fn is_complete(&self) -> bool {
        !self.app_id.trim().is_empty()
            && !self.platform.trim().is_empty()
            && !self.school.trim().is_empty()
            && !self.open_id.trim().is_empty()
    }
}

/// 一次 HTTP 请求的封装。
pub struct Transport<'a> {
    client: &'a Client,
    auth: Auth<'a>,
    host: Option<HostIdentity>,
}

impl<'a> Transport<'a> {
    pub fn new(client: &'a Client, auth: Auth<'a>) -> Self {
        Self {
            client,
            auth,
            host: None,
        }
    }

    /// 用门户 JWT 构造。
    pub fn portal(client: &'a Client, token: &'a str) -> Self {
        Self::new(client, Auth::Portal(token))
    }

    /// 用 ucontent 自铸 token 构造。
    pub fn annotator(client: &'a Client, token: &'a str) -> Self {
        Self::new(client, Auth::Annotator(token))
    }

    /// 用门户 JWT 构造（伪装成 annotator 头）。
    ///
    /// 只在登录响应缺 `ssoId` 时用。**不要**拿它打进度/答案。
    pub fn lenient(client: &'a Client, token: &'a str) -> Self {
        Self::new(client, Auth::Lenient(token))
    }

    /// 带上门户微应用宿主身份。**打 `/api/aca/*` 必须加**，否则 `6011`。
    pub fn with_host(mut self, host: HostIdentity) -> Self {
        self.host = Some(host);
        self
    }

    /// 给请求加上鉴权头。
    fn decorate(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        let request = match self.auth {
            Auth::None => request,
            Auth::Portal(token) => request.header("Authorization", token),
            Auth::Annotator(token) => request.header(AUTH_HEADER, token),
            Auth::Lenient(token) => request.header(AUTH_HEADER, token),
        };
        match &self.host {
            Some(host) => request
                .header(HOST_APP_ID_HEADER, &host.app_id)
                .header(HOST_PLATFORM_HEADER, &host.platform)
                .header(HOST_SCHOOL_HEADER, &host.school)
                .header(HOST_OPENID_HEADER, &host.open_id),
            None => request,
        }
    }

    /// 发 GET 并解析 JSON。
    pub fn get_json(&self, url: &str) -> Result<Value> {
        self.get_json_with_timeout(url, DEFAULT_TIMEOUT)
    }

    /// 发 GET 并解析 JSON，指定超时。
    pub fn get_json_with_timeout(&self, url: &str, timeout: Duration) -> Result<Value> {
        let response = self
            .decorate(self.client.get(url))
            .header("Content-Type", "application/json; charset=utf-8")
            .timeout(timeout)
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    Error::network(format!("请求超时（{}s）：{url}", timeout.as_secs()))
                } else {
                    Error::network(format!("请求失败：{error}"))
                }
            })?;
        read_json(response)
    }

    /// 发 GET 并返回**原始文本**（内容/答案接口要拿密文，不能先过 JSON）。
    pub fn get_text(&self, url: &str) -> Result<String> {
        let response = self
            .decorate(self.client.get(url))
            .header("Content-Type", "application/json; charset=utf-8")
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .map_err(|error| Error::network(format!("请求失败：{error}")))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .map_err(|error| Error::network(format!("读取响应失败：{error}")))?;
        if !(200..300).contains(&status) {
            return Err(Error::api(readable_error(status, &bytes)));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|error| Error::parse(format!("响应不是合法 UTF-8：{error}")))
    }

    /// 发 POST JSON 并解析 JSON。
    pub fn post_json(&self, url: &str, body: &Value) -> Result<Value> {
        let response = self
            .decorate(self.client.post(url))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .map_err(|error| Error::network(format!("请求失败：{error}")))?;
        read_json(response)
    }
}

/// 把响应读成 JSON，失败时给出**带正文摘要**的错误。
pub fn read_json(response: Response) -> Result<Value> {
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .map_err(|error| Error::network(format!("读取响应失败：{error}")))?;
    if !(200..300).contains(&status) {
        return Err(Error::api(readable_error(status, &bytes)));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| Error::parse(format!("{}（{}）", readable_error(status, &bytes), error)))
}

/// 判断 ucontent 响应是否成功（`code == 0`）。
pub fn ucontent_ok(body: &Value) -> bool {
    body.get("code").and_then(Value::as_i64) == Some(0)
}

/// 判断门户响应是否成功（**`code == 1`**，与 ucontent 相反）。
pub fn portal_ok(body: &Value) -> bool {
    matches!(
        body.get("code"),
        Some(Value::Number(n)) if n.as_i64() == Some(1)
    )
}

/// 取响应里的可读消息。
pub fn body_message(body: &Value, fallback: &str) -> String {
    for key in ["message", "msg", "error", "errMsg"] {
        if let Some(text) = body.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.to_owned();
        }
    }
    fallback.to_owned()
}

/// 该业务码是否表示限流。
pub fn is_rate_limited(code: i64) -> bool {
    matches!(code, 600001 | 600002)
}

/// 带限流退避的重试。`attempt` 从 0 开始，退避为 `基数 × (attempt + 1)`。
///
/// 传入的闭包返回 `Ok(Some(code))` 表示拿到业务码，`Ok(None)` 表示无需判断。
pub fn with_rate_limit_retry<T, F>(mut operation: F) -> Result<T>
where
    F: FnMut(u32) -> Result<(T, Option<i64>)>,
{
    let mut attempt = 0;
    loop {
        let (value, code) = operation(attempt)?;
        match code {
            Some(code) if is_rate_limited(code) && attempt < MAX_RATE_LIMIT_RETRIES => {
                let wait = RATE_LIMIT_BACKOFF * (attempt + 1);
                std::thread::sleep(wait);
                attempt += 1;
            }
            _ => return Ok(value),
        }
    }
}

/// 校验 URL 属于预期的域名，防止把 A 站的令牌发到 B 站。
///
/// 旧工程出过「用门户 JWT 打 ucontent 进度接口」的事故，根因就是拼 URL 时
/// 拼错了域名而没人发现。这里提供一个廉价的断言。
pub fn assert_host(url: &str, expected_base: &str) -> Result<()> {
    if url.starts_with(expected_base) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "URL 域名不符合预期：期望以 {expected_base} 开头，实际是 {url}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 门户成功码是 `1`，ucontent 是 `0`，两者不能混用。
    #[test]
    fn success_codes_differ_between_apps() {
        assert!(portal_ok(&json!({"code": 1})));
        assert!(!portal_ok(&json!({"code": 0})));
        assert!(ucontent_ok(&json!({"code": 0})));
        assert!(!ucontent_ok(&json!({"code": 1})));
    }

    /// 限流码必须是这两个，其他码不能误判成限流。
    #[test]
    fn only_two_codes_are_rate_limit() {
        assert!(is_rate_limited(600001));
        assert!(is_rate_limited(600002));
        assert!(!is_rate_limited(0));
        assert!(!is_rate_limited(300100));
    }

    /// HTML 响应要给出「多半是鉴权失败」的提示，而不是干巴巴的 HTTP 码。
    #[test]
    fn html_response_mentions_auth() {
        let message = readable_error(200, b"<html><body>login</body></html>");
        assert!(message.contains("HTML"), "{message}");
        assert!(message.contains("鉴权"), "{message}");
    }

    /// 空响应不能 panic。
    #[test]
    fn empty_response_is_reported() {
        let message = readable_error(502, b"");
        assert!(message.contains("502"), "{message}");
    }

    /// 消息提取按 message/msg/error 顺序，空串要跳过。
    #[test]
    fn body_message_prefers_non_empty() {
        assert_eq!(body_message(&json!({"msg": "boom"}), "x"), "boom");
        assert_eq!(
            body_message(&json!({"message": "  ", "msg": "b"}), "x"),
            "b"
        );
        assert_eq!(body_message(&json!({}), "兜底"), "兜底");
    }

    /// 域名断言：跨域必须被拦住。
    #[test]
    fn assert_host_rejects_cross_domain() {
        assert!(assert_host("https://uai.unipus.cn/api/x", "https://uai.unipus.cn").is_ok());
        let error =
            assert_host("https://ucontent.unipus.cn/x", "https://uai.unipus.cn").unwrap_err();
        assert!(error.message().contains("域名不符合预期"), "{error}");
    }

    /// `Lenient` 与 `Annotator` 都发同一个头名——这正是它能「蒙对」的原因。
    #[test]
    fn lenient_and_annotator_share_header_name() {
        assert_eq!(AUTH_HEADER, "x-annotator-auth-token");
    }
    /// ★ 回归测试：不存在「铸了 token 却不带上」的构造器。
    ///
    /// 曾经有 `Transport::from_open_id`：它内部 `annotator::mint` 铸好 token，
    /// 却因为借用期限制只能返回 `(Transport, String)`，而那个 Transport 的
    /// auth 是 **`Auth::None`** —— 于是调用方以为自己在用铸好的 token，
    /// 实际每个请求都是**未鉴权**的。进度/答案接口会 401，
    /// 而目录/内容接口不校验鉴权，所以这个 bug 会藏得很深。
    ///
    /// 现在只保留显式传 token 的构造器：`annotator()` / `portal()` / `lenient()`。
    /// 这是编译期保证（没有那个函数了），此处用行为断言把它记录下来。
    #[test]
    fn no_constructor_silently_drops_a_minted_token() {
        let client = build_client().unwrap();
        let transport = Transport::annotator(&client, "my-token");
        assert_eq!(
            transport.auth,
            Auth::Annotator("my-token"),
            "显式传入的 token 必须真的挂在 Auth 上"
        );
        // Auth::None 只能由调用方自己显式要求，不能由某个构造器代为决定。
        assert_ne!(transport.auth, Auth::None, "不该悄悄退化成不鉴权");
    }

    /// 三种鉴权头各自打到正确的 header 上。
    #[test]
    fn auth_variants_use_expected_headers() {
        let client = build_client().unwrap();
        for (auth, header) in [
            (Auth::Portal("p"), "Authorization"),
            (Auth::Annotator("a"), AUTH_HEADER),
            (Auth::Lenient("l"), AUTH_HEADER),
        ] {
            let transport = Transport::new(&client, auth);
            let request = transport
                .decorate(client.get("https://example.com"))
                .build()
                .unwrap();
            assert!(
                request.headers().contains_key(header),
                "{auth:?} 应当写 {header} 头"
            );
        }
    }
}
