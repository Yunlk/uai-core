//! 课程目录：`GET /course/api/course/{courseId}/{mode}`。
//!
//! ## 两种 ID 都接受，但含义不同
//!
//! `course-v1:`（资源 ID）与 `course-v2:`（实例 ID）都能返回真实目录，
//! 但**只有实例 ID 同时能用于进度/提交接口**，所以调用方统一传实例 ID 更省事。
//!
//! ## 响应结构
//!
//! `course` 字段是**字符串化 JSON**，需要二次解析。层级是
//! `unit` → `node`（可嵌套）→ `group`。
//!
//! ⚠️ **只有 `role == "group"` 的节点才是真分组。** 不在目录里的 ID
//! （如 `u8g357`）是虚构的，内容为空、progress 返回空骨架。
//!
//! ## ★ 目录里**没有**必修标记 —— 别在这里找
//!
//! 实测一个分组节点的全部字段只有：
//!
//! ```text
//! base / name / ref-out / role / tab_type / url
//! ```
//!
//! **没有 `required`，也没有 `flowStrategy`。** 我一度在这里读
//! `flowStrategy.required`，于是所有分组都判成非必修，`--required`
//! 筛出 0 个（实测 334 个分组 → 0 必修，而真实必修是 49 个）。
//!
//! 必修标记在另外两个接口，且是**两级**的：
//!
//! | 级别 | 来源 | 成本 |
//! | --- | --- | --- |
//! | 单元级 | [`crate::api::progress`] 的 `rt.units.<uN>.strategies.required` | 1 次请求 |
//! | **任务级** | [`crate::api::group_state`] 的 `data.flowStrategy.required` | **每组 1 次请求** |
//!
//! 两级都要用：单元级先粗筛（本文件提供 [`Catalog::groups_in_units`]），
//! 任务级再精筛（见 [`crate::api::targets`]）。任务级才是班级计分点。
//!
//! ## `base` 是逗号分隔的多值列表
//!
//! 实测 9 种，例如 `short_answer,short_answer,multiFileUpload`、
//! `oral-personal-state,oral-personal-state`。用 `==` 全等比较会**整类漏掉**，
//! 必须拆开按标签集合匹配（见 [`Group::base_tags`]）。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::catalog_url;
use crate::error::{Error, Result};
use crate::transport::{Auth, Transport};

/// 一个分组（一次可提交的活动）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// 分组 ID，如 `u1g2`。
    pub id: String,
    /// 活动类型标签原文，**可能是逗号分隔的多值**。
    pub base: String,
    /// 展示名。
    pub name: String,
}

impl Group {
    /// 把 `base` 拆成标签集合。
    ///
    /// `base` 可能是多值列表（实测 9 种），例如
    /// `short_answer,short_answer,multiFileUpload`。
    /// 用 `contains()` 判断会误匹配（`sequence` 是 `sequence_course` 的子串），
    /// 所以拆开后**按元素精确比较**。
    pub fn base_tags(&self) -> Vec<&str> {
        self.base
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .collect()
    }

    /// 是否纯内容页（没有完成概念，应当过滤掉）。
    ///
    /// 判据：**所有**标签都是 `purecontent`。
    pub fn is_content_only(&self) -> bool {
        let tags = self.base_tags();
        !tags.is_empty() && tags.iter().all(|tag| *tag == "purecontent")
    }

    /// 是否客观题类（界面标注用）。
    ///
    /// 比「可提交」窄：只认 5 个明确的客观题 base。
    pub fn is_objective(&self) -> bool {
        self.base_tags().iter().any(|tag| {
            matches!(
                *tag,
                "objectivesimple_course"
                    | "objectivesimple_scoop_course"
                    | "objectivesimpleselect_course"
                    | "objectivesone_course"
                    | "sequence_course"
            )
        })
    }

    /// 是否可以作为提交目标。
    ///
    /// **排除法**，不是白名单：只要不是纯内容页就能交。
    /// 用白名单会漏掉教师自建活动（`magic_19_*`）和未见过的新 base。
    pub fn is_submittable(&self) -> bool {
        !self.is_content_only()
    }
}

/// 一个单元。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unit {
    pub id: String,
    pub name: String,
    pub groups: Vec<Group>,
}

