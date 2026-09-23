//! 必修筛选：把「目录 + 进度」合成**该提交哪几组**。
//!
//! ## 判据只有一个：`rt.leafs.<gid>.strategies.required`
//!
//! 提交目标是**当前班级的必修**，不是「这一组里有没有可判分题」。
//! 权威口径在 [`crate::api::progress`] 的一次请求里：
//!
//! ```text
//! 目录（334 组）
//!   └─ rt.leafs.<gid>.strategies.required     ← ★ 提交目标就是它（实测 49）
//!        └─ 排除已达标的（逐组 progress/v2）
//! ```
//!
//! ## ⚠️ 不要再拿 `flowStrategy.required` 当提交判据
//!
//! 旧实现是「单元级粗筛（148）→ 逐组扫 `flowStrategy.required`（45）」，
//! 两级下来有两个问题：
//!
//! 1. **漏 4 个**：`u5g204`/`u6g300`/`u7g301`/`u8g302` 是 `purecontent`
//!    （没题目、没分数门槛），单元级算必修、`flowStrategy` 不认，但门户
//!    的计分点分母里有它们；
//! 2. **白扫几百次**：那 45 是逐组发请求换来的，而 `leafs` 一次请求就给全。
//!
//! ## 门户侧的独立确证（2026-09，`resource-detail/20000000001`）
//!
//! 门户「综合成绩 → 教程学习成绩」直接印着每本教材的分母：
//!
//! ```text
//! 新一代大学英语（基础篇）视听说思政数字课程2   11/49   20   40%
//!                                          ↑ 分母就是 49 = leafs.required
//! ```
//!
//! 且 `11+44+13+49 = 117`、`11+6+0+11 = 28`，与同一行的
//! `finishProcess = "28/117 任务点"` 完全对上。
//!
//! ⚠️ `totalPointNum` 是**班级级**的：同一本《视听说2》挂在两个班里，
//! 一个 49（`strategyId=900001`）、一个 35（`strategyId=1001600`）。
//! 所以班级必须由调用方显式指定（CLI 的 `--class`），本模块只负责
//! 把「这门课的必修分组」筛出来。
//!
//! ## ⚠️ 降级规则：拿不到 `leafs` 就一组都不交
//!
//! 空表可能是「信息缺失」而不是「确实没有必修」。宁可不动，也不赌一把
//! 去提交非必修内容——见 [`TargetPlan::blocked_by_missing_required`]。

use reqwest::blocking::Client;
use std::collections::BTreeMap;

use crate::api::{catalog::Catalog, class_required::ClassScope, group_state, progress};
use crate::error::Result;
use crate::transport::RATE_LIMIT_BACKOFF;

/// 筛选出的提交目标。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetPlan {
    /// 必修单元 ID（仅用于展示与粗筛交叉验证）。
    pub required_units: Vec<String>,
    /// 待提交的分组 ID（`leafs.required`，已排除已达标的）。
    pub groups: Vec<String>,
    /// `leafs.required` 的**总数**（未排除达标前）。
    pub required_total: usize,
    /// 因已达标而被排除的分组数。
    pub skipped_passed: usize,
    /// 分组级状态**是否真的扫过**（决定 `skipped_passed` 是否可信）。
    ///
    /// `false` 表示没扫状态：**已达标的不排除**，宁可重复交也不漏交。
    pub state_scanned: bool,
    /// 本次为排除已达标而发出的请求数（供排查）。
    pub state_requests: usize,
    /// 必修集来自哪个口径。**必须显示给用户**：按班与账号级会给出不同的集合。
    pub source: String,
}

impl TargetPlan {
    /// 执行计划时预计要发的请求数（扫描 + 每组一次提交）。
    pub fn estimated_requests(&self) -> usize {
        self.groups.len() * 2
    }

    /// 是否因为拿不到 `leafs.required` 而放弃（而非「确实没有必修」）。
    pub fn blocked_by_missing_required(&self) -> bool {
        self.required_total == 0
    }

    /// 面向用户的一句话说明。
    pub fn describe(&self) -> String {
        if self.blocked_by_missing_required() {
            return "没读到 leafs.required，一个分组都不提交（拿不到必修口径时不动，避免误交非必修内容）"
                .to_owned();
        }
        let units = if self.required_units.is_empty() {
            "（未读到单元级标记）".to_owned()
        } else {
            self.required_units.join(",")
        };
        let source = if self.source.is_empty() {
            "必修口径未知"
        } else {
            self.source.as_str()
        };
        if !self.state_scanned {
            return format!(
                "必修单元 {units}；必修集（{source}）{} 组（**未扫分组状态，已达标的不排除**）",
                self.required_total
            );
        }
        format!(
            "必修单元 {units}；必修集（{source}）{} 组，已排除达标 {} 组，待提交 {} 组",
            self.required_total,
            self.skipped_passed,
            self.groups.len()
        )
    }
}

