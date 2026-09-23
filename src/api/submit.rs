//! **提交作答**：`POST /course/api/v3/newExploration/submit`。
//!
//! ⚠️ **这是全库唯一会写服务端的接口**（`media` 的上传链尚未接入）。
//!
//! ## 请求体
//!
//! ```json
//! { "courseId": "course-v2:…", "openId": "<open_id>", "version": "default",
//!   "quesDatas": [{ "instanceId": "…", "answer": "<字符串化 JSON>",
//!                   "context": "{\"state\":\"submitted\"}",
//!                   "contextVersion": 1, "answerVersion": 1 }],
//!   "groupId": "u1g6", "isCompleted": [true],
//!   "thirdPartyJudges": "<字符串化 JSON 数组>",
//!   "submitType": 1, "associationGroupId": "", "associationUnitId": "" }
//! ```
//!
//! ## 五个必须守住的坑
//!
//! 1. **`instanceId` 必须取内容接口的 `id`**，且不经 `f64`
//!    （19 位超过 2^53）。见 [`crate::question::parse::number_text`]。
//! 2. **`answer` 与 `context` 是字符串化 JSON**，不是嵌套对象。
//! 3. **`progress` 的键是 `contents` 数组下标**，承载音视频「已播完」标记。
//! 4. **`thirdPartyJudges` 必须逐子题填，长度与 `isCompleted` 严格相等。**
//!    这是 `code:300100` 的**真正原因**——不是 `isCompleted` 长度，
//!    而是 `thirdPartyJudges` 发成了空数组：
//!
//!    | thirdPartyJudges | isCompleted | 结果 |
//!    | --- | --- | --- |
//!    | `[]` | 2 项 | `code:300100 题目数量不匹配` |
//!    | 2 项 | 2 项 | `code:0`，`score=[1,1]`，`score_pct=1` |
//!    | 2 项 | 1 项 | `code:2 runtime error: index out of range [1] with length 1` |
//!
//! 5. **`isCompleted` 长度随题型变化**，见 [`crate::question`]。
//!
//! ## 策略：一组只留首次记录
//!
//! `strategy.record_every_submit == false` **不代表不落库**：
//! 一组只留首次记录，后续提交不改写。所以重复刷同一组通常没有意义。

use reqwest::blocking::Client;
use serde_json::Value;
use std::time::Duration;

use crate::endpoints::submit_url;
use crate::error::{Error, Result};
use crate::question::{GroupSubmission, QuestionSubmission, SubmitVersions};
use crate::transport::{
    Auth, MAX_RATE_LIMIT_RETRIES, RATE_LIMIT_BACKOFF, Transport, body_message, is_rate_limited,
};

/// 提交结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// 业务码，`0` 表示服务端接受了本次提交。
    pub code: i64,
    pub message: String,
    /// `state.state`，`1` 表示判分通过。
    pub state: Option<i64>,
    /// 逐题得分展开后的文本。
    pub score: String,
    pub score_pct: String,
    /// 服务端为本次提交分配的版本号。
    pub version: String,
    /// `strategy.record_every_submit`：逐次留存。
    pub record_every_submit: bool,
    /// `strategy.record_max_submit`：只留最高分。
    pub record_max_submit: bool,
}

impl SubmitOutcome {
    /// 服务端是否接受了这次提交。
    pub fn accepted(&self) -> bool {
        self.code == 0
    }

    /// 是否被判分通过。
    pub fn passed(&self) -> bool {
        self.state == Some(1)
    }

    /// 是否处于限流状态。
    pub fn rate_limited(&self) -> bool {
        is_rate_limited(self.code)
    }

    /// 面向用户的一句话结论，并说明留存策略。
    pub fn verdict(&self) -> String {
        if self.rate_limited() {
            return format!("被限流：{}（等待后重试）", self.message);
        }
        if !self.accepted() {
            return format!("提交被拒绝：code={} {}", self.code, self.message);
        }
        let mut text = format!("服务端已接受（code=0 {}）", self.message);
        if self.passed() {
            text.push_str("，判分通过");
        }
        if self.record_every_submit {
            text.push_str("；策略为逐次留存");
        } else if self.record_max_submit {
            text.push_str("；策略为只留最高分");
        } else {
            text.push_str("；策略为一组只留首次记录（后续提交不改写）");
        }
        text
    }
}

