//! 填空题（`replyType = fillblank`）。
//!
//! ## 一个空一个子题
//!
//! 填空题的"子题"就是**空**：一篇短文挖 5 个空，就是 5 个子题，
//! `inCompleted` 与 `thirdPartyJudges` 各 5 项。
//!
//! 实测（`examples/dump_submit_info.rs` 读服务端快照）：
//!
//! ```json
//! "reply_type": "fillblank", "child_count": 5
//! answer.children[i].value = ["答案文本"]
//! ```
//!
//! ## 也承载"连读"变体
//!
//! `basic-scoop-content` + `fillblank` 是同一套载荷（实测 8 例）。
//! 连读的差别只在**是否需要播放媒体**，不影响判分结构——
//! 媒体下标单独走 `answer.progress`（见 [`crate::question`] 的模块文档）。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 填空题。
pub struct FillBlank;

impl QuestionKind for FillBlank {
    fn name(&self) -> &'static str {
        "fill-blank"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        matches!(meta.reply_type.as_str(), "fillblank" | "fillblankScoop")
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "填空题：每个空占一项，value 为该空的答案文本，由服务端逐空判分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    /// `fillblank` 与连读变体 `fillblankScoop` 都归这里。
    #[test]
    fn matches_both_fillblank_flavours() {
        let plain = QuestionMeta {
            reply_type: "fillblank".to_owned(),
            ..Default::default()
        };
        assert!(FillBlank.matches(&plain));

        let scoop = QuestionMeta {
            reply_type: "fillblankScoop".to_owned(),
            ..Default::default()
        };
        assert!(FillBlank.matches(&scoop));

        let subjective = QuestionMeta {
            reply_type: "text-area".to_owned(),
            ..Default::default()
        };
        assert!(
            !FillBlank.matches(&subjective),
            "text-area 是主观题，不是填空"
        );
    }

    /// ★ 5 个空 = 5 项，不是 1 项。
    #[test]
    fn each_blank_is_one_slot() {
        let meta = QuestionMeta {
            question_type: "basic-scoop-content".to_owned(),
            reply_type: "fillblank".to_owned(),
            category: Some(1),
            child_count: 5,
            ..Default::default()
        };
        assert_eq!(FillBlank.completed_len(&meta), 5);
    }

    /// 每个空上报自己的答案，互不串位。
    #[test]
    fn each_blank_reports_its_own_value() {
        let meta = QuestionMeta {
            question_type: "basic-scoop-content".to_owned(),
            reply_type: "fillblank".to_owned(),
            category: Some(1),
            child_count: 3,
            ..Default::default()
        };
        let values = vec![
            vec!["alpha".to_owned()],
            vec!["beta".to_owned()],
            vec!["gamma".to_owned()],
        ];
        let versions = SubmitVersions::default().to_json();
        for (index, expected) in ["alpha", "beta", "gamma"].iter().enumerate() {
            let context = JudgeContext {
                meta: &meta,
                values: &values,
                child_index: index,
                versions: &versions,
                role_play_payloads: None,
            };
            assert_eq!(
                FillBlank.judge(&context)["value"],
                serde_json::json!(expected)
            );
        }
    }
}
