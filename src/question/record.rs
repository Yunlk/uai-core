//! 口语 / 录音题的 `record` 包：**默认直接发满分**。
//!
//! ## 为什么可以这么发（协议层的事实）
//!
//! 全 bundle 只有一处给 `recordDetail.score` 赋值，就在 `saveRecord` 里，
//! 值取自**本地 state**：
//!
//! ```js
//! e.score = e.recordDetail?.score || 0    // POST /course/api/v3/voiceEvaluation
//! ```
//!
//! 没有任何代码把服务端响应写回该字段。`birdflock.unipus.cn` 也不是评测服务，
//! 它只把题库 URL 的 `qs-dev/`、`qs-test/` 前缀重写成 `qs-prod/`。
//!
//! **也就是说：这个字段填多少，服务端就记多少。**
//!
//! 更进一步，`UAI-Textbook-Console` 里实测过（`src/submit.rs`）：
//! 提交后服务端存下来的，就是我们在 `thirdPartyJudges[].payloads[]` 里
//! 塞的那个 `record` 对象——不是服务端自己生成的。
//!
//! ## 所以本库的默认行为是「发满分」
//!
//! [`full_marks_record`] 是口语/录音类的**默认**产出（见 [`super::oral`]、
//! [`super::oral_state`]、[`super::role_play`]），`audioUrl` 保持空串。
//!
//! 另一条路是「真实录音 + 服务端评测」：
//! `speech_eval`（只评分，能拿到 `total` 与真实 `audioUrl`）+
//! `media`（上传链，见 [`crate::api::media`]，尚未接入）。
//! 那条链要求手上有真实音频，本库不强制。
//!
//! [`default_record`]（全 0 的结构包）保留为对照与兜底，不再是默认。

use serde_json::{Map, Value, json};

/// 构造一个默认（全 0 分）的 `record`。
///
/// `role` 仅 `AISpoken` 分支使用，其余传 `None`。
///
/// ⚠️ 这已经不是口语题的默认产出了——默认走 [`full_marks_record`]。
/// 留在这里是为了：需要「只补结构、不报分」时有一个明确的名字可用。
pub fn default_record(url: &str, role: Option<&str>) -> Value {
    let mut record = json!({
        "url": url,
        "specific_scores": zero_scores(),
        "recordDetail": {
            "score": 0, "smooth": 0, "completed": 0, "correctness": 0,
            "audioUrl": url, "details": [], "relevance": 0,
        },
    });
    // `AISpoken` 分支额外带 `role`。
    if let Some(role) = role
        && let Some(map) = record.as_object_mut()
    {
        map.insert("role".to_owned(), Value::String(role.to_owned()));
    }
    record
}

/// 口语题通用的四项 `specific_scores`（全 0）。
pub fn zero_scores() -> Value {
    json!({ "total": 0, "accuracy": 0, "integrity": 0, "fluency": 0 })
}

/// 口语题通用的四项 `specific_scores`（满分）。
///
/// ⚠️ **量纲是 0~100，不是 0~1。** 实测服务端快照里真实录音存的是
/// `specific_scores.total = 100`；`speech_eval` 那边才需要折算成 0~1
/// （见 [`crate::api::speech_eval`] 的 `score_ratio`），两者别混。
pub fn full_marks_scores() -> Value {
    json!({ "total": 100, "accuracy": 100, "integrity": 100, "fluency": 100 })
}

/// 满分 `record`：口语 / 录音题与角色扮演的**默认**产出。
///
/// `url` 会同时写进 `url` 与 `recordDetail.audioUrl`（前端就是这么补的）。
/// 手上没有真实录音时传空串——分数照样是满分，因为服务端只认客户端报的值。
pub fn full_marks_record(url: &str, role: Option<&str>) -> Value {
    let mut record = default_record(url, role);
    if let Some(map) = record.as_object_mut() {
        map.insert("specific_scores".to_owned(), full_marks_scores());
        map.insert(
            "recordDetail".to_owned(),
            json!({
                "score": 100, "smooth": 100, "completed": 100, "correctness": 100,
                "audioUrl": url, "details": [], "relevance": 100,
            }),
        );
    }
    record
}

/// 空 map，用于少数分支需要的占位。
pub fn empty_map() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `default_record` 仍是全 0——它现在是「不报分」的显式选择。
    #[test]
    fn default_record_is_all_zero() {
        let record = default_record("", None);
        assert_eq!(record["specific_scores"]["total"], json!(0));
        assert_eq!(record["specific_scores"]["fluency"], json!(0));
        assert_eq!(record["recordDetail"]["score"], json!(0));
        assert_eq!(record["recordDetail"]["smooth"], json!(0));
        assert_eq!(record["recordDetail"]["completed"], json!(0));
        assert_eq!(record["recordDetail"]["correctness"], json!(0));
        assert_eq!(record["recordDetail"]["relevance"], json!(0));
    }

    /// ★ 满分包必须四项全 100，量纲是 0~100（不是 0~1）。
    #[test]
    fn full_marks_record_is_all_hundred() {
        let full = full_marks_record("", None);
        for key in ["total", "accuracy", "integrity", "fluency"] {
            assert_eq!(full["specific_scores"][key], json!(100), "{key}");
        }
        for key in ["score", "smooth", "completed", "correctness", "relevance"] {
            assert_eq!(full["recordDetail"][key], json!(100), "{key}");
        }
    }

    /// 满分包与全 0 结构包必须明显不同（否则「默认发满分」是句空话）。
    #[test]
    fn full_marks_record_differs_from_default() {
        assert_ne!(default_record("", None), full_marks_record("", None));
    }

    /// `audioUrl` 必须跟 `url` 一致——前端就是这么补的。
    #[test]
    fn audio_url_mirrors_url() {
        let record = default_record("http://x/a.mp3", None);
        assert_eq!(record["url"], json!("http://x/a.mp3"));
        assert_eq!(record["recordDetail"]["audioUrl"], json!("http://x/a.mp3"));

        let full = full_marks_record("http://x/a.mp3", None);
        assert_eq!(full["url"], json!("http://x/a.mp3"));
        assert_eq!(full["recordDetail"]["audioUrl"], json!("http://x/a.mp3"));
    }

    /// 不传 `role` 时不能凭空多出该字段；传了要带上（AISpoken 分支要用）。
    #[test]
    fn role_is_absent_by_default() {
        assert!(default_record("", None).get("role").is_none());
        assert_eq!(default_record("", Some("A")).get("role"), Some(&json!("A")));
        assert!(full_marks_record("", None).get("role").is_none());
        assert_eq!(
            full_marks_record("", Some("A")).get("role"),
            Some(&json!("A"))
        );
    }

    /// `details` 必须是空数组，不是 null。
    #[test]
    fn details_is_empty_array() {
        assert_eq!(
            default_record("", None)["recordDetail"]["details"],
            json!([])
        );
        assert_eq!(
            full_marks_record("", None)["recordDetail"]["details"],
            json!([])
        );
    }
}
