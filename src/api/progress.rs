//! 进度：单元级（`units`）与**分组级（`leafs`）**。
//!
//! ## 两个端点，各管一段
//!
//! | 端点 | 给什么 | 可信度 |
//! | --- | --- | --- |
//! | `course_progress/{inst}/{open}/default` | `rt.units.<uN>.strategies.required` | ✅ 单元级 |
//! | `course_progress/{inst}/tasks/{open}/default?tasks=<逗号分隔ID>` | **`rt.leafs.<gid>.strategies.required`** | ✅ **分组级（提交口径）** |
//!
//! ### ★ `leafs` 只在 tasks 端点里，单元级端点没有
//!
//! 2026-09 实测《基础篇综合教程2》：单元级响应里
//! `rt` 的键只有 `units / statistic / last_record / …`，**没有 `leafs`**；
//! 而同一个 `rt.leafs` 在 tasks 端点里是齐的。
//! 所以「分组级必修」必须走 tasks 端点——**并且必须显式给出分组 ID 列表**。
//!
//! ```text
//! tasks 端点（184 个 ID 一次发）：leafs 184 项，required=true → 44 个
//! 门户「教程学习成绩」分母：                                  44   ← 精确吻合
//! ```
//!
//! ### ⚠️ tasks 端点的 `state.pass` 不可信，但 `leafs` 可信
//!
//! - `tasks` 传空格、JSON 数组、`all`、`*` 都只得到 `leafs:{}`
//!   （**静默空结果，不报错**）；
//! - 它**对从未做过的教材也回 `state.pass:1`**。所以 [`Progress::required_leafs`]
//!   之外的东西（尤其 `state.pass`、[`fetch_group_progress_raw`] 的原始输出）
//!   不要拿来做完成度判定——完成度看 [`crate::api::course_list`]。
//!
//! ### ⚠️ 单元级 `state.pass` 同样不可信
//!
//! 对一本用户确认 100% 的教材，单元级 `state.pass` **实测全为 0**。
//! 能信的只有 `strategies.required` 这一个字段。
//!
//! ### ★ 真实完成度在哪
//!
//! 在门户：[`crate::api::course_list`]。本模块**不产出任何完成数**。

use std::collections::BTreeMap;

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{course_progress_tasks_url, course_progress_url};
use crate::error::Result;
use crate::transport::{Auth, Transport};

/// 一个单元的必修信息。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitInfo {
    /// 单元 ID，如 `u5`。
    pub id: String,
    /// 该单元是否必修。
    pub required: bool,
    /// 及格门槛（`min_score_pct`）。`0` 表示无门槛。
    pub min_score_pct: f64,
}

impl UnitInfo {
    /// 是否有分数门槛。
    pub fn has_score_gate(&self) -> bool {
        self.min_score_pct > 0.0
    }
}

/// 一个**分组**（`leafs`）的必修信息。
///
/// 这是提交目标的权威口径——见模块文档。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeafInfo {
    /// 分组 ID，如 `u5g341`。
    pub id: String,
    /// 该分组是否必修。
    pub required: bool,
    /// 及格门槛（`min_score_pct`）。`0` 表示无门槛。
    pub min_score_pct: f64,
}

impl LeafInfo {
    /// 是否有分数门槛。
    ///
    /// 实测：`required = true` 的 49 个里有 45 个 `min_score_pct > 0`，
    /// 差的 4 个正是没题目、没门槛的 `purecontent`。
    pub fn has_score_gate(&self) -> bool {
        self.min_score_pct > 0.0
    }
}

/// 整课进度快照。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub units: Vec<UnitInfo>,
    /// 分组级必修表，按分组 ID 排序（`BTreeMap` 保证顺序稳定）。
    pub leafs: BTreeMap<String, LeafInfo>,
}

impl Progress {
    /// 必修单元 ID 列表。
    pub fn required_units(&self) -> Vec<&str> {
        self.units
            .iter()
            .filter(|unit| unit.required)
            .map(|unit| unit.id.as_str())
            .collect()
    }

    /// 查某单元是否必修。**未读到该单元时返回 `None`**，与「已知非必修」不同。
    pub fn is_required(&self, unit_id: &str) -> Option<bool> {
        self.units
            .iter()
            .find(|unit| unit.id == unit_id)
            .map(|unit| unit.required)
    }

    /// **必修分组 ID 列表**（叶子口径，已排序去重）。
    ///
    /// 这就是提交目标集合。未读到 `leafs` 时返回空——调用方必须把空当成
    /// 「信息缺失、不许动」，不能当成「没有必修」。
    pub fn required_leafs(&self) -> Vec<&str> {
        self.leafs
            .values()
            .filter(|leaf| leaf.required)
            .map(|leaf| leaf.id.as_str())
            .collect()
    }

    /// 是否是「信息缺失」而不是「确实没有必修」。
    pub fn leafs_missing(&self) -> bool {
        self.leafs.is_empty()
    }
}

