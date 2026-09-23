//! 分组状态与作答快照。
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/mobile/user_module/{inst}/{gid}/progress/v2` | **单条任务状态的真源** |
//! | `GET /api/mobile/user_module/{inst}/{gid}-{lastSubmit}` | 服务端存的**作答原文** |
//!
//! ## ★ `state.state == 1` 是账号级学习痕迹，**不是**班级完成度
//!
//! 这是本项目最重要的一条实测结论。三班《视听说2》的 **49 个必修分组全部
//! `state=1`**，但门户只算 **11/49**：
//!
//! 反证：纯内容页（`purecontent`）没有完成概念，也是 `state=1`；得 0 分的
//! `presentation` 也是 `state=1`；**同一个 `instanceId` 被多个班共用**
//! （《视听说2》二班 **35/35**、三班 **11/49** 是同一个实例），
//! 学生在二班做过的痕迹会串到三班的读数上。
//!
//! **所以本模块的 `passed()` 是宽松的「有痕迹」判定，绝不能用来数完成数。**
//! 数总量必须用 [`crate::api::course_list`]（它按班分别保留，不跨班合并）。
//!
//! ## 落库读法（比写法更容易错）
//!
//! 判断「做过没有」看 **`first_pass.datetime`** 是否为 `null`。
//!
//! ⚠️ `first_pass` 对象**永远存在**，只有里面的 `datetime` 能说明问题。
//! 而 `-review-version` 只存作答明细，**不反映是否通过**——用它会得出错误结论。
//!
//! ## 快照必须带 `lastSubmit`
//!
//! 少了它服务端回 `{"code":1,"data":null,"msg":"data not exist"}`。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{group_state_url, snapshot_url};
use crate::error::Result;
use crate::transport::{Auth, Transport};

/// 单条任务在服务端的痕迹。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupState {
    /// `first_pass.datetime`：首次通过时间戳；`None` 表示从未做过。
    pub first_pass_at: Option<i64>,
    /// `last_pass.lastSubmit`：最近一次提交时间戳。
    pub last_submit_at: Option<i64>,
    /// `state.state`：`1` 表示服务端留下了记录（**账号级痕迹**）。
    pub state: Option<i64>,
    /// `state.score` 原文，如 `[1,1,1,0]` 或 `[]`。
    pub score: String,
    /// `state.score_pct`，0~1。
    pub score_pct: String,
    /// `flowStrategy.required`：**任务级必修** = 班级计分点。
    pub flow_required: bool,
    /// `strategy.task_mini_score_pct`：及格门槛。
    ///
    /// 实测只有两个真实取值：**`0`（288 组）与 `0.6`（46 组）**。
    pub min_score_pct: String,
}

impl GroupState {
    /// 是否从未做过该分组。
    pub fn untouched(&self) -> bool {
        self.first_pass_at.is_none() && self.last_submit_at.is_none()
    }

    /// 服务端是否留下了记录。
    pub fn submitted(&self) -> bool {
        self.state == Some(1)
    }

    /// 逐子题得分里是否**含 0 或 1**（即服务端确实判过分）。
    ///
    /// 空数组 `[]` 表示没有判分题（口语、主观题、上传题）。
    pub fn has_judged(&self) -> bool {
        self.score.contains('0') || self.score.contains('1')
    }

    /// 得分率。
    pub fn score_ratio(&self) -> f64 {
        self.score_pct.trim().parse().unwrap_or(0.0)
    }

    /// 门槛。
    pub fn min_ratio(&self) -> f64 {
        self.min_score_pct.trim().parse().unwrap_or(0.0)
    }

    /// 是否达标。
    ///
    /// ## ★ 两条分支，`verdict()` 必须与之一致
    ///
    /// ① **有判分位且有门槛** → 按分数判：`score_pct >= min_score_pct`；
    /// ② 否则 → 按服务端痕迹判：`submitted`。
    ///
    /// ⚠️ 分支 ② **刻意宽松**：它只说明「账号在这道题上留过记录」，
    /// 不代表班级计分点拿到了。实测 49 个必修分组里：
    ///
    /// | 分支 | 组数 |
    /// | --- | --- |
    /// | ① 真按分数判 | **10** |
    /// | ② 有判分位但门槛为 0 → 落回痕迹 | 35 |
    /// | ② 无判分位 | 4 |
    ///
    /// 也就是说 **~70% 的必修任务，其"达标"是由宽松的痕迹分支决定的**。
    /// 这个函数**不能用来统计完成数**。
    pub fn passed(&self) -> bool {
        if self.has_judged() && self.min_ratio() > 0.0 {
            return self.score_ratio() >= self.min_ratio();
        }
        self.submitted() || self.score_ratio() >= 1.0
    }

    /// 面向用户的一句话结论。
    ///
    /// **必须与 [`Self::passed`] 的分支逐条对齐**——曾经这里只看
    /// `has_judged()` 就宣布「分数达标」，而 `passed()` 走的是痕迹分支，
    /// 于是打印出 `分数达标（1/0）` 这种自相矛盾的谎话。
    pub fn verdict(&self) -> String {
        if self.has_judged() && self.min_ratio() > 0.0 {
            let score = self.score_ratio();
            let min = self.min_ratio();
            return if score >= min {
                format!("分数达标（{score:.2}/{min:.2}）")
            } else {
                format!("分数未达标（{score:.2}/{min:.2}）")
            };
        }
        if self.has_judged() {
            return if self.passed() {
                format!("已完成（无门槛，得分率 {}）", self.score_ratio())
            } else {
                format!("未完成（无门槛，得分率 {}）", self.score_ratio())
            };
        }
        if self.passed() {
            "已完成".to_owned()
        } else {
            "未完成".to_owned()
        }
    }
}

/// 解析 `progress/v2` 响应。
pub fn parse(body: &Value) -> GroupState {
    let data = body.get("data").unwrap_or(body);
    let state = data.get("state");

    GroupState {
        first_pass_at: data.pointer("/first_pass/datetime").and_then(Value::as_i64),
        last_submit_at: state
            .and_then(|value| value.get("lastSubmit"))
            .and_then(Value::as_i64),
        state: state
            .and_then(|value| value.get("state"))
            .and_then(Value::as_i64),
        score: state
            .and_then(|value| value.get("score"))
            .map(text_of)
            .unwrap_or_default(),
        score_pct: state
            .and_then(|value| value.get("score_pct"))
            .map(text_of)
            .unwrap_or_default(),
        flow_required: data
            .pointer("/flowStrategy/required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        min_score_pct: data
            .pointer("/strategy/task_mini_score_pct")
            .map(text_of)
            .unwrap_or_default(),
    }
}

/// 把数字或字符串取成文本。
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// 读取分组状态。
pub fn fetch(client: &Client, token: &str, course_id: &str, group_id: &str) -> Result<GroupState> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let body = transport.get_json(&group_state_url(course_id, group_id))?;
    Ok(parse(&body))
}

/// 读取作答快照的**原始 JSON**（服务端存了什么就是什么）。
///
/// ⚠️ `last_submit` **必填**，通常取自 [`GroupState::last_submit_at`]。
pub fn fetch_snapshot(
    client: &Client,
    token: &str,
    course_id: &str,
    group_id: &str,
    last_submit: &str,
) -> Result<Value> {
    let transport = Transport::new(client, Auth::Annotator(token));
    transport.get_json(&snapshot_url(course_id, group_id, last_submit))
}

/// 从快照里抽出 `__SUBMIT_INFO__`。
///
/// 三个位置都要试——不同版本的服务端放在不同层级。
pub fn submit_info(snapshot: &Value) -> Option<&Value> {
    snapshot
        .pointer("/data/state/__EXTEND_DATA__/__SUBMIT_INFO__")
        .or_else(|| snapshot.pointer("/data/__EXTEND_DATA__/__SUBMIT_INFO__"))
        .or_else(|| snapshot.pointer("/__EXTEND_DATA__/__SUBMIT_INFO__"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(score: &str, pct: &str, gate: &str, state: i64) -> Value {
        json!({
            "code": 0,
            "data": {
                "flowStrategy": {"required": true},
                "strategy": {"task_mini_score_pct": gate},
                "state": {"state": state, "score": score, "score_pct": pct, "lastSubmit": 123},
                "first_pass": {"datetime": 100},
            }
        })
    }

    /// ★ 有判分位且有门槛 → 按分数判。
    #[test]
    fn branch_one_uses_score_when_gate_exists() {
        let state = parse(&body("[1,1,1]", "1", "0.6", 1));
        assert!(state.has_judged());
        assert_eq!(state.min_ratio(), 0.6);
        assert!(state.passed());
        assert!(state.verdict().contains("分数达标"), "{}", state.verdict());
    }

    /// 分数不够就不达标，即使 `state=1`。
    #[test]
    fn branch_one_fails_on_low_score() {
        let state = parse(&body("[0,1,0]", "0.33", "0.6", 1));
        assert!(!state.passed(), "分数没到门槛，不应算达标");
        assert!(
            state.verdict().contains("分数未达标"),
            "{}",
            state.verdict()
        );
    }

    /// ★ 门槛为 0 时落回痕迹分支——这是 70% 必修任务的实际情况。
    #[test]
    fn zero_gate_falls_back_to_trace() {
        let state = parse(&body("[1,1]", "1", "0", 1));
        assert!(state.has_judged(), "确实有判分位");
        assert_eq!(state.min_ratio(), 0.0, "但门槛是 0");
        assert!(state.passed(), "于是走痕迹分支");
        assert!(
            state.verdict().contains("无门槛"),
            "verdict 必须说明是痕迹分支，而不是分数达标：{}",
            state.verdict()
        );
    }

    /// ★★ 回归测试：`verdict` 绝不能与 `passed` 的分支矛盾。
    ///
    /// 这是曾经的 bug——`verdict()` 只看 `has_judged()` 就宣布「分数达标」，
    /// 而 `passed()` 走痕迹分支，于是打印出 `分数达标（1/0）`。
    #[test]
    fn verdict_never_contradicts_passed() {
        let scores = ["[]", "[1]", "[0]", "[1,1]", "[0,1]", "[1,0]"];
        let pcts = ["", "0", "0.5", "1"];
        let gates = ["", "0", "0.6"];
        let states = [0, 1];

        for score in scores {
            for pct in pcts {
                for gate in gates {
                    for state in states {
                        let parsed = parse(&body(score, pct, gate, state));
                        let verdict = parsed.verdict();
                        let passed = parsed.passed();

                        // 「分数达标」只能在真的按分数判、且确实达标时出现。
                        if verdict.contains("分数达标") {
                            assert!(
                                passed,
                                "verdict={verdict} 说达标，passed={passed} 说没达标（score={score} pct={pct} gate={gate}）"
                            );
                            assert!(
                                parsed.has_judged() && parsed.min_ratio() > 0.0,
                                "verdict={verdict} 却在痕迹分支上（score={score} gate={gate}）"
                            );
                        }
                        if verdict.contains("分数未达标") {
                            assert!(!passed, "verdict={verdict} 与 passed={passed} 矛盾");
                        }
                        // ★ 绝不能出现「门槛为 0」的分母。
                        assert!(
                            !verdict.contains("/0）"),
                            "verdict={verdict} 拿 0 当门槛分母（score={score} gate={gate}）"
                        );
                        // 「已完成/未完成」只能在痕迹分支上出现。
                        if verdict.starts_with("已完成") || verdict.starts_with("未完成") {
                            let judged_with_gate = parsed.has_judged() && parsed.min_ratio() > 0.0;
                            assert!(
                                !judged_with_gate,
                                "verdict={verdict} 在分数分支上却说痕迹话术"
                            );
                        }
                    }
                }
            }
        }
    }

    /// 无判分位（口语/主观题）走痕迹分支。
    #[test]
    fn no_judged_slot_uses_trace() {
        let state = parse(&body("[]", "0", "0.6", 1));
        assert!(!state.has_judged(), "空数组 = 没有判分题");
        assert!(state.passed(), "但有痕迹");
        assert_eq!(state.verdict(), "已完成");
    }

    /// 完全没做过 → 未完成。
    #[test]
    fn untouched_group_is_not_passed() {
        let body = json!({
            "code": 0,
            "data": {
                "state": {"score": "[]", "score_pct": "0"},
                "first_pass": {"datetime": null},
            }
        });
        let state = parse(&body);
        assert!(state.untouched());
        assert!(!state.passed());
        assert_eq!(state.verdict(), "未完成");
    }

    /// 有 score_pct=1 但 `state` 缺失时仍算达标（痕迹分支的 `>=1.0`）。
    #[test]
    fn full_score_pct_passes_without_state_flag() {
        let state = parse(&json!({
            "code": 0,
            "data": {"state": {"score": "[]", "score_pct": "1"}}
        }));
        assert!(state.passed());
    }

    /// 任务级必修从 `flowStrategy.required` 读，缺失降级为 false。
    #[test]
    fn flow_required_parses_and_degrades() {
        assert!(parse(&body("[]", "0", "0", 1)).flow_required);
        let missing = parse(&json!({"code": 0, "data": {}}));
        assert!(!missing.flow_required, "缺字段时降级为 false，不能 panic");
    }

    /// `first_pass` 对象存在但 `datetime` 为 null → 从未做过。
    #[test]
    fn first_pass_object_alone_means_nothing() {
        let state = parse(&json!({
            "code": 0,
            "data": {"state": {"lastSubmit": 5}, "first_pass": {"datetime": null}}
        }));
        assert_eq!(state.first_pass_at, None);
        assert!(!state.untouched(), "有 lastSubmit 就不算 untouched");
    }

    /// 快照里的 `__SUBMIT_INFO__` 三个位置都要能找到。
    #[test]
    fn submit_info_found_at_any_level() {
        let a = json!({"data": {"state": {"__EXTEND_DATA__": {"__SUBMIT_INFO__": {"x": 1}}}}});
        let b = json!({"data": {"__EXTEND_DATA__": {"__SUBMIT_INFO__": {"x": 2}}}});
        let c = json!({"__EXTEND_DATA__": {"__SUBMIT_INFO__": {"x": 3}}});
        assert_eq!(submit_info(&a).unwrap()["x"], json!(1));
        assert_eq!(submit_info(&b).unwrap()["x"], json!(2));
        assert_eq!(submit_info(&c).unwrap()["x"], json!(3));
        assert!(submit_info(&json!({})).is_none());
    }
}
