//! 学习题：**不贡献任何提交项**。
//!
//! ## 它为什么值得单独一个文件
//!
//! 学习题的特殊之处不在载荷（它根本没有载荷），而在**它是唯一一类被计数的
//! 排除项**：`isCompleted` 与 `thirdPartyJudges` 都不收它，但 `quesDatas`
//! 要收。少算一项就是 `code:300100`，多算一项就是 `code:2 index out of range`。
//!
//! ## 怎么判定
//!
//! 复刻前端 `isStudyQuestion()`：
//!
//! - `video-popup` **例外地不算**学习题；
//! - 其余看 `category == 0`（`Study`）；
//! - `discussion` / `rich-text-read` 无论 `category` 都算。
//!
//! ## 实测分布（`examples/probe_category.rs`，三班《视听说2》89 道题）
//!
//! | category | 题型 | 条数 | 提交？ |
//! | --- | --- | --- | --- |
//! | `0` | `basic`/`default`、`vocabulary`、`exit-ticket` | 14 | ✘ 学习题 |
//! | `1` | `singlechoice`、`multichoice`、`fillblank`… | 46 | ✔ |
//! | `7` | `basic-oral`、`role-play`… | 24 | ✔ |
//! | `8` | `basic`/`text-area`（主观题） | 5 | ✔ |
//!
//! **真实客观题是 `category=1` 不是 `0`** —— 按直觉填 0 会把所有客观题判成
//! 学习题，一个都交不上。写单测时极易踩到。

use serde_json::{Value, json};

use super::{JudgeContext, QuestionKind, QuestionMeta};

/// 学习题。
pub struct Study;

impl QuestionKind for Study {
    fn name(&self) -> &'static str {
        "study"
    }

    fn matches(&self, meta: &QuestionMeta) -> bool {
        meta.is_study()
    }

    /// 恒 0：学习题不贡献任何项。
    fn completed_len(&self, _meta: &QuestionMeta) -> usize {
        0
    }

    /// 不会被调用（`GroupSubmission::to_body` 提前跳过学习题）。
    /// 返回空对象而非 panic，防止将来有人改了跳过逻辑就立刻崩。
    fn judge(&self, _ctx: &JudgeContext) -> Value {
        json!({})
    }

    fn scoring_note(&self) -> &'static str {
        "学习题：不计分也不计完成，提交时完全不贡献 isCompleted/thirdPartyJudges"
    }
}
