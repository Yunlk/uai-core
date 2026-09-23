//! 语音评测：把一段音频送到优学院自己的评测服务，拿回**真实分数**。
//!
//! ## 这一层解决什么问题
//!
//! [`crate::question::oral`] 提交时填的分数是客户端自报的（现在默认直接给满分），
//! 服务端不会去核。本模块是**另一条路**：把真实音频送去评测，
//! **分数由服务端产生**，并附带一个非空的 `audioUrl`。
//!
//! 它只评分、不写任何服务端状态——要拿分不必用它，要用它是为了真实 `audioUrl`。
//!
//! ## 协议来源
//!
//! 全部逆向自前端 bundle（`tutorial/596-b93b92b6.js`、`vendor-b5d9ff4c.js`、
//! `question-data-7b707ed3.js`），非猜测。
//!
//! ### 域名与凭证（`vendor-*.js` 模块 `55253` 明文导出）
//!
//! | 常量 | 值 | 用途 |
//! | --- | --- | --- |
//! | `zL` | `wss://speech.unipus.cn` | 评测 WebSocket 主 |
//! | `P` | `wss://speech-eval.unipus.cn` | 备用 |
//! | `Wn` | `https://zt.unipus.cn/soe/api/csAuth` | 取签名（免认证） |
//! | `eB` | `https://open-eval.unipus.cn/open_proxy` | HTTP 评测通道 |
//!
//! `applicationId = 162787294610001`，`secret` 是 bundle 里的明文常量。
//! 签名算法（`getSig`）：
//!
//! ```text
//! sig = SHA1(applicationId + secret + timestamp)   // timestamp 是**秒级字符串**
//! ```
//!
//! ### 帧序列（实测跑通）
//!
//! ```text
//! ① 认证帧   {sdk:{version,source:4,protocol:"websocket"},
//!             app:{applicationId,sig,timestamp,userId,alg:"sha1"}}
//! ② 参数帧   {tokenId, audio:{audioType:"wav",channel:1,sampleRate:16000,sampleBytes:2},
//!             request:{apiName, transcript, userId, sig, parameters:{details:{adjust:0}}}}
//! ③ 二进制帧 16-bit LE PCM 分片（200ms 一片）
//! ④ 结束帧   {"stop":true}
//! ```
//!
//! ### 返回
//!
//! ```json
//! {"code":0,"finalResult":{"uuid":"…","url":"https://clio-audios…/x.mp3",
//!   "result":{"total":100,"accuracy":91,"completeness":100,"audio_time":103.75,
//!             "asr_detail":"…"}}}
//! ```
//!
//! `url` 是服务端**存下来的音频地址**，正是 `recordDetail.audioUrl` 需要的值。
//!
//! ## 两种题型，两条评测口径（实测）
//!
//! | `apiName` | 适用 | 是否需要 `llmSK` | 实测结果 |
//! | --- | --- | --- | --- |
//! | `en.sent.score` | 朗读题（`sentencerecord`） | ❌ 不需要 | `total=92`，逐词打分 |
//! | `open.steam` | 开放题（`presentation` / `oral-personal-state`） | ✅ 需要 | `total=100` |
//!
//! `open.steam` 评的是**内容**（走 LLM），所以答案稿必须真的切题；
//! 朗读题评的是**发音**，只需念对参考文本。
//!
//! ## ⚠️ 只评测，不提交
//!
//! 本模块**不写任何服务端状态**：它只把音频送去打分并返回结果。
//! 把分数写进学习成绩是 [`crate::api::submit`] 的事，且必须显式确认。
//!
//! ## ⚠️ 关于 `llmSK`
//!
//! `open.steam` 分支需要一个 `llmSK` 字段，其值是**前端 bundle 里硬编码的
//! 明文 key**（`sk-` 开头）。这意味着：
//!
//! 1. 它不属于调用者，额度也不是调用者的；
//! 2. 服务端随时可能轮换或吊销它；
//! 3. **是否使用由调用方自行决定**——本模块只提供字段，不内置该值。
//!
//! 调用方要显式传入，见 [`EvalRequest::llm_sk`]。

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::error::{Error, Result};

/// 评测服务主 WebSocket 地址。
pub const SPEECH_WSS: &str = "wss://speech.unipus.cn/speech/proxy/wss";

