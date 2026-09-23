//! 纯客观题：**载荷形状的基准**，也是兜底实现。
//!
//! `payloads` 为**空数组**——服务端自己判分，客户端不上报任何东西。
//!
//! ```json
//! { "value": "A,B,D",
//!   "question_type": "basic",
//!   "reply_type": "multichoice",
//!   "versions": { "course": "", "group": 1, "template": 1, "answer": 3, "content": "0" },
//!   "payloads": [] }
//! ```
//!
//! ## 为什么是兜底
//!
//! 未知题型（教师自建活动 `magic_19_*` 之类）落到这里。它们的载荷形状与
//! 普通客观题一致：`value` 给字符串、`payloads` 留空。这样新题型**不会导致
//! 提交失败**，只是拿不到特殊处理。

use serde_json::{Value, json};

use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 通用客观题。
pub struct Objective;

impl QuestionKind for Objective {
    fn name(&self) -> &'static str {
        "objective"
    }

    /// 兜底：任何未被前面题型认领的题都归这里。
    fn matches(&self, _meta: &QuestionMeta) -> bool {
        true
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
        "纯客观题：答案取自服务端标准答案，payloads 留空，由服务端判分"
    }
}
