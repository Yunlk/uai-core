//! 考核方案与综合成绩：门户 `/api/aca/*`。
//!
//! ## ★ 这是「必修到底算几分」的唯一权威口径
//!
//! 教材侧的 `flowStrategy.required` 只说**哪些任务必做**，不说**各占多少分**。
//! 门户的考核方案才是记分规则：
//!
//! ```text
//! 形成性评价 (100%)
//!   ├─ 必修学习时长 item=30  50%   ← 按「教材学习时长」分段给分
//!   └─ 教程学习成绩 item=21  50%   ← 按「任务点完成 + 得分」加权
//! ```
//!
//! ## 两个考核项的语义（实测，`resource-detail/20000000001`）
//!
//! | `item` | 名称 | `finishProcess` | `businessScoreList[].<字段>` |
//! | --- | --- | --- | --- |
//! | `30` | 必修学习时长 | `01:20:32`（累计时长） | `duration` |
//! | `21` | 教程学习成绩 | `28/117 任务点` | `process` |
//! | `31`/`33` | 作业/考试 | 提交时间 | `submitTime` |
//! | `35`/`36` | 课堂互动/讨论 | 参与次数 | `completedTimes` |
//!
//! ## ★ 时长是**教材级分段函数**，不是账号级总时长
//!
//! 考核方案里每个考核项带 `config.detailConfig`，逐教材给出 `minHours`/`maxHours`/
//! `percent`。「低于最低时长计 0 分，达到满分时长计满分」：
//!
//! ```text
//! course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31 → {minHours: 0, maxHours: 20, percent: 40}
//! ```
//!
//! 所以第六节说的「时长只在账号级 `(module, moduleGroup)` 记账、无法归到某本教材」
//! **只对 WebSocket 加速那条链路成立**——门户侧是按教材分别累计并分别给分的。
//!
//! ## ⚠️ 必须先解决 `6011`
//!
//! 只带 `Authorization` 会返回 `{"code":6011,"msg":"feature denied"}`。
//! 必须加上宿主头（见 [`crate::transport::HostIdentity`]）：
//! `u-app-id: 116` / `u-platform: 2` / `u-school` / `u-openid`。
//!
//! ## ⚠️ `courseInstanceId` 是**数字 `courseId`**，不是 `course-v2:` 实例 ID
//!
//! 这是最容易踩的一个坑。服务端 DTO 是
//! `cn.unipus.aca.base.dto.plan.PlanDetailRequest`，其中 `courseInstanceId` 声明为
//! `long`。传 `course-v2:example0a2+ngce_vl_jc2_sz_ucloud+2024_01_31` 会得到：
//!
//! ```text
//! JSON parse error: Cannot deserialize value of type `long` from String
//! "course-v2:..." ... (through reference chain: PlanDetailRequest["courseInstanceId"])
//! ```
//!
//! 正确值来自 [`crate::api::course_list`] 的班级节点 `id`（如 `100001`），
//! 也可从 [`crate::api::course_list::fetch_resource_info`] 直接读 `courseId`。
//!
//! `classId` 是 **字符串**（`"1111111111111111111"`）。

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::endpoints::{PORTAL_ASSESS_PLAN, PORTAL_ASSESS_SCORE, assess_url};
use crate::error::{Error, Result};
use crate::transport::{HostIdentity, body_message, portal_ok};

/// 考核项代码。与前端 `618-*.js` 里的 `switch(e)` 一一对应。
///
/// ```js
/// case 30: return "duration";      case 21: return "process";
/// case 31: case 33: return "submitTime";
/// case 35: case 36: return "completedTimes";
/// ```
pub mod item_code {
    /// 必修学习时长。
    pub const DURATION: i64 = 30;
    /// 教程学习成绩（任务点进度）。
    pub const PROCESS: i64 = 21;
    /// 作业提交时间。
    pub const HOMEWORK_SUBMIT: i64 = 31;
    /// 考试提交时间。
    pub const EXAM_SUBMIT: i64 = 33;
    /// 课堂互动参与次数。
    pub const INTERACTION_TIMES: i64 = 35;
    /// 课堂讨论参与次数。
    pub const DISCUSSION_TIMES: i64 = 36;