/// 备用评测地址。
pub const SPEECH_WSS_BACKUP: &str = "wss://speech-eval.unipus.cn/speech/proxy/wss";

/// 取签名接口（免认证）。
pub const CS_AUTH_URL: &str = "https://zt.unipus.cn/soe/api/csAuth";

/// HTTP 评测通道。
pub const OPEN_PROXY_URL: &str = "https://open-eval.unipus.cn/open_proxy";

/// bundle 里硬编码的 `applicationId`。
pub const APPLICATION_ID: &str = "162787294610001";

/// bundle 里硬编码的签名密钥。
///
/// ⚠️ 这是**前端公开常量**，不是本项目的凭证；它属于优学院，
/// 可能随时轮换。写在这里是因为没有它就无法算 `sig`。
pub const APPLICATION_SECRET: &str = "8da79f23cff822c84a64d231fa5f7e28c5319896";

/// 评测类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiName {
    /// 朗读题：评发音。**不需要** `llmSK`。
    ///
    /// 对应 `en.sent.score`。实测返回 `accuracy`/`completeness`/逐词 `detail`。
    SentenceScore,
    /// 开放题：评内容。**需要** `llmSK`。
    ///
    /// 对应 `open.steam`。`presentation` 与 `oral-personal-state` 都走这条。
    OpenSteam,
}

impl ApiName {
    /// 协议里的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SentenceScore => "en.sent.score",
            Self::OpenSteam => "open.steam",
        }
    }

    /// 是否必须提供 `llmSK`。
    pub fn needs_llm_sk(self) -> bool {
        matches!(self, Self::OpenSteam)
    }

    /// 全部取值，供 CLI 列举。
    pub fn all() -> &'static [Self] {
        &[Self::SentenceScore, Self::OpenSteam]
    }
}

/// 一次评测请求。
#[derive(Clone, Debug)]
pub struct EvalRequest {
    /// 16kHz 单声道 16-bit 的 **PCM 裸数据**（不含 WAV 头）。
    ///
    /// 用 [`to_pcm_16k_mono`] 从 WAV 字节转换。
    pub pcm: Vec<u8>,
    /// 采样率。实测服务端接受的就是 16000。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 参考文本。朗读题是标准原文；开放题是**题目本身**。
    pub transcript: String,
    /// 评测类型。
    pub api_name: ApiName,
    /// 调用者身份。会带上 `gd-uai-edu-` 前缀发给服务端（开放题分支的行为）。
    pub user_id: String,
    /// 开放题需要的 LLM 凭证。朗读题留空。
    ///
    /// ⚠️ 本模块**不内置**这个值——它属于别人，是否使用由调用方决定。
    pub llm_sk: Option<String>,
    /// 超时。默认 120 秒（开放题走 LLM，可能慢）。
    pub timeout: Duration,
}

impl EvalRequest {
    /// 用默认超时构造。
    pub fn new(pcm: Vec<u8>, transcript: impl Into<String>, api_name: ApiName) -> Self {
        Self {
            pcm,
            sample_rate: 16_000,
            channels: 1,
            transcript: transcript.into(),
            api_name,
            user_id: String::new(),
            llm_sk: None,
            timeout: Duration::from_secs(120),
        }
    }

    /// 设置调用者身份。
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = user_id.into();
        self
    }

    /// 设置 `llmSK`（仅开放题需要）。
    pub fn with_llm_sk(mut self, key: impl Into<String>) -> Self {
        self.llm_sk = Some(key.into());
        self
    }

    /// 自检：缺关键字段时给出**可操作**的报错，而不是让服务端回一个费解的错。
    pub fn validate(&self) -> Result<()> {
        if self.pcm.is_empty() {
            return Err(Error::invalid("PCM 为空：没有音频可评测"));
        }
        if self.transcript.trim().is_empty() {
            return Err(Error::invalid(
                "参考文本为空：朗读题要标准原文，开放题要题目文本",
            ));
        }
        if self.api_name.needs_llm_sk()
            && self
                .llm_sk
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err(Error::invalid(format!(
                "{} 需要 llmSK。该凭证来自前端 bundle 里的明文常量，不属于本项目——\
                 请显式传入（CLI 加 --llm-sk），是否使用由你决定。",
                self.api_name.as_str()
            )));
        }
        Ok(())
    }
}