/// 教材目录。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub units: Vec<Unit>,
    /// 目录里含有的 `course-v1:` 资源 ID（用于回填书架缺失的资源 ID）。
    pub resource_id: Option<String>,
}

impl Catalog {
    /// 分组总数。
    pub fn group_count(&self) -> usize {
        self.units.iter().map(|unit| unit.groups.len()).sum()
    }

    /// 指定单元集合下的可提交分组。
    ///
    /// 这是必修筛选的**第一级**（单元级粗筛）。拿 [`crate::api::progress`]
    /// 的 `required_units()` 传进来即可。真正的任务级精筛在
    /// [`crate::api::targets`]——它需要逐组读 `progress/v2`。
    pub fn groups_in_units<'a>(&'a self, unit_ids: &[&str]) -> Vec<&'a Group> {
        self.units
            .iter()
            .filter(|unit| unit_ids.contains(&unit.id.as_str()))
            .flat_map(|unit| &unit.groups)
            .filter(|group| group.is_submittable())
            .collect()
    }

    /// 摊平全部可提交的分组。
    pub fn submittable_groups(&self) -> Vec<&Group> {
        self.units
            .iter()
            .flat_map(|unit| &unit.groups)
            .filter(|group| group.is_submittable())
            .collect()
    }
}

/// 读取目录。
pub fn fetch(client: &Client, token: &str, course_id: &str) -> Result<Catalog> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let body = transport.get_json(&catalog_url(course_id))?;
    parse(&body)
}

/// 解析目录响应。
pub fn parse(body: &Value) -> Result<Catalog> {
    // `course` 字段是字符串化 JSON。
    let course: Value = match body.get("course") {
        Some(Value::String(text)) => serde_json::from_str(text)
            .map_err(|error| Error::parse(format!("解析 course 字段失败：{error}")))?,
        Some(value) => value.clone(),
        None => body.clone(),
    };

    let mut units: Vec<Unit> = Vec::new();
    collect_units(&course, &mut units);

    // 收集资源 ID。
    let resource_id = find_course_v1(body).or_else(|| find_course_v1(&course));

    Ok(Catalog { units, resource_id })
}