/// 从 `rt.units` 里解析单元信息。
fn parse_units(body: &Value) -> Vec<UnitInfo> {
    let mut out = Vec::new();

    // 单元表可能在 `rt.units` / `units` / `data.units`。
    let units = body
        .pointer("/rt/units")
        .or_else(|| body.get("units"))
        .or_else(|| body.pointer("/data/units"))
        .and_then(Value::as_object);

    let Some(units) = units else {
        return out;
    };

    for (id, value) in units {
        let required = value
            .pointer("/strategies/required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let min_score_pct = value
            .pointer("/strategies/min_score_pct")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        out.push(UnitInfo {
            id: id.clone(),
            required,
            min_score_pct,
        });
    }

    // 按单元编号排序，让输出稳定（`u10` 要排在 `u9` 之后）。
    out.sort_by_key(|unit| unit_number(&unit.id));
    out
}

/// 从 `u12` 里取数字 12，取不到给 `usize::MAX`（排到最后）。
fn unit_number(id: &str) -> usize {
    id.trim_start_matches(|ch: char| !ch.is_ascii_digit())
        .parse()
        .unwrap_or(usize::MAX)
}

/// 从 `rt.leafs` 里解析**分组级**必修。
///
/// 这是提交目标的权威口径（见模块文档）。字段缺失时降级为
/// 「非必修 + 无门槛」——但整张表缺失要能被调用方识别出来，
/// 所以返回空表时 [`Progress::leafs_missing`] 为真。
fn parse_leafs(body: &Value) -> BTreeMap<String, LeafInfo> {
    let mut out = BTreeMap::new();

    let leafs = body
        .pointer("/rt/leafs")
        .or_else(|| body.get("leafs"))
        .or_else(|| body.pointer("/data/leafs"))
        .and_then(Value::as_object);

    let Some(leafs) = leafs else {
        return out;
    };

    for (id, value) in leafs {
        // `required` 可能是 bool，也可能是 0/1 数字——两种都认，
        // 判错方向是「该交的不交」，比多交更贵。
        let required = match value.pointer("/strategies/required") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
            _ => false,
        };
        let min_score_pct = value
            .pointer("/strategies/min_score_pct")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        out.insert(
            id.clone(),
            LeafInfo {
                id: id.clone(),
                required,
                min_score_pct,
            },
        );
    }

    out
}

/// 读取单元级 + 分组级进度（**一次请求**）。
pub fn fetch(client: &Client, token: &str, course_id: &str, open_id: &str) -> Result<Progress> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let body = transport.get_json(&course_progress_url(course_id, open_id))?;
    Ok(Progress {
        units: parse_units(&body),
        leafs: parse_leafs(&body),
    })
}

/// 目录级批量进度（**只用来读 `leafs`**）。
///
/// 返回原始 JSON。⚠️ 它的 `state.pass` 已知不可信（见模块文档），
/// 但 `rt.leafs.<gid>.strategies.required` 是权威的——用
/// [`fetch_leafs`] 而不是直接啃这份原始 JSON。
pub fn fetch_group_progress_raw(
    client: &Client,
    token: &str,
    course_id: &str,
    open_id: &str,
    group_ids: &[String],
) -> Result<Value> {
    if group_ids.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let transport = Transport::new(client, Auth::Annotator(token));
    transport.get_json(&course_progress_tasks_url(
        course_id,
        open_id,
        &group_ids.join(","),
    ))
}

