//! 多选题（`replyType = multichoice`）。
//!
//! ## 与单选的关键差别：多值的**顺序**
//!
//! 单选一个子题一个值；多选一个子题**多个值**，`answer.children[i].value`
//! 是数组，而 `thirdPartyJudges[i].value` 把它们**逗号连接**成字符串：
//!
//! ```json
//! answer.children[0].value          = ["A","B","D"]
//! thirdPartyJudges[0].value         = "A,B,D"
//! ```
//!
//! 实测服务端存回的原文就是这个形状（`u1g17`：`"value": "A,B,D,F,G"`）。
//!
//! ⚠️ **顺序由服务端标准答案决定，不要自己排序。** 我们只是把读到的
//! 标准答案原样转发；擅自排序或去重会改变上报内容。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 多选题。
pub struct MultiChoice;

impl QuestionKind for MultiChoice {
    fn name(&self) -> &'static str {
        "multi-choice"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.reply_type == "multichoice"
    }

    /// 与客观题一致（多值由 `judge_value()` 逗号连接）。
    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "多选题：value 为逗号连接的多个选项，顺序沿用标准答案，由服务端判分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    /// 只认 `multichoice`。
    #[test]
    fn matches_only_multichoice() {
        let multi = QuestionMeta {
            reply_type: "multichoice".to_owned(),
            ..Default::default()
        };
        assert!(MultiChoice.matches(&multi));

        let single = QuestionMeta {
            reply_type: "singlechoice".to_owned(),
            ..Default::default()
        };
        assert!(!MultiChoice.matches(&single));
    }

    /// ★ 多值上报是逗号连接的字符串，且**保持原顺序**（不排序）。
    #[test]
    fn multiple_values_join_with_comma_in_order() {
        let meta = QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "multichoice".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        // 故意给一个非字典序，验证不会被重排。
        let values = vec![vec!["D".to_owned(), "A".to_owned(), "F".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(
            MultiChoice.judge(&context)["value"],
            serde_json::json!("D,A,F")
        );
    }

    /// `answer` 里同一子题的 `value` 仍是数组（未被逗号连接）。
    #[test]
    fn answer_json_keeps_array_shape() {
        let meta = QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "multichoice".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        let question = crate::question::QuestionSubmission::new(
            meta,
            vec![vec!["A".to_owned(), "B".to_owned()]],
        );
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(
            answer["children"][0]["value"],
            serde_json::json!(["A", "B"])
        );
    }

    /// 单值多选（只选了一个）也要能正常上报，不能因为"多选"就要求 2 个值。
    #[test]
    fn single_selected_value_works() {
        let meta = QuestionMeta {
            reply_type: "multichoice".to_owned(),
            child_count: 1,
            ..Default::default()
        };
        let values = vec![vec!["A".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(MultiChoice.judge(&context)["value"], serde_json::json!("A"));
    }
}