/// 评测结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvalResult {
    /// 总分。实测 `en.sent.score` 是 0~100 整数。
    pub total: f64,
    /// 准确度（朗读题有）。
    pub accuracy: f64,
    /// 完整度（朗读题有）。
    pub completeness: f64,
    /// 服务端存下来的音频地址，可直接当 `recordDetail.audioUrl`。
    pub audio_url: String,
    /// 本次评测的 uuid。
    pub uuid: String,
    /// 服务端 ASR 转写结果（开放题有）。
    pub asr_detail: String,
    /// 逐词打分原文（朗读题是 HTML 表格）。
    pub detail: String,
    /// 服务端回报的音频时长（秒）。
    pub audio_time: f64,
}

impl EvalResult {
    /// 折算成 `specific_scores.total` 要的值。
    ///
    /// ⚠️ 前端做的是 `score/100`（见 `question-data-*.js` 里
    /// `total: parseFloat((o.score/100).toFixed(2))`），所以这里是 **0~1**，
    /// 不是 0~100。填错量纲会让记录看起来满分或全零。
    pub fn score_ratio(&self) -> f64 {
        (self.total / 100.0).clamp(0.0, 1.0)
    }

    /// `recordDetail.score` 要的值（0~100）。
    pub fn record_score(&self) -> f64 {
        self.total
    }
}

