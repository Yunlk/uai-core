//! 口语陈述题（`question_type = "oral-state"`）：**只取最后一个子题**。
//!
//! ## 这是全库唯一两种"整题只算 1 项"的题型之一
//!
//! 前端 `Ut()` 里 `case a.xib.OralState` 只 push 一次，无论有多少子题。
//! 剩下子题的作答不会被丢掉，而是收进 `value` 的**映射**里：
//!
//! ```json
//! { "specific_scores": { total:100, accuracy:100, integrity:100, fluency:100 },
//!   "payloads": [ {record…} ],          ← 只有一个 record，满分
//!   "versions": {...},
//!   "question_type": "oral-state",
//!   "reply_type": "record",
//!   "value": { "0": "第一句", "1": ["多值","也支持"] } }   ← 前面各子题
//! ```
//!
//! ## ⚠️ 注册顺序是语义的一部分
//!
//! 本类型的 `reply_type` 也是 `"record"`，因此**同时满足** [`super::oral`] 的
//! 条件。若它在注册表里排在 `oral` 之后，逐子题规则会先命中，
//! 「只取最后一个子题」就静默失效——提交仍会成功，但子题数对不上，
//! 服务端报 `300100`。见 [`super::KINDS`]。

use serde_json::{Map, Value, json};

use super::record::{full_marks_record, full_marks_scores};
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 口语陈述题。
pub struct OralState;

impl QuestionKind for OralState {
    fn name(&self) -> &'static str {
        "oral-state"
    }

    /// 只认题型名 `oral-state`（不看 `reply_type`——那是 `record`，太宽）。
    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_oral_state()
    }

    /// 恒 1 项，与子题数无关。
    fn completed_len(&self, _meta: &QuestionMeta) -> usize {
        1
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        let meta = ctx.meta;
        // 最后一个子题是"真正的"作答；前面的收进 value 映射。
        let last = meta.child_count.saturating_sub(1);
        let mut value_map = Map::new();
        for index in 0..last {
            let raw = ctx.values.get(index).cloned().unwrap_or_default();
            // 单值给字符串，多值给数组——复刻前端原样。
            let entry = if raw.len() > 1 {
                Value::Array(raw.into_iter().map(Value::String).collect())
            } else {
                Value::String(raw.first().cloned().unwrap_or_default())
            };
            value_map.insert(index.to_string(), entry);
        }
        json!({
            "specific_scores": full_marks_scores(),
            "payloads": [full_marks_record("", None)],
            "versions": ctx.versions,
            "question_type": meta.question_type,
            "reply_type": meta.reply_type,
            "value": Value::Object(value_map),
        })
    }

    fn scoring_note(&self) -> &'static str {
        "口语陈述(oral-state)：整题只算 1 项，前面子题收进 value 映射，record 给满分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    fn state_meta(children: usize) -> QuestionMeta {
        QuestionMeta {
            question_type: "oral-state".to_owned(),
            reply_type: "record".to_owned(),
            category: Some(7),
            child_count: children,
            ..Default::default()
        }
    }

    /// ★ 无论几个子题，只算 1 项。
    #[test]
    fn always_contributes_exactly_one_slot() {
        for children in [1, 2, 5, 8] {
            let meta = state_meta(children);
            assert_eq!(
                OralState.completed_len(&meta),
                1,
                "{children} 个子题时仍应 1 项"
            );
            assert_eq!(meta.is_completed_len(), 1);
        }
    }

    /// ★ 前面的子题必须收进 `value` 映射，不能丢。
    #[test]
    fn earlier_children_go_into_value_map() {
        let meta = state_meta(3);
        let values = vec![
            vec!["第一句".to_owned()],
            vec!["第二句".to_owned()],
            vec!["最后一句".to_owned()],
        ];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = OralState.judge(&context);

        // 下标 0、1 进映射；下标 2（最后一个）不进。
        assert_eq!(judged["value"]["0"], json!("第一句"));
        assert_eq!(judged["value"]["1"], json!("第二句"));
        assert!(judged["value"].get("2").is_none(), "最后一个子题不该进映射");
    }

    /// 多值子题在映射里给数组，单值给字符串。
    #[test]
    fn multi_value_child_becomes_array_in_map() {
        let meta = state_meta(2);
        let values = vec![
            vec!["A".to_owned(), "B".to_owned()],
            vec!["最后".to_owned()],
        ];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = OralState.judge(&context);
        assert_eq!(judged["value"]["0"], json!(["A", "B"]));
    }

    /// 只有一个子题时映射为空对象，不能是 null。
    #[test]
    fn single_child_yields_empty_map() {
        let meta = state_meta(1);
        let values = vec![vec!["唯一".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(OralState.judge(&context)["value"], json!({}));
    }

    /// `payloads` 只有一个 record，且是满分。
    #[test]
    fn payloads_hold_single_full_marks_record() {
        let meta = state_meta(4);
        let values: Vec<Vec<String>> = vec![vec![String::new()]; 4];
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        let judged = OralState.judge(&context);
        assert_eq!(judged["payloads"].as_array().map(Vec::len), Some(1));
        assert_eq!(judged["payloads"][0]["recordDetail"]["score"], json!(100));
        assert_eq!(
            judged["payloads"][0]["specific_scores"]["total"],
            json!(100)
        );
        assert_eq!(judged["specific_scores"]["total"], json!(100));
    }

    /// 子题为 0 时不能 panic（`saturating_sub`）。
    #[test]
    fn zero_children_does_not_panic() {
        let meta = state_meta(0);
        let values: Vec<Vec<String>> = Vec::new();
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(OralState.judge(&context)["value"], json!({}));
    }
}
