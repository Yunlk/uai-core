//! 单选题（`replyType = singlechoice`）。
//!
//! 最基础的一种：一个子题一个答案，`payloads` 留空由服务端判分。
//!
//! ```json
//! answer.children[0].value = ["D"]
//! thirdPartyJudges[i].value = "D"
//! ```
//!
//! ## 满分条件
//!
//! 答案必须**逐字**等于服务端标准答案（`/course/api/v3/answer` 下发）。
//! 实测（`u1g405`、`u1g17`）用标准答案提交后 `score_pct = 1`、`state = 1`。
//!
//! 自己猜答案没有意义：服务端比对的是字符串，大小写、空格、多选的顺序
//! 都可能影响判分。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 单选题。
pub struct SingleChoice;

impl QuestionKind for SingleChoice {
    fn name(&self) -> &'static str {
        "single-choice"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.reply_type == "singlechoice"
    }

    /// 与客观题完全一致，直接委托，保证形状不漂移。
    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "单选题：value 为单个选项字符串，服务端判分，满分需逐字匹配标准答案"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    /// 单选只认 `reply_type == "singlechoice"`。
    #[test]
    fn matches_only_singlechoice() {
        let single = QuestionMeta {
            reply_type: "singlechoice".to_owned(),
            ..Default::default()
        };
        assert!(SingleChoice.matches(&single));

        let multi = QuestionMeta {
            reply_type: "multichoice".to_owned(),
            ..Default::default()
        };
        assert!(!SingleChoice.matches(&multi));
    }

    /// 载荷形状与客观题一致：`value` 是字符串、`payloads` 为空。
    #[test]
    fn judge_shape_matches_objective() {
        let meta = QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "singlechoice".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        let values = vec![vec!["D".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = SingleChoice.judge(&context);
        assert_eq!(judged["value"], serde_json::json!("D"));
        assert_eq!(judged["payloads"], serde_json::json!([]));
        assert_eq!(judged["question_type"], serde_json::json!("basic"));
        assert_eq!(judged["reply_type"], serde_json::json!("singlechoice"));

        // 与兜底实现逐字节一致。
        assert_eq!(judged, Objective.judge(&context));
    }

    /// 未作答时 `value` 是空串，不能是 null —— 服务端会因此报数量不匹配。
    #[test]
    fn empty_answer_yields_empty_string() {
        let meta = QuestionMeta {
            reply_type: "singlechoice".to_owned(),
            child_count: 1,
            ..Default::default()
        };
        let values: Vec<Vec<String>> = vec![];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(SingleChoice.judge(&context)["value"], serde_json::json!(""));
    }
}
