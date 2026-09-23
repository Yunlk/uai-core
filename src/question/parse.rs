//! 从解密后的内容/答案文本里解析出题目元信息与标准答案。
//!
//! ## 两个精度陷阱
//!
//! ### 1. `instanceId` 绝不能经 `f64`
//!
//! 19 位十进制数超过 `f64` 的 2^53，前端 JS `JSON.parse` 会把它舍入
//! （实测 `1431562773572042765` 变成 `1431562773572042800`）。
//!
//! Rust 的 `serde_json` 对整数保留到 `u64` 精度，所以这里解析后**立刻转成
//! 字符串**（见 [`number_text`]），绝不让它流经浮点。
//!
//! ### 2. 两种 `courseId` 返回**不同 schema**
//!
//! | ID 形式 | 解密后 | 元素形状 |
//! | --- | --- | --- |
//! | `course-v1:` | 对象 | key 形如 `questions:questions` |
//! | `course-v2:` | **数组** | `{"id":…, "content":"<JSON 字符串>"}` |
//!
//! **提交必须用 `course-v2:`** —— `instanceId` 只能从这条分支取。

use serde_json::Value;

use crate::crypto;
use crate::error::{Error, Result};

use super::QuestionMeta;

/// 代表音频的内容项类型（前端 `cME.audio`）。
pub const CONTENT_AUDIO: i64 = 1;

/// 代表视频的内容项类型（前端 `cME.video`）。
pub const CONTENT_VIDEO: i64 = 2;

/// 内容接口的密文字段名。
pub const CONTENT_FIELD: &str = "content";

/// 答案接口的密文字段名。
///
/// ## ★ 两个接口的字段名**不一样**，别统一
///
/// 实测（2026-09）：
///
/// | 接口 | 响应 | 密文字段 |
/// | --- | --- | --- |
/// | `/course/api/v3/content/…` | `{"code":0,"k":"…","content":"unipus.…"}` | **`content`** |
/// | `/course/api/v3/answer/…` | `{"code":0,"k":"…","data":"unipus.…"}` | **`data`** |
///
/// 都是 `{code, message, publish_version, k, …}` 的外壳，只有密文那一个键
/// 换了名字。答案是 `data`，**不是 `content`**。
///
/// 我一度把两者合用一个只看 `content` 的解析函数，结果所有客观题的标准答案
/// 都报「响应缺少 content 字段」——而客观题正是唯一能拿满分的一类。
pub const ANSWER_FIELD: &str = "data";

/// 解析外壳，按给定字段名取密文。
///
/// 返回 `(明文, k)`。`k` 是解密密钥后缀（形如 `20260923`），
/// 未加密时为空串。
pub fn parse_envelope_at(body: &Value, field: &str) -> Result<(String, String)> {
    if let Some(code) = body.get("code").and_then(Value::as_i64)
        && code != 0
    {
        let message = crate::transport::body_message(body, "接口返回失败");
        return Err(Error::api(format!("接口返回 code={code}：{message}")));
    }

    // 大小写/拼写有出入时，退而在已知字段名里找一个字符串密文。
    let blob = body
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| {
            body.as_object()?
                .values()
                .find_map(|value| value.as_str().filter(|text| text.starts_with("unipus.")))
        })
        .ok_or_else(|| {
            Error::parse(format!(
                "响应里找不到密文字段 `{field}`（{field} 之外也没有 unipus. 开头的字符串）"
            ))
        })?;
    let k = body.get("k").and_then(Value::as_str).unwrap_or_default();

    // 不带 `unipus.` 前缀时按明文返回。
    if !blob.starts_with("unipus.") {
        return Ok((blob.to_owned(), k.to_owned()));
    }

    let plain = crypto::decrypt_blob(blob, k)?;
    Ok((plain, k.to_owned()))
}

/// 解析**内容**接口的密文外壳（字段 `content`）。
pub fn parse_encrypted_envelope(body: &Value) -> Result<(String, String)> {
    parse_envelope_at(body, CONTENT_FIELD)
}

/// 解析**答案**接口的密文外壳（字段 `data`）。
pub fn parse_answer_envelope(body: &Value) -> Result<(String, String)> {
    parse_envelope_at(body, ANSWER_FIELD)
}

