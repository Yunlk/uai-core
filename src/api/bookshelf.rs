//! 门户书架：`GET /api/cmgt/course/my/bookshelf`。
//!
//! ## 这个接口**不返回任何进度**
//!
//! 它给的是**班级 → 教材**的结构（谁是哪个教学班、班里有哪几本书），
//! 进度数字全是占位。真实进度在 [`crate::api::course_list`]。
//!
//! 旧工程曾因此把「教材进度」永远显示成 `0/0`。
//!
//! ## 资源 ID 不一定有
//!
//! `course-v1:`（资源 ID）有时只藏在封面 `imgUrl` 里，有时完全没有。
//! 缺失时靠**目录响应回收**（目录里含真实 `course-v1:`），见
//! [`crate::api::catalog`] 的 `find_course_v1`。
//!
//! ## 关键字段
//!
//! | 字段 | 含义 |
//! | --- | --- |
//! | `resourceId` / `instanceId` | `course-v2:` 实例 ID |
//! | `activation` | 激活状态，**数字 0/1**（0 = 已激活，与直觉相反） |

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{PORTAL_BOOKSHELF, portal_url};
use crate::error::{Error, Result};
use crate::transport::{body_message, portal_ok};

/// 一本书（教材）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Textbook {
    /// 展示名。
    pub name: String,
    /// `course-v2:` 实例 ID，**提交与进度都要用它**。
    pub instance_id: String,
    /// `course-v1:` 资源 ID，可能为空。
    pub resource_id: String,
    /// 是否已激活。
    pub active: bool,
    /// 激活状态是否已知（区别于「已知未激活」）。
    pub activation_known: bool,
}

/// 一个教学班。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Class {
    pub name: String,
    pub course_id: String,
    pub textbooks: Vec<Textbook>,
}

/// 书架快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookshelf {
    pub classes: Vec<Class>,
}

impl Bookshelf {
    /// 教材总数（去重后的实例 ID 个数）。
    pub fn textbook_count(&self) -> usize {
        let mut seen: Vec<&str> = Vec::new();
        for class in &self.classes {
            for book in &class.textbooks {
                if !book.instance_id.is_empty() && !seen.contains(&book.instance_id.as_str()) {
                    seen.push(&book.instance_id);
                }
            }
        }
        seen.len()
    }
}

/// 从任意深度的对象里取第一个非空字符串。
fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    return Some(text.to_owned());
                }
            }
            map.values().find_map(|child| find_string(child, keys))
        }
        Value::Array(items) => items.iter().find_map(|child| find_string(child, keys)),
        _ => None,
    }
}