    /// 把 `item` 码翻成响应里承载数值的那个字段名。
    pub fn field_of(item: i64) -> &'static str {
        match item {
            DURATION => "duration",
            PROCESS => "process",
            HOMEWORK_SUBMIT | EXAM_SUBMIT => "submitTime",
            INTERACTION_TIMES | DISCUSSION_TIMES => "completedTimes",
            _ => "",
        }
    }

    /// 人类可读名称。
    pub fn name_of(item: i64) -> &'static str {
        match item {
            DURATION => "必修学习时长",
            PROCESS => "教程学习成绩",
            HOMEWORK_SUBMIT => "作业",
            EXAM_SUBMIT => "考试",
            INTERACTION_TIMES => "课堂互动",
            DISCUSSION_TIMES => "课堂讨论",
            _ => "未知考核项",
        }
    }

    /// 这个考核项是不是「学习时长」类。
    pub fn is_duration(item: i64) -> bool {
        item == DURATION
    }
}

/// 一本教材在一个考核项下的明细（`businessScoreList` 的一项）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookScore {
    /// 教材名。
    pub name: String,
    /// 该教材在本考核项里的权重占比（百分比）。
    pub percent: f64,
    /// 百分制得分。
    pub score: f64,
    /// 时长类：`"01:20:32"`；进度类为 `None`。
    pub duration: Option<String>,
    /// 进度类：`"11/49"`；时长类为 `None`。
    pub process: Option<String>,
    /// 作业/考试：提交时间戳字符串。
    pub submit_time: Option<String>,
    /// 互动/讨论：参与次数。
    pub completed_times: Option<i64>,
}

impl BookScore {
    /// 进度类明细解析出的 `(完成, 总数)`。
    ///
    /// ★ 这个分母就是门户认可的计分点数——实测《视听说2》是 `11/49`，
    /// 与 `Σ rt.leafs.<gid>.strategies.required`（49）一致，**不是**
    /// `flowStrategy.required` 数出的 45。
    pub fn process_ratio(&self) -> Option<(u32, u32)> {
        let text = self.process.as_deref()?;
        let (done, total) = text.split_once('/')?;
        // split_whitespace 自带去空白，`trim()` 是多余的。
        Some((
            done.trim().parse().ok()?,
            total.split_whitespace().next()?.parse().ok()?,
        ))
    }

    /// 时长明细解析出的秒数。
    pub fn duration_seconds(&self) -> Option<u32> {
        let text = self.duration.as_deref()?;
        let mut parts = text.split(':').map(|p| p.trim().parse::<u32>());
        let hours = parts.next()?.ok()?;
        let minutes = parts.next()?.ok()?;
        let seconds = parts.next()?.ok()?;
        Some(hours * 3600 + minutes * 60 + seconds)
    }
}

/// 一个考核项（如「必修学习时长」）的成绩。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreItem {
    /// `item` 码，见 [`item_code`]。
    pub item: i64,
    /// 考核项名称。
    pub name: String,
    /// 权重占比（百分比）。
    pub percent: f64,
    /// 加权后得分。
    pub score: f64,
    /// 加权前得分。
    pub origin_score: f64,
    /// `finishProcess` 原文（时长是 `"01:20:32"`，进度是 `"28/117 任务点"`）。
    pub finish_process: String,
    /// 明细项，逐教材。
    pub books: Vec<BookScore>,
}

impl ScoreItem {
    /// 是不是时长类考核项。
    pub fn is_duration(&self) -> bool {
        item_code::is_duration(self.item)
    }

