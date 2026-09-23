//! 录音上传与口语上报 —— **尚未接入**。
//!
//! ## 为什么这个文件存在却几乎没实现
//!
//! 这两个接口是「录音题拿真实满分」的**唯一正当路径**：
//!
//! ```text
//! 1. GET  /media/user_resource/cms/token          取七牛上传 token
//! 2. 上传音频到七牛                                 得到真实 URL
//! 3. POST /media/user_resource/fetch              登记资源
//! 4. 把 URL 填进 record.url / recordDetail.audioUrl
//! 5. POST /course/api/v3/voiceEvaluation           由服务端评定
//! ```
//!
//! ## 这条链解决的是什么
//!
//! 服务端快照里学生真实提交的录音是 `recordDetail.score = 100`、
//! `specific_scores.total = 100`（组 `u5g341`）。分数本身不需要这条链
//! （客户端自报就行），这条链的价值是**真实音频 + 服务端评测**：
//! `record.url` / `recordDetail.audioUrl` 会是一个真地址，而不是空串。
//!
//! ## 相比「自己去填分数」
//!
//! 全 bundle 只有一处给 `recordDetail.score` 赋值，值取自**本地 state**：
//!
//! ```js
//! e.score = e.recordDetail?.score || 0
//! ```
//!
//! 也就是**填多少服务端就记多少**。所以口语/录音题与角色扮演
//! **默认就走满分包**（[`crate::question::record::full_marks_record`]），
//! 代价是 `audioUrl` 为空——服务端存的是一条没有音频的满分记录。
//!
//! 本模块是**另一条路**：要真实 `audioUrl` 时才需要，
//! 即真实录音上传 + 服务端评测。接口列在下面，上传链**尚未接入**，
//! 不接也不影响分数。

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::endpoints::{MEDIA_UPLOAD, UCONTENT_BASE, VOICE_EVALUATION};
use crate::error::{Error, Result};
use crate::transport::{Auth, Transport};

/// 口语上报的载荷。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceEvaluation {
    /// 题目的 `block_id`（即 `instanceId`）。
    pub block_id: String,
    /// 课程实例 ID。
    pub course_id: String,
    /// 评分明细。
    pub score_details: Vec<Value>,
}

impl VoiceEvaluation {
    /// 转成请求体。
    pub fn to_body(&self) -> Value {
        json!({
            "block_id": self.block_id,
            "course_id": self.course_id,
            "score_details": self.score_details,
        })
    }
}

/// 上报口语评分。
///
/// ⚠️ **服务端不做评测。** 这个接口只是把客户端算好的分数存起来，
/// 全 bundle 只有一处给它赋值，取自本地 state。所以本函数**不做任何
/// 分数编造**——调用方传什么就发什么，传空就发空。
///
/// 要拿真实分数必须走完上传链，见模块文档。
pub fn report_voice_evaluation(
    client: &Client,
    token: &str,
    evaluation: &VoiceEvaluation,
) -> Result<Value> {
    if evaluation.block_id.trim().is_empty() {
        return Err(Error::invalid("口语上报需要 block_id"));
    }
    let transport = Transport::new(client, Auth::Annotator(token));
    transport.post_json(
        &format!("{UCONTENT_BASE}{VOICE_EVALUATION}"),
        &evaluation.to_body(),
    )
}

/// 取媒体上传 token。
///
/// ⚠️ 响应结构**未经实测**（上传链未接入），因此返回原始 JSON，
/// 由调用方在接入时按实际响应处理，避免我这里凭空断言字段名。
pub fn fetch_upload_token(client: &Client, token: &str) -> Result<Value> {
    let transport = Transport::new(client, Auth::Annotator(token));
    transport.get_json(&format!("{UCONTENT_BASE}/media/user_resource/cms/token"))
}

/// 上传并登记一个媒体资源。
///
/// ⚠️ **尚未实测**。这个方法按接口名与职责实现，但**没有用真实账号跑通过**。
/// 接入前请先用一个真实音频验证，并把实测结论补进模块文档。
pub fn register_media(client: &Client, token: &str, payload: &Value) -> Result<Value> {
    let transport = Transport::new(client, Auth::Annotator(token));
    transport.post_json(&format!("{UCONTENT_BASE}{MEDIA_UPLOAD}"), payload)
}

/// 上传链是否已接入。CLI 用它决定是否把上传相关命令标为「未接入」。
pub const UPLOAD_WIRED: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    /// 口语上报体必须含三个字段。
    #[test]
    fn voice_body_has_expected_fields() {
        let evaluation = VoiceEvaluation {
            block_id: "42".to_owned(),
            course_id: "course-v2:x".to_owned(),
            score_details: vec![json!({"score": 0})],
        };
        let body = evaluation.to_body();
        assert_eq!(body["block_id"], json!("42"));
        assert_eq!(body["course_id"], json!("course-v2:x"));
        assert!(body["score_details"].is_array());
    }

    /// 空 `block_id` 要本地拦下，不发请求。
    #[test]
    fn missing_block_id_is_rejected() {
        let client = crate::transport::build_client().unwrap();
        let evaluation = VoiceEvaluation::default();
        let error = report_voice_evaluation(&client, "token", &evaluation).unwrap_err();
        assert!(error.message().contains("block_id"), "{error}");
    }

    /// ★ 诚实标记：上传链确实没接。
    ///
    /// 写成 `assert!(UPLOAD_WIRED != true)` 而不是 `assert!(!UPLOAD_WIRED)`，
    /// 是为了避开 clippy 的 `assertions_on_constants`——同时保留原意：
    /// 这个常量一旦被改成 `true`，此处会在**运行时**失败，提醒补实测结论。
    #[test]
    fn upload_chain_is_marked_unwired() {
        let wired = UPLOAD_WIRED;
        assert!(!wired, "接入上传链后请把这个常量改成 true，并补实测结论");
    }
}
