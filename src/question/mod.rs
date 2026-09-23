//! 题型层：**一文件一题型**。
//!
//! ## 每个文件只负责自己那一种题
//!
//! 旧工程把 13 种题型的载荷逻辑塞在一个 140 行的 `to_judge()` 里，用一串
//! `if` 分支区分。改一种题要读完整段，否则不知道自己的分支有没有被前面吃掉。
//!
//! 这里改成 [`QuestionKind`] trait + 有序注册表：
//!
//! - 每种题一个文件，只实现 `matches` / `judge`（以及必要时 `completed_len`、
//!   `child_value`）；
//! - 分发顺序在 [`KINDS`] 里**集中声明一次**，一眼可见谁先谁后；
//! - 顺序是**语义的一部分**：`oral-state`（`question_type="oral-state"`,
//!   `reply_type="record"`）同时满足 `oral` 的条件，必须排在它**前面**，
//!   否则「只取最后一个子题」的规则会被逐子题规则覆盖。
//!
//! ## 三条不变量
//!
//! 1. **`isCompleted` 与 `thirdPartyJudges` 长度必须严格相等。**
//!    不等会让服务端返回 `code:300100`（少了）或
//!    `code:2 runtime error: index out of range`（多了）。
//! 2. **学习题不贡献任何一项**，但仍要出现在 `quesDatas` 里。
//! 3. **口语类的分数直接给满分。** `recordDetail.score` 由客户端自报、
//!    服务端照存（见 [`crate::question::record`]），所以口语 / 角色扮演
//!    统一走 [`record::full_marks_record`]。客观题的分由服务端判，
//!    不在这里碰。

pub mod banked_cloze;
pub mod dropdown;
pub mod fill_blank;
pub mod multi_choice;
pub mod objective;
pub mod oral;
pub mod oral_state;
pub mod parse;
pub mod record;
pub mod role_play;
pub mod sequence;
pub mod single_choice;
pub mod study;
pub mod subjective;
pub mod upload;

use serde_json::{Map, Value, json};

/// 主观题没有标准答案时补的占位作答。
///
/// ⚠️ **这句不是学生写的。** 主观题完成度（`real_score_pct`）只要求作答非空，
/// 空着就永远交不上；但把它呈现给用户时必须说清这是程序补的。
pub const SUBJECTIVE_PLACEHOLDER: &str = "[已提交]";

/// 题目元信息：提交所需的最小集合。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestionMeta {
    /// 内容接口里的题目 `id`，即 `instanceId`。
    ///
    /// ⚠️ **必须是字符串。** 19 位十进制数超过 `f64` 的 2^53，前端 JS
    /// `JSON.parse` 会把它舍入（实测 `...042765` 变 `...042800`）。
    pub instance_id: String,
    /// 题型，例如 `basic`、`video-dub`。
    pub question_type: String,
    /// 作答类型，例如 `singlechoice`、`record`。**判别题型优先看它。**
    pub reply_type: String,
    /// 前端 `category`；`0` 表示 `Study`（学习题）。
    ///
    /// 实测取值：`0` 学习题 / `1` 客观题 / `7` 口语 / `8` 主观题。
    /// **真实客观题是 `1` 不是 `0`** —— 按直觉填 0 会把所有客观题判成学习题。
    pub category: Option<i64>,
    /// 子题数量。
    pub child_count: usize,
    /// `contents` 里属于音频/视频的下标，用于写入播放进度。
    pub media_indices: Vec<usize>,
}

impl QuestionMeta {
    /// 是否学习题（**不贡献任何提交项**的那一类）。
    ///
    /// 前三条复刻前端 `isStudyQuestion()`：`video-popup` 例外地不算学习题；
    /// 其余看 `category == 0`（`Study`）或题型为 `discussion`/`rich-text-read`。
    ///
    /// `examples/probe_category.rs` 抽查三班《视听说2》89 道题证实：
    /// `category=0` 只出现在 `basic/default`、`vocabulary`、`exit-ticket` 上，
    /// 全是纯学习活动；真实客观题落在 `category=1`。
    ///
    /// 第四条是**本库的补充**（不在前端那个函数里）：纯内容块既没子题也没
    /// 作答类型，交不出任何项——见函数体里的说明。
    pub fn is_study(&self) -> bool {
        if self.question_type == "video-popup" {
            return false;
        }
        if self.category == Some(0) {
            return true;
        }
        if matches!(self.question_type.as_str(), "discussion" | "rich-text-read") {
            return true;
        }
        // ★ 纯内容块：**没有子题、也没有作答类型** —— 它根本不是题。
        //
        // `purecontent` 分组就是这种（实测它在 `leafs.required` 里算必修，
        // 但 `flowStrategy.required` 不认）。它交不出任何 `isCompleted` /
        // `thirdPartyJudges` 项，`quesDatas` 却要收——形状与学习题完全一致，
        // 所以按 `submitType = 2` 走「只记进度」那条路。
        //
        // ⚠️ 与「有作答类型但子题为 0」区分开：后者是**真题目**，前端
        // `getAll()` 对 `subQuesNum === 0` 会回退成 `[this.content]`（1 项），
        // 不能一并当 0 项——所以判据里必须带 `reply_type` 为空。
        self.child_count == 0 && self.reply_type.trim().is_empty()
    }

