//! 角色扮演题（`question_type = "role-play"`）：**1 项完成，但逐子题 payloads**。
//!
//! ## 全库唯一"两个数组长度故意不一致"的题型
//!
//! 这是最容易写错的一种。规则是：
//!
//! | 数组 | 长度 |
//! | --- | --- |
//! | `isCompleted` | **1**（整题只算一项） |
//! | `thirdPartyJudges` | **1**（同样一项） |
//! | 那一项里的 `payloads` | **等于子题数**（每个角色一个 record） |
//!
//! 也就是说：外层数组是 1 项，但这一项内部装着一个长度为 `child_count`
//! 的 record 数组。误以为"外层要按子题展开"会导致 `isCompleted` 多出项，
//! 服务端报 `code:2 runtime error: index out of range`。
//!
//! ## 载荷形状
//!
//! ```json
//! { "role": "",
//!   "specific_scores": { total:100, accuracy:100, integrity:100, fluency:100 },
//!   "payloads": [ {record…}, {record…} ],     ← 逐子题，长度 = child_count，每个满分
//!   "versions": {...},
//!   "question_type": "role-play",
//!   "reply_type": "record" }
//! ```
//!
//! ⚠️ 注意它**没有 `value` 字段**——与口语题不同，角色扮演不上报文本作答。
//!
//! ## 分数同样给满分
//!
//! 理由与其他口语题一致，见 [`super::record`]：服务端只认客户端报的值。

use serde_json::{Value, json};

use super::record::full_marks_scores;
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 角色扮演题。
pub struct RolePlay;

impl QuestionKind for RolePlay {
    fn name(&self) -> &'static str {
        "role-play"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_role_play()
    }

    /// 恒 1 项，与子题数无关。
    fn completed_len(&self, _meta: &QuestionMeta) -> usize {
        1
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        json!({
            "role": "",
            "specific_scores": full_marks_scores(),
            "payloads": ctx.role_play_payloads.cloned().unwrap_or_else(|| json!([])),
            "versions": ctx.versions,
            "question_type": ctx.meta.question_type,
            "reply_type": ctx.meta.reply_type,
        })
    }

    fn scoring_note(&self) -> &'static str {
        "角色扮演：整题算 1 项，该项内 payloads 逐子题展开，每个 record 满分"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::record::default_record;
    use crate::question::{QuestionMeta, QuestionSubmission, SubmitVersions};

    fn play_meta(children: usize) -> QuestionMeta {
        QuestionMeta {
            question_type: "role-play".to_owned(),
            reply_type: "record".to_owned(),
            category: Some(7),
            child_count: children,
            ..Default::default()
        }
    }

    /// ★ 无论几个子题，外层只算 1 项。
    #[test]
    fn always_contributes_exactly_one_slot() {
        for children in [1, 2, 4, 9] {
            let meta = play_meta(children);
            assert_eq!(
                RolePlay.completed_len(&meta),
                1,
                "{children} 个子题时仍应 1 项"
            );
        }
    }

    /// ★ 那一项内的 `payloads` 长度必须等于子题数。
    #[test]
    fn payloads_expand_per_child_inside_single_slot() {
        for children in [1, 2, 4, 9] {
            let meta = play_meta(children);
            let question = QuestionSubmission::new(meta, Vec::new());
            let role_play = question.role_play_payloads();
            assert_eq!(
                role_play.as_array().map(Vec::len),
                Some(children),
                "payloads 应逐子题展开为 {children} 项"
            );
        }
    }

    /// ★ 端到端：提交体里两个数组都是 1 项，但内部 payloads 是 4 项。
    #[test]
    fn end_to_end_lengths_within_one_slot() {
        let question = QuestionSubmission::new(play_meta(4), Vec::new());
        let group = crate::question::GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u1g1".to_owned(),
            questions: vec![question],
            versions: SubmitVersions::default(),
        };
        let body = group.to_body("openid");
        let completed = body["isCompleted"].as_array().map(Vec::len).unwrap_or(0);
        let judges: Vec<Value> =
            serde_json::from_str(body["thirdPartyJudges"].as_str().unwrap_or("[]")).unwrap();

        assert_eq!(completed, 1, "外层只 1 项");
        assert_eq!(judges.len(), 1, "外层只 1 项");
        assert_eq!(
            judges[0]["payloads"].as_array().map(Vec::len),
            Some(4),
            "内部 payloads 逐子题展开为 4 项"
        );
        assert_eq!(judges[0]["role"], json!(""));
    }

    /// 载荷里**不应**有 `value` 字段。
    #[test]
    fn payload_has_no_value_field() {
        let meta = play_meta(2);
        let values = vec![vec!["x".to_owned()], vec!["y".to_owned()]];
        let versions = SubmitVersions::default().to_json();
        let role_play = json!([{"a": 1}, {"b": 2}]);
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: Some(&role_play),
        };
        let judged = RolePlay.judge(&context);
        assert!(
            judged.get("value").is_none(),
            "角色扮演不上报文本作答：{judged}"
        );
        assert_eq!(judged["payloads"], role_play);
    }

    /// 所有分数都是满分。
    #[test]
    fn all_scores_are_full_marks() {
        let meta = play_meta(2);
        let values: Vec<Vec<String>> = Vec::new();
        let versions = SubmitVersions::default().to_json();
        let role_play = json!([]);
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: Some(&role_play),
        };
        let judged = RolePlay.judge(&context);
        assert_eq!(judged["specific_scores"]["total"], json!(100));
        assert_eq!(judged["specific_scores"]["fluency"], json!(100));
    }

    /// 没有传入 payloads 时退化为空数组，不能 panic 也不能是 null。
    #[test]
    fn missing_payloads_degrades_to_empty_array() {
        let meta = play_meta(3);
        let values: Vec<Vec<String>> = Vec::new();
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta: &meta,
            values: &values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        assert_eq!(RolePlay.judge(&context)["payloads"], json!([]));
    }

    /// 别名 `roleplay` 也认。
    #[test]
    fn matches_alias() {
        let meta = QuestionMeta {
            question_type: "roleplay".to_owned(),
            ..Default::default()
        };
        assert!(RolePlay.matches(&meta));
    }

    /// 不能吞掉普通口语题。
    #[test]
    fn does_not_swallow_plain_oral() {
        let meta = QuestionMeta {
            question_type: "basic-oral".to_owned(),
            reply_type: "record".to_owned(),
            ..Default::default()
        };
        assert!(!RolePlay.matches(&meta));
    }

    /// `record_default` 的 `details` 始终是空数组，不是 null。
    #[test]
    fn record_details_is_array() {
        let record = default_record("", None);
        assert_eq!(record["recordDetail"]["details"], json!([]));
    }
}
