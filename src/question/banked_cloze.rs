//! 选词填空 / 篇章选词填空（`bankedcloze`、`material-banked-cloze`）。
//!
//! 与普通填空的差别只在**答案来源**：选项从一个词库里选，但上报形状相同
//! ——每个空一项，`value` 是该空填入的单词。
//!
//! 题型名出现在题库 `base` 里（实测有 `bankedcloze_course`），
//! `replyType` 为 `bankedcloze` 或 `material-banked-cloze`。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 选词填空。
pub struct BankedCloze;

impl QuestionKind for BankedCloze {
    fn name(&self) -> &'static str {
        "banked-cloze"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        matches!(
            meta.reply_type.as_str(),
            "bankedcloze" | "material-banked-cloze"
        ) || meta.question_type.contains("bankedcloze")
            || meta.question_type.contains("banked-cloze")
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "选词填空：每个空一项，value 为填入词，由服务端判分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::QuestionMeta;

    #[test]
    fn matches_reply_type_variants() {
        for reply in ["bankedcloze", "material-banked-cloze"] {
            let meta = QuestionMeta {
                reply_type: reply.to_owned(),
                ..Default::default()
            };
            assert!(BankedCloze.matches(&meta), "{reply}");
        }
    }

    /// 题型名里带 `bankedcloze` 也要认（题库 `base` 是 `bankedcloze_course`）。
    #[test]
    fn matches_question_type_base() {
        let meta = QuestionMeta {
            question_type: "bankedcloze_course".to_owned(),
            ..Default::default()
        };
        assert!(BankedCloze.matches(&meta));
    }

    /// 不能把普通填空吞掉——那会让注册表顺序失去意义。
    #[test]
    fn does_not_swallow_plain_fillblank() {
        let meta = QuestionMeta {
            reply_type: "fillblank".to_owned(),
            ..Default::default()
        };
        assert!(!BankedCloze.matches(&meta));
    }
}