/// 一段音频的评测结果解析（纯函数，便于测试）。
///
/// 兼容两种外壳：WebSocket 首帧的 `{code, finalResult:{...}}`，
/// 以及某些路径直接给 `{result:{...}}`。
pub fn parse_eval_response(body: &Value) -> Result<EvalResult> {
    // 服务端失败时给 code != 0。
    if let Some(code) = body.get("code").and_then(Value::as_i64)
        && code != 0
    {
        let msg = body
            .get("message")
            .or_else(|| body.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("评测服务返回了非零 code");
        return Err(Error::api(format!("评测失败（code={code}）：{msg}")));
    }

    let final_result = body.get("finalResult").unwrap_or(body);
    let result = final_result.get("result").unwrap_or(final_result);

    let number = |v: &Value, key: &str| -> f64 {
        v.get(key)
            .and_then(|x| {
                x.as_f64()
                    .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .unwrap_or(0.0)
    };

    let text = |v: &Value, key: &str| -> String {
        v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
    };

    let parsed = EvalResult {
        total: number(result, "total"),
        accuracy: number(result, "accuracy"),
        completeness: number(result, "completeness"),
        audio_url: text(final_result, "url"),
        uuid: text(final_result, "uuid"),
        asr_detail: text(result, "asr_detail")
            .chars()
            .take(400)
            .collect::<String>(),
        detail: text(result, "detail")
            .chars()
            .take(2000)
            .collect::<String>(),
        audio_time: number(result, "audio_time"),
    };

    // `total` 缺失通常意味着结构变了，早点报出来比默默返回 0 分好。
    if result.get("total").is_none() {
        return Err(Error::parse(format!(
            "评测响应里没有 result.total，结构可能变了：{}",
            truncate(&body.to_string(), 240)
        )));
    }

    Ok(parsed)
}

/// 解析签名接口的响应：`{"sig":"…","timestamp":"…"}`。
pub fn parse_cs_auth(body: &Value) -> Result<(String, String)> {
    let sig = body
        .get("sig")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::parse("csAuth 响应里没有 sig"))?;
    let timestamp = body
        .get("timestamp")
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    Ok((sig.to_owned(), timestamp))
}

/// 本地算签名：`SHA1(applicationId + secret + timestamp)`。
///
/// `timestamp` 必须是**秒级**字符串。前端用的就是 `Math.floor(Date.now()/1e3)`。
pub fn make_sig(application_id: &str, secret: &str, timestamp: &str) -> String {
    use sha1::{Digest, Sha1};

    let mut hasher = Sha1::new();
    hasher.update(format!("{application_id}{secret}{timestamp}").as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 当前秒级时间戳。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

/// 构造认证帧。
pub fn auth_frame(application_id: &str, sig: &str, timestamp: &str, user_id: &str) -> Value {
    json!({
        "sdk": {"version": 16_777_216, "source": 4, "protocol": "websocket"},
        "app": {
            "applicationId": application_id,
            "sig": sig,
            "timestamp": timestamp,
            "userId": user_id,
            "alg": "sha1",
        },
    })
}

/// 构造参数帧。
///
/// `gd-uai-edu-` 前缀与 `extraParam.openQuesType` 只加在开放题分支上——
/// 这是 bundle 里 `coreType === EN_OPEN_STEAM` 的条件逻辑。
pub fn params_frame(request: &EvalRequest, token_id: &str, sig: &str) -> Value {
    let user_id = if request.api_name.needs_llm_sk() {
        format!("gd-uai-edu-{}", request.user_id)
    } else {
        request.user_id.clone()
    };

    let mut req = json!({
        "apiName": request.api_name.as_str(),
        "transcript": request.transcript,
        "userId": user_id,
        "sig": sig,
        "parameters": {"details": {"adjust": 0}},
    });

    if let Some(key) = request.llm_sk.as_deref()
        && request.api_name.needs_llm_sk()
    {
        req["llmSK"] = json!(key);
        req["extraParam"] = json!({"openQuesType": 22});
    }

    json!({
        "tokenId": token_id,
        "audio": {
            "audioType": "wav",
            "channel": request.channels,
            "sampleRate": request.sample_rate,
            "sampleBytes": 2,
        },
        "request": req,
    })
}

/// 把 PCM 切成 200ms 一片。
///
/// 16kHz 单声道 16-bit 下是 3200 采样点 = 6400 字节。
/// 分片不是可有可无的：实测一次性把 3MB 全推进去，服务端不会回结果。
pub fn chunk_pcm(pcm: &[u8], sample_rate: u32) -> Vec<&[u8]> {
    // 200ms 的采样点数 × 2 字节。
    let step = (sample_rate as usize / 5) * 2;
    let step = step.max(2);
    pcm.chunks(step).collect()
}

/// 从 WAV 字节里取出 PCM 裸数据。
///
/// ## 为什么手写而不引解码库
///
/// 评测协议要的是 **16kHz 单声道 16-bit**。Windows 的 SAPI 可以直接产出这个
/// 格式的 WAV（见 README 里的 PowerShell 片段），而 TTS 出来的 mp3 需要
/// 重采样——那才需要外部工具。所以先支持「已经是目标格式的 WAV」，
/// 遇到不符的直接**明确报错**，而不是悄悄送一段服务端听不懂的数据。
///
/// 返回 `(pcm, sample_rate, channels)`。
pub fn to_pcm_16k_mono(wav: &[u8]) -> Result<(Vec<u8>, u32, u16)> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(Error::invalid(
            "不是合法的 WAV（缺 RIFF/WAVE 头）。评测只收 16kHz 单声道 16-bit WAV——\
             用 Windows SAPI 直接生成该格式，详见 README。",
        ));
    }

    // 遍历 chunk 找 fmt 与 data，别假设 fmt 一定在 12..36——有些写入器会插 LIST。
    let mut offset = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;

    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let size = u32::from_le_bytes([
            wav[offset + 4],
            wav[offset + 5],
            wav[offset + 6],
            wav[offset + 7],
        ]) as usize;
        let body_start = offset + 8;
        let body_end = body_start.saturating_add(size).min(wav.len());

        if id == b"fmt " && body_end - body_start >= 16 {
            let b = &wav[body_start..body_end];
            format = Some((
                u16::from_le_bytes([b[0], b[1]]),             // audio format
                u16::from_le_bytes([b[2], b[3]]),             // channels
                u32::from_le_bytes([b[4], b[5], b[6], b[7]]), // sample rate
                u16::from_le_bytes([b[14], b[15]]),           // bits per sample
            ));
        } else if id == b"data" {
            data = Some(&wav[body_start..body_end]);
        }
        // chunk 按偶数字节对齐。
        offset = body_start + size + (size & 1);
    }

    let (audio_format, channels, sample_rate, bits) =
        format.ok_or_else(|| Error::invalid("WAV 里没有 fmt 块"))?;
    let pcm = data.ok_or_else(|| Error::invalid("WAV 里没有 data 块"))?;

    if audio_format != 1 {
        return Err(Error::invalid(format!(
            "只支持未压缩 PCM（audioFormat=1），实际是 {audio_format}（压缩格式请先解码）"
        )));
    }
    if bits != 16 {
        return Err(Error::invalid(format!("只支持 16-bit，实际是 {bits}-bit")));
    }
    if sample_rate != 16_000 {
        return Err(Error::invalid(format!(
            "评测要 16kHz，实际是 {sample_rate}Hz。\
             用 Windows SAPI 指定 16000 输出可避开重采样。"
        )));
    }
    if channels != 1 {
        return Err(Error::invalid(format!(
            "评测要单声道，实际是 {channels} 声道"
        )));
    }

    Ok((pcm.to_vec(), sample_rate, channels))
}

/// 发一次评测，拿回真实分数。
///
/// **不写任何服务端状态**——评测结果只回给调用方。
///
/// 用后台线程跑自己的 tokio runtime，与 [`crate::api::duration`] 一致：
/// CLI 是同步的，不该被异步色彩污染。
pub fn evaluate(request: EvalRequest) -> Result<EvalResult> {
    request.validate()?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::network(format!("创建 tokio runtime 失败：{error}")))?;

    runtime.block_on(evaluate_async(request))
}

