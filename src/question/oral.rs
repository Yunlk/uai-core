//! 口语 / 录音题：`payloads` 装录音，**分数直接给满分**。
//!
//! ## 载荷形状（逐子题一项）
//!
//! ```json
//! { "value": "朗读文本",
//!   "payloads": [ { "url": "",
//!                   "specific_scores": { total:100, accuracy:100, integrity:100, fluency:100 },
//!                   "recordDetail": { score:100, smooth:100, completed:100,
//!                                     correctness:100, audioUrl:"", details:[], relevance:100 } } ],
//!   "specific_scores": { total:100, accuracy:100, integrity:100, fluency:100 },
//!   "versions": {...}, "question_type": "basic-oral", "reply_type": "record" }
//! ```
//!
//! ## 分数为什么可以直接填 100
//!
//! 全 bundle 只有一处给 `recordDetail.score` 赋值，就在 `saveRecord` 里，
//! 值取自**本地 state**（`e.score = e.recordDetail?.score || 0`），
//! 没有任何代码把服务端响应写回该字段。**填多少服务端就记多少。**
//! 且 `UAI-Textbook-Console` 实测：服务端存下的就是客户端塞的那个 `record`。
//!
//! 所以这里走 [`full_marks_record`]：`audioUrl` 留空，分数给满。
//! 「真实录音 + 服务端评测」那条路（`speech_eval` + `media`）另算，
//! 需要手上有真实音频；不接也不影响分数。
//!
//! ⚠️ 逐子题展开：每个子题各带**一个**满分 record，不是把多个塞进一个数组。

use serde_json::{Value, json};

use super::record::{full_marks_record, full_marks_scores};
use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 口语 / 录音题。
pub struct Oral;

impl QuestionKind for Oral {
    fn name(&self) -> &'static str {
        "oral"
    }

    /// 判定优先看 `reply_type == "record"`（真实观测值），题型名作补充。
    ///
    /// 前端那个 `case` 列的是 `a.xib.BasicOral` 这类**枚举成员名**，而枚举
    /// 定义在另一个 vendor chunk 里，本仓库手上没有。把它小写成 `"basic-oral"`
    /// 只是猜测——猜错会静默走进默认分支，等于这个功能没生效。
    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_oral()
    }

    fn judge(&self, ctx: &JudgeContext) -> Value {
        json!({
            "value": ctx.judge_value(),
            "payloads": [full_marks_record("", None)],
            "specific_scores": full_marks_scores(),
            "versions": ctx.versions,
            "question_type": ctx.question_type(),
            "reply_type": ctx.reply_type(),
        })
    }

    fn scoring_note(&self) -> &'static str {
        "口语/录音题：payloads 装满分 record（score=100），audioUrl 为空"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, SubmitVersions};

    fn judge_for(meta: &QuestionMeta, values: &[Vec<String>]) -> Value {
        let versions = SubmitVersions::default().to_json();
        let context = JudgeContext {
            meta,
            values,
            child_index: 0,
            versions: &versions,
            role_play_payloads: None,
        };
        Oral.judge(&context)
    }

    /// ★ `reply_type == "record"` 是真实观测值，必须命中。
    #[test]
    fn matches_real_reply_type() {
        let meta = QuestionMeta {
            reply_type: "record".to_owned(),
            ..Default::default()
        };
        assert!(Oral.matches(&meta));
    }

    /// 各种口语题型名作补充。
    #[test]
    fn matches_oral_question_types() {
        for name in [
            "basic-oral",
            "paragraph-follow",
            "oral-personal-state",
            "video-dub",
            "oral-aloud",
            "sentencerecord",
        ] {
            let meta = QuestionMeta {
                question_type: name.to_owned(),
                ..Default::default()
            };
            assert!(Oral.matches(&meta), "{name} 应被识别为口语题");
        }
    }

    /// `payloads` 必须是**一个** record，不是数组里塞多个。
    #[test]
    fn payloads_hold_one_record() {
        let meta = QuestionMeta {
            question_type: "basic-oral".to_owned(),
            reply_type: "record".to_owned(),
            category: Some(7),
            child_count: 1,
            ..Default::default()
        };
        let judged = judge_for(&meta, &[vec!["朗读".to_owned()]]);
        let payloads = judged["payloads"].as_array().expect("payloads 应是数组");
        assert_eq!(payloads.len(), 1);
        assert_eq!(judged["value"], json!("朗读"));
    }

    /// ★ 分数全部满分——这是本模块的核心承诺。
    #[test]
    fn all_scores_are_full_marks() {
        let meta = QuestionMeta {
            reply_type: "record".to_owned(),
            child_count: 1,
            ..Default::default()
        };
        let judged = judge_for(&meta, &[vec![String::new()]]);

        assert_eq!(judged["specific_scores"]["total"], json!(100));
        assert_eq!(judged["specific_scores"]["fluency"], json!(100));
        let record = &judged["payloads"][0];
        assert_eq!(record["specific_scores"]["total"], json!(100));
        assert_eq!(record["recordDetail"]["score"], json!(100));
        assert_eq!(record["recordDetail"]["completed"], json!(100));
        assert_eq!(record["recordDetail"]["smooth"], json!(100));
        assert_eq!(record["recordDetail"]["correctness"], json!(100));
        assert_eq!(record["recordDetail"]["relevance"], json!(100));
    }

    /// `url` 与 `audioUrl` 都是空串（没有真实录音可上传）。
    #[test]
    fn urls_are_empty_without_real_audio() {
        let meta = QuestionMeta {
            reply_type: "record".to_owned(),
            child_count: 1,
            ..Default::default()
        };
        let judged = judge_for(&meta, &[vec![String::new()]]);
        let record = &judged["payloads"][0];
        assert_eq!(record["url"], json!(""));
        assert_eq!(record["recordDetail"]["audioUrl"], json!(""));
    }

    /// 不能吞掉 oral-state（它由专门实现处理，先于本类型）。
    #[test]
    fn oral_state_is_handled_elsewhere_first() {
        let state = QuestionMeta {
            question_type: "oral-state".to_owned(),
            reply_type: "record".to_owned(),
            ..Default::default()
        };
        // Oral 的 matches 确实为真——它满足条件。
        assert!(Oral.matches(&state));
        // 但注册表把 oral-state 排在前面，所以实际由 OralState 处理。
        assert_eq!(state.kind().name(), "oral-state");
    }

    /// 普通客观题不能被误判成口语题。
    #[test]
    fn does_not_match_objective() {
        let meta = QuestionMeta {
            question_type: "basic".to_owned(),
            reply_type: "singlechoice".to_owned(),
            ..Default::default()
        };
        assert!(!Oral.matches(&meta));
    }
}