/// 逐组读 `progress/v2`，**只用来排除已达标的**。
///
/// 必修与否已由 `leafs` 决定，这里不再承担筛选职责——那件事曾经靠
/// `flowStrategy.required` 逐组判，既漏 4 个 `purecontent` 又白扫几百次。
///
/// `should_stop` 让长扫描可以中途取消——一本教材几百组要跑很久。
pub fn filter_passed(
    client: &Client,
    token: &str,
    course_id: &str,
    candidates: &[String],
    skip_passed: bool,
    should_stop: impl Fn() -> bool,
) -> Result<(Vec<String>, usize)> {
    let mut kept = Vec::new();
    let mut requests = 0usize;

    for group_id in candidates {
        if should_stop() {
            break;
        }
        // 不排除达标时，这一组的请求纯属浪费。
        if !skip_passed {
            kept.push(group_id.clone());
            continue;
        }
        requests += 1;

        let state = match group_state::fetch(client, token, course_id, group_id) {
            Ok(state) => state,
            // 限流不该让整轮白跑：退避后重试一次，仍失败就跳过这一组。
            Err(error) => {
                if is_rate_limit_error(&error) {
                    std::thread::sleep(RATE_LIMIT_BACKOFF);
                    requests += 1;
                    match group_state::fetch(client, token, course_id, group_id) {
                        Ok(state) => state,
                        Err(_) => continue,
                    }
                } else {
                    continue;
                }
            }
        };

        // 已确认达标的不重复提交（服务端多为「一组只留首次记录」）。
        if state.passed() {
            continue;
        }
        kept.push(group_id.clone());
    }

    Ok((kept, requests))
}

/// 判断错误是否为限流。
fn is_rate_limit_error(error: &crate::error::Error) -> bool {
    error.message().contains("600001") || error.message().contains("600002")
}

/// 筛选选项。
///
/// 收进一个结构体而不是铺成几个 bool 参数——调用点写
/// `plan(.., true, false, ..)` 时没人看得出哪个 true 是哪个。
#[derive(Clone, Copy, Debug)]
pub struct PlanOptions {
    /// 是否排除**已达标**的分组。
    ///
    /// ## ⚠️ 这是**账号级**判据，比门户的班级口径宽
    ///
    /// 排除用的是逐组 `progress/v2` 的 `passed()`，那是**账号级痕迹**
    /// （同一实例跨班共用）。2026-09 实测《基础篇综合教程2》三班：
    /// 本判据认为「已达标 42/44」，而门户「教程学习成绩」只认 **6/44**。
    /// 也就是说**默认会漏交**——想让门户分子真的涨，得用 `--redo` 全扫。
    ///
    /// 真正的班级级「已算分」集合目前**没有找到**：`leafs.<gid>.score.score`
    /// 同样是账号级的（视听说2 数出 39/49，门户 11/49；综合教程2 25/44，门户 6/44）。
    pub skip_passed: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { skip_passed: true }
    }
}

/// 完整的计划：`leafs.required` 圈定目标，再可选地排除已达标。
/// 一次筛选要吃到的输入。
///
/// 打包成结构体是因为参数已经 8 个了——调用点写
/// `plan(&client, &token, inst, open, &catalog, Some(&scope), .., ..)`
/// 时没人看得出哪个 `&str` 是哪个（clippy 也不让）。
pub struct PlanRequest<'a> {
    pub client: &'a Client,
    /// annotator token（打 ucontent）。
    pub token: &'a str,
    /// `course-v2:` 实例 ID。
    pub course_id: &'a str,
    pub open_id: &'a str,
    pub catalog: &'a Catalog,
    /// 按班口径的上下文；`None` 或不可用时退回账号级 `leafs`。
    pub scope: Option<&'a ClassScope>,
}