/// 递归找出单元节点。
fn collect_units(value: &Value, out: &mut Vec<Unit>) {
    match value {
        Value::Object(map) => {
            let role = map.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "unit" {
                let id = map
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = map
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let mut groups = Vec::new();
                collect_groups(value, &mut groups);
                // 同一单元 ID 只收一次（目录里可能重复引用）。
                if let Some(existing) = out.iter_mut().find(|unit| !id.is_empty() && unit.id == id)
                {
                    existing.groups.extend(groups);
                } else {
                    out.push(Unit { id, name, groups });
                }
            }
            for child in map.values() {
                collect_units(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_units(child, out);
            }
        }
        _ => {}
    }
}

/// 递归找 `role == "group"` 的节点。
fn collect_groups(value: &Value, out: &mut Vec<Group>) {
    match value {
        Value::Object(map) => {
            let role = map.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "group" {
                let id = map
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !id.is_empty() && !out.iter().any(|group| group.id == id) {
                    // 只取目录里真实存在的字段。这里**刻意不读 required**：
                    // 目录响应里根本没有它（见模块文档）。
                    out.push(Group {
                        id,
                        base: map
                            .get("base")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        name: map
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    });
                }
            }
            for child in map.values() {
                collect_groups(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_groups(child, out);
            }
        }
        _ => {}
    }
}

/// 在响应里找 `course-v1:` 资源 ID。
///
/// 书架不保证提供资源 ID（有时只在封面 `imgUrl` 里），
/// 目录响应是**可靠的回填来源**。
pub fn find_course_v1(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => extract_course_v1(text),
        Value::Object(map) => map.values().find_map(find_course_v1),
        Value::Array(items) => items.iter().find_map(find_course_v1),
        _ => None,
    }
}

/// 从一段文本里用正则式扫描 `course-v1:`。
fn extract_course_v1(text: &str) -> Option<String> {
    let start = text.find("course-v1:")?;
    let rest = &text[start..];
    // 资源 ID 由字母数字、`+`、`_`、`-`、`.` 组成；遇到分隔符就停。
    let end = rest
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '_' | '-' | '.' | ':')))
        .unwrap_or(rest.len());
    let candidate = &rest[..end];
    // 去掉尾部可能混入的引号或斜杠。
    let cleaned = candidate.trim_end_matches(['"', '\'', '\\', '/']);
    if cleaned.len() > "course-v1:".len() {
        Some(cleaned.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(base: &str) -> Group {
        Group {
            id: "u1g1".to_owned(),
            base: base.to_owned(),
            name: String::new(),
        }
    }

    /// ★ 多值 `base` 必须拆开，不能整体比较。
    #[test]
    fn base_tags_splits_comma_list() {
        let multi = group("short_answer,short_answer,multiFileUpload");
        assert_eq!(
            multi.base_tags(),
            vec!["short_answer", "short_answer", "multiFileUpload"]
        );
    }

    /// ★ 多值 base 也要能被识别为可提交（旧代码用 == 比较会整类漏掉）。
    #[test]
    fn multi_value_base_is_submittable() {
        let multi = group("basic-scoop-content,short_answer,writing,short_answer,multiFileUpload");
        assert!(multi.is_submittable());
        assert_eq!(multi.base_tags().len(), 5);
    }

    /// ★ 可提交是**排除法**：未知 base 也算可提交。
    #[test]
    fn submittable_is_exclusion_not_whitelist() {
        assert!(group("magic_19_whatever").is_submittable());
        assert!(group("从未见过的类型").is_submittable());
        assert!(!group("purecontent").is_submittable());
    }

    /// 纯内容页要求**所有**标签都是 purecontent。
    #[test]
    fn content_only_requires_all_tags_pure() {
        assert!(group("purecontent").is_content_only());
        assert!(
            !group("purecontent,short_answer").is_content_only(),
            "只要有一个非纯内容标签，就不是纯内容页"
        );
        assert!(!group("").is_content_only(), "空 base 不算纯内容页");
    }

    /// `sequence` 不能误匹配 `sequence_course`（这正是用 contains 的坑）。
    #[test]
    fn sequence_does_not_falsely_match_sequence_course() {
        assert!(group("sequence_course").is_objective());
        assert!(
            !group("sequence_other").is_objective(),
            "按元素精确比较，不能子串匹配"
        );
    }

    /// 客观题白名单的 5 个 base。
    #[test]
    fn objective_recognizes_all_five_bases() {
        for base in [
            "objectivesimple_course",
            "objectivesimple_scoop_course",
            "objectivesimpleselect_course",
            "objectivesone_course",
            "sequence_course",
        ] {
            assert!(group(base).is_objective(), "{base}");
        }
        assert!(!group("presentation").is_objective());
    }

    /// 从目录响应解析单元与分组（**单元名也要读到**）。
    #[test]
    fn parses_units_and_groups() {
        let body = json!({
            "code": 0,
            "course": json!({
                "role": "course",
                "children": [{
                    "role": "unit",
                    "url": "u5",
                    "name": "第五单元",
                    "children": [
                        {"role": "group", "url": "u5g341", "base": "objectivesimple_course",
                         "name": "听力"},
                        {"role": "group", "url": "u5g342", "base": "purecontent",
                         "name": "阅读"},
                    ]
                }]
            }).to_string(),
        });
        let catalog = parse(&body).unwrap();
        assert_eq!(catalog.units.len(), 1);
        assert_eq!(catalog.units[0].id, "u5");
        assert_eq!(catalog.units[0].name, "第五单元");
        assert_eq!(catalog.units[0].groups.len(), 2);
        assert_eq!(catalog.group_count(), 2);
        assert_eq!(catalog.submittable_groups().len(), 1, "纯内容页不算可提交");
    }

    /// ★★ 回归测试：目录节点里**没有** `required` 字段。
    ///
    /// 这是真实事故：旧版 `catalog.rs` 从 `flowStrategy.required` 读必修，
    /// 而目录响应里根本没有这个字段，于是 334 个分组全被判成非必修，
    /// `--required` 筛出 0 个。必修其实在 `progress`（单元级）与
    /// `progress/v2`（任务级），不在目录里。
    ///
    /// 用一个真实形状的节点（实测字段：base/name/ref-out/role/tab_type/url）
    /// 验证解析不会凭空造出必修标记。
    #[test]
    fn catalog_nodes_have_no_required_field() {
        let body = json!({
            "code": 0,
            "course": {
                "role": "unit", "url": "u5", "name": "第五单元",
                "children": [{
                    "role": "group", "url": "u5g205",
                    "base": "objectivesimple_course", "name": "听力",
                    "ref-out": Some(0), "tab_type": "task",
                }]
            }
        });
        let catalog = parse(&body).unwrap();
        let found = &catalog.units[0].groups[0];
        assert_eq!(found.id, "u5g205");
        assert_eq!(found.base, "objectivesimple_course");
        // 单元级粗筛能拿到它（第一级）。
        assert_eq!(catalog.groups_in_units(&["u5"]).len(), 1);
        // 不在必修单元里就筛不到。
        assert!(catalog.groups_in_units(&["u1"]).is_empty());
        assert!(
            catalog.groups_in_units(&["u5"])[0].is_submittable(),
            "可提交性只由 base 决定，与必修无关"
        );
    }

    /// 嵌套 node 里的 group 也要能找到。
    #[test]
    fn finds_groups_nested_under_nodes() {
        let body = json!({
            "code": 0,
            "course": {
                "role": "unit", "url": "u1",
                "children": [{
                    "role": "node",
                    "children": [{
                        "role": "node",
                        "children": [{"role": "group", "url": "u1g9", "base": "x"}]
                    }]
                }]
            }
        });
        let catalog = parse(&body).unwrap();
        assert_eq!(catalog.group_count(), 1);
        assert_eq!(catalog.units[0].groups[0].id, "u1g9");
    }

    /// 非 group 的 role 不能被当成分组。
    #[test]
    fn non_group_roles_are_ignored() {
        let body = json!({
            "code": 0,
            "course": {
                "role": "unit", "url": "u1",
                "children": [
                    {"role": "node", "url": "u1n1"},
                    {"role": "question", "url": "u1q1"},
                ]
            }
        });
        let catalog = parse(&body).unwrap();
        assert_eq!(catalog.group_count(), 0);
    }

    /// 重复的分组 ID 只收一次。
    #[test]
    fn duplicate_group_id_is_deduped() {
        let body = json!({
            "code": 0,
            "course": {
                "role": "unit", "url": "u1",
                "children": [
                    {"role": "group", "url": "u1g1", "base": "a"},
                    {"role": "group", "url": "u1g1", "base": "a"},
                ]
            }
        });
        assert_eq!(parse(&body).unwrap().group_count(), 1);
    }

    /// ★ 资源 ID 能从目录响应里回收。
    #[test]
    fn recovers_resource_id_from_catalog() {
        let body = json!({
            "code": 0,
            "course": {"role": "course", "resourceId": "course-v1:unipus+ngce+2024"},
        });
        let catalog = parse(&body).unwrap();
        assert_eq!(
            catalog.resource_id.as_deref(),
            Some("course-v1:unipus+ngce+2024")
        );
    }

    /// 资源 ID 嵌在 URL 里也能抠出来。
    #[test]
    fn recovers_resource_id_from_url() {
        let found = find_course_v1(&json!({
            "imgUrl": "https://x.com/img/course-v1:unipus+ngce+2024/cover.png"
        }));
        assert_eq!(found.as_deref(), Some("course-v1:unipus+ngce+2024"));
    }

    /// 没有资源 ID 时返回 None。
    #[test]
    fn missing_resource_id_is_none() {
        assert!(find_course_v1(&json!({"a": "no id here"})).is_none());
        assert!(find_course_v1(&json!({"a": "course-v1:"})).is_none());
    }

    /// 非法 course 字符串要报可读错误。
    #[test]
    fn invalid_course_string_reports_error() {
        let body = json!({"code": 0, "course": "{ 不是 JSON"});
        let error = parse(&body).unwrap_err();
        assert!(error.message().contains("course"), "{error}");
    }

    /// 空目录不 panic。
    #[test]
    fn empty_catalog_is_ok() {
        let catalog = parse(&json!({"code": 0, "course": {}})).unwrap();
        assert_eq!(catalog.group_count(), 0);
        assert!(catalog.groups_in_units(&["u1"]).is_empty());
    }
}