/// 激活字段解析。
///
/// ⚠️ 实测**数字 `0` 表示已激活**（字段语义与命名相反）。
/// 也接受布尔形式。
pub fn parse_activation(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|code| code == 0),
        Value::String(text) => match text.trim() {
            "0" => Some(true),
            "1" => Some(false),
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// 读取书架。
pub fn fetch(client: &Client, portal_token: &str) -> Result<Bookshelf> {
    let body = client
        .get(portal_url(PORTAL_BOOKSHELF))
        .header("Authorization", portal_token)
        .send()
        .map_err(|error| Error::network(format!("读取书架请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析书架响应失败：{error}")))?;

    if !portal_ok(&body) {
        return Err(Error::api(body_message(&body, "书架接口返回失败")));
    }
    parse(&body)
}

/// 把书架响应摊成班级 → 教材。
pub fn parse(body: &Value) -> Result<Bookshelf> {
    let root = body
        .get("data")
        .or_else(|| body.get("value"))
        .unwrap_or(body);

    let mut classes: Vec<Class> = Vec::new();

    // 班级可能在 `classList` / `classes` / `courseList` 下，也可能本身就是数组。
    let items = root
        .get("classList")
        .or_else(|| root.get("classes"))
        .or_else(|| root.get("courseList"))
        .and_then(Value::as_array);

    if let Some(items) = items {
        for item in items {
            let name = find_string(item, &["className", "name", "courseName", "title"])
                .unwrap_or_else(|| "未命名班级".to_owned());
            let course_id = find_string(item, &["courseId", "classId", "id"]).unwrap_or_default();
            let mut textbooks = Vec::new();
            collect_textbooks(item, &mut textbooks);
            classes.push(Class {
                name,
                course_id,
                textbooks,
            });
        }
    }

    // 没有显式班级列表时，把整棵树当作一个班级，直接收集教材。
    if classes.is_empty() {
        let mut textbooks = Vec::new();
        collect_textbooks(root, &mut textbooks);
        if !textbooks.is_empty() {
            classes.push(Class {
                name: "全部教材".to_owned(),
                course_id: String::new(),
                textbooks,
            });
        }
    }

    Ok(Bookshelf { classes })
}

/// 递归找出所有教材节点。
fn collect_textbooks(value: &Value, out: &mut Vec<Textbook>) {
    match value {
        Value::Object(map) => {
            // 教材节点的判据：有 instanceId 或 resourceId。
            let instance_id = find_string(value, &["instanceId"]).unwrap_or_default();
            let resource_id =
                find_string(value, &["resourceId", "resource_id"]).unwrap_or_default();
            if !instance_id.is_empty() || !resource_id.is_empty() {
                let name = find_string(value, &["resourceName", "name", "courseName", "title"])
                    .unwrap_or_else(|| "未命名教材".to_owned());
                // 去重：同一本书可能挂在多个班下。
                let duplicate = out.iter().any(|book| {
                    (!instance_id.is_empty() && book.instance_id == instance_id)
                        || (instance_id.is_empty()
                            && !resource_id.is_empty()
                            && book.resource_id == resource_id)
                });
                if !duplicate {
                    let activation = map.get("activation").and_then(parse_activation);
                    out.push(Textbook {
                        name,
                        instance_id,
                        resource_id,
                        active: activation.unwrap_or(false),
                        activation_known: activation.is_some(),
                    });
                }
            }
            for child in map.values() {
                collect_textbooks(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_textbooks(child, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ★ 激活字段：数字 0 表示**已激活**（与命名相反）。
    #[test]
    fn activation_zero_means_active() {
        assert_eq!(parse_activation(&json!(0)), Some(true));
        assert_eq!(parse_activation(&json!(1)), Some(false));
        assert_eq!(parse_activation(&json!("0")), Some(true));
        assert_eq!(parse_activation(&json!(true)), Some(true));
        assert_eq!(parse_activation(&json!("垃圾")), None);
    }

    /// 班级列表按 classList 解析，教材挂在各自班下。
    #[test]
    fn parses_classes_with_textbooks() {
        let body = json!({
            "code": 1,
            "data": {
                "classList": [{
                    "className": "三班",
                    "courseId": "c1",
                    "textbookList": [
                        {"name": "视听说2", "instanceId": "course-v2:a"},
                        {"name": "综合教程1", "instanceId": "course-v2:b"},
                    ]
                }]
            }
        });
        let shelf = parse(&body).unwrap();
        assert_eq!(shelf.classes.len(), 1);
        assert_eq!(shelf.classes[0].name, "三班");
        assert_eq!(shelf.classes[0].textbooks.len(), 2);
        assert_eq!(shelf.textbook_count(), 2);
    }

    /// 同一本书挂在多个班下要去重。
    #[test]
    fn duplicate_textbook_is_deduped() {
        let body = json!({
            "code": 1,
            "data": {
                "classList": [
                    {"className": "一班", "textbookList": [{"name": "书", "instanceId": "course-v2:x"}]},
                    {"className": "二班", "textbookList": [{"name": "书", "instanceId": "course-v2:x"}]},
                ]
            }
        });
        let shelf = parse(&body).unwrap();
        assert_eq!(shelf.textbook_count(), 1, "同一实例 ID 只算一本");
        assert_eq!(shelf.classes.len(), 2, "但两个班都要保留");
    }

    /// 只有 resourceId 没有 instanceId 的节点也要收（资源 ID 回填靠它）。
    #[test]
    fn resource_only_node_is_collected() {
        let body = json!({
            "code": 1,
            "data": {"list": [{"resourceName": "书", "resourceId": "course-v1:abc"}]}
        });
        let shelf = parse(&body).unwrap();
        assert_eq!(shelf.classes.len(), 1);
        assert_eq!(shelf.classes[0].textbooks[0].resource_id, "course-v1:abc");
    }

    /// 没有班级结构时退化成一个「全部教材」班。
    #[test]
    fn flat_tree_becomes_single_class() {
        let body = json!({
            "code": 1,
            "data": {"books": [{"name": "书", "instanceId": "course-v2:z"}]}
        });
        let shelf = parse(&body).unwrap();
        assert_eq!(shelf.classes.len(), 1);
        assert_eq!(shelf.classes[0].name, "全部教材");
        assert_eq!(shelf.textbook_count(), 1);
    }

    /// 空响应不 panic。
    #[test]
    fn empty_body_yields_empty_shelf() {
        let shelf = parse(&json!({"code": 1, "data": {}})).unwrap();
        assert!(shelf.classes.is_empty());
        assert_eq!(shelf.textbook_count(), 0);
    }
}
