//! 下拉选择题（`replyType = dropdownSelection`）。
//!
//! ## 一个易错的拼写
//!
//! 前端枚举里这个名字**少一个 `n`**：真实值是 `dropdownSelection`，
//! 而 `content.rs` 的中文标签表里写的是 `dropdownchoic`（也是原代码的拼法）。
//! 两者指同一类题，但**匹配时必须用 `dropdownSelection`**——那才是
//! 内容接口实际下发的 `replyType`。
//!
//! 实测（`examples/probe_category.rs`）：`basic-scoop-content` +
//! `dropdownSelection` 共 2 例，载荷与填空一致。

use serde_json::Value;

use super::objective::Objective;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 下拉选择题。
pub struct Dropdown;

impl QuestionKind for Dropdown {
    fn name(&self) -> &'static str {
        "dropdown"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        // 把两种拼法都认下，避免将来服务端统一拼写时静默失效。
        matches!(
            meta.reply_type.as_str(),
            "dropdownSelection" | "dropdownchoic" | "dropdown"
        )
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        Objective.judge(ctx)
    }

    fn scoring_note(&self) -> &'static str {
        "下拉选择题：每个下拉框一项，value 为选中项文本，由服务端判分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    /// ★ 真实 `replyType` 是 `dropdownSelection`（注意 `n`），必须认。
    #[test]
    fn matches_real_reply_type() {
        let meta = QuestionMeta {
            reply_type: "dropdownSelection".to_owned(),
            ..Default::default()
        };
        assert!(Dropdown.matches(&meta), "内容接口下发的就是这个名字");
    }

    /// 中文标签表里的旧拼法也认，防止两种情况分叉。
    #[test]
    fn matches_legacy_spelling() {
        let meta = QuestionMeta {
            reply_type: "dropdownchoic".to_owned(),
            ..Default::default()
        };
        assert!(Dropdown.matches(&meta));
    }

    /// 与填空不能互相吞。
    #[test]
    fn does_not_swallow_fillblank() {
        let meta = QuestionMeta {
            reply_type: "fillblank".to_owned(),
            ..Default::default()
        };
        assert!(!Dropdown.matches(&meta));
    }

    /// 载荷形状与客观题一致。
    #[test]
    fn judge_shape_matches_objective() {
        let meta = QuestionMeta {
            question_type: "basic-scoop-content".to_owned(),
            reply_type: "dropdownSelection".to_owned(),
            category: Some(1),
            child_count: 1,
            ..Default::default()
        };
        let values = vec![vec!["option-b".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(Dropdown.judge(&context), Objective.judge(&context));
    }
}