/// 读**分组级必修**（`leafs` 口径）——提交目标就是它。
///
/// ## 必须显式传全部分组 ID
///
/// `leafs` 只出现在 tasks 端点里（单元级端点没有，实测），而这个端点
/// **不给 ID 就返回空 `leafs` 且不报错**。所以要先把目录里的分组 ID 收集齐。
///
/// ## 实测对齐
///
/// 《基础篇综合教程2》（184 组）→ `required=true` **44** 个，
/// 与门户「教程学习成绩」分母 **44** 精确吻合。
pub fn fetch_leafs(
    client: &Client,
    token: &str,
    course_id: &str,
    open_id: &str,
    group_ids: &[String],
) -> Result<BTreeMap<String, LeafInfo>> {
    if group_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let body = fetch_group_progress_raw(client, token, course_id, open_id, group_ids)?;
    Ok(parse_leafs(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 从 `rt.units.<uN>.strategies` 读出必修与门槛。
    #[test]
    fn parses_required_and_gate() {
        let body = json!({
            "code": 0,
            "rt": {"units": {
                "u5": {"strategies": {"required": true, "min_score_pct": 0.6}},
                "u1": {"strategies": {"required": false, "min_score_pct": 0}},
            }}
        });
        let progress = Progress {
            units: parse_units(&body),
            leafs: parse_leafs(&body),
        };
        assert_eq!(progress.units.len(), 2);
        // 排序后 u1 在前。
        assert_eq!(progress.units[0].id, "u1");
        assert!(!progress.units[0].required);
        assert_eq!(progress.units[1].id, "u5");
        assert!(progress.units[1].required);
        assert_eq!(progress.units[1].min_score_pct, 0.6);
        assert!(progress.units[1].has_score_gate());
        assert!(!progress.units[0].has_score_gate());
    }

    /// 单元编号排序要正确：`u10` 排在 `u9` 之后，不是字典序。
    #[test]
    fn units_sort_numerically_not_lexically() {
        let body = json!({
            "code": 0,
            "rt": {"units": {
                "u10": {"strategies": {"required": false}},
                "u9": {"strategies": {"required": false}},
                "u2": {"strategies": {"required": false}},
            }}
        });
        let ids: Vec<String> = parse_units(&body).into_iter().map(|unit| unit.id).collect();
        assert_eq!(ids, vec!["u2", "u9", "u10"]);
    }

    /// 缺失 `strategies` 时降级为「非必修 + 无门槛」，不能 panic。
    #[test]
    fn missing_strategies_degrades_gracefully() {
        let body = json!({"code": 0, "rt": {"units": {"u1": {}}}});
        let units = parse_units(&body);
        assert_eq!(units.len(), 1);
        assert!(!units[0].required);
        assert_eq!(units[0].min_score_pct, 0.0);
    }

    /// ★ 未读到该单元时 `is_required` 返回 `None`，不能当成 false。
    #[test]
    fn unknown_unit_is_none_not_false() {
        let progress = Progress {
            units: vec![UnitInfo {
                id: "u1".to_owned(),
                required: true,
                min_score_pct: 0.0,
            }],
            leafs: BTreeMap::new(),
        };
        assert_eq!(progress.is_required("u1"), Some(true));
        assert_eq!(progress.is_required("u99"), None, "没读到 ≠ 非必修");
    }

    /// 必修单元列表只含必修的。
    #[test]
    fn required_units_filters() {
        let progress = Progress {
            units: vec![
                UnitInfo {
                    id: "u1".to_owned(),
                    required: false,
                    min_score_pct: 0.0,
                },
                UnitInfo {
                    id: "u5".to_owned(),
                    required: true,
                    min_score_pct: 0.6,
                },
            ],
            leafs: BTreeMap::new(),
        };
        assert_eq!(progress.required_units(), vec!["u5"]);
    }

    /// 没有 units 时返回空表，不 panic。
    #[test]
    fn missing_units_yields_empty() {
        assert!(parse_units(&json!({"code": 0})).is_empty());
    }

    /// ★ `leafs` 才是提交口径：解析出分组级必修与门槛。
    #[test]
    fn parses_leafs_required_and_gate() {
        let body = json!({
            "code": 0,
            "rt": {"leafs": {
                "u5g341": {"strategies": {"required": true, "min_score_pct": 0.6}},
                "u5g204": {"strategies": {"required": true, "min_score_pct": 0}},
                "u5g999": {"strategies": {"required": false, "min_score_pct": 0}},
            }}
        });
        let leafs = parse_leafs(&body);
        assert_eq!(leafs.len(), 3);
        assert!(leafs["u5g341"].required);
        assert!(leafs["u5g341"].has_score_gate());
        // ★ 没门槛但仍是必修——正是那 4 个 purecontent 的形状。
        assert!(leafs["u5g204"].required);
        assert!(!leafs["u5g204"].has_score_gate());
        assert!(!leafs["u5g999"].required);

        let progress = Progress {
            units: Vec::new(),
            leafs,
        };
        assert_eq!(progress.required_leafs(), vec!["u5g204", "u5g341"]);
        assert!(!progress.leafs_missing());
    }

    /// `required` 是 0/1 数字也要认——判错方向是「该交的不交」。
    #[test]
    fn leafs_required_accepts_numeric_flag() {
        let body = json!({
            "code": 0,
            "rt": {"leafs": {"u1g1": {"strategies": {"required": 1}},
                             "u1g2": {"strategies": {"required": 0}}}}
        });
        let leafs = parse_leafs(&body);
        assert!(leafs["u1g1"].required);
        assert!(!leafs["u1g2"].required);
    }

    /// 整张表缺失 ⇒ `leafs_missing()` 为真（调用方据此「一组都不交」）。
    #[test]
    fn missing_leafs_is_detectable() {
        let progress = Progress {
            units: Vec::new(),
            leafs: parse_leafs(&json!({"code": 0})),
        };
        assert!(progress.leafs_missing());
        assert!(progress.required_leafs().is_empty());
    }

    /// 一条请求同时给出两级：`units` 与 `leafs` 并行解析，互不影响。
    #[test]
    fn units_and_leafs_are_parsed_together() {
        let body = json!({
            "code": 0,
            "rt": {
                "units": {"u5": {"strategies": {"required": true}}},
                "leafs": {"u5g1": {"strategies": {"required": true}}},
            }
        });
        let progress = Progress {
            units: parse_units(&body),
            leafs: parse_leafs(&body),
        };
        assert_eq!(progress.required_units(), vec!["u5"]);
        assert_eq!(progress.required_leafs(), vec!["u5g1"]);
    }
}