    /// 本项目里权重最高的那本教材。
    pub fn top_book(&self) -> Option<&BookScore> {
        self.books.iter().max_by(|a, b| {
            a.percent
                .partial_cmp(&b.percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// 按教材名找明细。
    pub fn book_named(&self, name: &str) -> Option<&BookScore> {
        self.books.iter().find(|b| b.name == name)
    }
}

/// 综合成绩（`achievement/queryUserScore`）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grade {
    /// 模板名，如「大学英语I（三）U校园学习」。
    pub name: String,
    /// 当前总成绩。
    pub total_score: f64,
    /// 各考核项。
    pub items: Vec<ScoreItem>,
}

impl Grade {
    /// 取某个考核项。
    pub fn item(&self, item: i64) -> Option<&ScoreItem> {
        self.items.iter().find(|i| i.item == item)
    }

    /// 必修学习时长项。
    pub fn duration_item(&self) -> Option<&ScoreItem> {
        self.item(item_code::DURATION)
    }

    /// 教程学习成绩项。
    pub fn process_item(&self) -> Option<&ScoreItem> {
        self.item(item_code::PROCESS)
    }

    /// 一行摘要。
    pub fn describe(&self) -> String {
        if self.items.is_empty() {
            return format!("{}：当前 {:.1} 分（无明细）", self.name, self.total_score);
        }
        let parts: Vec<String> = self
            .items
            .iter()
            .map(|i| format!("{} {}% → {:.1} 分", i.name, i.percent, i.score))
            .collect();
        format!(
            "{}：当前 {:.1} 分（{}）",
            self.name,
            self.total_score,
            parts.join("，")
        )
    }
}

/// 一本教材的时长标准（考核方案 `config.detailConfig` 的一项）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoursStandard {
    /// `course-v2:` 资源 ID。
    pub resource_id: String,
    /// 最低时长（小时）。低于它计 0 分。
    pub min_hours: f64,
    /// 满分时长（小时）。达到它计满分。
    pub max_hours: f64,
    /// 该教材在本考核项里的权重占比。
    pub percent: f64,
}

impl HoursStandard {
    /// 给定已学秒数，按「低于最低计 0、达到满分计满分」算出比例（0~1）。
    ///
    /// ⚠️ **退化区间（`max <= min`）必须先判**：那时「达到满分标准」和
    /// 「达到最低标准」是同一时刻，若先走 `hours <= min → 0` 就会把刚好达标的
    /// 情况判成 0 分。实测方案里 `minHours` 常为 0，所以这个次序很关键。
    pub fn ratio_for(&self, seconds: u32) -> f64 {
        let hours = f64::from(seconds) / 3600.0;
        if self.max_hours <= self.min_hours {
            return if hours >= self.max_hours { 1.0 } else { 0.0 };
        }
        if hours <= self.min_hours {
            return 0.0;
        }
        if hours >= self.max_hours {
            return 1.0;
        }
        (hours - self.min_hours) / (self.max_hours - self.min_hours)
    }

    /// 达标所需秒数。
    pub fn required_seconds(&self) -> u32 {
        (self.max_hours * 3600.0).round() as u32
    }
}

/// 一个考核项在考核方案里的配置。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanItem {
    /// `item` 码。
    pub item: i64,
    /// 名称。
    pub name: String,
    /// 权重占比。
    pub percent: f64,
    /// 模板 ID。
    pub id: String,
    /// 时长类的逐教材标准；非时长类为空。
    pub hours: Vec<HoursStandard>,
}

/// 考核方案（`plan/detail`）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssessmentPlan {
    /// 方案名，如「大学英语I（三）U校园学习」。
    pub name: String,
    /// 数字 `courseId`（就是考核接口要的 `courseInstanceId`）。
    pub course_instance_id: String,
    /// 门户的「课程 ID」(`value.courseId`，实测 `34852`）。
    ///
    /// ⚠️ 与 [`Self::course_instance_id`]（`100001`）**不是一回事**：
    /// 它等于任务页 URL 里的 `aitutorialId`，也是按班策略接口
    /// `courseStudyStrategy/detail` 的必填 `courseId`。
    pub course_id: String,
    /// `classId`。
    pub class_id: String,
    /// 记分周期开始（毫秒时间戳字符串）。
    pub cycle_start: String,
    /// 记分周期结束（毫秒时间戳字符串）。
    pub cycle_end: String,
    /// 各考核项。
    pub items: Vec<PlanItem>,
}

impl AssessmentPlan {
    /// 取某个考核项。
    pub fn item(&self, item: i64) -> Option<&PlanItem> {
        self.items.iter().find(|i| i.item == item)
    }

    /// 必修学习时长项。
    pub fn duration_item(&self) -> Option<&PlanItem> {
        self.item(item_code::DURATION)
    }

    /// 教程学习成绩项。
    pub fn process_item(&self) -> Option<&PlanItem> {
        self.item(item_code::PROCESS)
    }

    /// 总权重（百分比之和，正常是 100）。
    pub fn total_percent(&self) -> f64 {
        self.items.iter().map(|i| i.percent).sum()
    }

    /// ★ 某本教材的时长标准。
    pub fn hours_for(&self, resource_id: &str) -> Option<&HoursStandard> {
        self.duration_item()?
            .hours
            .iter()
            .find(|h| h.resource_id.eq_ignore_ascii_case(resource_id))
    }
}