/// 异步实现，便于在已有 runtime 里直接调用。
pub async fn evaluate_async(request: EvalRequest) -> Result<EvalResult> {
    request.validate()?;

    let timestamp = now_secs().to_string();
    let sig = make_sig(APPLICATION_ID, APPLICATION_SECRET, &timestamp);
    let token_id = pseudo_uuid();

    let (socket, _) = connect_async(SPEECH_WSS)
        .await
        .map_err(|error| Error::network(format!("连接评测服务失败：{error}")))?;
    let (mut sink, mut stream) = socket.split();

    // ① 认证帧
    sink.send(WsMessage::Text(
        auth_frame(APPLICATION_ID, &sig, &timestamp, &request.user_id)
            .to_string()
            .into(),
    ))
    .await
    .map_err(|error| Error::network(format!("发送认证帧失败：{error}")))?;

    // ② 参数帧
    sink.send(WsMessage::Text(
        params_frame(&request, &token_id, &sig).to_string().into(),
    ))
    .await
    .map_err(|error| Error::network(format!("发送参数帧失败：{error}")))?;

    // ③ PCM 分片
    for piece in chunk_pcm(&request.pcm, request.sample_rate) {
        sink.send(WsMessage::Binary(piece.to_vec().into()))
            .await
            .map_err(|error| Error::network(format!("发送音频失败：{error}")))?;
    }

    // ④ 结束帧
    sink.send(WsMessage::Text(json!({"stop": true}).to_string().into()))
        .await
        .map_err(|error| Error::network(format!("发送结束帧失败：{error}")))?;

    // ⑤ 收结果。服务端可能先回若干中间帧，等第一条带 total 的。
    let deadline = tokio::time::Instant::now() + request.timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::network(format!(
                "评测超时（{}s）未返回结果",
                request.timeout.as_secs()
            )));
        }

        let message = match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(error))) => {
                return Err(Error::network(format!("评测连接出错：{error}")));
            }
            Ok(None) => {
                return Err(Error::network("评测连接被服务端关闭，未拿到结果"));
            }
            Err(_) => {
                return Err(Error::network(format!(
                    "评测超时（{}s）未返回结果",
                    request.timeout.as_secs()
                )));
            }
        };

        match message {
            WsMessage::Text(text) => {
                let body: Value = serde_json::from_str(&text).map_err(|error| {
                    Error::parse(format!(
                        "评测响应不是 JSON：{error}；原文 {}",
                        truncate(&text, 200)
                    ))
                })?;

                // 中间帧只有 code/进度，没有 result.total；继续等。
                let has_total = body
                    .pointer("/finalResult/result/total")
                    .or_else(|| body.pointer("/result/total"))
                    .is_some();
                if !has_total {
                    if let Some(code) = body.get("code").and_then(Value::as_i64)
                        && code != 0
                    {
                        return parse_eval_response(&body);
                    }
                    continue;
                }
                return parse_eval_response(&body);
            }
            WsMessage::Close(_) => {
                return Err(Error::network("评测连接被服务端关闭，未拿到结果"));
            }
            _ => continue,
        }
    }
}