/// 解析提交响应。
pub fn parse_response(body: &Value) -> SubmitOutcome {
    let data = body.get("data");
    let state = data.and_then(|value| value.get("state"));
    let strategy = data.and_then(|value| value.get("strategy"));

    SubmitOutcome {
        code: body.get("code").and_then(Value::as_i64).unwrap_or(-1),
        message: body_message(body, ""),
        state: state
            .and_then(|value| value.get("state"))
            .and_then(Value::as_i64),
        score: state
            .and_then(|value| value.get("score"))
            .map(number_text)
            .unwrap_or_default(),
        score_pct: state
            .and_then(|value| value.get("score_pct"))
            .map(number_text)
            .unwrap_or_default(),
        version: data
            .and_then(|value| value.get("version"))
            .map(number_text)
            .unwrap_or_default(),
        record_every_submit: strategy
            .and_then(|value| value.get("record_every_submit"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        record_max_submit: strategy
            .and_then(|value| value.get("record_max_submit"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn number_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// 组装一个分组的提交载荷：读内容 → 读标准答案 → 逐题填答案。
///
/// ## 这是「提交」的唯一正当入口
///
/// 它把 [`crate::api::content`] 与 [`crate::api::answer`] 的结果拼成
/// [`GroupSubmission`]：客观题用**服务端下发的标准答案**；
/// 口语 / 角色扮演的 `record` 由题型层填**满分**（`recordDetail.score`
/// 本来就完全由客户端自报，见 [`crate::question::record`]）；
/// 主观题补占位文本（见 [`QuestionSubmission::fill_subjective_placeholder`]）。
///
/// ## 标准答案读不到时**不报错**
///
/// 口语、主观题、上传题在这里就是没有标准答案（实测答案接口返回空）。
/// 所以答案失败只降级成「空答案表」，让这些题型仍能拿到完成度。
/// 但**内容读不到就必须失败**——没有题目就没有可提交的东西。
pub fn build_submission(
    client: &Client,
    token: &str,
    course_instance_id: &str,
    group_id: &str,
) -> Result<GroupSubmission> {
    let plain = crate::api::content::fetch_decrypted(client, token, course_instance_id, group_id)?;
    let metas = crate::question::parse::parse_questions(&plain)?;
    if metas.is_empty() {
        // 纯内容分组（`purecontent`）可能一条题目项都解析不出来。
        // ⚠️ 这时**没有实测过的载荷形状**可用（`quesDatas: []` 没测过），
        //    所以宁可明确失败，也不凭空发一个空包。
        return Err(Error::invalid(format!(
            "{group_id} 的内容里没有解析出任何题目项，无法提交\
             （纯内容分组若连内容块都没有，需要先补实测：`quesDatas: []` 是否被接受）"
        )));
    }

    // 答案缺失是正常情况（口语/主观题），降级为空表。
    let answers = crate::api::answer::fetch_decrypted(client, token, course_instance_id, group_id)
        .and_then(|text| crate::question::parse::parse_standard_answers(&text))
        .unwrap_or_default();

    let questions = metas
        .into_iter()
        .enumerate()
        .map(|(index, meta)| {
            let values = answers.get(index).cloned().unwrap_or_default();
            QuestionSubmission::new(meta, values).fill_subjective_placeholder()
        })
        .collect();

    Ok(GroupSubmission {
        course_instance_id: course_instance_id.to_owned(),
        group_id: group_id.to_owned(),
        questions,
        versions: SubmitVersions::default(),
    })
}

/// 提交一个分组，遇到限流按退避重试。
///
/// ⚠️ **`open_id` 是必填参数**，不是从 `submission` 里猜的。
/// 曾经这里有个只收 `submission` 的重载，靠一个返回空串的辅助函数凑
/// `open_id`——那会让所有提交带上空 `openId`，服务端要么拒要么记到错误的账号上。
/// 删掉了，只留这个必须显式传的版本。
pub fn submit(
    client: &Client,
    token: &str,
    submission: &GroupSubmission,
    open_id: &str,
) -> Result<SubmitOutcome> {
    let transport = Transport::new(client, Auth::Annotator(token));
    let body = submission.to_body(open_id);

    let mut attempt = 0u32;
    loop {
        let response = transport.post_json(&submit_url(), &body)?;
        let outcome = parse_response(&response);
        if outcome.rate_limited() && attempt < MAX_RATE_LIMIT_RETRIES {
            let wait = RATE_LIMIT_BACKOFF * (attempt + 1);
            std::thread::sleep(wait);
            attempt += 1;
            continue;
        }
        return Ok(outcome);
    }
}

/// 已组装并**通过本地校验**、但尚未发送的一个分组。
///
/// dry-run 与真提交共用这一层：预览时只打印 [`PreparedSubmission::body`]，
/// 真提交时把它发出去。两条路走的是**同一个**组装函数，所以预览看到的
/// 就是实际会发的东西——不会出现「预览挺好、发出去是另一份」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSubmission {
    /// 组装好的分组提交。
    pub submission: GroupSubmission,
    /// 已自检过的请求体。
    pub body: Value,
    /// 该组**只有学习题**（`submitType = 2`）。
    ///
    /// 这种组照样要发：它贡献不了分数，但**进度就靠这一次写请求**
    /// 记下去（见 [`GroupSubmission::is_study_only`]）。
    pub study_only: bool,
}

impl PreparedSubmission {
    /// 本题组的题目数（含学习题）。
    pub fn question_count(&self) -> usize {
        self.submission.questions.len()
    }

    /// 本次会贡献的完成项数（= `isCompleted` 长度 = `thirdPartyJudges` 长度）。
    ///
    /// 纯学习题组恒为 `0`——那两个数组本来就是空的。
    pub fn completed_count(&self) -> usize {
        self.body
            .get("isCompleted")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    }
}

/// 组装一个分组并做本地自检。**不发送**。
///
/// `open_id` 现在就传进来（而不是等到发送时），是为了让 dry-run 打印的
/// 请求体与真正会发出去的那份**逐字节一致**——预览里 `openId` 为空的话，
/// 预览就是在骗人。
///
/// ## 纯学习题组也返回 `Ok`，不再跳过
///
/// 学习题不贡献 `isCompleted` / `thirdPartyJudges`，但**进度是靠这次提交
/// 记下去的**：`quesDatas` 里带着 `answer.progress`（媒体已播下标）与
/// `isDone`，前端在媒体播完后正是用 `submitType = 2` 自动提交。
/// 跳过它等于进度永远不记——详见 [`GroupSubmission::is_study_only`]。
pub fn prepare(
    client: &Client,
    token: &str,
    course_instance_id: &str,
    group_id: &str,
    open_id: &str,
) -> Result<PreparedSubmission> {
    let submission = build_submission(client, token, course_instance_id, group_id)?;
    let study_only = submission.is_study_only();
    // 本地自检：长度不等会被服务端拒（300100），提前拦下。
    let body = submission.to_body(open_id);
    validate(&body)?;
    Ok(PreparedSubmission {
        submission,
        body,
        study_only,
    })
}

/// 一个分组的提交结果（含回读）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupOutcome {
    pub group_id: String,
    /// 业务码，`0` 表示服务端接受了本次提交。
    pub code: i64,
    pub accepted: bool,
    /// 回读判定是否达标。回读失败时为 `false`。
    pub passed: bool,
    pub rate_limited: bool,
    pub message: String,
    /// 提交的题目数（含学习题）。
    pub question_count: usize,
    /// 本次贡献的完成项数。
    pub completed_count: usize,
    /// 该组只有学习题（`submitType = 2`）：没有分数，只记进度。
    pub study_only: bool,
    /// 回读到的得分率原文。
    pub score_pct: String,
    /// 回读结论（`GroupState::verdict`）。回读失败时为空。
    pub readback: String,
}

impl GroupOutcome {
    /// 一句话结论。
    pub fn verdict(&self) -> String {
        if self.rate_limited {
            return format!("被限流：{}", self.message);
        }
        if !self.accepted {
            return format!("被拒绝：code={} {}", self.code, self.message);
        }
        let mut text = if self.study_only {
            format!(
                "已接受（纯学习题 {} 题，submitType=2 只记进度）",
                self.question_count
            )
        } else {
            format!(
                "已接受（{} 题 / {} 项）",
                self.question_count, self.completed_count
            )
        };
        if !self.readback.is_empty() {
            text.push_str(&format!("，回读：{}", self.readback));
        }
        text
    }
}

/// 批量提交的汇总。
///
/// 提交是**唯一写服务端**的操作，跑完一本教材可能几百组，所以结果必须
/// 逐组留痕、分类计数——只报一个「成功 N 组」没法排查是哪几组没上去。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// 真正发出去并拿到响应的分组。
    pub outcomes: Vec<GroupOutcome>,
    /// **已发出去**的纯学习题组（`submitType = 2`）。它们不贡献分数，
    /// 但进度就是靠这一次写请求记下去的。
    pub study_only: Vec<String>,
    /// 组装阶段就失败的分组（`(分组ID, 原因)`）。没发请求。
    pub build_failures: Vec<(String, String)>,
    /// 是否被中途停止（Ctrl-C 或 `should_stop`）。
    pub stopped_early: bool,
}

impl BatchReport {
    /// 尝试过的分组数。
    pub fn attempted(&self) -> usize {
        self.outcomes.len()
    }

    /// 服务端接受（`code == 0`）的分组数。
    pub fn accepted(&self) -> usize {
        self.outcomes.iter().filter(|item| item.accepted).count()
    }

    /// 回读判定达标的分组数。
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|item| item.passed).count()
    }

    /// 被拒绝的分组数（含限流）。
    pub fn rejected(&self) -> usize {
        self.outcomes.iter().filter(|item| !item.accepted).count()
    }

    /// 其中被限流的。
    pub fn rate_limited(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|item| item.rate_limited)
            .count()
    }

    /// 记一次提交结果。
    pub fn record(&mut self, outcome: GroupOutcome) {
        self.outcomes.push(outcome);
    }

    /// 记一次「纯学习题组已提交」（`submitType = 2`，只记进度）。
    pub fn record_study_only(&mut self, group_id: impl Into<String>) {
        self.study_only.push(group_id.into());
    }

    /// 记一次组装失败。
    pub fn record_build_failure(&mut self, group_id: impl Into<String>, reason: impl Into<String>) {
        self.build_failures.push((group_id.into(), reason.into()));
    }

    /// 是否有任何一组没能拿到 `code:0`。
    pub fn has_failure(&self) -> bool {
        !self.build_failures.is_empty() || self.rejected() > 0 || self.stopped_early
    }

    /// 面向用户的汇总文本。
    pub fn describe(&self) -> String {
        let mut text = format!(
            "尝试 {} 组：接受 {}，达标 {}，被拒 {}（其中限流 {}）",
            self.attempted(),
            self.accepted(),
            self.passed(),
            self.rejected(),
            self.rate_limited()
        );
        if !self.study_only.is_empty() {
            text.push_str(&format!(
                "；其中纯学习题 {} 组（只记进度，不产生分数）",
                self.study_only.len()
            ));
        }
        if !self.build_failures.is_empty() {
            text.push_str(&format!("；组装失败 {} 组", self.build_failures.len()));
        }
        if self.stopped_early {
            text.push_str("；**被中途停止**");
        }
        text
    }
}