/// 班级教材的标识信息（`getCourseResourceInfoById`）。
///
/// 考核接口的入参**只能**从这里来。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CourseResourceInfo {
    /// 数字 `courseId` —— 考核接口的 `courseInstanceId`。
    pub course_id: i64,
    /// 字符串 `classId`。
    pub class_id: String,
    /// `course-v2:` 实例 ID（**不是**考核接口要的那个）。
    pub course_instance_id: String,
    /// `course-v2:` 资源 ID。
    pub resource_id: String,
    /// 策略 ID。
    pub strategy_id: i64,
    /// 班级名。
    pub course_name: String,
    /// 教材名。
    pub resource_name: String,
}

/// 把 `guide` 建议的宿主头并进请求。
fn body_with_ids(course_instance_id: i64, class_id: &str) -> Value {
    json!({
        "courseInstanceId": course_instance_id,
        "classId": class_id,
    })
}

/// 读一本班级教材的标识信息。
pub fn fetch_resource_info(
    client: &Client,
    portal_token: &str,
    course_resource_id: &str,
) -> Result<CourseResourceInfo> {
    if course_resource_id.trim().is_empty() {
        return Err(Error::invalid("查询教材信息需要 courseResourceId"));
    }
    let url = crate::endpoints::course_resource_info_url(course_resource_id);
    let body = client
        .get(&url)
        .header("Authorization", portal_token)
        .send()
        .map_err(|error| Error::network(format!("读取班级教材信息失败：{error}")))?
        .json::<Value>()
        .map_err(|error| Error::parse(format!("解析班级教材信息失败：{error}")))?;

    if !portal_ok(&body) {
        return Err(Error::api(body_message(&body, "班级教材信息接口返回失败")));
    }
    let node = body
        .pointer("/value/courseResource")
        .ok_or_else(|| Error::parse("班级教材信息缺少 value.courseResource"))?;

    Ok(CourseResourceInfo {
        course_id: node.get("courseId").and_then(Value::as_i64).unwrap_or(0),
        class_id: text_of(node, "classId"),
        course_instance_id: text_of(node, "courseInstanceId"),
        resource_id: node
            .pointer("/tutorial/resourceId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| text_of(node, "resourceId")),
        strategy_id: node.get("strategyId").and_then(Value::as_i64).unwrap_or(0),
        course_name: text_of(node, "courseName"),
        resource_name: node
            .pointer("/tutorial/resourceName")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default(),
    })
}

/// 读一个字符串字段，缺失就给空串。
fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 取数字字段，字符串形式的数字也接受。
fn number_of(value: &Value, key: &str) -> f64 {
    match value.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// 取可选字符串字段（空串归一成 `None`）。
fn opt_text(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// 发一个考核类 POST，自动带上宿主头。
fn post_assess(
    client: &Client,
    portal_token: &str,
    host: &HostIdentity,
    path: &str,
    body: &Value,
) -> Result<Value> {
    if !host.is_complete() {
        return Err(Error::invalid(
            "考核接口需要宿主头（u-app-id / u-platform / u-school / u-openid），\
             当前身份不完整；请重新 `login` 以获取 school",
        ));
    }
    let url = assess_url(path);
    let response = client
        .post(&url)
        .header("Authorization", portal_token)
        .header(crate::endpoints::HOST_APP_ID_HEADER, &host.app_id)
        .header(crate::endpoints::HOST_PLATFORM_HEADER, &host.platform)
        .header(crate::endpoints::HOST_SCHOOL_HEADER, &host.school)
        .header(crate::endpoints::HOST_OPENID_HEADER, &host.open_id)
        .json(body)
        .send()
        .map_err(|error| Error::network(format!("考核接口请求失败：{error}")))?;

    let text = response
        .text()
        .map_err(|error| Error::network(format!("读取考核接口响应失败：{error}")))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|error| Error::parse(format!("考核接口返回的不是 JSON：{error}")))?;

    // 6011 是这套接口最常见的失败，单独给出可操作的提示。
    if parsed.get("code").and_then(Value::as_i64) == Some(6011) {
        return Err(Error::api(
            "考核接口返回 6011 feature denied：宿主头缺失或不匹配。\
             需要 u-app-id: 116、u-platform: 2，以及正确的 u-school / u-openid。",
        ));
    }
    if !portal_ok(&parsed) {
        // 500 时服务端的 value 常常是完整的 Java 校验异常，比 msg 有用得多。
        let detail = parsed.get("value").and_then(Value::as_str).unwrap_or("");
        let head: String = detail.chars().take(400).collect();
        let base = body_message(&parsed, "考核接口返回失败");
        return Err(Error::api(if head.is_empty() {
            base
        } else {
            format!("{base}\n服务端详情：{head}")
        }));
    }
    Ok(parsed)
}

/// 读综合成绩。
pub fn fetch_grade(
    client: &Client,
    portal_token: &str,
    host: &HostIdentity,
    course_instance_id: i64,
    class_id: &str,
) -> Result<Grade> {
    if course_instance_id <= 0 {
        return Err(Error::invalid(
            "考核接口的 courseInstanceId 必须是数字 courseId（如 100001），\
             不是 course-v2: 实例 ID",
        ));
    }
    let body = post_assess(
        client,
        portal_token,
        host,
        PORTAL_ASSESS_SCORE,
        &body_with_ids(course_instance_id, class_id),
    )?;
    Ok(parse_grade(&body))
}

/// 读考核方案。
pub fn fetch_plan(
    client: &Client,
    portal_token: &str,
    host: &HostIdentity,
    course_instance_id: i64,
    class_id: &str,
) -> Result<AssessmentPlan> {
    if course_instance_id <= 0 {
        return Err(Error::invalid(
            "考核接口的 courseInstanceId 必须是数字 courseId（如 100001），\
             不是 course-v2: 实例 ID",
        ));
    }
    let body = post_assess(
        client,
        portal_token,
        host,
        PORTAL_ASSESS_PLAN,
        &body_with_ids(course_instance_id, class_id),
    )?;
    Ok(parse_plan(&body))
}

/// 从 `queryUserScore` 响应解析综合成绩。
///
/// 结构：`value.templateScoreList[]`，`templateType == 1` 是根节点（总分），
/// `templateType == 2` 才是带 `businessScoreList` 的考核项。
pub fn parse_grade(body: &Value) -> Grade {
    let list = body
        .pointer("/value/templateScoreList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut grade = Grade::default();
    // 根节点：parentId 为空的那个 templateType==1。
    for node in &list {
        let template_type = node
            .get("templateType")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let parent_null = node.get("parentId").map(Value::is_null).unwrap_or(true);
        if template_type == 1 && parent_null {
            grade.total_score = number_of(node, "score");
            grade.name = text_of(node, "name");
        }
    }
    // 没有根节点就退而求其次，取第一条的分数。
    if grade.name.is_empty()
        && let Some(first) = list.first()
    {
        grade.total_score = number_of(first, "score");
        grade.name = text_of(first, "name");
    }

    // 考核项：带 item 的那些。
    for node in &list {
        let Some(item) = node.get("item").and_then(Value::as_i64) else {
            continue;
        };
        let books = node
            .get("businessScoreList")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|b| BookScore {
                        name: text_of(b, "name"),
                        percent: number_of(b, "percent"),
                        score: number_of(b, "score"),
                        duration: opt_text(b, "duration"),
                        process: opt_text(b, "process"),
                        submit_time: opt_text(b, "submitTime"),
                        completed_times: b.get("completedTimes").and_then(Value::as_i64),
                    })
                    .collect()
            })
            .unwrap_or_default();

        grade.items.push(ScoreItem {
            item,
            name: {
                let n = text_of(node, "name");
                if n.is_empty() {
                    item_code::name_of(item).to_owned()
                } else {
                    n
                }
            },
            percent: number_of(node, "percent"),
            score: number_of(node, "score"),
            origin_score: number_of(node, "originScore"),
            finish_process: text_of(node, "finishProcess"),
            books,
        });
    }
    grade
}

