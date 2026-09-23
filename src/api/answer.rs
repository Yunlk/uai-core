//! 标准答案：`GET /course/api/v3/answer/{courseId}/{groupId}/{mode}`。
//!
//! 加密方式与 [`crate::api::content`] 完全相同，解密后每题形如
//! `{ "answer": "<字符串化 JSON>", … }`，二次解析后取 `children[i].answers[]`。
//!
//! ## ⚠️ 密文字段名是 `data`，**不是 `content`**
//!
//! ```text
//! content 接口：{"code":0,"k":"…","content":"unipus.…"}
//! answer  接口：{"code":0,"k":"…","data"   :"unipus.…"}   ← 这里
//! ```
//!
//! 外壳一样，只有密文那一个键换了名字。见
//! [`crate::question::parse::ANSWER_FIELD`]。
//!
//! ## 只有客观题有标准答案
//!
//! 口语、主观题此处为**空**。这正是主观题必须补占位文本的原因
//! （见 [`crate::question::subjective`]）。
//!
//! ## 满分的唯一正当来源
//!
//! 客观题的满分条件是**逐字匹配**这里的答案。实测（`u1g405`、`u1g17`）
//! 用标准答案提交后 `score_pct = 1`、`state = 1`。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::answer_url;
use crate::error::Result;
use crate::question::parse::{parse_answer_envelope, parse_standard_answers};
use crate::transport::{Auth, Transport};

/// 读取并解密标准答案，返回明文 JSON 文本。
pub fn fetch_decrypted(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
) -> Result<String> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let raw = transport.get_text(&answer_url(course_id, group_id))?;
    let body: Value = serde_json::from_str(&raw)
        .map_err(|error| crate::error::Error::parse(format!("答案响应不是 JSON：{error}")))?;
    let (plain, _k) = parse_answer_envelope(&body)?;
    Ok(plain)
}

/// 读取标准答案，返回「每题 → 每子题 → 答案列表」。
///
/// 外层下标与 [`crate::api::content::fetch_questions`] 的顺序一一对应。
pub fn fetch_answers(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
) -> Result<Vec<Vec<Vec<String>>>> {
    let plain = fetch_decrypted(client, token, course_id, group_id)?;
    parse_standard_answers(&plain)
}
