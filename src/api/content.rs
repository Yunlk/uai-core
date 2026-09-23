//! 分组题目内容：`GET /course/api/v3/content/{courseId}/{groupId}/{mode}`。
//!
//! ## ★ 两种 `courseId` 返回**不同 schema**（这不是口味，是两种数据）
//!
//! | ID 形式 | 解密后 | 用途 |
//! | --- | --- | --- |
//! | `course-v1:` | 对象，key 形如 `questions:questions` | 界面预览 |
//! | `course-v2:` | **数组**，元素 `{"id":…,"content":"<JSON 字符串>"}` | **提交必须用这个** |
//!
//! `instanceId` 只能是 `course-v2:` 分支里的 `id`。
//!
//! ## 加密
//!
//! ```text
//! 响应: { "code":0, "k":"20260921", "content":"unipus.<hex>" }
//! 算法: AES-128-ECB + ZeroPadding
//! 密钥: "1a2b3c4d" + k
//! ```
//!
//! 见 [`crate::crypto::decrypt_blob`]。
//!
//! ## 不校验鉴权
//!
//! 这个接口**发错令牌也返回 `code:0`** —— 所以只测它发现不了鉴权错误。
//! 见 [`crate::api::progress`]。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::content_url;
use crate::error::Result;
use crate::question::QuestionMeta;
use crate::question::parse::{parse_encrypted_envelope, parse_questions};
use crate::transport::{Auth, Transport};

/// 读取并解密分组内容，返回**明文 JSON 文本**。
pub fn fetch_decrypted(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
) -> Result<String> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let raw = transport.get_text(&content_url(course_id, group_id))?;
    let body: Value = serde_json::from_str(&raw)
        .map_err(|error| crate::error::Error::parse(format!("内容响应不是 JSON：{error}")))?;
    let (plain, _k) = parse_encrypted_envelope(&body)?;
    Ok(plain)
}

/// 读取分组内容并解析成题目元信息。
pub fn fetch_questions(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
) -> Result<Vec<QuestionMeta>> {
    let plain = fetch_decrypted(client, token, course_id, group_id)?;
    parse_questions(&plain)
}

/// 读取分组内容，同时返回明文与解析结果（排查用）。
pub fn fetch_with_meta(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
) -> Result<(String, Vec<QuestionMeta>)> {
    let plain = fetch_decrypted(client, token, course_id, group_id)?;
    let metas = parse_questions(&plain)?;
    Ok((plain, metas))
}