    /// 命中的题型实现。
    pub fn kind(&self) -> &'static dyn QuestionKind {
        kind_for(self)
    }

    /// 该题贡献的 `isCompleted` 项数。
    ///
    /// 学习题恒为 0。`oral-state` / `role-play` 恒为 1。其余按子题数展开，
    /// 且子题为 0 时仍计 1（前端对纯阅读页也会走一次 `getAll()`）。
    pub fn is_completed_len(&self) -> usize {
        if self.is_study() {
            return 0;
        }
        self.kind().completed_len(self)
    }

    /// 该题贡献的 `thirdPartyJudges` 项数。
    ///
    /// 恒等于 [`Self::is_completed_len`]——前端每个分支都同时 push 两边。
    pub fn judge_count(&self) -> usize {
        self.is_completed_len()
    }

    /// 该题型是否**可以提交**（会贡献一项）。
    ///
    /// 比「客观题」宽：口语、角色扮演、主观题、上传题都能交，只是拿不到判分。
    /// 学习题不贡献任何项，被排除。
    pub fn is_submittable(&self) -> bool {
        !self.is_study()
    }

    /// 是否口语/录音类题。
    ///
    /// 判定**优先看 `reply_type`**：前端那个 `case` 列的是 `a.xib.BasicOral`
    /// 这类枚举成员名，而枚举定义在另一个 vendor chunk 里，本仓库手上没有。
    /// 把它小写成 `"basic-oral"` 只是猜测——猜错会静默走进默认分支，
    /// 等于这个功能没生效。所以以**真实观测到**的 `reply_type == "record"` 为准，
    /// `question_type` 只作补充。
    pub fn is_oral(&self) -> bool {
        if self.reply_type == "record" {
            return true;
        }
        matches!(
            self.question_type.as_str(),
            "basic-oral"
                | "paragraph-follow"
                | "oral-personal-state"
                | "video-dub"
                | "oral-aloud"
                | "ai-spoken"
                | "sentencerecord"
                | "paragraphrecord"
                | "paragraphrecord2"
                | "video_dubbing"
        ) || self.question_type.contains("record")
            || self.question_type.contains("oral")
    }

    /// 是否「仅最后一个子题参与判分」的口语题（`oral-state`）。
    pub fn is_oral_state(&self) -> bool {
        self.question_type == "oral-state"
    }

    /// 是否角色扮演题。`payloads` 是**逐子题**的 record 数组，
    /// 但题目本身只算 1 项完成。
    pub fn is_role_play(&self) -> bool {
        matches!(self.question_type.as_str(), "role-play" | "roleplay")
    }

    /// 是否需要随作答带上**录音记录**（口语 / 角色扮演）。
    ///
    /// 这些题的 `record` 由客户端自报分数，也是唯一会被填满分的题型；
    /// 客观题由服务端判分，不该碰 `record`。
    pub fn is_recording(&self) -> bool {
        self.is_oral() || self.is_role_play()
    }

    /// 是否主观题（`replyType = text-area`）。
    ///
    /// ## 主观题**不评分，只算完成度**
    ///
    /// 实测（用户**手工**在网页上填的组 `u7g419`，服务端存回的原文）：
    ///
    /// ```json
    /// "student_answer": { "value": "你好", "reply_type": "text-area",
    ///                     "rule": "subjective", "payloads": [] }
    /// "__SUBMIT_INFO__.state": { "real_score_pct": 1,   ← 完成度 100%
    ///                            "score_pct": 0 }       ← 判分恒 0
    /// ```
    ///
    /// 即：完成度看 `real_score_pct`，它只要求「交了非空作答」；
    /// `score_pct` 对主观题结构性恒 `0`，不存在机评。
    pub fn is_subjective(&self) -> bool {
        self.reply_type == "text-area"
    }

    /// 是否多媒体上传题。
    pub fn is_multi_file_upload(&self) -> bool {
        self.question_type == "multiFileUpload"
            || self.reply_type == "multiFileUpload"
            || self.reply_type == "multimedia_upload"
    }
}