/// 提交**一个**分组：组装 → 自检 → 发送 → 回读，撞限流就退避重试。
///
/// 返回的 [`GroupOutcome::study_only`] 为真时，该组只有学习题（`submitType = 2`）：
/// 不产生分数，但**进度靠这次提交记录**，所以照样发。
///
/// ## ⚠️ 批量链路**不要**用它，用 [`submit_one_once`]
///
/// 它内部会 `sleep(RATE_LIMIT_BACKOFF × n)`——单组命令这样最省事，但在并发
/// 批量里会把 worker 睡死。实测 `--jobs 16` 全撞限流后退避睡眠占满所有
/// worker，50 秒只推了 6 组，**比串行还慢**。退避该由调度器统一做。
pub fn submit_one(
    client: &Client,
    token: &str,
    course_instance_id: &str,
    group_id: &str,
    open_id: &str,
) -> Result<GroupOutcome> {
    let prepared = prepare(client, token, course_instance_id, group_id, open_id)?;
    let body = prepared.body.clone();
    let transport = Transport::new(client, Auth::Annotator(token));

    let mut attempt = 0u32;
    let outcome = loop {
        let response = transport.post_json(&submit_url(), &body)?;
        let outcome = parse_response(&response);
        if outcome.rate_limited() && attempt < MAX_RATE_LIMIT_RETRIES {
            std::thread::sleep(RATE_LIMIT_BACKOFF * (attempt + 1));
            attempt += 1;
            continue;
        }
        break outcome;
    };

    finish_outcome(
        client,
        token,
        course_instance_id,
        group_id,
        prepared,
        outcome,
        true,
    )
}

