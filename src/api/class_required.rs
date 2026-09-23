//! **按班**的必修集（门户侧）：`chapters/unitTaskSituation`。
//!
//! ## 为什么必须有它
//!
//! 同一本教材挂在不同班里，**必修点完全不同**——实测《基础篇视听说2》：
//!
//! | 绑定 | `courseResourceId` | required | 落在哪些单元 |
//! | --- | --- | --- | --- |
//! | 三班 | `20000000001` | **49** | 全在 u5–u8 |
//! | 二班 | `20000000005` | **35** | 全在 u1–u4 |
//!
//! 两者**交集为 0**。而 ucontent 的 `leafs` 端点只按 `(实例, openId)` 记账，
//! 返回的是**某一个绑定**的集合，不随班级变——所以之前会时而对得上、时而对不上
//! （`⚠️ 门户口径不一致` 的根因）。
//!
//! ## 端点与参数（实测）
//!
//! ```text
//! GET /api/tla/learningDetail/chapters/unitTaskSituation?id=<courseResourceId>&nodeId=<单元>
//! ```
//!
//! - **`id` 必须是按 (班,书) 绑定的 `courseResourceId`**（如 `20000000001`）。
//!   传 `course-v2:` 实例或数字 `courseId` 都不行（一个回空、一个 `value: null`）。
//! - 鉴权要**门户 JWT + 宿主头**（`Authorization` 之外还要
//!   `u-app-id/u-platform/u-school/u-openid`）。
//! - 返回大纲树 `value.list[].children[]` 递归；叶子 `role == "group"`
//!   的 `required` 就是本班口径。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{
    HOST_APP_ID_HEADER, HOST_OPENID_HEADER, HOST_PLATFORM_HEADER, HOST_SCHOOL_HEADER, portal_url,
};
use crate::error::{Error, Result};
use crate::transport::HostIdentity;

/// 按班取数的上下文。
#[derive(Clone, Debug)]
pub struct ClassScope {
    /// 门户 JWT。
    pub portal_token: String,
    /// 宿主身份（`u-school` 必须有值，否则服务端认不出）。
    pub host: HostIdentity,
    /// 该 (班,书) 绑定的「班级教材ID」。
    pub course_resource_id: i64,
    /// 课程资源 ID（`course-v2:unipus+…`）。
    pub resource_id: String,
    /// `course-v2:` 实例 ID。
    pub instance_id: String,
    /// 该班绑定的策略 ID。
    pub strategy_id: i64,
    /// 门户 `value.courseId`（如 `34852`，等于任务页的 `aitutorialId`）。
    pub course_id: String,
}

impl ClassScope {
    /// 是否能打**按班策略**接口（五项 + 宿主头 + 令牌）。
    pub fn is_usable(&self) -> bool {
        self.course_resource_id > 0
            && self.strategy_id > 0
            && !self.resource_id.trim().is_empty()
            && !self.instance_id.trim().is_empty()
            && !self.course_id.trim().is_empty()
            && self.host.is_complete()
            && !self.portal_token.trim().is_empty()
    }

    /// 是否能打**逐单元任务树**接口（只要绑定的资源 ID）。
    pub fn can_read_tree(&self) -> bool {
        self.course_resource_id > 0
            && self.host.is_complete()
            && !self.portal_token.trim().is_empty()
    }
}

/// 按班策略端点：**一次请求就能拿到逐单元必修集与门槛**。
pub const STRATEGY_DETAIL_PATH: &str = "/api/tla/courseStudyStrategy/detail";

/// 一个单元的班级策略。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitStrategy {
    pub unit_id: String,
    pub unit_name: String,
    /// 及格门槛（百分制，实测必修单元是 `60`）。`0` = 无门槛。
    pub pass_score: i64,
    /// 本单元的必修任务 ID。
    pub required_tasks: Vec<String>,
}

/// 一个 (班,书) 绑定上的完整策略。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassStrategy {
    pub class_name: String,
    pub term: String,
    pub units: Vec<UnitStrategy>,
}

impl ClassStrategy {
    /// 全部必修任务（按单元顺序拼接）。
    pub fn required_tasks(&self) -> Vec<String> {
        self.units
            .iter()
            .flat_map(|unit| unit.required_tasks.iter().cloned())
            .collect()
    }

    /// 必修任务总数。
    pub fn required_total(&self) -> usize {
        self.units
            .iter()
            .map(|unit| unit.required_tasks.len())
            .sum()
    }
}

