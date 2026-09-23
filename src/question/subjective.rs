//! 主观题（`replyType = text-area`）：**不评分，只算完成度**。
//!
//! ## 两条与直觉相反的规则
//!
//! ### 1. `payloads` 必须是**空数组**
//!
//! 这是实测出来的，不是推的。用户**手工**在网页上填的组 `u7g419`，
//! 服务端存回的原文是：
//!
//! ```json
//! { "answers": "你好", "value": "你好",
//!   "payloads": [],                    ← ★ 空数组
//!   "question_type": "basic", "reply_type": "text-area",
//!   "rule": "subjective" }
//! ```
//!
//! 我原先按 bundle 默认分支推出 `payloads:[{recordDetail:…}]`，**推错了**。
//! 证据优先于推断：这里对齐真实前端，留空。
//!
//! ### 2. `answer.children[i].value` 必须是**字符串**
//!
//! 不是数组。实测交空数组则完成度恒 0 —— 这正是主观题一直「交不上」的原因。
//!
//! ## 分数结构性恒 0
//!
//! ```json
//! "__SUBMIT_INFO__.state": { "real_score_pct": 1,   ← 完成度 100%
//!                            "score_pct": 0 }       ← 判分恒 0
//! ```
//!
//! 主观题没有判分器，只有「交没交」。所以**不存在"满分"这个维度**，
//! 能拿到的就是完成度 100%。
//!
//! ## 占位作答
//!
//! 主观题**没有服务端标准答案**（答案接口此处为空），而完成度要求作答非空，
//! 所以 [`crate::question::QuestionSubmission::fill_subjective_placeholder`]
//! 会补一句 [`crate::question::SUBJECTIVE_PLACEHOLDER`]。
//!
//! ⚠️ **那句不是学生写的**，界面必须让用户清楚这一点。

use serde_json::{Value, json};

use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 主观题。
pub struct Subjective;

impl QuestionKind for Subjective {
    fn name(&self) -> &'static str {
        "subjective"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_subjective()
    }

    /// ★ `value` 给字符串，不给数组。
    fn child_value(&self, raw: Vec<String>) -> Value {
        Value::String(raw.join("\n"))
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        json!({
            "value": ctx.judge_value(),
            "question_type": ctx.question_type(),
            "reply_type": ctx.reply_type(),
            "versions": ctx.versions,
            "payloads": [],
        })
    }

    fn scoring_note(&self) -> &'static str {
        "主观题：不评分（score_pct 恒 0），只看完成度；value 必须是字符串"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{
        QuestionMeta, QuestionSubmission, SUBJECTIVE_PLACEHOLDER, SubmitVersions,
    };

    fn subj_meta(children: usize) -> QuestionMeta {
        QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "text-area".to_owned(),
            category: Some(8),
            child_count: children,
            ..Default::default()
        }
    }

    /// 只认 `text-area`。
    #[test]
    fn matches_only_text_area() {
        assert!(Subjective.matches(&subj_meta(1)));
        let fill = QuestionMeta {
            reply_type: "fillblank".to_owned(),
            ..Default::default()
        };
        assert!(
            !Subjective.matches(&fill),
            "填空的 value 是数组，不能归主观题"
        );
    }

    /// ★ `payloads` 必须是空数组（实测，非推断）。
    #[test]
    fn payloads_must_be_empty_array() {
        let meta = subj_meta(1);
        let values = vec![vec!["你好".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = Subjective.judge(&context);
        assert_eq!(judged["payloads"], json!([]), "服务端存回的就是空数组");
        assert_eq!(judged["value"], json!("你好"));
    }

    /// ★ `answer.children[i].value` 必须是字符串。
    #[test]
    fn child_value_is_string_not_array() {
        let question = QuestionSubmission::new(subj_meta(1), vec![vec!["你好".to_owned()]]);
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["children"][0]["value"], json!("你好"));
    }

    /// 多段作答合并成一个字符串（用换行连接）。
    #[test]
    fn multiple_parts_join_with_newline() {
        let question = QuestionSubmission::new(
            subj_meta(1),
            vec![vec!["第一段".to_owned(), "第二段".to_owned()]],
        );
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["children"][0]["value"], json!("第一段\n第二段"));
    }

    /// ★ 空作答必须被补上占位文本，否则完成度永远是 0。
    #[test]
    fn empty_answer_gets_placeholder() {
        let question =
            QuestionSubmission::new(subj_meta(2), Vec::new()).fill_subjective_placeholder();
        assert_eq!(question.values.len(), 2, "按子题数补齐");
        for slot in &question.values {
            assert_eq!(slot, &vec![SUBJECTIVE_PLACEHOLDER.to_owned()]);
        }
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(
            answer["children"][0]["value"],
            json!(SUBJECTIVE_PLACEHOLDER)
        );
    }

    /// 纯空白也算空，同样要补。
    #[test]
    fn whitespace_only_answer_gets_placeholder() {
        let question = QuestionSubmission::new(subj_meta(1), vec![vec!["   ".to_owned()]])
            .fill_subjective_placeholder();
        assert_eq!(question.values[0], vec![SUBJECTIVE_PLACEHOLDER.to_owned()]);
    }

    /// 已有作答时**不能**被覆盖。
    #[test]
    fn real_answer_is_not_overwritten() {
        let question = QuestionSubmission::new(subj_meta(1), vec![vec!["学生写的".to_owned()]])
            .fill_subjective_placeholder();
        assert_eq!(question.values[0], vec!["学生写的".to_owned()]);
    }

    /// 占位文本必须显眼，让人一眼看出不是学生写的。
    #[test]
    fn placeholder_is_visibly_synthetic() {
        assert!(SUBJECTIVE_PLACEHOLDER.starts_with('['));
        assert!(SUBJECTIVE_PLACEHOLDER.ends_with(']'));
        assert!(SUBJECTIVE_PLACEHOLDER.contains("提交"));
    }
}