/// 一次判分上报所需的全部上下文。
pub struct JudgeContext<'a> {
    pub meta: &'a QuestionMeta,
    /// 每个子题的答案值，例如 `["D"]`、`["A","B","D"]`。
    pub values: &'a [Vec<String>],
    /// 该题内部的子题序号。
    pub child_index: usize,
    /// 随提交下发的版本信息（已序列化）。
    pub versions: &'a Value,
    /// `role-play` 的逐子题 `payloads` 数组。仅该题型使用。
    pub role_play_payloads: Option<&'a Value>,
}

impl JudgeContext<'_> {
    /// 该子题上报的 `value`（字符串形式，多值逗号连接）。
    pub fn judge_value(&self) -> String {
        self.values
            .get(self.child_index)
            .cloned()
            .unwrap_or_default()
            .join(",")
    }

    /// 本题的 `question_type`。
    pub fn question_type(&self) -> &str {
        &self.meta.question_type
    }

    /// 本题的 `reply_type`。
    pub fn reply_type(&self) -> &str {
        &self.meta.reply_type
    }
}

/// 一种题型的载荷实现。
///
/// 实现者只需回答两个问题：**我管哪些题**、**我的载荷长什么样**。
pub trait QuestionKind: Sync {
    /// 唯一名字，用于 CLI 列出题型与错误信息。
    fn name(&self) -> &'static str;

    /// 是否由本类型处理。注册表**按顺序**询问，先匹配者胜。
    fn matches(&self, meta: &QuestionMeta) -> bool;

    /// 贡献的 `isCompleted` 项数。默认按子题展开（至少 1 项）。
    fn completed_len(&self, meta: &QuestionMeta) -> usize {
        meta.child_count.max(1)
    }

    /// `answer` 里 `children[i].value` 的形状。
    ///
    /// 绝大多数题是**数组**；主观题必须是**字符串**——实测交空数组完
    /// 成度恒 0，这正是主观题一直「交不上」的原因。
    fn child_value(&self, raw: Vec<String>) -> Value {
        Value::Array(raw.into_iter().map(Value::String).collect())
    }

    /// 构造 `thirdPartyJudges` 的一项。
    fn judge(&self, ctx: &JudgeContext) -> Value;

    /// 一句话说明这种题的分数从哪来。CLI 用它打印「本组有哪些题型、怎么判」。
    fn scoring_note(&self) -> &'static str;
}

/// 题型注册表。**顺序即优先级**，改动前先读模块文档里的第 3 条注意事项。
///
/// `study` 与 `objective` 不在表内：前者由 [`QuestionMeta::is_study`] 提前拦下，
/// 后者是兜底。
pub static KINDS: &[&dyn QuestionKind] = &[
    // ⚠️ oral_state 必须排在 oral 之前：oral-state 的 reply_type 也是 "record"，
    //    也满足 oral 的条件，顺序反了就会丢掉「只取最后一个子题」的规则。
    &role_play::RolePlay,
    &oral_state::OralState,
    &oral::Oral,
    &upload::Upload,
    &subjective::Subjective,
    // 以下都是纯客观题：载荷形状相同（payloads 为空），
    // 分开成文件是为了让「支持哪些题型」一目了然，并让 CLI 能逐个列出。
    &single_choice::SingleChoice,
    &multi_choice::MultiChoice,
    &dropdown::Dropdown,
    &fill_blank::FillBlank,
    &banked_cloze::BankedCloze,
    &sequence::Sequence,
];

/// 找出负责某道题的实现。
pub fn kind_for(meta: &QuestionMeta) -> &'static dyn QuestionKind {
    if meta.is_study() {
        return &study::Study;
    }
    for kind in KINDS {
        if kind.matches(meta) {
            return *kind;
        }
    }
    &objective::Objective
}

/// 按名字查题型实现，供 CLI `--list-types` 与按类型过滤使用。
pub fn kind_by_name(name: &str) -> Option<&'static dyn QuestionKind> {
    if name == study::Study.name() {
        return Some(&study::Study);
    }
    if name == objective::Objective.name() {
        return Some(&objective::Objective);
    }
    KINDS.iter().copied().find(|kind| kind.name() == name)
}

/// 列出全部题型名（含兜底与学习题）。
pub fn all_kind_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = KINDS.iter().map(|kind| kind.name()).collect();
    names.push(objective::Objective.name());
    names.push(study::Study.name());
    names
}