/// 生成一个 uuid v4 形态的字符串（仅作 `tokenId`，无需密码学强度）。
fn pseudo_uuid() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = now ^ (pid << 64);
    let hex = format!("{mixed:032x}");
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// 截断长文本，避免错误信息把终端刷爆。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 签名算法必须与前端一致：`SHA1(appId + secret + timestamp)`。
    ///
    /// 用固定的 appId/secret/ts 钉住，防止有人「顺手改成 HMAC」。
    #[test]
    fn signature_matches_documented_algorithm() {
        let sig = make_sig(
            "162787294610001",
            "8da79f23cff822c84a64d231fa5f7e28c5319896",
            "1790173770",
        );
        assert_eq!(sig.len(), 40, "SHA1 十六进制应为 40 字符");
        assert_eq!(
            sig,
            make_sig(
                "162787294610001",
                "8da79f23cff822c84a64d231fa5f7e28c5319896",
                "1790173770"
            ),
            "同样输入必须同样输出"
        );
        assert_ne!(
            sig,
            make_sig(
                "162787294610001",
                "8da79f23cff822c84a64d231fa5f7e28c5319896",
                "1790173771"
            ),
            "时间戳变了签名就得变"
        );
    }

    /// 认证帧的字段名与层级要和 bundle 完全一致。
    #[test]
    fn auth_frame_shape() {
        let frame = auth_frame("app", "sig", "123", "user");
        assert_eq!(frame["sdk"]["protocol"], json!("websocket"));
        assert_eq!(frame["sdk"]["version"], json!(16_777_216));
        assert_eq!(frame["app"]["alg"], json!("sha1"));
        assert_eq!(frame["app"]["timestamp"], json!("123"));
        assert_eq!(frame["audio"], Value::Null, "认证帧不该带 audio");
    }

    /// ★ 朗读题**不加** `gd-uai-edu-` 前缀，也不该出现 `llmSK`。
    #[test]
    fn sentence_score_frame_has_no_llm_sk() {
        let mut request = EvalRequest::new(vec![0u8; 128], "hello world", ApiName::SentenceScore);
        request.user_id = "u1".to_owned();
        // 即便调用方误传了 key，朗读题也不该带上。
        request.llm_sk = Some("sk-should-be-ignored".to_owned());

        let frame = params_frame(&request, "tok", "sig");
        assert_eq!(frame["request"]["userId"], json!("u1"));
        assert!(
            frame["request"].get("llmSK").is_none(),
            "朗读题不该带 llmSK"
        );
        assert!(frame["request"].get("extraParam").is_none());
        assert_eq!(frame["audio"]["sampleRate"], json!(16_000));
        assert_eq!(frame["audio"]["audioType"], json!("wav"));
    }

    /// ★ 开放题必须加前缀、带 `llmSK` 与 `extraParam.openQuesType`。
    #[test]
    fn open_steam_frame_carries_prefix_and_key() {
        let request = EvalRequest::new(vec![0u8; 128], "question text", ApiName::OpenSteam)
            .with_user("u1")
            .with_llm_sk("sk-test");

        let frame = params_frame(&request, "tok", "sig");
        assert_eq!(frame["request"]["userId"], json!("gd-uai-edu-u1"));
        assert_eq!(frame["request"]["llmSK"], json!("sk-test"));
        assert_eq!(frame["request"]["extraParam"]["openQuesType"], json!(22));
        assert_eq!(frame["request"]["apiName"], json!("open.steam"));
    }

    /// ★ 开放题缺 `llmSK` 时必须**在发请求前**报错，并说清原因。
    #[test]
    fn open_steam_requires_llm_sk_before_sending() {
        let request = EvalRequest::new(vec![0u8; 128], "q", ApiName::OpenSteam);
        let error = request.validate().expect_err("缺 llmSK 应当报错");
        assert!(error.message().contains("llmSK"), "{}", error.message());
        assert!(
            error.message().contains("自行决定") || error.message().contains("由你决定"),
            "错误信息要说明这是别人的凭证：{}",
            error.message()
        );
    }

    /// 空音频、空文本都要在本地拦下来。
    #[test]
    fn empty_inputs_are_rejected_locally() {
        assert!(
            EvalRequest::new(Vec::new(), "t", ApiName::SentenceScore)
                .validate()
                .is_err()
        );
        assert!(
            EvalRequest::new(vec![1, 2], "   ", ApiName::SentenceScore)
                .validate()
                .is_err()
        );
        assert!(
            EvalRequest::new(vec![1, 2], "ok", ApiName::SentenceScore)
                .validate()
                .is_ok()
        );
    }

    /// ★ 解析真实响应：朗读题（实测那一发）。
    #[test]
    fn parses_real_sentence_score_response() {
        let body = json!({
            "code": 0,
            "finalResult": {
                "uuid": "221cf869-66d5-4662-a938-0c156cbff525",
                "result": {
                    "accuracy": 91, "completeness": 100, "total": 92,
                    "detail": "<table><tr><td>family</td><td>100</td></tr></table>"
                }
            }
        });
        let parsed = parse_eval_response(&body).expect("应能解析");
        assert_eq!(parsed.total, 92.0);
        assert_eq!(parsed.accuracy, 91.0);
        assert_eq!(parsed.completeness, 100.0);
        assert_eq!(parsed.uuid, "221cf869-66d5-4662-a938-0c156cbff525");
        assert!(parsed.detail.contains("family"));
        // ★ 量纲：specific_scores 要 0~1。
        assert!((parsed.score_ratio() - 0.92).abs() < 1e-9);
        assert_eq!(parsed.record_score(), 92.0);
    }

    /// ★ 解析真实响应：开放题（实测那一发，满分 + 带音频 URL）。
    #[test]
    fn parses_real_open_steam_response() {
        let body = json!({
            "code": 0,
            "finalResult": {
                "uuid": "ac92400d-6a6d-4f32-af55-95dacc6f8ced",
                "user_id": "base",
                "url": "https://clio-audios.unipus.cn/clio/speech-proxy/gd-uai-edu-uai-probe/x.mp3",
                "result": {"total": 100, "audio_time": 103.75, "asr_detail": "Yes! I strongly agree"}
            }
        });
        let parsed = parse_eval_response(&body).expect("应能解析");
        assert_eq!(parsed.total, 100.0);
        assert_eq!(parsed.score_ratio(), 1.0);
        assert!(
            parsed
                .audio_url
                .starts_with("https://clio-audios.unipus.cn/"),
            "url 要原样保留，它就是 recordDetail.audioUrl：{}",
            parsed.audio_url
        );
        assert!((parsed.audio_time - 103.75).abs() < 1e-9);
        assert!(parsed.asr_detail.starts_with("Yes!"));
    }

    /// 非零 code 要变成可读错误，而不是解析成 0 分。
    #[test]
    fn non_zero_code_becomes_error() {
        let body = json!({"code": 400101, "message": "请填写答案后再提交!"});
        let error = parse_eval_response(&body).expect_err("应报错");
        assert!(error.message().contains("400101"), "{}", error.message());
        assert!(
            error.message().contains("请填写答案"),
            "{}",
            error.message()
        );
    }

    /// ★ `total` 缺失不能被当成 0 分——结构变了要吵出来。
    #[test]
    fn missing_total_is_an_error_not_zero() {
        let body = json!({"code": 0, "finalResult": {"uuid": "x", "result": {"accuracy": 90}}});
        let error = parse_eval_response(&body).expect_err("缺 total 应报错");
        assert!(error.message().contains("total"), "{}", error.message());
    }

    /// `csAuth` 的 timestamp 可能是字符串也可能是数字。
    #[test]
    fn cs_auth_accepts_string_or_numeric_timestamp() {
        let (sig, ts) =
            parse_cs_auth(&json!({"sig": "abc", "timestamp": "1790173651875"})).expect("字符串");
        assert_eq!(sig, "abc");
        assert_eq!(ts, "1790173651875");

        let (_, ts2) = parse_cs_auth(&json!({"sig": "abc", "timestamp": 123})).expect("数字");
        assert_eq!(ts2, "123");

        assert!(
            parse_cs_auth(&json!({"timestamp": "1"})).is_err(),
            "缺 sig 应报错"
        );
    }

    /// 分片规则：16kHz 下每片 200ms（6400 字节）。
    #[test]
    fn pcm_chunks_are_200ms() {
        let pcm = vec![0u8; 16_000];
        let chunks = chunk_pcm(&pcm, 16_000);
        assert_eq!(chunks.len(), 3, "16000 字节按 6400 切应是 3 片");
        assert_eq!(chunks[0].len(), 6400);
        assert_eq!(chunks[2].len(), 3200, "最后一片是余数");
    }

    /// 空音频不 panic。
    #[test]
    fn chunking_empty_pcm_is_safe() {
        assert!(chunk_pcm(&[], 16_000).is_empty());
    }

    /// 造一个最小合法 WAV 用于测试。
    fn wav(sample_rate: u32, channels: u16, bits: u16, pcm: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * channels as u32 * (bits as u32 / 8)).to_le_bytes());
        out.extend_from_slice(&(channels * (bits / 8)).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }

    /// ★ 目标格式的 WAV 要能顺利取出 PCM。
    #[test]
    fn extracts_pcm_from_target_format_wav() {
        let bytes = wav(16_000, 1, 16, &[1, 2, 3, 4]);
        let (pcm, rate, channels) = to_pcm_16k_mono(&bytes).expect("应能解析");
        assert_eq!(pcm, vec![1, 2, 3, 4]);
        assert_eq!(rate, 16_000);
        assert_eq!(channels, 1);
    }

    /// ★ 采样率不对要明确拒绝——悄悄送上去服务端会给出误导性的低分。
    #[test]
    fn rejects_wrong_sample_rate_with_actionable_hint() {
        let bytes = wav(48_000, 1, 16, &[0, 0]);
        let error = to_pcm_16k_mono(&bytes).expect_err("48kHz 应被拒绝");
        assert!(error.message().contains("48000"), "{}", error.message());
        assert!(
            error.message().contains("16000") || error.message().contains("16kHz"),
            "错误信息要说明要什么：{}",
            error.message()
        );
    }

    /// 立体声、非 16-bit、非 PCM、非 WAV 都要拒绝。
    #[test]
    fn rejects_other_unusable_encodings() {
        assert!(
            to_pcm_16k_mono(&wav(16_000, 2, 16, &[0, 0, 0, 0])).is_err(),
            "立体声"
        );
        assert!(
            to_pcm_16k_mono(&wav(16_000, 1, 8, &[0, 0])).is_err(),
            "8-bit"
        );
        assert!(to_pcm_16k_mono(b"not a wav at all").is_err(), "非 WAV");
        assert!(to_pcm_16k_mono(&[]).is_err(), "空");
    }

    /// 带 `LIST` 附加块的 WAV 也要能解析（不能假设 fmt 固定在偏移 12）。
    #[test]
    fn handles_extra_chunks_before_fmt() {
        let base = wav(16_000, 1, 16, &[9, 9]);
        // 在 WAVE 之后插入一个 LIST 块。
        let mut bytes = base[0..12].to_vec();
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(b"INFO");
        bytes.extend_from_slice(&base[12..]);
        // RIFF 长度就不改了——解析器按 chunk 走，不依赖它。
        let (pcm, rate, _) = to_pcm_16k_mono(&bytes).expect("带 LIST 也应能解析");
        assert_eq!(pcm, vec![9, 9]);
        assert_eq!(rate, 16_000);
    }

    /// `tokenId` 要是 uuid 形态（8-4-4-4-12）。
    #[test]
    fn token_id_looks_like_uuid() {
        let id = pseudo_uuid();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5, "{id}");
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{id}"
        );
    }

    /// 两种题型都要出现在 `all()` 里，供 CLI 列举。
    #[test]
    fn api_names_are_enumerable() {
        let names: Vec<&str> = ApiName::all().iter().map(|n| n.as_str()).collect();
        assert!(names.contains(&"en.sent.score"));
        assert!(names.contains(&"open.steam"));
        assert!(ApiName::OpenSteam.needs_llm_sk());
        assert!(!ApiName::SentenceScore.needs_llm_sk());
    }

    /// 域名常量不能串（主/备/HTTP 三条路）。
    #[test]
    fn endpoints_are_distinct() {
        assert!(SPEECH_WSS.contains("speech.unipus.cn"));
        assert!(SPEECH_WSS_BACKUP.contains("speech-eval.unipus.cn"));
        assert!(OPEN_PROXY_URL.contains("open-eval.unipus.cn"));
        assert_ne!(SPEECH_WSS, SPEECH_WSS_BACKUP);
    }
}
