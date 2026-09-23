//! 排序题（`replyType = sequence`，题库 `base` 为 `sequence_course`）。
//!
//! ## 一个顺序陷阱
//!
//! 排序题的"答案"本身就是**一串有序项**。上报时 `value` 是逗号连接的字符串，
//! 而**顺序就是答案**：
//!
//! ```json
//! answer.children[0].value  = ["2","1","3"]     ← 数组保持顺序
//! thirdPartyJudges[0].value = "2,1,3"           ← 逗号连接后顺序不变
//! ```
//!
//! 一旦在这里排序、去重或做集合处理，答案就被改写了。本实现**原样转发**，
//! 不做任何规整——这与多选的处理原则相同，但排序题更危险：
//! 多选排序可能只是判错，排序题排序**必然**判错。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 排序题。
pub struct Sequence;

impl QuestionKind for Sequence {
    fn name(&self) -> &'static str {
        "sequence"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.reply_type == "sequence"
            || meta.question_type == "sequence"
            || meta.question_type.contains("sequence")
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "排序题：value 是逗号连接的有序项，顺序即答案，绝不重排"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    #[test]
    fn matches_reply_and_question_type() {
        let by_reply = QuestionMeta {
            reply_type: "sequence".to_owned(),
            ..Default::default()
        };
        assert!(Sequence.matches(&by_reply));

        let by_base = QuestionMeta {
            question_type: "sequence_course".to_owned(),
            ..Default::default()
        };
        assert!(Sequence.matches(&by_base));
    }

    /// ★ 顺序必须保持——这是排序题唯一可能判错的地方。
    #[test]
    fn order_is_preserved_exactly() {
        let meta = QuestionMeta {
            question_type: "sequence".to_owned(),
            reply_type: "sequence".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        // 故意用数字串，排序后会被改变。
        let values = vec![vec!["3".to_owned(), "1".to_owned(), "2".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(
            Sequence.judge(&context)["value"],
            serde_json::json!("3,1,2")
        );
    }

    /// `answer` 里的数组同样保持原顺序。
    #[test]
    fn answer_array_keeps_order() {
        let meta = QuestionMeta {
            question_type: "sequence".to_owned(),
            reply_type: "sequence".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        let question = crate::question::QuestionSubmission::new(
            meta,
            vec![vec!["3".to_owned(), "1".to_owned(), "2".to_owned()]],
        );
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(
            answer["children"][0]["value"],
            serde_json::json!(["3", "1", "2"])
        );
    }

    /// 不能吞掉普通客观题。
    #[test]
    fn does_not_swallow_objective() {
        let meta = QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "singlechoice".to_owned(),
            ..Default::default()
        };
        assert!(!Sequence.matches(&meta));
    }
}