/// 一道题的作答。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestionSubmission {
    pub meta: QuestionMeta,
    /// 每个子题的答案值，例如 `["D"]`、`["A","B","D"]`。
    pub values: Vec<Vec<String>>,
    /// 已播完的内容项下标（音频/视频）。空表示该题没有媒体。
    pub played: Vec<usize>,
    /// 作答版本，前端从 1 开始。
    pub answer_version: u32,
}

impl QuestionSubmission {
    /// 依据题目元信息构造，`answer_version` 取 1。
    ///
    /// 媒体下标默认**全部标记为已播完** —— 这就是「刷视频进度」的载体。
    pub fn new(meta: QuestionMeta, values: Vec<Vec<String>>) -> Self {
        let played = meta.media_indices.clone();
        Self {
            meta,
            values,
            played,
            answer_version: 1,
        }
    }

    /// 构造 `answer` 字段的字符串化 JSON。
    ///
    /// `progress` 的键是**内容项下标**（字符串形式），值恒为 `true`。
    /// 视频进度**没有独立接口**，就装在这里一起发。
    ///
    /// 主观题额外带 `progress.reply = true`：复刻前端 `saveReplyProgress(true)`
    /// —— 文本作答组件的完成事件写的就是 `content.progress.reply`，
    /// 与媒体下标那套 `progress[i]` 互不相干。
    ///
    /// `record` 对口语 / 角色扮演填**满分包**（与 `thirdPartyJudges` 里那份
    /// 一致——前端本来就是把同一个本地 record 两处都发），其余题型只留
    /// `{ "url": "" }` 的结构。
    pub fn answer_json(&self) -> String {
        let kind = self.meta.kind();
        let child_count = self.meta.child_count.max(1);
        let children: Vec<Value> = (0..child_count)
            .map(|index| {
                let raw = self.values.get(index).cloned().unwrap_or_default();
                json!({ "value": kind.child_value(raw), "isDone": true })
            })
            .collect();

        let mut progress = Map::new();
        for index in &self.played {
            progress.insert(index.to_string(), Value::Bool(true));
        }
        if self.meta.is_subjective() {
            progress.insert("reply".to_owned(), Value::Bool(true));
        }

        let record = if self.meta.is_recording() {
            record::full_marks_record("", None)
        } else {
            json!({ "url": "" })
        };

        json!({
            "value": [],
            "children": children,
            "progress": progress,
            "record": record,
        })
        .to_string()
    }

    /// 转成请求体里的 `quesDatas` 项。
    pub fn to_param(&self) -> Value {
        json!({
            "instanceId": self.meta.instance_id,
            "answer": self.answer_json(),
            "context": "{\"state\":\"submitted\"}",
            "contextVersion": 1,
            "answerVersion": self.answer_version,
        })
    }

    /// 构造一个 `thirdPartyJudges` 项。
    pub fn to_judge(
        &self,
        versions: &Value,
        child_index: usize,
        role_play_payloads: Option<&Value>,
    ) -> Value {
        let context = JudgeContext {
            meta: &self.meta,
            values: &self.values,
            child_index,
            versions,
            role_play_payloads,
        };
        self.meta.kind().judge(&context)
    }

    /// `role-play` 的逐子题 `payloads` 数组（前端 `Ut()` 里的 `i`）。
    ///
    /// 每个子题一个**满分** record——角色扮演的分数同样由客户端自报。
    pub fn role_play_payloads(&self) -> Value {
        let count = self.meta.child_count.max(1);
        Value::Array(
            (0..count)
                .map(|_| record::full_marks_record("", None))
                .collect(),
        )
    }

    /// 补齐主观题的空作答。
    ///
    /// 主观题没有服务端标准答案，`values` 必然是空的；而完成度
    /// （`real_score_pct`）要求作答**非空**，空着就永远交不上。
    ///
    /// 只动主观题，其他题型原样返回。
    pub fn fill_subjective_placeholder(mut self) -> Self {
        if !self.meta.is_subjective() {
            return self;
        }
        let count = self.meta.child_count.max(1);
        self.values.resize(count, Vec::new());
        for slot in self.values.iter_mut() {
            if slot.iter().all(|part| part.trim().is_empty()) {
                *slot = vec![SUBJECTIVE_PLACEHOLDER.to_owned()];
            }
        }
        self
    }
}

/// 提交时随 `thirdPartyJudges` 一起下发的版本信息。
///
/// 服务端并不强校验这组值（实测缺省也能通过），因此给出安全默认。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitVersions {
    /// 课程版本，前端取书目里的 `version`。
    pub course: String,
    /// 分组内容版本。
    pub content: String,
}