/// 取字符串字段，数字也转成文本。
fn text_of(value: &Value, keys: &[&str]) -> Option<String> {
    let found = keys.iter().find_map(|key| value.get(*key))?;
    match found {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// 把可能是数字/字符串的 `id` 取成**字符串**，避免 `f64` 舍入。
///
/// `serde_json` 对整数保留到 `u64`，所以数字分支是安全的；
/// 字符串分支原样返回。其他类型返回空串。
pub fn number_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// 从解密后的内容文本里解析出每道题的元信息。
pub fn parse_questions(decrypted: &str) -> Result<Vec<QuestionMeta>> {
    let parsed: Value = serde_json::from_str(decrypted)
        .map_err(|error| Error::parse(format!("解析内容失败：{error}")))?;
    let items: Vec<&Value> = match &parsed {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&parsed],
        _ => return Ok(Vec::new()),
    };

    Ok(items
        .iter()
        .map(|item| {
            // 题目的 content 可能是字符串化 JSON，也可能是对象。
            let inner = match item.get("content") {
                Some(Value::String(text)) => {
                    serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
                }
                Some(value) => value.clone(),
                None => Value::Null,
            };

            let child_count = inner
                .get("children")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();

            QuestionMeta {
                instance_id: number_text(item.get("id")),
                question_type: text_of(item, &["type"])
                    .or_else(|| text_of(&inner, &["type"]))
                    .unwrap_or_default(),
                reply_type: text_of(&inner, &["replyType"])
                    .or_else(|| text_of(item, &["replyType"]))
                    .unwrap_or_default(),
                category: inner
                    .get("category")
                    .or_else(|| item.get("category"))
                    .and_then(Value::as_i64),
                child_count,
                media_indices: media_indices_of(&inner),
            }
        })
        .collect())
}

/// 挑出 `contents` 里属于音频/视频的下标。
///
/// ## ★ 只有音频(1)与视频(2)算媒体
///
/// 实测 `contents[].type` 的真实取值与字段形状
/// （`examples/probe_media_types.rs`，三班《视听说2》）：
///
/// | type | 含义 | 字段样本 | 标记播放？ |
/// | --- | --- | --- | --- |
/// | 1 | 音频 | `duration`(193200) `path` `subtitles` `text` | ★ 要 |
/// | 2 | 视频 | `duration`(236600) `path` `vttPath` `subtitlesPath` | ★ 要 |
/// | 4 | 正文 | `children` `explanations` `text` | 不（无播放概念） |
/// | 5 | 词汇/术语条目 | 只有 `id`/`name`，`text` 空，**无 `path`/`duration`** | 不 |
/// | 6 | 单词卡 | `category` `examples` `sound` `name` | 不 |
///
/// **`type=5` 是实测新发现的**（前端已知常量只到 1/2/4/6）。它没有 `path`、
/// 没有 `duration`，是不可播放的条目。
///
/// ⚠️ 若将来发现某个 `type` 也带 `path` + `duration`，说明它是媒体，
/// 需要加进这里。判据建议直接看「有没有 `path`」而非硬编码数字，
/// 但当前实测 1/2 与 4/5/6 用 `path` 也能分开，故暂不改，以免引入未验证行为。
pub fn media_indices_of(inner: &Value) -> Vec<usize> {
    inner
        .get("contents")
        .and_then(Value::as_array)
        .map(|contents| {
            contents
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    matches!(
                        entry.get("type").and_then(Value::as_i64),
                        Some(CONTENT_AUDIO) | Some(CONTENT_VIDEO)
                    )
                })
                .map(|(index, _)| index)
                .collect()
        })
        .unwrap_or_default()
}