/// **按班**取策略详情：一次请求给出逐单元必修集与 `passScore`。
///
/// 实测《基础篇视听说2》：三班 `u5:12 + u6:12 + u7:12 + u8:13 = 49`（门户分母 49），
/// 二班则是 `u1–u4` 的另一套 35 个——两个班**交集为 0**。
pub fn fetch_strategy(client: &Client, scope: &ClassScope) -> Result<ClassStrategy> {
    if !scope.is_usable() {
        return Err(Error::invalid(
            "按班策略需要 (courseResourceId, strategyId, resourceId, courseInstanceId, courseId) \
             五项俱全，外加宿主头与门户令牌",
        ));
    }
    // ⚠️ 必填字段名是 **`id`**，不是 `strategyId`——实测传 `strategyId` 会回
    //    「策略ID不能为空」（Spring 的 NotNull 校验在 `id` 上）。
    let body = serde_json::json!({
        "id": scope.strategy_id,
        "strategyId": scope.strategy_id,
        "courseResourceId": scope.course_resource_id,
        "courseId": scope.course_id,
        "resourceId": scope.resource_id,
        "courseInstanceId": scope.instance_id,
    });
    let response = client
        .post(portal_url(STRATEGY_DETAIL_PATH))
        .header("Authorization", &scope.portal_token)
        .header(HOST_APP_ID_HEADER, &scope.host.app_id)
        .header(HOST_PLATFORM_HEADER, &scope.host.platform)
        .header(HOST_SCHOOL_HEADER, &scope.host.school)
        .header(HOST_OPENID_HEADER, &scope.host.open_id)
        .json(&body)
        .send()
        .map_err(|error| Error::network(format!("读取按班策略请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("按班策略响应不是 JSON：{error}")))?;

    let code = response.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 1 {
        return Err(Error::api(format!(
            "按班策略返回 code={code}：{}",
            crate::transport::body_message(&response, "无消息")
        )));
    }

    let value = response.get("value").cloned().unwrap_or(Value::Null);
    let info = value.get("courseInfo").cloned().unwrap_or(Value::Null);
    let mut strategy = ClassStrategy {
        class_name: info
            .get("curriculaName")
            .or_else(|| info.get("courseName"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        term: info
            .get("term")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        units: Vec::new(),
    };

    if let Some(list) = value
        .get("courseUnitStrategyList")
        .and_then(Value::as_array)
    {
        for node in list {
            let required_tasks: Vec<String> = node
                .get("requiredTask")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            strategy.units.push(UnitStrategy {
                unit_id: node
                    .get("unitId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                unit_name: node
                    .get("unitName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                // `passScore` 是**字符串**（实测 `"60"`），数字也认。
                pass_score: node
                    .get("passScore")
                    .and_then(Value::as_str)
                    .and_then(|text| text.parse().ok())
                    .or_else(|| node.get("passScore").and_then(Value::as_i64))
                    .unwrap_or(0),
                required_tasks,
            });
        }
    }
    Ok(strategy)
}

/// 任务树端点。
pub const UNIT_TASK_PATH: &str = "/api/tla/learningDetail/chapters/unitTaskSituation";

/// 取某单元的**本班必修分组 ID**。
pub fn fetch_unit(client: &Client, scope: &ClassScope, unit: &str) -> Result<Vec<String>> {
    if !scope.host.is_complete() {
        return Err(Error::invalid(
            "按班取必修需要宿主头（u-school 等）；请重新 `login` 以获取 school",
        ));
    }
    let url = portal_url(UNIT_TASK_PATH);
    let body = client
        .get(&url)
        .query(&[
            ("id", scope.course_resource_id.to_string()),
            ("nodeId", unit.to_owned()),
        ])
        .header("Authorization", &scope.portal_token)
        .header(HOST_APP_ID_HEADER, &scope.host.app_id)
        .header(HOST_PLATFORM_HEADER, &scope.host.platform)
        .header(HOST_SCHOOL_HEADER, &scope.host.school)
        .header(HOST_OPENID_HEADER, &scope.host.open_id)
        .send()
        .map_err(|error| Error::network(format!("读取本班任务树请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("本班任务树响应不是 JSON：{error}")))?;

    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 1 {
        return Err(Error::api(format!(
            "本班任务树返回 code={code}：{}",
            crate::transport::body_message(&body, "无消息")
        )));
    }

    let mut out = Vec::new();
    if let Some(list) = body.pointer("/value/list").and_then(Value::as_array) {
        for node in list {
            collect_required(node, &mut out);
        }
    }
    Ok(out)
}

/// 逐个单元取（按单元顺序拼接、去重）。
///
/// 单个单元失败不毁掉整轮——记下最后一条错误，其它单元照收；全空才报错。
pub fn fetch_units(client: &Client, scope: &ClassScope, units: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut last_error: Option<Error> = None;
    for unit in units {
        match fetch_unit(client, scope, unit) {
            Ok(mut ids) => out.append(&mut ids),
            Err(error) => last_error = Some(error),
        }
    }
    if out.is_empty()
        && let Some(error) = last_error
    {
        return Err(error);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|id| seen.insert(id.clone()));
    Ok(out)
}

/// 递归收集 `role == "group"` 且未被标为非必修的节点。
fn collect_required(node: &Value, out: &mut Vec<String>) {
    let role = node.get("role").and_then(Value::as_str).unwrap_or("");
    let required = node
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if role == "group"
        && required
        && let Some(id) = node.get("nodeId").and_then(Value::as_str)
        && !id.is_empty()
    {
        out.push(id.to_owned());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_required(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 只收必修的 group 叶子；`required` 缺失按必修处理（宁多勿漏）。
    #[test]
    fn collects_required_groups_only() {
        let tree = json!({
            "nodeId": "u5", "role": "unit", "required": true,
            "children": [{
                "nodeId": "u5g93", "role": "node", "required": true,
                "children": [
                    {"nodeId": "u5g205", "role": "group", "required": true},
                    {"nodeId": "u5g999", "role": "group", "required": false},
                    {"nodeId": "u5g998", "role": "group"},
                ]
            }]
        });
        let mut out = Vec::new();
        collect_required(&tree, &mut out);
        assert_eq!(out, vec!["u5g205", "u5g998"]);
    }

    /// 单元节点本身不会被当成分组。
    #[test]
    fn unit_node_is_not_a_group() {
        let tree = json!({"nodeId": "u5", "role": "unit", "required": true});
        let mut out = Vec::new();
        collect_required(&tree, &mut out);
        assert!(out.is_empty());
    }

    /// 身份不完整时判为不可用（免得白打一轮拿到 6011/空表）。
    #[test]
    fn incomplete_scope_is_not_usable() {
        let scope = ClassScope {
            portal_token: "t".to_owned(),
            host: HostIdentity::default(),
            course_resource_id: 1,
            resource_id: "course-v2:x".to_owned(),
            instance_id: "course-v2:y".to_owned(),
            strategy_id: 7,
            course_id: "34852".to_owned(),
        };
        // 缺 `u-school` 等宿主头 ⇒ 不可用。
        assert!(!scope.is_usable());
        assert!(!scope.can_read_tree());
    }

    /// 五项齐全 + 宿主头完整才算可用。
    #[test]
    fn complete_scope_is_usable() {
        let scope = ClassScope {
            portal_token: "t".to_owned(),
            host: HostIdentity::new("1000", "open"),
            course_resource_id: 20000000001,
            resource_id: "course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31".to_owned(),
            instance_id: "course-v2:example0a2+ngce_vl_jc2_sz_ucloud+2024_01_31".to_owned(),
            strategy_id: 900001,
            course_id: "34852".to_owned(),
        };
        assert!(scope.is_usable());
        // 缺 courseId 就只剩爬树那条路。
        let no_course = ClassScope {
            course_id: String::new(),
            ..scope.clone()
        };
        assert!(!no_course.is_usable());
        assert!(no_course.can_read_tree());
    }

    /// `passScore` 是字符串（实测 `"60"`）也要解析出来。
    #[test]
    fn unit_strategy_totals() {
        let strategy = ClassStrategy {
            class_name: "大学英语I（三）A班".to_owned(),
            term: String::new(),
            units: vec![
                UnitStrategy {
                    unit_id: "u5".to_owned(),
                    unit_name: String::new(),
                    pass_score: 60,
                    required_tasks: vec!["u5g205".to_owned(), "u5g206".to_owned()],
                },
                UnitStrategy {
                    unit_id: "u6".to_owned(),
                    unit_name: String::new(),
                    pass_score: 60,
                    required_tasks: vec!["u6g300".to_owned()],
                },
            ],
        };
        assert_eq!(strategy.required_total(), 3);
        assert_eq!(
            strategy.required_tasks(),
            vec!["u5g205", "u5g206", "u6g300"]
        );
    }
}