impl Default for SubmitVersions {
    fn default() -> Self {
        Self {
            course: String::new(),
            content: "0".to_owned(),
        }
    }
}

impl SubmitVersions {
    pub fn to_json(&self) -> Value {
        json!({
            "course": self.course,
            "group": 1,
            "template": 1,
            "answer": 3,
            "content": self.content,
        })
    }
}

/// 一次分组提交的完整载荷。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSubmission {
    pub course_instance_id: String,
    pub group_id: String,
    pub questions: Vec<QuestionSubmission>,
    pub versions: SubmitVersions,
}

impl GroupSubmission {
    /// `submitType`：该组全是学习题时为 `2`，否则为 `1`。
    pub fn submit_type(&self) -> i64 {
        if self.questions.iter().all(|q| q.meta.is_study()) {
            2
        } else {
            1
        }
    }

    /// 该分组是否**只有学习题**（整组都是 `category=0` 那类）。
    ///
    /// ## 这种组**必须交**，别跳过
    ///
    /// 学习题不贡献 `isCompleted` / `thirdPartyJudges`，两边都是空数组——
    /// 但它照样是一次**写请求**：服务端从 `quesDatas` 里的
    /// `answer.progress`（媒体已播下标）与 `isDone` 记进度。
    /// 前端就是这么干的：媒体播完触发 `EVENT_COMPLETE_PLAY_MEDIA_COMP`，
    /// 只要 `getSubmitType() === 2` 就自动提交。
    ///
    /// 曾经这里拿 [`Self::has_judgeable_question`] 当「不值得交」的判据，
    /// 于是纯学习内容的进度**永远不会被记录**。
    pub fn is_study_only(&self) -> bool {
        !self.questions.is_empty() && self.questions.iter().all(|q| q.meta.is_study())
    }

    /// 该分组里是否存在**可判分**的题（非学习题）。
    ///
    /// ⚠️ 它**不是**「值不值得提交」的判据——纯学习题组照样要交，见
    /// [`Self::is_study_only`]。它只回答「这次提交会不会贡献
    /// `isCompleted` / `thirdPartyJudges` 的项」。
    pub fn has_judgeable_question(&self) -> bool {
        self.questions.iter().any(|q| q.meta.is_submittable())
    }

