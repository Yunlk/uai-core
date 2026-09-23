//! 多媒体上传题（`multiFileUpload`）：`payloads` 取 `fileList`。
//!
//! ## 当前是**空壳**
//!
//! 上传链路（`GET /media/user_resource/cms/token` 取七牛 token → 上传文件 →
//! 把返回的 URL 填进 `payloads`）**尚未接入**。所以这里 `payloads` 恒为空数组。
//!
//! 后果要说清楚：**这类题目前只能拿到"已提交"，拿不到分**。
//! 服务端收不到文件，自然无法评分。实测这类题的 `base` 是
//! `multiFileUpload`，出现在多值 `base` 列表里（如
//! `short_answer,short_answer,multiFileUpload`）。
//!
//! ## 载荷形状
//!
//! ```json
//! { "question_type": "multiFileUpload",
//!   "reply_type": "multiFileUpload",
//!   "payloads": [],          ← 接入上传后这里放 fileList
//!   "versions": {...},
//!   "desc": "" }
//! ```
//!
//! ⚠️ 注意它**没有 `value` 字段**——与角色扮演一样，不上报文本作答。

use serde_json::{Value, json};

use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 多媒体上传题。
pub struct Upload;

impl QuestionKind for Upload {
    fn name(&self) -> &'static str {
        "upload"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_multi_file_upload()
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        json!({
            "question_type": ctx.question_type(),
            "reply_type": ctx.reply_type(),
            "payloads": [],
            "versions": ctx.versions,
            "desc": "",
        })
    }

    fn scoring_note(&self) -> &'static str {
        "上传题：payloads 恒空（上传链路未接入），只能拿完成度，拿不到分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    fn upload_meta(reply: &str) -> QuestionMeta {
        QuestionMeta {
            question_type: "multiFileUpload".to_owned(),
            reply_type: reply.to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        }
    }

    /// 三种命名都要认（题型名、两种 `replyType` 拼法）。
    #[test]
    fn matches_all_three_naming_conventions() {
        for reply in ["multiFileUpload", "multimedia_upload"] {
            assert!(Upload.matches(&upload_meta(reply)), "{reply}");
        }
        let by_type = QuestionMeta {
            question_type: "multiFileUpload".to_owned(),
            reply_type: String::new(),
            ..Default::default()
        };
        assert!(Upload.matches(&by_type), "只给 question_type 也要认");
    }

    /// ★ `payloads` 恒为空数组（上传未接入）。
    #[test]
    fn payloads_are_always_empty_for_now() {
        let meta = upload_meta("multiFileUpload");
        let values = vec![vec!["file.jpg".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(Upload.judge(&context)["payloads"], json!([]));
    }

    /// 载荷里没有 `value` 字段（与其他"非文本"题型一致）。
    #[test]
    fn payload_has_no_value_field() {
        let meta = upload_meta("multiFileUpload");
        let values: Vec<Vec<String>> = Vec::new();
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = Upload.judge(&context);
        assert!(judged.get("value").is_none(), "{judged}");
        assert_eq!(judged["desc"], json!(""));
    }

    /// 不能吞掉主观题。
    #[test]
    fn does_not_swallow_subjective() {
        let meta = QuestionMeta {
            reply_type: "text-area".to_owned(),
            ..Default::default()
        };
        assert!(!Upload.matches(&meta));
    }
}
