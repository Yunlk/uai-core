//! 班级真实进度：`GET /api/cmgt/course/getCourseListByStudent`。
//!
//! ## ★ 这是班级完成度的**唯一权威来源**
//!
//! 按「教学班 + 教材」给出 `finishPointNum / totalPointNum`。
//! 界面的总进度**只能**用这里的数字。
//!
//! ## ⚠️ 进度是**班级级**的：同一本教材在不同班读数完全不同
//!
//! 同一个 `instanceId` 会出现在多个班里，**读数各不相同**：
//!
//! | 班级 | `strategyId` | 读数 |
//! | --- | --- | --- |
//! | 大学英语I（三）A班（`courseId=100001`） | `900001` | **11/49** |
//! | 大学英语I（二）A班（`courseId=100002`） | `1001600` | **35/35** |
//!
//! 所以本模块**按「班级 + 教材」成对保留**，绝不再跨班去重。
//! 早先的实现按 `instanceId` 全局去重并保留「进度最大的那条」，
//! 结果把二班的 35/35 覆盖到三班头上，三班真实是 11/49 —— 这个坑很隐蔽，
//! 因为两侧都是「合法读数」，只是**不属于同一个班**。
//!
//! 需要班级身份时用 [`CourseProgress::class_id`]；跨班比较前先想清楚口径。
//!
//! ## 为什么不能用 `progress/v2` 数完成数
//!
//! 三班《视听说2》的 **49 个必修分组全部 `state=1`**，但门户只算 **11/49**。
//! 七个候选口径没有一个能数出这个数：
//!
//! | 口径 | 数出 |
//! | --- | --- |
//! | `state.state == 1` | 49 |
//! | `score_pct >= 1` | 40 |
//! | `score` 全为 1 | 40 |
//! | `last_pass` / `first_pass` / `record_grade` 存在 | 49 |
//!
//! 反证更直接：
//!
//! - **纯内容页（`purecontent`）没有完成概念，也是 `state=1`**；
//! - 得 0 分的 `presentation` 同样 `state=1`；
//! - **同一个 `instanceId` 被多个班共用**（《视听说2》二班 35/35、三班 11/49
//!   是**同一个实例**），学生在二班做过的痕迹会串到三班的读数上。
//!
//! **结论：**`progress/v2` 只能用来**定位具体哪个分组**有痕迹，
//! 数总量必须用本接口。

use reqwest::blocking::Client;
use serde_json::Value;

use crate::endpoints::{PORTAL_COURSE_LIST, portal_url};
use crate::error::{Error, Result};
use crate::transport::{body_message, portal_ok};

/// 一本教材在一个班里的真实进度。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CourseProgress {
    /// `course-v2:` 实例 ID。
    pub instance_id: String,
    /// 教材名。
    pub name: String,
    /// 已完成计分点。
    pub finish: u32,
    /// 总计分点。
    pub total: u32,
    /// 所在班级 ID（字符串）。**同一实例跨班读数不同，必须一起用。**
    pub class_id: String,
    /// 班级名，仅用于展示。
    pub class_name: String,
    /// 数字 `courseId`。考核接口的 `courseInstanceId` 就是它。
    pub course_id: i64,
    /// 门户「班级教材ID」(`courseResourceId`)：**教材节点自己的 `id`**。
    ///
    /// 任务页 URL 里的 `courseResourceId=` 就是它（实测《基础篇综合教程2》
    /// 在这班是 `20000000002`）。⚠️ 不是 `resourceId`（那是 ucontent 的
    /// `course-v2:…` 课程资源 ID），也不是班级节点的 `id`。
    pub course_resource_id: i64,
    /// 课程资源 ID（`course-v2:unipus+…`）——**按班策略接口要它**。
    pub resource_id: String,
    /// 该 (班,书) 绑定的班级策略 ID。**按班取必修/门槛的钥匙**。
    pub strategy_id: i64,
}