/// 提交一个分组，但**只发一次**：被限流就原样返回，一秒都不睡。
///
/// 给批量调度器用——它拿到 `rate_limited` 之后把该组**重排到队尾**，
/// 而不是原地睡 20~80 秒。这样并发度不会被一个撞限流的组拖垮。
///
/// `readback` 为 `false` 时**省掉最后那次回读 GET**：一组本该 4 次往返
/// （内容 → 答案 → 提交 → 回读），实测每次往返 70~100ms，回读占了 1/4。
/// 提交响应里 `data.state.score_pct` 已经是权威值，够判达标。
pub fn submit_one_once(
    client: &Client,
    token: &str,
    course_instance_id: &str,
    group_id: &str,
    open_id: &str,
    readback: bool,
) -> Result<GroupOutcome> {
    let prepared = prepare(client, token, course_instance_id, group_id, open_id)?;
    let body = prepared.body.clone();
    let transport = Transport::new(client, Auth::Annotator(token));
    let response = transport.post_json(&submit_url(), &body)?;
    let outcome = parse_response(&response);
    finish_outcome(
        client,
        token,
        course_instance_id,
        group_id,
        prepared,
        outcome,
        readback,
    )
}

/// 回读 + 组装 [`GroupOutcome`]（[`submit_one`] 与 [`submit_one_once`] 共用）。
fn finish_outcome(
    client: &Client,
    token: &str,
    course_instance_id: &str,
    group_id: &str,
    prepared: PreparedSubmission,
    outcome: SubmitOutcome,
    readback: bool,
) -> Result<GroupOutcome> {
    if !readback {
        // 不回读：用提交响应里的判分结论（`state.state` / `state.score_pct`）。
        return Ok(GroupOutcome {
            group_id: group_id.to_owned(),
            code: outcome.code,
            accepted: outcome.accepted(),
            passed: outcome.passed(),
            rate_limited: outcome.rate_limited(),
            message: outcome.message,
            question_count: prepared.question_count(),
            completed_count: prepared.completed_count(),
            study_only: prepared.study_only,
            score_pct: outcome.score_pct,
            readback: "（未回读，取提交响应的 state）".to_owned(),
        });
    }

    // 回读一次：服务端「接受」不等于「记下了」，也不等于「达标」。
    let readback = crate::api::group_state::fetch(client, token, course_instance_id, group_id);
    let (passed, score_pct, text) = match &readback {
        Ok(state) => (state.passed(), state.score_pct.clone(), state.verdict()),
        Err(_) => (false, String::new(), String::new()),
    };

    Ok(GroupOutcome {
        group_id: group_id.to_owned(),
        code: outcome.code,
        accepted: outcome.accepted(),
        passed,
        rate_limited: outcome.rate_limited(),
        message: outcome.message,
        question_count: prepared.question_count(),
        completed_count: prepared.completed_count(),
        study_only: prepared.study_only,
        score_pct,
        readback: text,
    })
}

