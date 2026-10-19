//! 学习记录：教材 / 单元 / 子任务三级的真实时长。
//!
//! ## 为什么需要这个模块
//!
//! course_progress 的 rt.leafs.<gid>.duration 恒为 0，实测挂了 9 分钟
//! 仍是 0 —— 那个字段不参与记账，不能当口径。
//!
//! 门户「教材学习 → 学习记录」页用的是 /api/tla/learningDetail/studyRecord/*：
//!
//! 端点  粒度  关键字段
//! [endpoints::PORTAL_STUDY_RECORD_TOTAL]  教材 + 单元  totalDetail.duration、unitList[].duration
//! [endpoints::PORTAL_STUDY_RECORD_UNIT]  单元内叶子分组  叶子上的 duration / required
//!
//! ## 实测口径闭包（2026-10-19，综合教程2）
//!
//! totalDetail.duration = 1084 秒 = 00:18:04   ← 与 grade 的教材时长一致
//! unitList: u5=3, u6=986, u7=95                ← 3+986+95 = 1084 ✅
//!
//! 子任务树形状为 unit → section → node → group，duration 只出现在
//! role == "group" 的叶子上，required 标记该叶子是否必修。
//! 所以「时长落到每个子单元」这句话，在数据上等价于：
//! 每个 role=group 且 required=true 的节点各自有非零 duration。
//!
//! ## 只读
//!
//! 两个端点都是 GET，不改服务端任何状态。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints;
use crate::error::{Error, Result};
use crate::transport::portal_ok;

/// 单元级时长。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitDuration {
    pub node_id: String,
    pub name: String,
    pub required: bool,
    pub seconds: u64,
}

/// 教材级汇总。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StudyRecord {
    /// 教材累计时长（秒）。等于各单元之和。
    pub total_seconds: u64,
    pub units: Vec<UnitDuration>,
}

/// 叶子分组（真正的任务节点）的时长。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDuration {
    pub node_id: String,
    pub name: String,
    /// 是否必修。非必修叶子不计入考核时长。
    pub required: bool,
    pub seconds: u64,
    pub tab_type: String,
    pub finish_progress: u64,
}

/// 拉「教材 + 单元」级时长汇总。
pub fn fetch_total(
    client: &Client,
    portal_token: &str,
    course_resource_id: &str,
    app_user_id: &str,
) -> Result<StudyRecord> {
    if course_resource_id.trim().is_empty() {
        return Err(Error::invalid("查询学习记录需要 courseResourceId"));
    }
    if app_user_id.trim().is_empty() {
        return Err(Error::invalid(
            "查询学习记录需要 appUserId（重新 `uai login` 获取会话）",
        ));
    }

    let url = endpoints::study_record_total_url(course_resource_id, app_user_id);
    let body = get_json(client, portal_token, &url, "学习记录汇总")?;
    let value = body
        .get("value")
        .ok_or_else(|| Error::parse("学习记录汇总缺少 value"))?;

    let total_seconds = value
        .pointer("/totalDetail/duration")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let units = value
        .get("unitList")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(unit_of).collect())
        .unwrap_or_default();

    Ok(StudyRecord {
        total_seconds,
        units,
    })
}

/// 拉某个单元内的全部叶子分组时长。unit_id 形如 u5。
pub fn fetch_unit_tasks(
    client: &Client,
    portal_token: &str,
    course_resource_id: &str,
    app_user_id: &str,
    unit_id: &str,
) -> Result<Vec<TaskDuration>> {
    if unit_id.trim().is_empty() {
        return Err(Error::invalid("查询单元明细需要 unitId（如 u5）"));
    }

    let url = endpoints::study_record_unit_url(course_resource_id, app_user_id, unit_id);
    let body = get_json(client, portal_token, &url, "学习记录单元明细")?;
    let value = body
        .get("value")
        .ok_or_else(|| Error::parse("学习记录单元明细缺少 value"))?;

    let mut tasks = Vec::new();
    if let Some(list) = value.get("list").and_then(Value::as_array) {
        for node in list {
            collect_tasks(node, &mut tasks);
        }
    }
    Ok(tasks)
}

/// 统一的 GET + 门户成功码判定。
fn get_json(client: &Client, portal_token: &str, url: &str, what: &str) -> Result<Value> {
    let body = client
        .get(url)
        .header("Authorization", portal_token)
        .send()
        .map_err(|error| Error::network(format!("读取{what}失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析{what}失败：{error}")))?;

    if !portal_ok(&body) {
        let message = body
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("接口返回失败");
        return Err(Error::api(format!("{what}：{message}")));
    }
    Ok(body)
}

fn unit_of(node: &Value) -> UnitDuration {
    UnitDuration {
        node_id: text(node, "nodeId"),
        name: text(node, "name"),
        required: node
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        seconds: node.get("duration").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// 递归收集 role == "group" 的叶子。
///
/// 树形是 unit → section → node → group，中间层没有 duration，
/// 只有叶子有。遇到 group 就收下并停止下钻。
fn collect_tasks(node: &Value, out: &mut Vec<TaskDuration>) {
    let role = node.get("role").and_then(Value::as_str).unwrap_or("");
    if role == "group" {
        out.push(TaskDuration {
            node_id: text(node, "nodeId"),
            name: text(node, "name"),
            required: node
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            seconds: node.get("duration").and_then(Value::as_u64).unwrap_or(0),
            tab_type: text(node, "tabType"),
            finish_progress: node
                .get("finishProgress")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
        return;
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_tasks(child, out);
        }
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ★ 叶子只在 role == "group" 上收，中间层的 duration 不参与。
    #[test]
    fn collects_only_group_leaves() {
        let tree = json!({
            "nodeId": "u5", "role": "unit", "duration": 999,
            "children": [
                {
                    "name": "iExplore 1", "role": "section",
                    "children": [
                        {
                            "nodeId": "u5g67", "role": "node",
                            "children": [
                                {
                                    "nodeId": "u5g70", "name": "Viewing", "role": "group",
                                    "required": true, "duration": 3,
                                    "finishProgress": 100, "tabType": "task"
                                },
                                {
                                    "nodeId": "u5g71", "name": "Reading", "role": "group",
                                    "required": false, "duration": 0,
                                    "finishProgress": 100, "tabType": "text"
                                }
                            ]
                        }
                    ]
                }
            ]
        });

        let mut out = Vec::new();
        collect_tasks(&tree, &mut out);
        assert_eq!(out.len(), 2, "只应收集两个 group 叶子");
        assert_eq!(out[0].node_id, "u5g70");
        assert_eq!(out[0].seconds, 3);
        assert!(out[0].required);
        assert!(!out[1].required);
        assert_eq!(out[1].seconds, 0);
    }

    /// 缺字段时给默认值，不 panic。
    #[test]
    fn tolerates_missing_fields() {
        let mut out = Vec::new();
        collect_tasks(&json!({"role": "group"}), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].seconds, 0);
        assert!(!out[0].required);
    }

    /// 单元汇总解析：total 与 unitList 都要取到。
    #[test]
    fn parses_unit_list() {
        let node = json!({
            "nodeId": "u6", "name": "To be or not to be",
            "required": true, "duration": 986
        });
        let unit = unit_of(&node);
        assert_eq!(unit.node_id, "u6");
        assert_eq!(unit.seconds, 986);
        assert!(unit.required);
    }
}