pub fn plan(
    request: PlanRequest<'_>,
    options: PlanOptions,
    should_stop: impl Fn() -> bool,
) -> Result<TargetPlan> {
    let PlanRequest {
        client,
        token,
        course_id,
        open_id,
        catalog,
        scope,
    } = request;
    let PlanOptions { skip_passed } = options;

    // ① 目录里的全部分组 ID（退回 `leafs` 时要用；`leafs` 不给 ID 就只回空表）。
    let mut all_ids: Vec<String> = catalog
        .units
        .iter()
        .flat_map(|unit| unit.groups.iter())
        .map(|group| group.id.clone())
        .collect();
    all_ids.sort();
    all_ids.dedup();

    // ② 单元级（只用于展示）。
    let snapshot = progress::fetch(client, token, course_id, open_id)?;
    let units: Vec<String> = snapshot
        .required_units()
        .iter()
        .map(|id| (*id).to_owned())
        .collect();

    // ③ 提交目标：**优先按班**。
    //
    //    同一本教材在不同班里的必修集可以**完全不相交**（实测《基础篇视听说2》：
    //    三班 49 个全在 u5–u8、二班 35 个全在 u1–u4，交集 0）。ucontent 的
    //    `leafs` 只按 `(实例, openId)` 记账，返回**某一个绑定**的集合，不随班级变，
    //    所以它时对时错——这就是以前那句「⚠️ 门户口径不一致」的根因。
    //
    //    按班的口径在门户：`chapters/unitTaskSituation?id=<courseResourceId>&nodeId=<单元>`。
    let (mut candidates, source) = match scope.filter(|scope| scope.is_usable()) {
        Some(scope) => {
            // ★ **一次请求**拿到逐单元必修集（附带 `passScore` 门槛）。
            let strategy = crate::api::class_required::fetch_strategy(client, scope)?;
            let ids = strategy.required_tasks();
            let gates: Vec<String> = strategy
                .units
                .iter()
                .filter(|unit| !unit.required_tasks.is_empty())
                .map(|unit| {
                    format!(
                        "{}×{}门{}",
                        unit.unit_id,
                        unit.required_tasks.len(),
                        unit.pass_score
                    )
                })
                .collect();
            (
                ids,
                format!(
                    "按班策略（{}；{}）",
                    if strategy.class_name.is_empty() {
                        "未知班级"
                    } else {
                        strategy.class_name.as_str()
                    },
                    gates.join(" ")
                ),
            )
        }
        None => {
            let leafs = progress::fetch_leafs(client, token, course_id, open_id, &all_ids)?;
            let ids: Vec<String> = leafs
                .values()
                .filter(|leaf| leaf.required)
                .map(|leaf| leaf.id.clone())
                .collect();
            (ids, "账号级 leafs（**不按班**，可能多列或漏列）".to_owned())
        }
    };
    candidates.sort();
    candidates.dedup();

    if candidates.is_empty() {
        // 降级规则：拿不到必修集就不动，别赌一把去交非必修内容。
        return Ok(TargetPlan {
            required_units: units,
            groups: Vec::new(),
            required_total: 0,
            skipped_passed: 0,
            state_scanned: false,
            state_requests: 0,
            source,
        });
    }

    // ④ 只用来排已达标；必修与否已由上面那个口径定死。
    let (groups, state_requests) = filter_passed(
        client,
        token,
        course_id,
        &candidates,
        skip_passed,
        should_stop,
    )?;
    let skipped_passed = candidates.len().saturating_sub(groups.len());

    Ok(TargetPlan {
        required_units: units,
        groups,
        required_total: candidates.len(),
        skipped_passed,
        state_scanned: skip_passed,
        state_requests,
        source,
    })
}