    /// 本组涉及哪些题型（按注册表顺序去重）。
    pub fn kind_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for question in &self.questions {
            let name = question.meta.kind().name();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// 构造请求体。
    pub fn to_body(&self, open_id: &str) -> Value {
        let ques_datas: Vec<Value> = self
            .questions
            .iter()
            .map(QuestionSubmission::to_param)
            .collect();

        // 学习题既不贡献 isCompleted 也不贡献 thirdPartyJudges——
        // 前端 `Ut()` 的结果只在非学习题时才被 concat 进汇总。
        // 两者长度必须严格相等，否则服务端返回 300100 或直接越界。
        let versions = self.versions.to_json();
        let mut completed = Vec::new();
        let mut judges = Vec::new();
        for question in &self.questions {
            if question.meta.is_study() {
                continue;
            }
            for _ in 0..question.meta.is_completed_len() {
                completed.push(Value::Bool(true));
            }
            // role-play 整题只算 1 项，但它那一项的 `payloads` 需要**逐子题**的
            // record 数组，所以先把数组算好传进去（前端 `Ut()` 里的 `i`）。
            let role_play = question.role_play_payloads();
            for index in 0..question.meta.judge_count() {
                judges.push(question.to_judge(&versions, index, Some(&role_play)));
            }
        }

        debug_assert_eq!(
            completed.len(),
            judges.len(),
            "isCompleted 与 thirdPartyJudges 长度必须相等，否则服务端报 300100"
        );

        json!({
            "courseId": self.course_instance_id,
            "openId": open_id,
            "version": crate::endpoints::DEFAULT_MODE,
            "quesDatas": ques_datas,
            "groupId": self.group_id,
            "isCompleted": completed,
            "thirdPartyJudges": serde_json::to_string(&judges).unwrap_or_else(|_| "[]".to_owned()),
            "submitType": self.submit_type(),
            "associationGroupId": "",
            "associationUnitId": "",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn meta(
        question_type: &str,
        reply_type: &str,
        category: i64,
        children: usize,
    ) -> QuestionMeta {
        QuestionMeta {
            instance_id: "1431562773572042765".to_owned(),
            question_type: question_type.to_owned(),
            reply_type: reply_type.to_owned(),
            category: Some(category),
            child_count: children,
            media_indices: Vec::new(),
        }
    }

    /// ★ 分发顺序：`oral-state` 必须赢过 `oral`，否则「只取最后一个子题」失效。
    #[test]
    fn oral_state_wins_over_oral() {
        let state = meta("oral-state", "record", 7, 3);
        assert!(state.is_oral(), "它确实也满足 oral 的条件");
        assert_eq!(
            state.kind().name(),
            "oral-state",
            "但必须由 oral-state 处理"
        );
    }

    /// `role-play` 整题只算 1 项，即使有 4 个子题。
    #[test]
    fn role_play_contributes_single_slot() {
        let play = meta("role-play", "record", 7, 4);
        assert_eq!(play.is_completed_len(), 1);
        assert_eq!(play.judge_count(), 1);
    }

    /// `oral-state` 同样只算 1 项。
    #[test]
    fn oral_state_contributes_single_slot() {
        let state = meta("oral-state", "record", 7, 5);
        assert_eq!(state.is_completed_len(), 1);
    }

    /// 普通客观题逐子题展开。
    #[test]
    fn objective_expands_per_child() {
        let basic = meta("basic", "singlechoice", 1, 3);
        assert_eq!(basic.is_completed_len(), 3);
    }

    /// 子题为 0 的题仍计 1 项（前端对纯阅读页也走一次 `getAll()`）。
    #[test]
    fn zero_children_still_counts_one() {
        let empty = meta("basic", "singlechoice", 1, 0);
        assert_eq!(empty.is_completed_len(), 1);
    }

    /// ★ 学习题不贡献任何项，但仍由 `Study` 处理。
    #[test]
    fn study_contributes_nothing() {
        let vocab = meta("vocabulary", "", 0, 1);
        assert!(vocab.is_study());
        assert_eq!(vocab.is_completed_len(), 0);
        assert_eq!(vocab.kind().name(), "study");
        assert!(!vocab.is_submittable());
    }

    /// `video-popup` 例外地**不算**学习题。
    #[test]
    fn video_popup_is_not_study() {
        let popup = meta("video-popup", "", 0, 0);
        assert!(!popup.is_study());
        assert!(popup.is_submittable());
    }

    /// ★ 纯内容块（没子题、没作答类型）按学习题处理：不贡献项，但 `quesDatas` 照收。
    ///
    /// 这是 `purecontent` 分组的形状——它在 `leafs.required` 里算必修，
    /// 不这样处理就会去凑一个假的判分项（`submitType=1`）。
    #[test]
    fn content_only_block_behaves_like_study() {
        let block = QuestionMeta {
            question_type: "purecontent".to_owned(),
            ..Default::default()
        };
        assert!(block.is_study(), "没有作答类型 ⇒ 交不出任何项");
        assert_eq!(block.is_completed_len(), 0);
        assert!(!block.is_submittable());

        // ★ 有作答类型、只是子题为 0 的**是真题目**：前端 `getAll()` 会回退成
        //    一项，所以它必须仍然算 1 项，别被上面那条规则吞掉。
        let empty_but_real = meta("basic", "singlechoice", 1, 0);
        assert!(!empty_but_real.is_study());
        assert_eq!(empty_but_real.is_completed_len(), 1);

        // 整组都是纯内容块 ⇒ submitType=2（只记进度）。
        let group = GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u5g204".to_owned(),
            questions: vec![QuestionSubmission::new(block, Vec::new())],
            versions: SubmitVersions::default(),
        };
        assert!(group.is_study_only());
        assert_eq!(group.submit_type(), 2);
        let body = group.to_body("openid");
        assert_eq!(body["isCompleted"].as_array().map(Vec::len), Some(0));
        assert_eq!(body["thirdPartyJudges"], json!("[]"));
        assert_eq!(
            body["quesDatas"].as_array().map(Vec::len),
            Some(1),
            "quesDatas 照收"
        );
    }

    /// `discussion` 即使 `category=1` 也仍是学习题。
    #[test]
    fn discussion_is_study_regardless_of_category() {
        let discussion = meta("discussion", "discussion", 1, 2);
        assert!(discussion.is_study());
    }

    /// 兄弟题型各自命中自己的文件，不互相吞。
    #[test]
    fn sibling_types_are_not_swallowed() {
        assert_eq!(
            meta("basic", "singlechoice", 1, 1).kind().name(),
            "single-choice"
        );
        assert_eq!(
            meta("basic", "multichoice", 1, 1).kind().name(),
            "multi-choice"
        );
        assert_eq!(
            meta("basic-scoop-content", "fillblank", 1, 1).kind().name(),
            "fill-blank"
        );
        assert_eq!(
            meta("basic", "dropdownSelection", 1, 1).kind().name(),
            "dropdown"
        );
        assert_eq!(meta("sequence", "sequence", 1, 1).kind().name(), "sequence");
        assert_eq!(meta("basic", "text-area", 8, 1).kind().name(), "subjective");
        assert_eq!(
            meta("multiFileUpload", "multiFileUpload", 1, 1)
                .kind()
                .name(),
            "upload"
        );
        assert_eq!(meta("basic-oral", "record", 7, 1).kind().name(), "oral");
    }

    /// 未知题型必须落到兜底实现，而不是 panic。
    #[test]
    fn unknown_type_falls_back_to_objective() {
        let weird = meta("magic_19_whatever", "something-new", 1, 2);
        assert_eq!(weird.kind().name(), "objective");
        assert_eq!(weird.is_completed_len(), 2);
    }

    /// ★ 三条不变量的核心：两个数组长度必须相等。
    #[test]
    fn completed_and_judges_lengths_always_match() {
        let group = GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u1g1".to_owned(),
            questions: vec![
                QuestionSubmission::new(meta("basic", "singlechoice", 1, 3), Vec::new()),
                QuestionSubmission::new(meta("role-play", "record", 7, 4), Vec::new()),
                QuestionSubmission::new(meta("oral-state", "record", 7, 5), Vec::new()),
                // 学习题：进 quesDatas，但不进任何一个数组。
                QuestionSubmission::new(meta("vocabulary", "", 0, 1), Vec::new()),
                QuestionSubmission::new(meta("basic-scoop-content", "fillblank", 1, 2), Vec::new()),
            ],
            versions: SubmitVersions::default(),
        };
        let body = group.to_body("openid");
        let completed = body["isCompleted"].as_array().map(Vec::len).unwrap_or(0);
        let judges: Vec<Value> =
            serde_json::from_str(body["thirdPartyJudges"].as_str().unwrap_or("[]")).unwrap();

        // 3 + 1 + 1 + 0(学习题) + 2 = 7
        assert_eq!(completed, 7, "3+1+1+2，学习题不计");
        assert_eq!(completed, judges.len(), "长度必须严格相等");
        assert_eq!(
            body["quesDatas"].as_array().map(Vec::len),
            Some(5),
            "quesDatas 收全部题"
        );
        assert_eq!(body["submitType"], json!(1));
    }

    /// 整组学习题时 `submitType=2`，且两个数组都是空的。
    #[test]
    fn study_only_group_uses_submit_type_two() {
        let group = GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u1g489".to_owned(),
            questions: vec![QuestionSubmission::new(
                meta("vocabulary", "", 0, 1),
                Vec::new(),
            )],
            versions: SubmitVersions::default(),
        };
        assert!(!group.has_judgeable_question());
        assert_eq!(group.submit_type(), 2);
        let body = group.to_body("openid");
        assert_eq!(body["isCompleted"].as_array().map(Vec::len), Some(0));
        assert_eq!(body["thirdPartyJudges"], json!("[]"));
    }

    /// ★ 主观题的 `value` 必须是字符串，不是数组。
    #[test]
    fn subjective_child_value_is_string() {
        let question = QuestionSubmission::new(
            meta("basic", "text-area", 8, 1),
            vec![vec!["你好".to_owned()]],
        );
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["children"][0]["value"], json!("你好"));
    }