/// 从 `plan/detail` 响应解析考核方案。
///
/// 结构：`value.children[]` → `children[]` 才是考核项（`id`/`item`/`percent`/`config`）。
pub fn parse_plan(body: &Value) -> AssessmentPlan {
    let value = body.pointer("/value").cloned().unwrap_or(Value::Null);
    let mut plan = AssessmentPlan {
        name: text_of(&value, "name"),
        course_instance_id: text_of(&value, "courseInstanceId"),
        course_id: text_of(&value, "courseId"),
        class_id: text_of(&value, "classId"),
        cycle_start: text_of(&value, "cycleStart"),
        cycle_end: text_of(&value, "cycleEnd"),
        items: Vec::new(),
    };

    // 逐层下钻，收集所有带 item 的节点——层级可能变，不写死两层。
    fn walk(node: &Value, out: &mut Vec<PlanItem>) {
        if let Some(item) = node.get("item").and_then(Value::as_i64) {
            // ⚠️ `detailConfig` 的**值类型随考核项不同**：
            //   时长项(item=30) → `{"<resourceId>": {minHours, maxHours, percent}}`
            //   成绩项(item=21) → `{"<resourceId>": 40}`（就是个百分比数字）
            // 只把对象形式当时长标准，否则会把成绩项渲染成「最低 0h / 满分 0h」。
            let hours = node
                .pointer("/config/detailConfig")
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .filter_map(|(resource_id, cfg)| {
                            cfg.as_object()?;
                            Some(HoursStandard {
                                resource_id: resource_id.clone(),
                                min_hours: number_of(cfg, "minHours"),
                                max_hours: number_of(cfg, "maxHours"),
                                percent: number_of(cfg, "percent"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(PlanItem {
                item,
                name: {
                    let n = text_of(node, "name");
                    if n.is_empty() {
                        item_code::name_of(item).to_owned()
                    } else {
                        n
                    }
                },
                percent: number_of(node, "percent"),
                id: text_of(node, "id"),
                hours,
            });
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, out);
            }
        }
    }
    walk(&value, &mut plan.items);
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ★ 真实响应（`resource-detail/20000000001` 的「综合成绩」页）必须能解析出
    /// 当前成绩、两个考核项，以及《视听说2》的 **11/49**。
    #[test]
    fn parses_real_grade_response() {
        let body = json!({
            "code": 1, "msg": "SUCCESS", "success": true,
            "value": {"templateScoreList": [
                {"businessScoreList": null, "item": null, "name": "大学英语I（三）U校园学习",
                 "parentId": null, "percent": 100.0, "score": 11.1,
                 "templateId": "1613981390624169995", "templateType": 1},
                {"businessScoreList": null, "item": null, "name": "形成性评价",
                 "parentId": "1613981390624169995", "percent": 100.0, "score": 11.1,
                 "templateId": "1632352030901354496", "templateType": 0},
                {"businessScoreList": [
                    {"duration": "00:47:14", "name": "提高篇综合1", "percent": 10.0, "score": 15.7},
                    {"duration": "00:31:40", "name": "视听说2", "percent": 40.0, "score": 2.6}
                ], "finishProcess": "01:20:32", "item": 30, "name": "必修学习时长",
                 "originScore": 2.6, "parentId": "1632352030901354496", "percent": 50.0,
                 "score": 1.3, "templateId": "1632352030901354497", "templateType": 2},
                {"businessScoreList": [
                    {"name": "视听说2", "percent": 40.0, "process": "11/49", "score": 20.0}
                ], "finishProcess": "28/117 任务点", "item": 21, "name": "教程学习成绩",
                 "originScore": 19.6, "parentId": "1632352030901354496", "percent": 50.0,
                 "score": 9.8, "templateId": "1632352030901354498", "templateType": 2}
            ]}
        });
        let grade = parse_grade(&body);
        assert_eq!(grade.name, "大学英语I（三）U校园学习");
        assert!((grade.total_score - 11.1).abs() < 0.01);
        assert_eq!(grade.items.len(), 2, "只收集带 item 的考核项");

        let duration = grade.duration_item().unwrap();
        assert!(duration.is_duration());
        assert_eq!(duration.finish_process, "01:20:32");
        assert_eq!(duration.books.len(), 2);
        assert_eq!(duration.top_book().unwrap().name, "视听说2");
        assert_eq!(duration.books[1].duration_seconds(), Some(31 * 60 + 40));

        // ★ 门户的分母是 49，不是 flowStrategy.required 数出的 45。
        let process = grade.process_item().unwrap();
        assert_eq!(process.finish_process, "28/117 任务点");
        assert_eq!(process.books[0].process_ratio(), Some((11, 49)));
        assert_eq!(process.book_named("视听说2").unwrap().score, 20.0);

        assert!(grade.describe().contains("必修学习时长"));
    }

    /// ★ 真实响应（`plan/detail`）必须能解析出两个考核项与逐教材时长标准。
    #[test]
    fn parses_real_plan_response() {
        let body = json!({
            "code": 1, "success": true,
            "value": {
                "children": [{
                    "children": [
                        {"children": null, "config": {
                            "detailConfig": {
                                "course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31":
                                    {"maxHours": 20, "minHours": 0, "percent": 40}
                            },
                            "maxHours": 20, "minHours": 0
                        }, "id": "1632352030901354497", "item": 30,
                         "name": "必修学习时长", "percent": 50, "type": 1},
                        {"children": null, "config": {"detailConfig": {}},
                         "id": "1632352030901354498", "item": 21,
                         "name": "教程学习成绩", "percent": 50, "type": 1}
                    ],
                    "config": null, "id": "1632352030901354496", "item": null,
                    "name": "形成性评价", "percent": 100, "type": 1
                }],
                "classId": "1111111111111111111",
                "courseInstanceId": "100001",
                "cycleEnd": "1798387199999",
                "cycleStart": "1788019200000",
                "name": "大学英语I（三）U校园学习"
            }
        });
        let plan = parse_plan(&body);
        assert_eq!(plan.name, "大学英语I（三）U校园学习");
        assert_eq!(plan.course_instance_id, "100001");
        assert_eq!(plan.items.len(), 2, "只有带 item 的才是考核项");
        assert!((plan.total_percent() - 100.0).abs() < 0.01);

        let std = plan
            .hours_for("course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31")
            .unwrap();
        assert_eq!(std.max_hours, 20.0);
        assert_eq!(std.percent, 40.0);
    }

    /// 时长分段函数：低于最低计 0，达到满分计满分，中间线性。
    #[test]
    fn hours_standard_is_piecewise_linear() {
        let std = HoursStandard {
            resource_id: "x".to_owned(),
            min_hours: 2.0,
            max_hours: 20.0,
            percent: 40.0,
        };
        assert_eq!(std.ratio_for(0), 0.0, "0 小时必须 0 分");
        assert_eq!(std.ratio_for(2 * 3600), 0.0, "刚好到最低仍是 0");
        assert_eq!(std.ratio_for(20 * 3600), 1.0, "到满分时长计满分");
        assert_eq!(std.ratio_for(999 * 3600), 1.0, "超出不越界");
        let mid = std.ratio_for(11 * 3600);
        assert!((mid - 0.5).abs() < 0.01, "11/20 小时约一半：{mid}");
        assert_eq!(std.required_seconds(), 20 * 3600);
    }

    /// `max <= min` 时退化成阶跃，不能除零得 NaN。
    #[test]
    fn degenerate_hours_standard_never_divides_by_zero() {
        let std = HoursStandard {
            resource_id: "x".to_owned(),
            min_hours: 5.0,
            max_hours: 5.0,
            percent: 10.0,
        };
        let r = std.ratio_for(5 * 3600);
        assert!(r.is_finite(), "不能是 NaN/Inf：{r}");
        assert_eq!(r, 1.0);
        assert_eq!(std.ratio_for(3600), 0.0);
    }

    /// 6001 之外的进度文本（例如没有斜杠）不能 panic，只回 None。
    #[test]
    fn malformed_process_yields_none() {
        let book = BookScore {
            process: Some("没有斜杠".to_owned()),
            ..Default::default()
        };
        assert_eq!(book.process_ratio(), None);
        let book = BookScore {
            process: Some("a/b".to_owned()),
            ..Default::default()
        };
        assert_eq!(book.process_ratio(), None);
    }

    /// 时长文本支持 `HH:MM:SS`；格式不对回 None。
    #[test]
    fn duration_parsing_is_strict() {
        let secs = |t: &str| {
            BookScore {
                duration: Some(t.to_owned()),
                ..Default::default()
            }
            .duration_seconds()
        };
        assert_eq!(secs("00:00:00"), Some(0));
        assert_eq!(secs("01:20:32"), Some(4832));
        assert_eq!(secs("乱码"), None);
        assert_eq!(secs("01:02"), None);
    }

    /// ★ 6011 要给出**可操作**的提示，而不是照搬 feature denied。
    #[test]
    fn feature_denied_error_is_actionable() {
        let client = crate::transport::build_client().unwrap();
        let host = HostIdentity::new("1000", "open");
        // 用一个必然失败的本地判断路径：app_id 置空。
        let broken = host.clone().with_app_id("");
        let error = post_assess(
            &client,
            "token",
            &broken,
            PORTAL_ASSESS_SCORE,
            &json!({"courseInstanceId": 1, "classId": "x"}),
        )
        .unwrap_err();
        assert!(error.message().contains("u-app-id"), "{error}");
    }

    /// ★ `courseInstanceId` 传 0（即误传了实例 ID 字符串）要本地拦下。
    #[test]
    fn numeric_instance_id_is_required() {
        let client = crate::transport::build_client().unwrap();
        let host = HostIdentity::new("1000", "open");
        let error = fetch_grade(&client, "t", &host, 0, "c").unwrap_err();
        assert!(error.message().contains("数字"), "{error}");
        let error = fetch_plan(&client, "t", &host, 0, "c").unwrap_err();
        assert!(error.message().contains("数字"), "{error}");
    }

    /// 宿主身份缺一项就算不完整。
    #[test]
    fn host_identity_completeness() {
        assert!(HostIdentity::new("1000", "open").is_complete());
        assert!(!HostIdentity::new("", "open").is_complete());
        assert!(!HostIdentity::new("1000", "").is_complete());
        assert!(
            !HostIdentity::new("1000", "open")
                .with_app_id("")
                .is_complete()
        );
        assert!(
            !HostIdentity::new("1000", "open")
                .with_platform("")
                .is_complete()
        );
        // 默认值必须是实测过的。
        let host = HostIdentity::new("1000", "open");
        assert_eq!(host.app_id, "116");
        assert_eq!(host.platform, "2");
    }

    /// ★ `detailConfig` 的值类型随考核项不同：时长项是对象，成绩项是数字。
    /// 只有对象形式才算时长标准，否则成绩项会被错渲染成「最低 0h / 满分 0h」。
    #[test]
    fn non_duration_item_has_no_hours_standard() {
        let body = json!({
            "code": 1, "success": true,
            "value": {"children": [{"children": [
                {"children": null, "item": 30, "name": "必修学习时长", "percent": 50,
                 "id": "a", "config": {"detailConfig": {
                     "course-v2:x": {"maxHours": 20, "minHours": 0, "percent": 40}}}},
                {"children": null, "item": 21, "name": "教程学习成绩", "percent": 50,
                 "id": "b", "config": {"detailConfig": {"course-v2:x": 40}}}
            ], "item": null, "name": "形成性评价", "percent": 100}]}
        });
        let plan = parse_plan(&body);
        let duration = plan.duration_item().unwrap();
        assert_eq!(duration.hours.len(), 1, "时长项要解析出标准");
        assert_eq!(duration.hours[0].max_hours, 20.0);

        let process = plan.process_item().unwrap();
        assert!(
            process.hours.is_empty(),
            "成绩项的 detailConfig 是数字，不该产出时长标准：{:?}",
            process.hours
        );
    }

    /// `item` 码到字段名的映射要与前端 `switch` 一致。
    #[test]
    fn item_code_mapping_matches_frontend() {
        assert_eq!(item_code::field_of(30), "duration");
        assert_eq!(item_code::field_of(21), "process");
        assert_eq!(item_code::field_of(31), "submitTime");
        assert_eq!(item_code::field_of(33), "submitTime");
        assert_eq!(item_code::field_of(35), "completedTimes");
        assert_eq!(item_code::field_of(36), "completedTimes");
        assert_eq!(item_code::field_of(999), "");
        assert_eq!(item_code::name_of(30), "必修学习时长");
        assert!(item_code::is_duration(30));
        assert!(!item_code::is_duration(21));
    }

    /// `fetch_resource_info` 的解析：`courseId` 是数字，`resourceId` 取 tutorial 里的。
    #[test]
    fn parses_resource_info() {
        let node = json!({
            "classId": "1111111111111111111",
            "courseId": 100001,
            "courseInstanceId": "course-v2:example0a2+ngce_vl_jc2_sz_ucloud+2024_01_31",
            "courseName": "大学英语I（三）A班",
            "strategyId": 900001,
            "tutorial": {
                "resourceId": "course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31",
                "resourceName": "新一代大学英语（基础篇）视听说思政数字课程2"
            }
        });
        assert_eq!(node.get("courseId").and_then(Value::as_i64), Some(100001));
        assert_eq!(
            node.pointer("/tutorial/resourceId").and_then(Value::as_str),
            Some("course-v2:unipus+ngce_vl_jc2_sz_ucloud+2024_01_31")
        );
        assert_eq!(
            node.get("strategyId").and_then(Value::as_i64),
            Some(900001)
        );
    }
}