/// 按分组查「任务级必修」标记，供界面核对 `leafs` 口径。
///
/// ⚠️ 它返回的是 `flowStrategy.required`，**不是提交判据**——实测它比
/// `leafs.required` 少 4 个 `purecontent`。留在这里只为排查口径差异。
pub fn required_by_group(
    client: &Client,
    token: &str,
    course_id: &str,
    group_ids: &[String],
) -> Result<BTreeMap<String, bool>> {
    let mut out = BTreeMap::new();
    for group_id in group_ids {
        if let Ok(state) = group_state::fetch(client, token, course_id, group_id) {
            out.insert(group_id.clone(), state.flow_required);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::catalog::Group;

    fn catalog_with(units: &[(&str, &[&str])]) -> Catalog {
        Catalog {
            units: units
                .iter()
                .map(|(unit_id, groups)| crate::api::catalog::Unit {
                    id: (*unit_id).to_owned(),
                    name: String::new(),
                    groups: groups
                        .iter()
                        .map(|group_id| Group {
                            id: (*group_id).to_owned(),
                            base: "objectivesimple_course".to_owned(),
                            name: String::new(),
                        })
                        .collect(),
                })
                .collect(),
            resource_id: None,
        }
    }

    /// 目录查询仍然可用（`leafs` 里可能有目录外的 ID，需要靠它剔掉）。
    #[test]
    fn unit_candidates_respect_unit_filter() {
        let catalog = catalog_with(&[("u1", &["u1g1", "u1g2"]), ("u5", &["u5g1"])]);
        let picked = catalog.groups_in_units(&["u5"]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "u5g1");
    }

    /// ★ 降级规则：没有必修集 ⇒ 一组都不提交。
    #[test]
    fn missing_required_source_blocks_everything() {
        let plan = TargetPlan {
            required_units: Vec::new(),
            groups: Vec::new(),
            required_total: 0,
            skipped_passed: 0,
            state_scanned: false,
            state_requests: 0,
            source: String::new(),
        };
        assert!(plan.blocked_by_missing_required());
        assert!(plan.groups.is_empty());
        assert!(
            plan.describe().contains("一个分组都不提交"),
            "{}",
            plan.describe()
        );
    }

    /// ★ 没扫状态 ⇒ **已达标的不排除**，且必须自报这一点。
    #[test]
    fn unscanned_admits_it_does_not_filter_passed() {
        let plan = TargetPlan {
            required_units: vec!["u5".to_owned()],
            groups: vec!["u5g1".to_owned(), "u5g2".to_owned()],
            required_total: 2,
            skipped_passed: 0,
            state_scanned: false,
            state_requests: 0,
            source: "按班".to_owned(),
        };
        assert!(!plan.blocked_by_missing_required());
        let text = plan.describe();
        assert!(text.contains("已达标的不排除"), "必须自报：{text}");
    }

    /// ★ 扫过状态后要说清「必修多少 / 排除多少 / 待交多少」，**且要报出口径来源**。
    #[test]
    fn scanned_plan_reports_counts_and_source() {
        let plan = TargetPlan {
            required_units: vec!["u5".to_owned(), "u6".to_owned()],
            groups: vec!["u5g1".to_owned()],
            required_total: 49,
            skipped_passed: 48,
            state_scanned: true,
            state_requests: 49,
            source: "按班（门户 chapters/unitTaskSituation）".to_owned(),
        };
        let text = plan.describe();
        assert!(text.contains("u5,u6"), "{text}");
        assert!(text.contains("按班"), "必须自报必修口径：{text}");
        assert!(text.contains("49 组"), "{text}");
        assert!(text.contains("已排除达标 48 组"), "{text}");
        assert!(text.contains("待提交 1 组"), "{text}");
    }

    /// 请求数估算：每组要扫描一次、提交一次。
    #[test]
    fn estimated_requests_counts_both_phases() {
        let plan = TargetPlan {
            groups: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            ..Default::default()
        };
        assert_eq!(plan.estimated_requests(), 6);
    }

    /// 限流错误识别（用于决定要不要退避重试）。
    #[test]
    fn recognizes_rate_limit_errors() {
        assert!(is_rate_limit_error(&crate::error::Error::api(
            "请求失败 code=600002 操作过于频繁"
        )));
        assert!(is_rate_limit_error(&crate::error::Error::api("600001")));
        assert!(!is_rate_limit_error(&crate::error::Error::api(
            "接口返回 code=300100"
        )));
        assert!(!is_rate_limit_error(&crate::error::Error::network(
            "连接超时"
        )));
    }

    /// `should_stop` 立刻为真时不做任何请求。
    #[test]
    fn stop_flag_prevents_all_work() {
        // 直接验证 filter_passed 的短路：候选非空但立刻要求停止。
        // 用一个必然失败的 client 也不该被调用到——若被调用会 panic/报错，
        // 所以这里只要能返回空且 requests==0 就说明短路生效。
        let client = crate::transport::build_client().unwrap();
        let (kept, requests) = filter_passed(
            &client,
            "invalid-token-should-never-be-used",
            "course-v2:never",
            &["u5g1".to_owned()],
            true,
            || true,
        )
        .unwrap();
        assert!(kept.is_empty());
        assert_eq!(requests, 0, "要求停止后不该再发请求");
    }

    /// ★ 不排除达标时**连状态都不扫**——必修组全交，零个状态请求。
    #[test]
    fn skipping_passed_filter_makes_no_state_requests() {
        let client = crate::transport::build_client().unwrap();
        let candidates = vec!["u5g1".to_owned(), "u5g2".to_owned()];
        let (kept, requests) = filter_passed(
            &client,
            "invalid-token-should-never-be-used",
            "course-v2:never",
            &candidates,
            false,
            || false,
        )
        .unwrap();
        assert_eq!(kept, candidates, "全留着交");
        assert_eq!(requests, 0, "不排达标就不该发状态请求");
    }
}