impl CourseProgress {
    /// 完成百分比（0~100）。`total` 为 0 时返回 0。
    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.finish as f32 / self.total as f32 * 100.0
    }

    /// 是否已完成。
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.finish >= self.total
    }

    /// `完成/总数` 文本。
    pub fn ratio_text(&self) -> String {
        format!("{}/{}", self.finish, self.total)
    }

    /// 带班级的完整标签，避免跨班混淆。
    pub fn label(&self) -> String {
        if self.class_name.trim().is_empty() {
            self.name.clone()
        } else {
            format!("{}／{}", self.class_name, self.name)
        }
    }
}

/// 读取按班给出的真实进度。
pub fn fetch(client: &Client, portal_token: &str) -> Result<Vec<CourseProgress>> {
    let body = client
        .get(portal_url(PORTAL_COURSE_LIST))
        .header("Authorization", portal_token)
        .send()
        .map_err(|error| Error::network(format!("读取课程列表请求失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析课程列表失败：{error}")))?;

    if !portal_ok(&body) {
        return Err(Error::api(body_message(&body, "课程列表接口返回失败")));
    }
    Ok(parse(&body))
}

/// 从响应里摊出「班级 × 教材」的进度。
///
/// ⚠️ **按班级分别保留**，不做跨班去重：同一 `instanceId` 在不同班的
/// `totalPointNum` 本来就不同（49 vs 35），合并会得到错误读数。
pub fn parse(body: &Value) -> Vec<CourseProgress> {
    let mut out: Vec<CourseProgress> = Vec::new();

    // 先找班级节点：有 `courseResourceList` 的就是。找到后按班级上下文摊平教材。
    fn walk_classes(value: &Value, out: &mut Vec<CourseProgress>) {
        if let Some(classes) = value.pointer("/value/courseList").and_then(Value::as_array) {
            for class in classes {
                collect_books(class, out);
            }
            return;
        }
        // 结构变了（比如包了 data）就继续往下找。
        match value {
            Value::Object(map) => {
                for child in map.values() {
                    walk_classes(child, out);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk_classes(child, out);
                }
            }
            _ => {}
        }
    }

    /// 把一个班级节点下的教材全部收下，并打上班级标记。
    fn collect_books(class: &Value, out: &mut Vec<CourseProgress>) {
        let class_id = class
            .get("classId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        // 班级名用 `name`（`className` 是「25级电子、机器人A1班」这类行政班名）。
        let class_name = class
            .get("name")
            .or_else(|| class.get("className"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let course_id = class.get("id").and_then(Value::as_i64).unwrap_or(0);

        let Some(books) = class.get("courseResourceList").and_then(Value::as_array) else {
            return;
        };
        for book in books {
            let (Some(finish), Some(total)) = (
                book.get("finishPointNum").and_then(Value::as_u64),
                book.get("totalPointNum").and_then(Value::as_u64),
            ) else {
                continue;
            };
            // ⚠️ 教材名要取**教材自己的** `name`，不能递归到班级的 `name`。
            let instance_id = book
                .get("instanceId")
                .or_else(|| book.get("courseInstanceId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let name = book
                .get("name")
                .or_else(|| book.get("resourceName"))
                .and_then(Value::as_str)
                .unwrap_or("未命名教材")
                .to_owned();

            out.push(CourseProgress {
                instance_id,
                name,
                finish: finish as u32,
                // 教材节点自己的 `id` —— 就是门户「班级教材ID」(`courseResourceId`)，
                // 也就是任务页 URL 里那个 `courseResourceId=`。
                course_resource_id: book.get("id").and_then(Value::as_i64).unwrap_or(0),
                resource_id: book
                    .get("resourceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                strategy_id: book.get("strategyId").and_then(Value::as_i64).unwrap_or(0),
                total: total as u32,
                class_id: class_id.clone(),
                class_name: class_name.clone(),
                course_id,
            });
        }
    }

    walk_classes(body, &mut out);
    // 没拿到 instance_id 的条目无法与教材对应，丢弃以免误导。
    // 同理，没拿到 class_id 的条目也无法确定口径，一并丢弃。
    out.retain(|item| !item.instance_id.is_empty() && !item.class_id.is_empty());
    out
}

/// 快速取某本教材的进度。
///
/// ⚠️ 同一实例可能跨多个班存在，这里返回**第一个**匹配项。
/// 需要确定班级请用 [`percent_of_in_class`]。
pub fn percent_of(items: &[CourseProgress], instance_id: &str) -> Option<f32> {
    items
        .iter()
        .find(|item| item.instance_id == instance_id)
        .map(CourseProgress::percent)
}

/// 在指定班级里取某本教材的进度。
///
/// ★ 跨班比较必须走这个函数——同一 `instanceId` 在不同班的读数不同。
pub fn percent_of_in_class(
    items: &[CourseProgress],
    instance_id: &str,
    class_id: &str,
) -> Option<f32> {
    items
        .iter()
        .find(|item| item.instance_id == instance_id && item.class_id == class_id)
        .map(CourseProgress::percent)
}

/// 取某个班里的全部教材进度。
pub fn progress_in_class<'a>(
    items: &'a [CourseProgress],
    class_id: &str,
) -> Vec<&'a CourseProgress> {
    items
        .iter()
        .filter(|item| item.class_id == class_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 从真实结构（`value.courseList[班级].courseResourceList[教材]`）解析出进度，
    /// 并带上班级身份。
    #[test]
    fn parses_real_portal_numbers() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [{
                "classId": "1111111111111111111",
                "className": "25级电子、机器人A1班",
                "id": 100001,
                "name": "大学英语I（三）A班",
                "courseResourceList": [{
                    "instanceId": "course-v2:abc",
                    "name": "新一代大学英语（基础篇）视听说思政数字课程2",
                    "finishPointNum": 11,
                    "totalPointNum": 49,
                }]
            }]}
        });
        let items = parse(&body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].finish, 11);
        assert_eq!(items[0].total, 49);
        assert_eq!(items[0].ratio_text(), "11/49");
        assert!(!items[0].is_complete());
        // 教材名取教材自己的，不能串成班级名。
        assert!(items[0].name.contains("视听说"), "{}", items[0].name);
        // 班级身份要带上。
        assert_eq!(items[0].class_id, "1111111111111111111");
        assert_eq!(items[0].class_name, "大学英语I（三）A班");
        assert_eq!(items[0].course_id, 100001);
        assert_eq!(
            items[0].label(),
            "大学英语I（三）A班／新一代大学英语（基础篇）视听说思政数字课程2"
        );
    }

    /// ★ 同一实例跨两个班时，**必须保留两条**，且读数各自独立。
    ///
    /// 真实数据：《视听说2》三班 11/49、二班 35/35 是同一个 `instanceId`。
    /// 早先按实例去重「保留进度最大者」会把 35/35 盖到三班头上。
    #[test]
    fn same_instance_in_two_classes_is_not_merged() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [
                {"classId": "C3", "id": 100001, "name": "大学英语I（三）A班",
                 "courseResourceList": [
                    {"instanceId": "course-v2:jc2", "name": "视听说2",
                     "finishPointNum": 11, "totalPointNum": 49}]},
                {"classId": "C2", "id": 100002, "name": "大学英语I（二）A班",
                 "courseResourceList": [
                    {"instanceId": "course-v2:jc2", "name": "视听说2",
                     "finishPointNum": 35, "totalPointNum": 35}]}
            ]}
        });
        let items = parse(&body);
        assert_eq!(items.len(), 2, "两个班各留一条，不能合并");

        // ★ 三班必须是 11/49，绝不能被二班的 35/35 覆盖。
        let c3 = progress_in_class(&items, "C3");
        assert_eq!(c3.len(), 1);
        assert_eq!(c3[0].ratio_text(), "11/49", "三班读数被串班了");

        let c2 = progress_in_class(&items, "C2");
        assert_eq!(c2[0].ratio_text(), "35/35");
        assert!(c2[0].is_complete());
        assert!(!c3[0].is_complete());

        // 同一个 instanceId 在两个班给出不同百分比。
        assert_eq!(
            percent_of_in_class(&items, "course-v2:jc2", "C2"),
            Some(100.0)
        );
        assert!(
            (percent_of_in_class(&items, "course-v2:jc2", "C3").unwrap() - 22.449).abs() < 0.01
        );
    }

    /// 没有班级归属的条目要丢弃 —— 口径不明，留着只会误导。
    #[test]
    fn entry_without_class_is_dropped() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [{
                "courseResourceList": [
                    {"instanceId": "course-v2:x", "name": "书",
                     "finishPointNum": 1, "totalPointNum": 2}]
            }]}
        });
        assert!(parse(&body).is_empty(), "缺 classId 不能产出条目");
    }

    /// 百分比计算正确。
    #[test]
    fn percent_is_computed() {
        let item = CourseProgress {
            instance_id: "x".to_owned(),
            name: "书".to_owned(),
            finish: 11,
            total: 49,
            class_id: "c".to_owned(),
            ..Default::default()
        };
        assert!((item.percent() - 22.449).abs() < 0.01, "{}", item.percent());
    }

    /// `total` 为 0 时百分比是 0，不能是 NaN（否则界面显示 NaN%）。
    #[test]
    fn zero_total_yields_zero_percent() {
        let item = CourseProgress {
            instance_id: "x".to_owned(),
            name: "书".to_owned(),
            finish: 0,
            total: 0,
            class_id: "c".to_owned(),
            ..Default::default()
        };
        assert_eq!(item.percent(), 0.0);
        assert!(!item.is_complete());
    }

    /// 满分才算完成。
    #[test]
    fn complete_requires_full_marks() {
        let full = CourseProgress {
            instance_id: "x".to_owned(),
            name: "书".to_owned(),
            finish: 35,
            total: 35,
            class_id: "c".to_owned(),
            ..Default::default()
        };
        assert!(full.is_complete());
        assert_eq!(full.percent(), 100.0);
    }

    /// 没有 finish/total 的教材不产出条目。
    #[test]
    fn node_without_numbers_is_skipped() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [{"classId": "c",
                "courseResourceList": [{"instanceId": "course-v2:x", "name": "书"}]}]}
        });
        assert!(parse(&body).is_empty());
    }

    /// 没有 instance_id 的条目要丢弃（无法与教材对应）。
    #[test]
    fn entry_without_instance_is_dropped() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [{"classId": "c",
                "courseResourceList": [{"name": "无 ID",
                    "finishPointNum": 1, "totalPointNum": 2}]}]}
        });
        assert!(parse(&body).is_empty());
    }

    /// 按实例查百分比；带班级的重载要能区分班级。
    #[test]
    fn percent_of_looks_up_by_instance() {
        let items = vec![CourseProgress {
            instance_id: "course-v2:x".to_owned(),
            name: "书".to_owned(),
            finish: 1,
            total: 2,
            class_id: "c1".to_owned(),
            ..Default::default()
        }];
        assert_eq!(percent_of(&items, "course-v2:x"), Some(50.0));
        assert_eq!(percent_of(&items, "course-v2:不存在"), None);
        assert_eq!(percent_of_in_class(&items, "course-v2:x", "c1"), Some(50.0));
        // 别的班没有这条记录，不能误报 50%。
        assert_eq!(percent_of_in_class(&items, "course-v2:x", "c2"), None);
    }

    /// 教材名缺失时用兜底名，但**不能**把班级名当教材名。
    #[test]
    fn missing_book_name_uses_placeholder() {
        let body = json!({
            "code": 1,
            "value": {"courseList": [{"classId": "c", "name": "某班",
                "courseResourceList": [{"instanceId": "course-v2:x",
                    "finishPointNum": 1, "totalPointNum": 2}]}]}
        });
        let items = parse(&body);
        assert_eq!(items[0].name, "未命名教材");
        assert_ne!(items[0].name, "某班");
    }
}