/// 从解密后的答案文本里取出每道题、每个子题的标准答案。
///
/// 外层下标与 [`parse_questions`] 的顺序**一一对应**。
pub fn parse_standard_answers(decrypted: &str) -> Result<Vec<Vec<Vec<String>>>> {
    let parsed: Value = serde_json::from_str(decrypted)
        .map_err(|error| Error::parse(format!("解析答案失败：{error}")))?;
    let items: Vec<&Value> = match &parsed {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&parsed],
        _ => return Ok(Vec::new()),
    };

    Ok(items
        .iter()
        .map(|item| {
            let raw = item
                .get("answer")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if raw.trim().is_empty() {
                return Vec::new();
            }
            let Ok(answer) = serde_json::from_str::<Value>(raw) else {
                return Vec::new();
            };
            answer
                .get("children")
                .and_then(Value::as_array)
                .map(|children| {
                    children
                        .iter()
                        .map(|child| {
                            child
                                .get("answers")
                                .and_then(Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .map(|item| match item {
                                            Value::String(text) => text.clone(),
                                            other => other.to_string(),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 造一份最小内容。用 `serde_json` 拼装，**不手写转义字符串**——
    /// 题目 `content` 本身是字符串化 JSON，手写时一个换行就会造出非法 JSON，
    /// 测试会以「解析内容失败」这种看着像产品 bug 的方式挂掉。
    fn content(
        question_type: &str,
        reply_type: &str,
        category: i64,
        contents: Value,
        children: usize,
    ) -> String {
        let inner = json!({
            "type": question_type,
            "replyType": reply_type,
            "category": category,
            "contents": contents,
            "children": (0..children).map(|_| json!({})).collect::<Vec<_>>(),
        });
        json!([{ "id": 1431562773572042765_i64, "content": inner.to_string() }]).to_string()
    }

    /// ★ `instanceId` 必须精确保留 19 位，不能被舍入。
    #[test]
    fn instance_id_keeps_full_precision() {
        let raw = content("basic", "singlechoice", 1, json!([]), 1);
        let metas = parse_questions(&raw).unwrap();
        assert_eq!(metas[0].instance_id, "1431562773572042765");
    }

    /// 字符串形式的 `id` 也要能取。
    #[test]
    fn instance_id_accepts_string_form() {
        let raw = r#"[{"id":"999","content":"{\"children\":[]}"}]"#;
        let metas = parse_questions(raw).unwrap();
        assert_eq!(metas[0].instance_id, "999");
    }

    /// ★ 只有音频(1)/视频(2) 进 `media_indices`。
    #[test]
    fn media_indices_only_audio_and_video() {
        let raw = content(
            "basic",
            "singlechoice",
            1,
            json!([
                {"type": 4, "text": "<p>hi</p>"},
                {"type": 1, "duration": 193200, "path": "a.mp3"},
                {"type": 2, "duration": 236600, "path": "b.mp4"},
                {"type": 5, "id": "x", "name": ""},
                {"type": 6, "name": "<p>word</p>", "sound": "w.mp3"},
            ]),
            1,
        );
        let metas = parse_questions(&raw).unwrap();
        assert_eq!(
            metas[0].media_indices,
            vec![1, 2],
            "文本(4)/条目(5)/单词卡(6) 都不是可播放进度项"
        );
    }

    /// 没有 `contents` 时不能 panic。
    #[test]
    fn media_indices_empty_without_contents() {
        let raw = r#"[{"id":1,"content":"{\"children\":[{}]}"}]"#;
        let metas = parse_questions(raw).unwrap();
        assert!(metas[0].media_indices.is_empty());
    }

    /// 属性从内层与外层都能取到（两种 schema 的差异）。
    #[test]
    fn reads_type_from_inner_or_outer() {
        // inner 里有 type 与 replyType
        let inner = content("basic-scoop-content", "fillblank", 1, json!([]), 5);
        let metas = parse_questions(&inner).unwrap();
        assert_eq!(metas[0].question_type, "basic-scoop-content");
        assert_eq!(metas[0].reply_type, "fillblank");
        assert_eq!(metas[0].child_count, 5);
        assert_eq!(metas[0].category, Some(1));
    }

    /// 对象形式的输入（`course-v1:` 分支）也要能解析。
    #[test]
    fn object_input_is_wrapped_as_single_item() {
        let raw = json!({
            "id": 42,
            "type": "basic",
            "replyType": "singlechoice",
            "category": 1,
            "children": [{}],
        })
        .to_string();
        let metas = parse_questions(&raw).unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].instance_id, "42");
        assert_eq!(metas[0].reply_type, "singlechoice");
    }

    /// 空 children 得到 0 子题。
    #[test]
    fn zero_children_is_reported_as_zero() {
        let raw = content("basic", "singlechoice", 1, json!([]), 0);
        let metas = parse_questions(&raw).unwrap();
        assert_eq!(metas[0].child_count, 0);
    }

    /// 非法 JSON 要报可读错误，不能 panic。
    #[test]
    fn invalid_json_reports_readable_error() {
        let error = parse_questions("{ 不是 JSON").unwrap_err();
        assert!(error.message().contains("解析内容失败"), "{error}");
    }

    /// ★ 标准答案逐子题展开。
    #[test]
    fn standard_answers_are_per_child() {
        let raw = json!([{
            "id": 1,
            "answer": json!({
                "children": [
                    { "answers": ["A"] },
                    { "answers": ["B", "D"] },
                ]
            }).to_string(),
        }])
        .to_string();
        let answers = parse_standard_answers(&raw).unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(
            answers[0],
            vec![vec!["A".to_owned()], vec!["B".to_owned(), "D".to_owned()]]
        );
    }

    /// 空 `answer` 给空 vec，不是报错。
    #[test]
    fn empty_answer_yields_empty_vec() {
        let raw = json!([{ "id": 1, "answer": "" }]).to_string();
        let answers = parse_standard_answers(&raw).unwrap();
        assert_eq!(answers, vec![Vec::<Vec<String>>::new()]);
    }

    /// 主观题的 answer 为空 —— 这正是它需要占位文本的原因。
    #[test]
    fn subjective_has_no_standard_answer() {
        let raw =
            json!([{ "id": 1, "answer": json!({"children": [{"answers": []}]}).to_string() }])
                .to_string();
        let answers = parse_standard_answers(&raw).unwrap();
        assert_eq!(answers[0], vec![Vec::<String>::new()], "主观题无标准答案");
    }

    /// `parse_encrypted_envelope` 对非 0 业务码要给出可读错误。
    #[test]
    fn envelope_rejects_non_zero_code() {
        let body = json!({ "code": 6, "message": "没有权限" });
        let error = parse_encrypted_envelope(&body).unwrap_err();
        assert!(error.message().contains("code=6"), "{error}");
        assert!(error.message().contains("没有权限"), "{error}");
    }

    /// 明文内容直接返回，`k` 允许缺失。
    #[test]
    fn envelope_passes_plain_text_through() {
        let body = json!({ "code": 0, "content": "{\"a\":1}" });
        let (plain, k) = parse_encrypted_envelope(&body).unwrap();
        assert_eq!(plain, "{\"a\":1}");
        assert_eq!(k, "");
    }

    /// 缺少 `content` 字段要报错。
    #[test]
    fn envelope_requires_content() {
        let error = parse_encrypted_envelope(&json!({ "code": 0 })).unwrap_err();
        assert!(error.message().contains("content"), "{error}");
    }
    /// ★★ 回归测试：答案接口的密文字段是 `data`，内容接口是 `content`。
    ///
    /// 这是真实事故：两者合用一个只看 `content` 的解析函数后，
    /// 所有客观题都报「响应缺少 content 字段」——而客观题正是唯一
    /// 能靠标准答案拿满分的一类。
    #[test]
    fn answer_envelope_reads_data_field_not_content() {
        // 答案外壳：密文在 `data`（用非 unipus. 前缀以免真去解密）。
        let answer_shell = serde_json::json!({
            "code": 0, "message": "success", "k": "20260923",
            "publish_version": 78982, "data": "plain-answer-json",
        });
        let (plain, k) = parse_answer_envelope(&answer_shell).unwrap();
        assert_eq!(plain, "plain-answer-json");
        assert_eq!(k, "20260923");

        // 同一个外壳交给内容解析器，必须失败——反过来也一样。
        assert!(
            parse_encrypted_envelope(&answer_shell).is_err(),
            "答案外壳不该被内容解析器接受"
        );

        let content_shell = serde_json::json!({
            "code": 0, "message": "success", "k": "20260923",
            "content": "plain-content-json",
        });
        let (plain, _) = parse_encrypted_envelope(&content_shell).unwrap();
        assert_eq!(plain, "plain-content-json");
        assert!(
            parse_answer_envelope(&content_shell).is_err(),
            "内容外壳不该被答案解析器接受"
        );
    }

    /// 字段名写错时，退路是找 `unipus.` 开头的字符串（容错但不静默）。
    #[test]
    fn envelope_falls_back_to_unipus_prefixed_field() {
        let odd = serde_json::json!({"code": 0, "k": "x", "somethingElse": "unipus.zz"});
        let error = parse_answer_envelope(&odd).unwrap_err();
        assert!(
            !error.message().contains("找不到密文字段"),
            "有 unipus. 字段时不该报「找不到字段」：{error}"
        );
    }

    /// 完全没有密文时给出可读错误，并点名缺的是哪个字段。
    #[test]
    fn missing_cipher_reports_field_name() {
        let empty = serde_json::json!({"code": 0, "k": "x"});
        let error = parse_answer_envelope(&empty).unwrap_err();
        assert!(error.message().contains("data"), "{error}");
        let error = parse_encrypted_envelope(&empty).unwrap_err();
        assert!(error.message().contains("content"), "{error}");
    }

    /// 业务错误码要优先报出来，而不是报「缺字段」。
    #[test]
    fn error_code_takes_priority_over_missing_field() {
        let failed = serde_json::json!({"code": 300100, "message": "题目数量不匹配"});
        let error = parse_answer_envelope(&failed).unwrap_err();
        assert!(error.message().contains("300100"), "{error}");
        assert!(!error.message().contains("找不到密文字段"), "{error}");
    }
}