/// 可取消的等待。
///
/// 阻塞式 reqwest **无法中途 abort**，所以「停止」的实现只能是
/// 缩短超时 + 分组级检查。见 [`crate::transport::CANCEL_TIMEOUT`]。
pub fn sleep_cancellable(should_stop: impl Fn() -> bool, total: Duration) -> bool {
    const SLICE: Duration = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total {
        if should_stop() {
            return false;
        }
        std::thread::sleep(SLICE.min(total - waited));
        waited += SLICE;
    }
    !should_stop()
}

/// 校验一个提交体是否自洽（两个数组等长）。
///
/// 提交前调用一次，把 `300100` 挡在本地——服务端报这个错时不会告诉你
/// 是哪道题错了。
pub fn validate(body: &Value) -> Result<()> {
    let completed = body
        .get("isCompleted")
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| Error::invalid("提交体缺少 isCompleted"))?;

    let judges_text = body
        .get("thirdPartyJudges")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("thirdPartyJudges 必须是字符串化 JSON"))?;
    let judges: Vec<Value> = serde_json::from_str(judges_text)
        .map_err(|error| Error::invalid(format!("thirdPartyJudges 不是合法 JSON：{error}")))?;

    if completed != judges.len() {
        return Err(Error::invalid(format!(
            "isCompleted({completed}) 与 thirdPartyJudges({}) 长度不相等，服务端会报 300100",
            judges.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionMeta, QuestionSubmission, SubmitVersions};
    use serde_json::json;

    fn meta(question_type: &str, reply_type: &str, category: i64, children: usize) -> QuestionMeta {
        QuestionMeta {
            instance_id: "1".to_owned(),
            question_type: question_type.to_owned(),
            reply_type: reply_type.to_owned(),
            category: Some(category),
            child_count: children,
            media_indices: Vec::new(),
        }
    }

    fn group(questions: Vec<QuestionSubmission>) -> GroupSubmission {
        GroupSubmission {
            course_instance_id: "course-v2:x".to_owned(),
            group_id: "u1g1".to_owned(),
            questions,
            versions: SubmitVersions::default(),
        }
    }

    /// 成功响应解析。
    #[test]
    fn parses_success_response() {
        let body = json!({
            "code": 0, "message": "成功",
            "data": {
                "version": 1790036971,
                "state": {"state": 1, "score": [1,1], "score_pct": 1},
                "strategy": {"record_every_submit": false, "record_max_submit": false},
            }
        });
        let outcome = parse_response(&body);
        assert!(outcome.accepted());
        assert!(outcome.passed());
        assert_eq!(outcome.score_pct, "1");
        assert_eq!(outcome.version, "1790036971");
        assert!(outcome.verdict().contains("一组只留首次记录"));
    }

    /// ★ 限流码识别。
    #[test]
    fn detects_rate_limit() {
        let limited = parse_response(&json!({"code": 600002, "message": "您的操作过于频繁"}));
        assert!(limited.rate_limited());
        assert!(!limited.accepted());
        assert!(limited.verdict().contains("被限流"));
    }

    /// `record_every_submit=true` → 逐次留存。
    #[test]
    fn reports_record_every_submit() {
        let body = json!({"code": 0, "data": {"strategy": {"record_every_submit": true}}});
        assert!(parse_response(&body).verdict().contains("逐次留存"));
    }

    /// `record_max_submit=true` → 只留最高分。
    #[test]
    fn reports_record_max_submit() {
        let body = json!({"code": 0, "data": {"strategy": {"record_max_submit": true}}});
        assert!(parse_response(&body).verdict().contains("只留最高分"));
    }

    /// 拒绝时给出 code。
    #[test]
    fn reports_rejection_code() {
        let outcome = parse_response(&json!({"code": 300100, "message": "题目数量不匹配"}));
        assert!(!outcome.accepted());
        let verdict = outcome.verdict();
        assert!(verdict.contains("300100"), "{verdict}");
        assert!(verdict.contains("题目数量不匹配"), "{verdict}");
    }

    /// ★ 本地校验：两个数组等长时通过。
    #[test]
    fn validate_accepts_balanced_body() {
        let submission = group(vec![QuestionSubmission::new(
            meta("basic", "singlechoice", 1, 3),
            Vec::new(),
        )]);
        let body = submission.to_body("openid");
        assert!(validate(&body).is_ok());
    }

    /// ★ 本地校验：长度不等必须拦下（把 300100 挡在本地）。
    #[test]
    fn validate_rejects_unbalanced_body() {
        // 手工造一个不平衡的提交体。
        let body = json!({
            "isCompleted": [true, true],
            "thirdPartyJudges": "[]",
        });
        let error = validate(&body).unwrap_err();
        assert!(error.message().contains("300100"), "{error}");
        assert!(error.message().contains("2"), "{error}");
    }

    /// 缺少字段要报错而不是 panic。
    #[test]
    fn validate_reports_missing_fields() {
        assert!(validate(&json!({"thirdPartyJudges": "[]"})).is_err());
        assert!(validate(&json!({"isCompleted": []})).is_err());
    }

    /// `thirdPartyJudges` 必须是**字符串**，不是数组。
    #[test]
    fn validate_requires_stringified_judges() {
        let body = json!({"isCompleted": [], "thirdPartyJudges": []});
        let error = validate(&body).unwrap_err();
        assert!(error.message().contains("字符串化"), "{error}");
    }

    /// ★ 端到端：造出的提交体必须过本地校验。
    #[test]
    fn generated_body_always_passes_validation() {
        let submission = group(vec![
            QuestionSubmission::new(meta("basic", "singlechoice", 1, 3), Vec::new()),
            QuestionSubmission::new(meta("role-play", "record", 7, 4), Vec::new()),
            QuestionSubmission::new(meta("oral-state", "record", 7, 5), Vec::new()),
            QuestionSubmission::new(meta("vocabulary", "", 0, 1), Vec::new()),
            QuestionSubmission::new(meta("basic", "text-area", 8, 1), Vec::new())
                .fill_subjective_placeholder(),
        ]);
        let body = submission.to_body("openid");
        assert!(validate(&body).is_ok(), "生成的提交体必须自洽：{body}");
    }

    /// 可取消等待：立即要求停止时马上返回 false。
    #[test]
    fn cancellable_sleep_stops_immediately() {
        let start = std::time::Instant::now();
        let completed = sleep_cancellable(|| true, Duration::from_secs(5));
        assert!(!completed, "要求停止时不应报告完成");
        assert!(start.elapsed() < Duration::from_secs(1), "应立刻返回");
    }

    /// 不要求停止时会睡满。
    #[test]
    fn cancellable_sleep_runs_to_completion() {
        let completed = sleep_cancellable(|| false, Duration::from_millis(250));
        assert!(completed);
    }

    fn outcome(group_id: &str, code: i64, passed: bool) -> GroupOutcome {
        GroupOutcome {
            group_id: group_id.to_owned(),
            code,
            accepted: code == 0,
            passed,
            rate_limited: is_rate_limited(code),
            message: String::new(),
            question_count: 3,
            completed_count: 4,
            study_only: false,
            score_pct: if passed {
                "1".to_owned()
            } else {
                String::new()
            },
            readback: if passed {
                "已完成".to_owned()
            } else {
                String::new()
            },
        }
    }

    /// ★ 汇总必须分类计数：接受 / 达标 / 被拒 / 限流各不相同。
    #[test]
    fn batch_report_counts_each_class_separately() {
        let mut report = BatchReport::default();
        report.record(outcome("u5g1", 0, true));
        report.record(outcome("u5g2", 0, false));
        report.record(outcome("u6g1", 300100, false));
        report.record(outcome("u6g2", 600002, false));
        report.record_study_only("u5g3");
        report.record_build_failure("u7g1", "内容里没有题目");

        assert_eq!(report.attempted(), 4);
        assert_eq!(report.accepted(), 2, "只有 code=0 算接受");
        assert_eq!(report.passed(), 1, "接受不等于达标");
        assert_eq!(report.rejected(), 2);
        assert_eq!(report.rate_limited(), 1);
        assert_eq!(report.study_only.len(), 1);
        assert_eq!(report.build_failures.len(), 1);
        assert!(report.has_failure(), "有被拒的分组就必须自报失败");

        let text = report.describe();
        assert!(text.contains("尝试 4 组"), "{text}");
        assert!(text.contains("限流 1"), "{text}");
        assert!(text.contains("纯学习题 1 组"), "{text}");
        assert!(text.contains("组装失败 1 组"), "{text}");
    }

    /// 全绿时 `has_failure` 必须为假，`describe` 不提失败。
    #[test]
    fn clean_batch_reports_no_failure() {
        let mut report = BatchReport::default();
        report.record(outcome("u5g1", 0, true));
        report.record(outcome("u5g2", 0, true));
        assert!(!report.has_failure());
        assert!(!report.describe().contains("失败"));
    }

    /// ★ 被中途停止本身就要算失败——否则「交了一半」会被报成成功。
    #[test]
    fn stopped_early_is_a_failure() {
        let mut report = BatchReport::default();
        report.record(outcome("u5g1", 0, true));
        report.stopped_early = true;
        assert!(report.has_failure());
        assert!(
            report.describe().contains("被中途停止"),
            "{}",
            report.describe()
        );
    }

    /// 限流的分组在 `verdict` 里要说清是限流，而不是「被拒绝」。
    #[test]
    fn rate_limited_verdict_is_not_a_plain_rejection() {
        let verdict = outcome("u5g1", 600002, false).verdict();
        assert!(verdict.contains("被限流"), "{verdict}");
        assert!(!verdict.contains("被拒绝"), "{verdict}");
    }

    /// `PreparedSubmission` 的两个计数来自不同地方，都要对。
    #[test]
    fn prepared_submission_reports_both_counts() {
        use crate::question::QuestionSubmission;
        let submission = group(vec![
            QuestionSubmission::new(meta("basic", "singlechoice", 1, 3), Vec::new()),
            QuestionSubmission::new(meta("vocabulary", "", 0, 1), Vec::new()),
        ]);
        let body = submission.to_body("openid");
        let prepared = PreparedSubmission {
            submission,
            body,
            study_only: false,
        };
        assert_eq!(prepared.question_count(), 2, "题目数含学习题");
        assert_eq!(prepared.completed_count(), 3, "完成项不含学习题");
        assert_eq!(prepared.completed_count(), {
            let judges: Vec<Value> =
                serde_json::from_str(prepared.body["thirdPartyJudges"].as_str().unwrap_or("[]"))
                    .unwrap();
            judges.len()
        });
    }

    /// ★ 整组学习题：**照样要交**（`submitType=2`），只是两边数组都空。
    ///
    /// 曾经的判据是「没有可判分题就不交」，结果是纯学习内容的进度
    /// 永远不会被记录——前端在媒体播完后正是用 `submitType=2` 提交的。
    #[test]
    fn study_only_group_is_submitted_with_type_two() {
        let only_study = group(vec![QuestionSubmission::new(
            meta("vocabulary", "", 0, 1),
            Vec::new(),
        )]);
        assert!(only_study.is_study_only(), "整组都是学习题");
        assert!(!only_study.has_judgeable_question(), "确实没有可判分题");
        assert_eq!(only_study.submit_type(), 2, "整组学习题用 submitType=2");

        let body = only_study.to_body("openid");
        assert_eq!(body["submitType"], json!(2));
        assert_eq!(
            body["isCompleted"].as_array().map(Vec::len),
            Some(0),
            "学习题不贡献完成项"
        );
        assert_eq!(body["thirdPartyJudges"], json!("[]"));
        assert_eq!(
            body["quesDatas"].as_array().map(Vec::len),
            Some(1),
            "但 quesDatas 照收——进度就靠它记"
        );

        let mixed = group(vec![
            QuestionSubmission::new(meta("vocabulary", "", 0, 1), Vec::new()),
            QuestionSubmission::new(meta("basic", "singlechoice", 1, 1), Vec::new()),
        ]);
        assert!(!mixed.is_study_only(), "有非学习题就不是纯学习题组");
        assert!(mixed.has_judgeable_question(), "会贡献完成项");
        assert_eq!(mixed.submit_type(), 1);
    }

    /// ★ 纯学习题组里的媒体进度必须原样下发——那才是"看完"的记录。
    #[test]
    fn study_only_group_carries_media_progress() {
        let mut info = meta("vocabulary", "", 0, 1);
        info.media_indices = vec![0, 1];
        let question = QuestionSubmission::new(info, Vec::new());
        let answer: Value = serde_json::from_str(&question.answer_json()).unwrap();
        assert_eq!(answer["progress"], json!({"0": true, "1": true}));
        assert_eq!(answer["children"][0]["isDone"], json!(true));
    }

    /// 纯学习题组的回读结论要说清"只记进度"，不能报成有分数的提交。
    #[test]
    fn study_only_verdict_says_progress_not_score() {
        let mut item = outcome("u1g489", 0, false);
        item.study_only = true;
        item.completed_count = 0;
        let verdict = item.verdict();
        assert!(verdict.contains("纯学习题"), "{verdict}");
        assert!(verdict.contains("只记进度"), "{verdict}");
    }
}