    /// 客观题的 `value` 是数组。
    #[test]
    fn objective_child_value_is_array() {
        let question = QuestionSubmission::new(
            meta("basic", "multichoice", 1, 1),
            vec![vec!["A".to_owned(), "B".to_owned()]],
        );
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["children"][0]["value"], json!(["A", "B"]));
    }

    /// ★ 媒体下标写进 `progress`，键是内容项下标字符串。
    #[test]
    fn media_indices_land_in_progress() {
        let mut info = meta("basic", "singlechoice", 1, 1);
        info.media_indices = vec![0, 2];
        let question = QuestionSubmission::new(info, Vec::new());
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["progress"], json!({"0": true, "2": true}));
    }

    /// 主观题空作答补占位文本；非主观题不碰。
    #[test]
    fn placeholder_only_touches_subjective() {
        let subj = QuestionSubmission::new(meta("basic", "text-area", 8, 2), Vec::new())
            .fill_subjective_placeholder();
        assert_eq!(subj.values.len(), 2);
        assert!(
            subj.values
                .iter()
                .all(|slot| slot == &vec![SUBJECTIVE_PLACEHOLDER.to_owned()])
        );

        let obj = QuestionSubmission::new(
            meta("basic", "singlechoice", 1, 1),
            vec![vec!["D".to_owned()]],
        )
        .fill_subjective_placeholder();
        assert_eq!(obj.values, vec![vec!["D".to_owned()]]);
    }

    /// 按名字查题型，名字覆盖注册表全部成员。
    #[test]
    fn kind_by_name_covers_registry() {
        for name in all_kind_names() {
            assert!(kind_by_name(name).is_some(), "查不到题型：{name}");
        }
        assert!(kind_by_name("不存在的题型").is_none());
    }

    /// 每种题型都要能说出自己的分数从哪来，不能留空。
    #[test]
    fn every_kind_explains_scoring() {
        for name in all_kind_names() {
            let kind = kind_by_name(name).unwrap();
            assert!(
                !kind.scoring_note().trim().is_empty(),
                "{name} 缺少评分说明"
            );
        }
    }

    /// ★ 口语 / 角色扮演的 `answer.record` 必须是满分包。
    #[test]
    fn answer_record_is_full_marks_for_recording_kinds() {
        for (question_type, reply_type) in [
            ("basic-oral", "record"),
            ("oral-state", "record"),
            ("role-play", "record"),
        ] {
            let question =
                QuestionSubmission::new(meta(question_type, reply_type, 7, 1), Vec::new());
            let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
            assert_eq!(
                answer["record"]["recordDetail"]["score"],
                json!(100),
                "{question_type} 的 answer.record 应是满分"
            );
            assert_eq!(
                answer["record"]["specific_scores"]["total"],
                json!(100),
                "{question_type}"
            );
        }
    }

    /// 客观题 / 主观题的 `answer.record` 保持 `{url:""}`，不塞分数
    /// ——它们的分数由服务端判，客户端报了反而可能盖掉真实判分。
    #[test]
    fn answer_record_stays_empty_for_server_judged_kinds() {
        for (question_type, reply_type, category) in [
            ("basic", "singlechoice", 1),
            ("basic", "text-area", 8),
            ("vocabulary", "", 0),
        ] {
            let question =
                QuestionSubmission::new(meta(question_type, reply_type, category, 1), Vec::new());
            let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
            assert_eq!(answer["record"], json!({"url": ""}), "{question_type}");
        }
    }

    /// ★ 端到端：口语类的 `thirdPartyJudges[].payloads[]` 里每个 record 都是满分，
    /// 角色扮演的逐子题数组也一个不漏。
    #[test]
    fn built_body_carries_full_marks_records() {
        let group = GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u5g341".to_owned(),
            questions: vec![
                QuestionSubmission::new(meta("sentencerecord", "record", 7, 1), Vec::new()),
                QuestionSubmission::new(meta("role-play", "record", 7, 3), Vec::new()),
                QuestionSubmission::new(meta("oral-state", "record", 7, 2), Vec::new()),
                QuestionSubmission::new(meta("basic", "singlechoice", 1, 1), Vec::new()),
            ],
            versions: SubmitVersions::default(),
        };
        let body = group.to_body("openid");
        let judges: Vec<Value> =
            serde_json::from_str(body["thirdPartyJudges"].as_str().unwrap_or("[]")).unwrap();
        assert_eq!(
            judges.len(),
            4,
            "1(oral) + 1(role-play) + 1(oral-state) + 1(客观)"
        );

        assert_eq!(
            judges[0]["payloads"][0]["recordDetail"]["score"],
            json!(100)
        );
        assert_eq!(
            judges[1]["payloads"].as_array().map(Vec::len),
            Some(3),
            "role-play 逐子题展开"
        );
        for record in judges[1]["payloads"].as_array().unwrap() {
            assert_eq!(record["recordDetail"]["score"], json!(100));
        }
        assert_eq!(
            judges[2]["payloads"][0]["recordDetail"]["score"],
            json!(100)
        );
        assert_eq!(
            judges[3]["payloads"],
            json!([]),
            "客观题 payloads 为空，分数由服务端判"
        );

        // `answer` 里那份 record 同样要带上，别只填 judges。
        let answers: Vec<Value> = body["quesDatas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| serde_json::from_str(item["answer"].as_str().unwrap_or("{}")).unwrap())
            .collect();
        assert_eq!(answers[0]["record"]["recordDetail"]["score"], json!(100));
        assert_eq!(answers[1]["record"]["recordDetail"]["score"], json!(100));
    }
}
