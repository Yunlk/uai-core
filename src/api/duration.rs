//! 学习时长加速：Socket.IO（engine.io v3），不走 HTTP。
//!
//! ## 协议真相（实测，勿凭直觉改）
//!
//! 端点      wss://ucontent.unipus.cn/unipusiopoint/?EIO=3&transport=websocket
//! 命名空间  /userActivities
//! 计分事件  kick-v2
//!
//! 只有 mobile 协议记账：client: "U校园mobile"。
//! 桌面端 unipusio 的 start 事件记账为 0。
//!
//! ## 放大原理
//!
//! 服务端按 (module, moduleGroup) 成对独立记账：
//!
//! 房间数  实测倍率
//! 6  4.20×
//! 16  11.40×
//! 20  15.79×
//! 60  26.66×（握手被随机拒绝）
//!
//! 同一房间开多条连接会被去重，只算一份。所以放大靠的是房间数，
//! 不是连接数。安全区约 20 房间。
//!
//! ⚠️ 起步有坡：r_set 广播约每 60 秒一批，前 60 秒记账为 0。
//! 短测（~150 秒）会看到 11.9× 的低值，长跑稳定在 14.7–16.4×。
//! 不要拿两分钟的结果判断倍率。
//!
//! ## 两条硬约束
//!
//! 1. 心跳：engine.io v3 服务端不发 ping，客户端必须每 20 秒发 2，
//!    否则连接在 45 秒整被断开。
//! 2. 不要重复 kick：每 20 秒重发 kick-v2 会得到
//!    kick-v2 task没有信息变更，且计时停止（倍率归零）。
//!    只在握手后发一次。
//!
//! ⚠️ 回填 timer 无效：delta 由服务端时钟生成。
//!
//! ## ⚠️ 两条不同的时长账本，别混
//!
//! 这个模块走的是 WebSocket 加速这条链路，它按账号级
//! (module, moduleGroup) 记账，kick-v2 载荷里没有课程 ID / 分组 ID，
//! 因此无法把时长限定到某本教材的必修项——这是本条链路的固有限制，
//! 不是没做过滤。
//!
//! 但门户侧还有另一套、按教材分别记账的账本：
//! /api/aca/achievement/queryUserScore 会把「必修学习时长」逐教材列出来
//! （实测《视听说2》00:31:40），考核方案里再逐教材给出
//! minHours / maxHours 标准。要不要拿分看的是那一套，
//! 见 [crate::api::assessment]。
//!
//! ⇒ 想拿「必修学习时长」的分，用 uai grade <班级教材ID> 看还差多少；
//! 本模块只负责在你已经决定刷时长时把账号级时长刷上去。
//!
//! ## 必修的计时机制（读前端 SDK 源码得出，非猜测）
//!
//! 前端是【两层】：general-timing-sdk.js 包一层，底层是
//! realtime-mobile-sdk.js。关键事实：
//!
//! 1. module / moduleGroup 的来源（parseRecordInfo）：
//!    - module：调用方传入；没传就从 URL 兜底
//!      getDefaultModuleFromUrl() —— 取 /app/<名字>/... 的第二段，
//!      匹配不到就是 'unipus-module'；
//!    - moduleGroup：调用方传入；没传按设备兜底 ——
//!      移动端 'app-h5'，桌面端 'h5'。
//!
//!    两者都不含课程 ID，与上面的限制一致。
//! 2. 应用身份：appId 默认 116，serviceId 默认 'point'，
//!    usePoint 默认 true。本模块的 kick_payload 正是照这套值写的。
//!    （宿主页也是这套：`window.generalTiming2026?.timing({... appId:116,
//!    serviceId:"point", timeout:1800 ...})`。）
//! 3. 自动停表：底层 timeout 默认 60 秒；PC 页显式传 1800 秒
//!    （30 分钟）。无用户活动满 timeout 就发 stop 并弹框，
//!    之后「随意活动鼠标可以重新计时」。所以长期挂机会被停表，
//!    这解释了为什么必须持续维持连接而不是 kick 一次就完事。
//! 4. 切页面即停表：visibilitychange 隐藏 → 停；可见 → 用上次
//!    recordInfo 重新 start。App 前后台同理。
//!    ⇒ 后台标签页不计时长，别指望最小化窗口攒时间。
//! 5. 状态机：STATE_READY → CONNECT → START → STOP / ERROR；
//!    kick-v2 只在 STATE_CONNECT 被接受，否则回
//!    {code:1, msg:"not connected"}。本模块的 run_room_once
//!    正是「握手 → 入命名空间 → kick 一次 → 心跳」这个顺序。
//!
//! ## 教材侧的 duration 字段：存在，但恒为 0；且它不参与记分
//!
//! 单元级 GET /course/api/v2/course_progress/{courseId}/{unitId}/{openId}/default
//! 会返回 rt.leafs.<groupId>.duration（每个分组一个累计秒数），
//! 字段在 task/text/video 所有 tab_type 上都存在。
//!
//! 但实测《视听说1/2》《综合1》三本（均 100% 完成）全部为 0，
//! 八个单元、334 个 leaf 无一例外。结论：
//!
//! - 这个字段是没启用的展示字段，不要拿它算学分；门户用的是自己那套汇总
//!   （[crate::api::assessment] 里逐教材的 duration）。
//! - 三本教材共 5 个班级窗口里，单元级与任务级的
//!   start_time / end_time（flowStrategy.startTime / endTime）
//!   全部是 null，state.expired 全为 false —— 教材侧没有截止时间。
//!   记分周期在班级考核方案里（实测 2026-08-29 ~ 2026-12-27）。
//! - ⇒ 时长走的是账号级 (module, moduleGroup) 记账；门户再按教材归集。
//!
//! ## 已接线
//!
//! start_boost 是常驻入口：多房间并发 + 心跳 + 断线重连 + 达标/手动停止，
//! 通过 mpsc 把状态报给调用方（CLI 用来打印进度）。

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::connect_async;

use crate::endpoints::DURATION_SOCKET_BASE;
use crate::error::{Error, Result};

/// 计分所在的命名空间。
pub const NAMESPACE: &str = "/userActivities";

/// 心跳间隔：服务端 45 秒断开，20 秒留足余量。
pub const PING_INTERVAL: Duration = Duration::from_secs(20);

/// 断线重连间隔。
pub const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// 状态汇报间隔。
pub const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// 打点页面 URL，kick-v2 要求带上。
pub const PING_PAGE: &str = "https://ucontent.unipus.cn/_explorationpc_default/pc.html";

/// 房间池的模块名。
pub const MODULES: [&str; 10] = [
    "cmgt", "ucmg", "course", "homework", "studio", "tla", "ach", "exam", "point", "cmgt2",
];

/// 房间池的客户端分组名。
pub const GROUPS: [&str; 6] = ["h5", "pc", "app", "wx", "web", "v2"];

/// 房间池容量上限（MODULES × GROUPS）。
pub const MAX_ROOMS: usize = MODULES.len() * GROUPS.len();

/// 实测安全房间数：再多会出现握手被随机拒绝。
pub const SAFE_ROOMS: usize = 20;

/// 单房间的经验倍率（墙钟 → 记账时长）。
///
/// 20 房间实测 15.79×，约合 0.79×/房间。
pub const PER_ROOM_RATIO: f32 = 0.79;

/// 构建房间池，取前 rooms 个 (module, moduleGroup) 组合。
///
/// 组合顺序：先遍历 group，再遍历 module。同一对只会出现一次——
/// 重复会被服务端去重，而去重后的倍数不会增加。
pub fn build_room_pool(rooms: usize) -> Vec<(&'static str, &'static str)> {
    let mut pool = Vec::new();
    for group in GROUPS {
        for module in MODULES {
            if pool.len() >= rooms {
                return pool;
            }
            pool.push((module, group));
        }
    }
    pool
}

/// 估算给定房间数的倍率。
///
/// ⚠️ 公式是 房间数 × 0.79，没有常数项。
/// 我曾写成 1.0 + 0.79 × 房间数，那不是实测口径：20 房间实测 15.79×，
/// 而 1 + 0.79×20 = 16.8 与之对不上。单房间本身不产生 1× 的基线，
/// 计时完全来自 kick-v2 的记账。
///
/// ## ⚠️ 这是在 20 房间标定的近似，房间少时会高估
///
/// 按实测反推单房间倍率，并不是常数：
///
/// | 房间数 | 实测倍率 | 反推单房间 |
/// | --- | --- | --- |
/// | 6 | 4.20× | 0.70 |
/// | 16 | 11.40× | 0.71 |
/// | 20 | 15.79× | 0.79 |
///
/// 单房间效率随房间数上升（r_set 广播按房间批量下发，房间越多摊得越匀），
/// 所以拿 0.79 去估 6 房间会得到 4.74×，比实测的 4.20× 高约 13%。
/// 本函数只适合在接近 20 房间时估值，不要拿它外推小房间数。
pub fn estimated_ratio(rooms: usize) -> f32 {
    PER_ROOM_RATIO * rooms as f32
}

/// 房间池使用的 client 取值。
///
/// 移动端页面用 "U校园mobile"；桌面的门户首页（general-timing v12）传
/// "pc"，而任务页（/unipusio/sdk）默认 "U校园pc" —— 三个页面三种取值。
/// 房间池走的是 /unipusiopoint，沿用实测能记账的移动端取值；
/// 集中在这个常量里，实测若要换只动这里。
pub const ROOM_CLIENT: &str = "U校园mobile";

/// 构建 kick-v2 载荷。
///
/// ## 与 realtime-mobile-sdk.js 的逐字对照
///
/// 底层是 Object.assign({}, data, {sendTimestamp, openId})，data 含
/// {module, moduleGroup, client, url, tag1, tag2, tag3, appId, serviceId}。
///
/// ⚠️ 没有 timer：早期实现多塞了这个字段，SDK 里 kick 载荷不含它
/// （timer 只出现在 stop 里，且取的是上次 kick 的 sendTimestamp）。
pub fn kick_payload(module: &str, group: &str, open_id: &str, now_ms: u64) -> Value {
    json!(["kick-v2", {
        "module": module,
        "moduleGroup": group,
        "client": ROOM_CLIENT,
        "url": PING_PAGE,
        "tag1": module,
        "tag2": group,
        "tag3": json!({
            "microBlock": format!("{module}/{group}"),
            "version": "1",
            "source": "cloud",
        }).to_string(),
        "appId": 116,
        "serviceId": "point",
        "sendTimestamp": now_ms,
        "openId": open_id,
    }])
}

/// 把 kick-v2 载荷包成 engine.io v3 报文。
pub fn kick_packet(module: &str, group: &str, open_id: &str, now_ms: u64) -> String {
    format!(
        "42{NAMESPACE},1{}",
        kick_payload(module, group, open_id, now_ms)
    )
}

/// 从 r_set 广播里取出 (delta, msUpdate)。
///
/// 报文形如 42/userActivities,["r_set",{ … "delta": N, "msUpdate": M … }]。
/// msUpdate 是去重的键（同一批可能重放）。
pub fn parse_r_set(text: &str) -> Option<(u64, u64)> {
    if !text.contains("\"r_set\"") {
        return None;
    }
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    let payload: Value = serde_json::from_str(&text[start..=end]).ok()?;
    let object = payload.get(1)?;
    // 必须是带 delta 的对象；没有 delta 的报文（如 r_set 的握手回执）忽略。
    let delta = object.get("delta").and_then(Value::as_u64)?;
    let ms_update = object.get("msUpdate").and_then(Value::as_u64).unwrap_or(0);
    Some((delta, ms_update))
}

/// 拼出带 EIO=3 的完整 URL。
///
/// ## ★ 必须带 uuid 与 token 查询参数
///
/// 实测握手 URL 形如：
///
/// wss://ucontent.unipus.cn/unipusiopoint/?EIO=3&transport=websocket
///     &uuid=<open_id 的 URL 编码>&token=<annotator token 的 URL 编码>
///
/// 我一度只写了 ?EIO=3&transport=websocket，那样不带身份，
/// 服务端不会把这批时长记到任何人账上。
pub fn socket_url(open_id: &str, token: &str) -> String {
    format!(
        "{DURATION_SOCKET_BASE}?EIO=3&transport=websocket&uuid={}&token={}",
        urlencode(open_id),
        urlencode(token)
    )
}

/// 最小 URL 编码：只处理会破坏查询串的字符。
///
/// 手写而不引 urlencoding 之类的依赖——只需要这一处。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 当前毫秒时间戳。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// 加速器的运行状态，供多房间共享。
#[derive(Debug, Default)]
pub struct BoostState {
    /// 已记账秒数（服务端 r_set 广播累加而来）。
    pub credited_secs: std::sync::atomic::AtomicU64,
    /// 当前在线房间数。
    pub live_rooms: std::sync::atomic::AtomicUsize,
    /// 累计重连次数。
    pub reconnects: std::sync::atomic::AtomicU64,
}

/// 加速器向调用方汇报的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoostEvent {
    /// 已启动，附带实际房间数。
    Started { rooms: usize },
    /// 周期性状态。
    Tick {
        credited_secs: u64,
        live_rooms: usize,
        reconnects: u64,
    },
    /// 一条可读日志。
    Log(String),
    /// 已结束。
    Finished { credited_secs: u64, reason: String },
}

/// 汇报一次当前状态。
fn report(
    tx: &std::sync::mpsc::Sender<BoostEvent>,
    state: &BoostState,
    start: std::time::Instant,
    total_rooms: usize,
) {
    use std::sync::atomic::Ordering;
    let credited = state.credited_secs.load(Ordering::Relaxed);
    let live = state.live_rooms.load(Ordering::Relaxed);
    let reconnects = state.reconnects.load(Ordering::Relaxed);
    let _ = tx.send(BoostEvent::Tick {
        credited_secs: credited,
        live_rooms: live,
        reconnects,
    });
    let wall = start.elapsed().as_secs_f32() / 3600.0;
    if wall > 0.0001 {
        let ratio = (credited as f32 / 3600.0) / wall;
        let _ = tx.send(BoostEvent::Log(format!(
            "记账 {:.3}h  墙钟 {:.2}h  倍率 {:.2}×  在线 {}/{}  重连 {}",
            credited as f32 / 3600.0,
            wall,
            ratio,
            live,
            total_rooms,
            reconnects
        )));
    }
}

/// 启动时长加速，返回一个「要求停止」的开关。
///
/// target_secs 为 0 表示不设目标，跑到手动停止。
///
/// ## ⚠️ 已知边界
///
/// kick-v2 载荷里没有课程/分组 ID（见 [kick_payload]），
/// 所以无法把时长限定到某本教材的必修项——这是协议本身的限制。
pub fn start_boost(
    open_id: String,
    token: String,
    target_secs: u64,
    rooms: usize,
    events: std::sync::mpsc::Sender<BoostEvent>,
) -> Result<std::sync::Arc<std::sync::atomic::AtomicBool>> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let pool = build_room_pool(rooms);
    if pool.is_empty() {
        return Err(Error::invalid("房间数为 0，无法启动时长加速"));
    }
    if open_id.trim().is_empty() || token.trim().is_empty() {
        return Err(Error::invalid(
            "时长加速需要 open_id 与 annotator token（检查登录会话）",
        ));
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stopped = stopped.clone();

    // 用后台线程跑自己的 tokio runtime：调用方（CLI）是同步的。
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(BoostEvent::Log(format!("创建 runtime 失败：{error}")));
                return;
            }
        };
        runtime.block_on(boost_main(
            open_id,
            token,
            target_secs,
            pool,
            events,
            worker_stopped,
        ));
    });

    Ok(stopped)
}

/// 常驻主循环。
async fn boost_main(
    open_id: String,
    token: String,
    target_secs: u64,
    pool: Vec<(&'static str, &'static str)>,
    events: std::sync::mpsc::Sender<BoostEvent>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let state = Arc::new(BoostState::default());
    let total_rooms = pool.len();
    let _ = events.send(BoostEvent::Started { rooms: total_rooms });
    let _ = events.send(BoostEvent::Log(format!(
        "已启动 {} 个房间，预计倍率 ≈{:.1}×{}",
        total_rooms,
        estimated_ratio(total_rooms),
        if target_secs > 0 {
            format!("，目标 {:.2}h", target_secs as f32 / 3600.0)
        } else {
            String::new()
        }
    )));

    // Ctrl-C 也走同一个停止标志：信号处理器只置位，主循环负责收尾。
    {
        let stopped = stopped.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopped.store(true, Ordering::Relaxed);
            }
        });
    }

    let start = std::time::Instant::now();
    let mut handles = Vec::with_capacity(total_rooms);
    for (module, group) in pool {
        let state = state.clone();
        let token = token.clone();
        let open_id = open_id.clone();
        let events = events.clone();
        let stopped = stopped.clone();
        handles.push(tokio::spawn(async move {
            room_loop(module, group, token, open_id, state, events, stopped).await;
        }));
    }

    loop {
        tokio::time::sleep(REPORT_INTERVAL).await;

        if stopped.load(Ordering::Relaxed) {
            report(&events, &state, start, total_rooms);
            let _ = events.send(BoostEvent::Finished {
                credited_secs: state.credited_secs.load(Ordering::Relaxed),
                reason: "手动停止".to_owned(),
            });
            return;
        }

        report(&events, &state, start, total_rooms);

        let credited = state.credited_secs.load(Ordering::Relaxed);
        if target_secs > 0 && credited >= target_secs {
            let _ = events.send(BoostEvent::Finished {
                credited_secs: credited,
                reason: "已达标".to_owned(),
            });
            return;
        }
    }
}

/// 单个房间：连上 → kick → 保活 → 断线重连。
async fn room_loop(
    module: &'static str,
    group: &'static str,
    token: String,
    open_id: String,
    state: std::sync::Arc<BoostState>,
    events: std::sync::mpsc::Sender<BoostEvent>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;

    while !stopped.load(Ordering::Relaxed) {
        if let Err(error) =
            run_room_once(module, group, &token, &open_id, &state, &events, &stopped).await
        {
            let _ = events.send(BoostEvent::Log(format!(
                "房间 {module}/{group} 断开：{error}"
            )));
        }
        if stopped.load(Ordering::Relaxed) {
            return;
        }
        state.reconnects.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// 单次连接的生命周期。返回时连接一定已断开。
async fn run_room_once(
    module: &str,
    group: &str,
    token: &str,
    open_id: &str,
    state: &BoostState,
    events: &std::sync::mpsc::Sender<BoostEvent>,
    stopped: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use std::sync::atomic::Ordering;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (socket, _) = connect_async(socket_url(open_id, token))
        .await
        .map_err(|error| Error::network(format!("时长连接失败：{error}")))?;
    let (mut sink, mut stream) = socket.split();

    let mut joined = false;
    let mut counted_live = false;
    // 同一批 r_set 可能重放，按 msUpdate 去重，否则倍率会虚高。
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await; // 第一次立即触发，跳过

    loop {
        if stopped.load(Ordering::Relaxed) {
            let _ = sink.close().await;
            if counted_live {
                state.live_rooms.fetch_sub(1, Ordering::Relaxed);
            }
            return Ok(());
        }

        tokio::select! {
            _ = ping.tick() => {
                // engine.io v3：客户端必须主动 ping，否则 45 秒被断。
                if sink.send(WsMessage::Text("2".into())).await.is_err() {
                    if counted_live { state.live_rooms.fetch_sub(1, Ordering::Relaxed); }
                    return Err(Error::network("心跳发送失败"));
                }
            }
            message = stream.next() => {
                let Some(message) = message else {
                    if counted_live { state.live_rooms.fetch_sub(1, Ordering::Relaxed); }
                    return Err(Error::network("连接被服务端关闭"));
                };
                let message = message.map_err(|error| Error::network(format!("读取失败：{error}")))?;
                let WsMessage::Text(text) = message else { continue };
                let text = text.to_string();

                // 服务端 ping → 回 pong。
                if text == "2" {
                    let _ = sink.send(WsMessage::Text("3".into())).await;
                    continue;
                }
                if text == "3" {
                    continue;
                }
                // ★ socket.io 的 ACK（43）与 ERROR（44）是服务端唯一的拒绝渠道。
                // 静默丢掉它们，就等于把「为什么 0 记账」的答案扔掉。
                if text.starts_with("43") || text.starts_with("44") {
                    let _ = events.send(BoostEvent::Log(format!(
                        "房间 {module}/{group} 服务端回执：{text}"
                    )));
                    continue;
                }
                // engine.io 握手包 → 加入命名空间。
                if text.starts_with('0') {
                    let _ = sink.send(WsMessage::Text(format!("40{NAMESPACE},").into())).await;
                    continue;
                }
                // 命名空间连接成功 → 只发一次 kick-v2（重发会停表）。
                if text.starts_with(&format!("40{NAMESPACE}")) && !joined {
                    joined = true;
                    if !counted_live {
                        counted_live = true;
                        state.live_rooms.fetch_add(1, Ordering::Relaxed);
                    }
                    let now = now_millis();
                    let payload = kick_payload(module, group, open_id, now);
                    let packet = format!("42{NAMESPACE},1{payload}");
                    if sink.send(WsMessage::Text(packet.into())).await.is_err() {
                        if counted_live { state.live_rooms.fetch_sub(1, Ordering::Relaxed); }
                        return Err(Error::network("发送 kick-v2 失败"));
                    }
                    continue;
                }
                // 计分广播：按 msUpdate 去重后累加 delta。
                if text.contains("\"r_set\"")
                    && let Some((delta, ms_update)) = parse_r_set(&text)
                    && seen.insert(ms_update)
                {
                    state.credited_secs.fetch_add(delta, Ordering::Relaxed);
                }
            }
        }
    }
}

/// 房间池校验：不重复，且长度等于 min(请求数, 上限)。
///
/// ⚠️ 超过上限是刻意钳制，不是错误。 build_room_pool 按 MAX_ROOMS
/// 截断——请求 9999 个房间只会得到 60 个。这是防护（再多握手会被服务端
/// 随机拒绝），所以校验的是「钳制是否生效」，而不是「是否超限」。
pub fn validate_room_pool(rooms: usize) -> Result<()> {
    let pool = build_room_pool(rooms);
    if pool.len() != rooms.min(MAX_ROOMS) {
        return Err(Error::invalid(format!(
            "房间池长度不符：请求 {rooms}，得到 {}",
            pool.len()
        )));
    }
    let mut seen = Vec::new();
    for pair in &pool {
        if seen.contains(pair) {
            return Err(Error::invalid(format!("房间重复：{pair:?}")));
        }
        seen.push(*pair);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 房间池必须唯一——重复会被服务端去重，倍数不会增加。
    #[test]
    fn room_pool_is_unique() {
        let pool = build_room_pool(60);
        let mut seen = Vec::new();
        for pair in &pool {
            assert!(!seen.contains(pair), "重复房间：{pair:?}");
            seen.push(*pair);
        }
        assert_eq!(pool.len(), 60);
    }

    /// 房间池不能超过上限，请求多了只给上限个。
    #[test]
    fn room_pool_is_clamped_to_max() {
        assert_eq!(build_room_pool(0).len(), 0);
        assert_eq!(build_room_pool(5).len(), 5);
        assert_eq!(build_room_pool(MAX_ROOMS).len(), MAX_ROOMS);
        assert_eq!(build_room_pool(9999).len(), MAX_ROOMS, "不能无限增长");
    }

    /// r_set 解析：delta 与 msUpdate 都要取到。
    #[test]
    fn parse_r_set_extracts_both_fields() {
        let raw = r#"42/userActivities,["r_set",{"openId":"abc","module":"cmgt","moduleGroup":"h5","delta":42,"msUpdate":1700000000000}]"#;
        assert_eq!(parse_r_set(raw), Some((42, 1700000000000)));
    }

    /// msUpdate 缺失时给 0，不报错。
    #[test]
    fn parse_r_set_tolerates_missing_ms_update() {
        let raw = r#"42/userActivities,["r_set",{"delta":7}]"#;
        assert_eq!(parse_r_set(raw), Some((7, 0)));
    }

    /// ★ 垃圾报文必须安静返回 None，不能 panic。
    #[test]
    fn parse_r_set_rejects_garbage() {
        assert_eq!(parse_r_set("42/userActivities,1[\"kick-v2\"]"), None);
        assert_eq!(parse_r_set("no bracket here"), None);
        assert_eq!(parse_r_set("42/userActivities,[\"r_set\",{}]"), None);
        assert_eq!(parse_r_set(""), None);
        assert_eq!(parse_r_set("[]"), None);
    }

    /// 没有 delta 的 r_set（握手回执）也要忽略。
    #[test]
    fn parse_r_set_ignores_ack_without_delta() {
        let raw = r#"42/userActivities,["r_set",{"openId":"abc","module":"cmgt"}]"#;
        assert_eq!(parse_r_set(raw), None);
    }

    /// 房间池的 client 沿用移动端取值。
    ///
    /// 三个页面三种 client：门户首页 "pc"、任务页 "U校园pc"、
    /// 移动端 "U校园mobile"。房间池走 /unipusiopoint，用移动端这档。
    #[test]
    fn room_pool_kick_uses_mobile_client() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        assert_eq!(payload[1]["client"], json!("U校园mobile"));
    }

    /// ★★ 回归测试：kick-v2 载荷不能有 timer。
    ///
    /// SDK 的 kick 载荷是 Object.assign({}, data, {sendTimestamp, openId})，
    /// 全程没有 timer。早期实现多塞了这个字段——它不参与记账，
    /// 但会让人误以为「回填 timer 能操纵时长」。
    #[test]
    fn kick_payload_has_no_timer() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        assert!(
            payload[1].get("timer").is_none(),
            "kick-v2 不该带 timer：{payload}"
        );
        // 反过来，服务端认的那两个字段必须在。
        assert_eq!(payload[1]["sendTimestamp"], json!(1000));
        assert_eq!(payload[1]["openId"], json!("open"));
    }

    /// kick-v2 载荷必须带应用身份，且与 SDK 默认值一致。
    #[test]
    fn kick_payload_carries_app_identity() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        assert_eq!(payload[1]["appId"], json!(116));
        assert_eq!(payload[1]["serviceId"], json!("point"));
    }

    /// 房间池的 kick 载荷不带课程 ID——这正是「房间池无法限定教材」的原因。
    ///
    /// 注意：真实任务模式（[TaskTarget::start_payload]）会带
    /// module=分组 ID、moduleGroup=数字 courseId，那是另一条链，别混。
    #[test]
    fn room_pool_kick_payload_carries_no_course_id() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        let object = payload[1].as_object().unwrap();
        for key in ["courseId", "groupId", "instanceId", "course_id"] {
            assert!(
                !object.contains_key(key),
                "载荷里出现了 {key} —— 房间池本就不该有课程维度"
            );
        }
    }

    /// 构造一个供协议断言用的真实任务靶子。
    fn sample_target() -> TaskTarget {
        TaskTarget {
            instance: "course-v2:example".to_owned(),
            unit: "u1".to_owned(),
            micro: "u1".to_owned(),
            group: "u1g1".to_owned(),
        }
    }

    /// ★★ 回归测试：真实任务链发 start，载荷与任务页 SDK 逐字对齐。
    ///
    /// 取证：/unipusio/sdk/realtime-sdk.js 里 P.emit("start", o)，
    /// `o = Object.assign({}, {module,moduleGroup,client,url,tag1,tag2,tag3},
    /// {timer: Date.now()})。它没有 appId/serviceId/sendTimestamp`/
    /// openId —— 那些是 /unipusiopoint 的 kick-v2 才有的。
    #[test]
    fn task_start_matches_unipusio_protocol() {
        let payload =
            sample_target().start_payload("https://example.test/pc.html", TASK_CLIENT, None, 1000);
        assert_eq!(payload[0], json!("start"), "{payload}");
        assert_eq!(payload[1]["module"], json!("u1g1"));
        // ★ moduleGroup 是 course-v2 实例 ID（来自 URL hash 第一段）。
        assert_eq!(payload[1]["moduleGroup"], json!("course-v2:example"));
        assert_eq!(payload[1]["client"], json!("pc"));
        assert_eq!(payload[1]["tag1"], json!("u1"));
        assert_eq!(payload[1]["tag2"], json!("u1"));
        assert_eq!(payload[1]["timer"], json!(1000));
        // 两条链的字段不能互相串。
        for absent in ["appId", "serviceId", "sendTimestamp", "openId"] {
            assert!(
                payload[1].get(absent).is_none(),
                "start 载荷不该带 {absent}：{payload}"
            );
        }
    }

    /// ★★ 回归测试：stop 是「上一次 start 的完整载荷 + 新 timer」。
    ///
    /// SDK 原文：r = Object.assign({}, T, {timer: new Date().getTime()})，
    /// T 是上一次 start 发出的完整对象。所以除了事件名与 timer，
    /// 其余字段必须与 start 完全一致。
    #[test]
    fn task_stop_reuses_start_fields_with_fresh_timer() {
        let target = sample_target();
        let start = target.start_payload("https://example.test/pc.html", TASK_CLIENT, None, 1000);
        let stop = target.stop_payload("https://example.test/pc.html", TASK_CLIENT, None, 2000);

        assert_eq!(stop[0], json!("stop"), "{stop}");
        assert_eq!(stop[1]["timer"], json!(2000), "stop 用当次时间");
        // 除事件名与 timer 外，逐字段与 start 相同。
        let start_object = start[1].as_object().unwrap();
        let stop_object = stop[1].as_object().unwrap();
        for (key, value) in start_object {
            if key == "timer" {
                continue;
            }
            assert_eq!(
                stop_object.get(key),
                Some(value),
                "字段 {key} 与 start 不一致"
            );
        }
        assert_eq!(start_object.len(), stop_object.len(), "字段数量应一致");
    }

    /// 任务页 URL 必须带上真实的实例/单元/分组，它是服务端认任务的唯一依据。
    #[test]
    fn task_page_url_carries_real_ids() {
        let url = sample_target().page_url("https://example.test/pc.html");
        assert!(
            url.ends_with("#/course-v2:example/courseware/u1/u1/u1g1"),
            "{url}"
        );
    }

    /// 报文前缀必须是命名空间 + engine.io v3 的 42。
    #[test]
    fn kick_packet_has_correct_framing() {
        let packet = kick_packet("cmgt", "h5", "open", 1000);
        assert!(packet.starts_with("42/userActivities,1["), "{packet}");
    }

    /// URL 必须带 EIO=3，否则 engine.io 协议版本不对。
    #[test]
    fn socket_url_specifies_engine_io_v3() {
        let url = socket_url("open", "tok");
        assert!(url.contains("EIO=3"), "{url}");
        assert!(url.contains("transport=websocket"), "{url}");
    }

    /// ★★ 回归测试：URL 必须带身份（uuid + token）。
    ///
    /// 这是真实事故：我一度只写 ?EIO=3&transport=websocket，
    /// 那样握手不带任何身份，服务端不会把这批时长记到账号上——
    /// 连接看着是通的，时长却是 0。
    #[test]
    fn socket_url_carries_identity() {
        let url = socket_url("my-open-id", "my-token");
        assert!(url.contains("uuid=my-open-id"), "{url}");
        assert!(url.contains("token=my-token"), "{url}");
    }

    /// 身份里的特殊字符必须被编码，否则会截断查询串。
    #[test]
    fn socket_url_encodes_identity() {
        let url = socket_url("a b&c=d", "t/1+2");
        assert!(!url.contains("a b"), "空格必须编码：{url}");
        assert!(url.contains("a%20b%26c%3Dd"), "{url}");
        assert!(url.contains("t%2F1%2B2"), "{url}");
    }

    /// URL 编码只放行 unreserved 字符。
    #[test]
    fn urlencode_matches_rfc3986_unreserved() {
        assert_eq!(urlencode("abcXYZ0189-_.~"), "abcXYZ0189-_.~");
        assert_eq!(urlencode(" "), "%20");
        assert_eq!(
            urlencode("+/="),
            "%2B%2F%3D",
            "+ 是 0x2B、/ 是 0x2F，按输入顺序编码"
        );
        assert_eq!(urlencode("中文"), "%E4%B8%AD%E6%96%87");
    }

    /// ★ 倍率估算必须与实测一致：20 房间 ≈ 15.79×（即 20 × 0.79）。
    #[test]
    fn documented_ratio_matches_safe_rooms() {
        let ratio = estimated_ratio(SAFE_ROOMS);
        assert!(
            (ratio - 15.79).abs() < 0.5,
            "20 房间估算 {ratio}，实测 15.79×"
        );
        // 不能有常数项：加了常数项就会偏成 16.8。
        assert_eq!(estimated_ratio(0), 0.0, "0 房间不该有基线倍率");
    }

    /// ★ 线性模型只在 20 房间附近准，房间少时会高估。
    ///
    /// 这不是缺陷而是已知边界：实测反推的单房间倍率并非常数
    /// （6 房间 0.70、16 房间 0.71、20 房间 0.79）。
    /// 把这个偏差写成测试，是为了防止有人拿它外推小房间数后
    /// 误以为「实测没达标」。
    #[test]
    fn linear_model_overestimates_at_low_room_counts() {
        // 6 房间：模型给 4.74，实测 4.20 —— 高估约 13%。
        let modeled = estimated_ratio(6);
        assert!(
            modeled > 4.20,
            "模型在低房间数应当高估，实际 {modeled} vs 实测 4.20"
        );
        // 但在 20 房间处必须贴合。
        assert!((estimated_ratio(SAFE_ROOMS) - 15.79).abs() < 0.5);
    }

    /// 校验通过：正常请求与上限请求都合法。
    #[test]
    fn validate_room_pool_accepts_valid_requests() {
        assert!(validate_room_pool(1).is_ok());
        assert!(validate_room_pool(20).is_ok());
        assert!(validate_room_pool(MAX_ROOMS).is_ok());
    }

    /// ★ 超过上限被钳制到上限，不是报错——这是防护，不是缺陷。
    #[test]
    fn over_request_is_clamped_not_rejected() {
        assert_eq!(build_room_pool(9999).len(), MAX_ROOMS);
        assert!(
            validate_room_pool(9999).is_ok(),
            "钳制是预期行为，校验应通过"
        );
    }

    /// 心跳间隔必须小于服务端的 45 秒断开阈值。
    #[test]
    fn ping_interval_is_below_server_timeout() {
        assert!(
            PING_INTERVAL < Duration::from_secs(45),
            "服务端 45 秒断开，心跳必须更短"
        );
    }
}

// ========================= 真实任务模式 =========================
//
// 与上面的「房间池模式」是两条不同的账，别混：
//
// 房间池（[start_boost]）  真实任务（[start_task_boost]）
// 端点  /unipusiopoint/  /unipusio/
// 事件  kick-v2（只发一次）  start（每 5~10 秒重发）
// client  U校园mobile  U校园pc
// 归属键  假名 (应用, 设备) → 账号级  (真实分组, 真实课程)
// 实测倍率  13.7~14.9×  1×（账号级封顶）
// 进 item 30  ❌ 实测 1.144h 后 9 分钟无变化  ✅ 当场落账
//
// ## 归属靠 url（实测，不是推断）
//
// 服务器从 start 的 url 里认任务：
//
// 发出的 url  回来的 r_set
// 编造的 /_pc_default/…#/…/u1/u1g1/…  module:"menu"，delta:1 ⇒ 通用桶，不进教材账
// 真实任务页 URL  module:"<分组ID>"，delta:20 ⇒ 落进这门课的这个分组
//
// ## timer 是客户端时间，但不参与记账
//
// 任务页 SDK 的 start 载荷是 Object.assign({}, data, {timer: Date.now()})，
// 确实带 timer。但实测回拨这个值不会换来额外记账——delta 与 msUpdate
// 全部由服务端时钟产出。它只是声明「本次计时从何时开始」，不是旋钮。

/// unipusio 端点（真实任务模式专用；房间池模式用的是 unipusiopoint）。
pub const TASK_SOCKET_BASE: &str = "wss://ucontent.unipus.cn/unipusio/";

/// 任务页 SDK 的 client 取值。
///
/// ## 实测取证（浏览器抓帧，不是推断）
///
/// 在真实任务页 pc.html?…&source=cloud&…#/… 上钩住
/// WebSocket.prototype.send，抓到的 stop 帧里：
///
/// ["stop",{"client":"pc","module":"u5g107","moduleGroup":"course-v2:…",…}]
///
/// 即 "pc"。之前的 "U校园pc" 判断错了：makeClient() 先看
/// getPlatformSource()，source=cloud 映射为 3，直接落在 "pc" 分支，
/// 根本走不到兜底的 "U校园pc"。
pub const TASK_CLIENT: &str = "pc";

/// 编造 module 池（与房间池同源）。
///
/// ## 为什么要有这个
///
/// 实测两条链的倍率差得很远：房间池用编造的 (module, moduleGroup) 能到 6.7×，
/// 任务链用真实 (分组ID, 实例ID) 只有 1.13×。同样的 r_set 累加机制，
/// 说明服务端对真实键做了账号级限速。
///
/// 于是提出假设：教材账只按 moduleGroup（实例 ID）归集，而限速键是
/// (module, moduleGroup) 这个对。若成立，module 换成分散名即可突破限速。
/// --spread 就是把 module 换成这些值来做验证。
pub const SPREAD_MODULES: [&str; 10] = [
    "cmgt", "ucmg", "course", "homework", "studio", "tla", "ach", "exam", "point", "cmgt2",
];

/// 一个真实必修任务。
///
/// 字段一一对应任务页 SDK 的
/// initTimeline(module, moduleGroup, openId, token, unitId, microId, platform)：
///
/// | 本结构 | SDK 形参 | 落到载荷 |
/// | --- | --- | --- |
/// | group | module | module |
/// | instance | moduleGroup | moduleGroup |
/// | unit | unitId | tag1 |
/// | micro | microId | tag2 |
///
/// ## moduleGroup 为什么是 course-v2: 实例 ID
///
/// 任务页 SDK 的 moduleGroup 取自 AlT().courseId，而 AlT() 是从
/// URL hash 的第一段解析的：
///
/// // #/<courseId>/courseware/<unitId>/<microId>/<groupId>
/// const s = location.hash.match(/^#\/([\w+|\d|\:|\|\_|\-]*\/courseware(...)/)[1].split("/");
/// return { courseId: s[0], unitId: s[2], microId: s[3], groupId: ... };
///
/// 第一段就是实例 ID（course-v2:…），不是数字 courseId。
/// 数字 courseId（如 342024）是 URL query 里的 cloudCurriculaId，两回事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTarget {
    /// course-v2: 实例 ID。同时用于拼页面 URL 与 moduleGroup。
    pub instance: String,
    pub unit: String,
    pub micro: String,
    pub group: String,
}

impl TaskTarget {
    /// 拼出真实任务页 URL。
    pub fn page_url(&self, page_base: &str) -> String {
        format!(
            "{page_base}#/{}/courseware/{}/{}/{}",
            self.instance, self.unit, self.micro, self.group
        )
    }

    /// tag3 原文（照抄页面：{microBlock, version, source}）。
    pub fn tag3(&self) -> String {
        format!(
            "{{\"microBlock\":\"{}/{}\",\"version\":\"1\",\"source\":\"ucontent\"}}",
            self.unit, self.micro
        )
    }

    /// 一次 start 的完整载荷。
    ///
    /// ## SDK 原文（/unipusio/sdk/realtime-sdk.js）
    ///
    /// var r = (new Date).getTime();
    /// o = Object.assign({}, e, { timer: r });   // e = {module, moduleGroup, client, url, tag1, tag2, tag3}
    /// P.emit("start", o, cb)
    ///
    /// ⚠️ 没有 appId / serviceId / sendTimestamp / openId ——
    /// 那是 /unipusiopoint 的 kick-v2 才有的字段。两条链的载荷不能互相套用。
    pub fn start_payload(
        &self,
        page_base: &str,
        client: &str,
        module_override: Option<&str>,
        now_ms: u64,
    ) -> Value {
        json!(["start", {
            "module": module_override.unwrap_or(&self.group),
            "moduleGroup": self.instance,
            "client": client,
            "url": self.page_url(page_base),
            "tag1": self.unit,
            "tag2": self.micro,
            "tag3": self.tag3(),
            "timer": now_ms,
        }])
    }

    /// 一次 stop 的载荷。
    ///
    /// SDK 原文：r = Object.assign({}, T, {timer: new Date().getTime()})，
    /// T 是上一次 start 发出的完整载荷。所以字段集合与
    /// [TaskTarget::start_payload] 完全一致，只有 timer 换成当前时间。
    pub fn stop_payload(
        &self,
        page_base: &str,
        client: &str,
        module_override: Option<&str>,
        now_ms: u64,
    ) -> Value {
        let mut payload = self.start_payload(page_base, client, module_override, now_ms);
        if let Some(array) = payload.as_array_mut()
            && let Some(event) = array.first_mut()
        {
            *event = json!("stop");
        }
        payload
    }
}

/// 任务模式参数。
#[derive(Clone, Debug)]
pub struct TaskOptions {
    /// 真实任务页的公共前缀（含 cid/cloudCurriculaId/courseResourceId 等查询串，
    /// 直接从浏览器地址栏抄）。
    pub page_base: String,
    /// 目标记账秒数；0 = 一直跑到 Ctrl-C。
    pub target_secs: u64,
    /// 每个分组停留多久（到点 stop 结算并轮换下一个）。
    ///
    /// 默认 60 秒：实测结算间隔越短越好——1 分钟约 0.87~1.17×，
    /// 2 分钟掉到 0.28×。真页面是 30 分钟 stop 一次，但那对刷时长不利。
    pub dwell: Duration,
    /// start 重发间隔。官方页面是 5 秒。
    pub start_interval: Duration,
    /// 并行连接数。每条连接分到一批任务并在自己那批里轮换。
    ///
    /// 1 = 单连接（早期行为）。上限受服务端并发风控约束，
    /// 超出部分会在握手阶段被 HTTP 503 拒绝。
    pub conns: usize,
    /// 实验开关：把所有连接都钉在同一个分组上。
    ///
    /// 用来验证「同一 (分组, 实例) 多条连接是否会被服务端去重」。
    /// 房间池的经验是同房间去重，但那是 kick-v2 链；start/stop 未必。
    /// 若不去重，N 条连接盯同一个子单元就能拿到 N 倍。
    pub pin: Option<String>,
    /// 实验开关：stop 时把 timer 向前拨 fake_secs 秒。
    ///
    /// 只用来验证「服务端是否信任客户端时间」这一个假设。
    /// 服务端若用 stop.timer - start.timer 计时，伪造差值就能直接刷时长；
    /// 若不信任，则本项应完全无效。默认 0（不伪造）。
    pub fake_secs: u64,
    /// 是否把 module 换成 [SPREAD_MODULES] 里的分散名。
    ///
    /// 这是一个待验证的加速假设：若教材账只认 moduleGroup，
    /// 那么分散 module 可能既进教材账又拿到倍率。默认关闭。
    pub spread: bool,
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            page_base: String::new(),
            target_secs: 0,
            dwell: Duration::from_secs(60),
            start_interval: Duration::from_secs(5),
            conns: 1,
            pin: None,
            fake_secs: 0,
            spread: false,
        }
    }
}

/// 启动真实任务模式：单连接、轮换必修任务、约 1× 记账、精确落教材账。
///
/// ## 为什么是单连接
///
/// 实测同一门课开 4 条连接只记 0.63×、跨 3 门课并行只记 0.69×——
/// 预算按账号 ~1× 封顶，多开只会摊薄（还倒贴连接建立与爬坡损耗）。
pub fn start_task_boost(
    open_id: String,
    token: String,
    targets: Vec<TaskTarget>,
    options: TaskOptions,
    events: std::sync::mpsc::Sender<BoostEvent>,
) -> Result<std::sync::Arc<std::sync::atomic::AtomicBool>> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    if targets.is_empty() {
        return Err(Error::invalid("任务模式至少要有一个真实必修任务"));
    }
    if open_id.trim().is_empty() || token.trim().is_empty() {
        return Err(Error::invalid(
            "任务模式需要 open_id 与 annotator token（检查登录会话）",
        ));
    }
    if options.page_base.trim().is_empty() {
        return Err(Error::invalid(
            "任务模式需要真实任务页前缀（`--page-base`），它是服务端认任务的唯一依据",
        ));
    }

    // --pin：只留一个分组，后面每条连接都盯它。
    let mut targets = targets;
    if let Some(pin) = options.pin.as_deref() {
        targets.retain(|target| target.group == pin);
        if targets.is_empty() {
            return Err(Error::invalid(format!(
                "`--pin {pin}` 不在必修集里（先用 targets 看看有哪些分组）"
            )));
        }
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stopped = stopped.clone();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(BoostEvent::Log(format!("创建 runtime 失败：{error}")));
                return;
            }
        };
        runtime.block_on(task_main(
            open_id,
            token,
            targets,
            options,
            events,
            worker_stopped,
        ));
    });

    Ok(stopped)
}

/// 单条任务连接：在自己的任务子集里轮换，收到 r_set 就累加（按 msUpdate 去重）。
// 参数是并行 worker 的固有配置（连接号、身份、任务、共享状态、事件通道），
// 拆成结构体只会多一层无意义的包装。
#[allow(clippy::too_many_arguments)]
async fn task_worker(
    conn: usize,
    open_id: String,
    token: String,
    targets: Vec<TaskTarget>,
    options: TaskOptions,
    state: std::sync::Arc<BoostState>,
    events: std::sync::mpsc::Sender<BoostEvent>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let target_secs = options.target_secs;
    // --spread 时本连接固定用一个编造 module（stop 必须与 start 一致）。
    let spread_module: Option<&str> = if options.spread {
        Some(SPREAD_MODULES[conn % SPREAD_MODULES.len()])
    } else {
        None
    };

    let start = std::time::Instant::now();
    let url = format!(
        "{TASK_SOCKET_BASE}?EIO=3&transport=websocket&uuid={}&token={}",
        urlencode(&open_id),
        urlencode(&token)
    );

    let mut index = 0usize;
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut reconnects = 0u64;

    'outer: loop {
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let (socket, _) = match connect_async(&url).await {
            Ok(pair) => pair,
            Err(error) => {
                let _ = events.send(BoostEvent::Log(format!("连接失败：{error}")));
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let (mut sink, mut stream) = socket.split();
        let mut joined = false;
        // socket.io 的 ack id：每条消息自增。硬编码同一个值会让回执对不上。
        let mut ack_id: u64 = 0;
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        // 结算周期：每 dwell 停一次表并轮换，服务端据此累计这段时长。
        let mut settle = tokio::time::interval(options.dwell);
        settle.tick().await;
        let _ = options.start_interval;

        loop {
            if stopped.load(Ordering::Relaxed) {
                let _ = sink.close().await;
                break 'outer;
            }
            if target_secs > 0 && state.credited_secs.load(Ordering::Relaxed) >= target_secs {
                let _ = sink.close().await;
                // ★ 达标只置停止位，由 task_main 统一收尾。
                // 若在这里直接 return，其余并行连接会继续空跑。
                stopped.store(true, Ordering::Relaxed);
                break 'outer;
            }

            tokio::select! {
                _ = ping.tick() => {
                    // engine.io v3：客户端必须主动发 2，否则 45 秒被断。
                    if sink.send(WsMessage::Text("2".into())).await.is_err() {
                        reconnects += 1;
                        continue 'outer;
                    }
                }
                _ = settle.tick() => {
                    if joined {
                        // ★ 结算：先 stop 当前任务，服务端据此累计这段时长。
                        //
                        // 抓包证实：真实页面 start 一次后不再重发，
                        // 时长在 stop 时提交。早先每 5 秒重发 start
                        // 只会拿到「task没有信息变更」，永远等不到结算。
                        let current = &targets[index % targets.len()];
                        // 实验：fake_secs > 0 时把 stop 的 timer 往前拨，
                        // 用来看服务端是否采信客户端时间。
                        let stop_at = now_millis() + options.fake_secs.saturating_mul(1000);
                        let stop = current.stop_payload(
                            &options.page_base,
                            TASK_CLIENT,
                            spread_module,
                            stop_at,
                        );
                        ack_id = ack_id.wrapping_add(1);
                        let packet = format!("42{NAMESPACE},{ack_id}{stop}");
                        if sink.send(WsMessage::Text(packet.into())).await.is_err() {
                            reconnects += 1;
                            continue 'outer;
                        }
                        // 轮换到下一个任务并立刻 start，形成「结算 → 继续」。
                        if targets.len() > 1 {
                            index = (index + 1) % targets.len();
                        }
                        let next = &targets[index];
                        let payload = next.start_payload(
                            &options.page_base,
                            TASK_CLIENT,
                            spread_module,
                            now_millis(),
                        );
                        ack_id = ack_id.wrapping_add(1);
                        let packet = format!("42{NAMESPACE},{ack_id}{payload}");
                        if sink.send(WsMessage::Text(packet.into())).await.is_err() {
                            reconnects += 1;
                            continue 'outer;
                        }
                        let _ = events.send(BoostEvent::Log(format!(
                            "结算并轮换到 {}/{}/{}",
                            next.unit, next.micro, next.group
                        )));
                    }
                }
                message = stream.next() => {
                    let Some(Ok(message)) = message else {
                        reconnects += 1;
                        continue 'outer;
                    };
                    let WsMessage::Text(text) = message else { continue };
                    let text = text.to_string();
                    if text == "2" {
                        let _ = sink.send(WsMessage::Text("3".into())).await;
                        continue;
                    }
                    if text == "3" { continue; }
                    // ★ socket.io 的 ACK（43）与 ERROR（44）。
                    //
                    // 服务端拒绝 start / stop 时只在这里说话，回执形如
                    // 43/userActivities,1[{"code":…,"msg":"…"}]。早先实现把
                    // 这两类报文静默丢掉，于是「连上了但一直 0 记账」无从定位。
                    if text.starts_with("43") || text.starts_with("44") {
                        let _ = events.send(BoostEvent::Log(format!("服务端回执：{text}")));
                        continue;
                    }
                    if text.starts_with('0') && !text.starts_with("40") {
                        // 命名空间连接不带 query：uuid/token 在握手 URL 里
                        // （SDK 的 io.connect(wsURL + "/userActivities", {query})
                        // 会把 query 拼进 engine.io 握手 URL）。
                        // 早期实现把 query 塞在这里，与 SDK 不符。
                        let join = format!("40{NAMESPACE},");
                        let _ = sink.send(WsMessage::Text(join.into())).await;
                        continue;
                    }
                    if text.starts_with(&format!("40{NAMESPACE}")) {
                        joined = true;
                        // 连上后立刻 start 第一个任务——真实页面也是这样，
                        // 而且之后不再重发（抓包证实）。
                        index = 0;
                        let target = &targets[0];
                        let payload = target.start_payload(
                            &options.page_base,
                            TASK_CLIENT,
                            spread_module,
                            now_millis(),
                        );
                        ack_id = ack_id.wrapping_add(1);
                        let packet = format!("42{NAMESPACE},{ack_id}{payload}");
                        let _ = sink.send(WsMessage::Text(packet.into())).await;
                        settle.reset();
                        continue;
                    }
                    if let Some((delta, ms_update)) = parse_r_set(&text)
                        && seen.insert(ms_update)
                    {
                        state.credited_secs.fetch_add(delta, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    let _ = reconnects;
    let _ = start;
}

/// 任务模式入口：把任务分片给 options.conns 条连接，各自在自己那批里轮换。
///
/// ## 为什么可以多连接
///
/// 服务端按 (module, moduleGroup) 成对记账，不同任务天然落在不同键上。
/// 多条连接各自负责一批任务，互不覆盖；同一任务不会被两条连接同时 start。
///
/// ⚠️ 并发数受服务端风控约束：超出部分会在握手阶段被 HTTP 503 拒绝。
/// 实测 /unipusiopoint 的单账号上限约 35 条；/unipusio 是否共用同一额度
/// 尚未单独标定，所以默认仍是 1，需要时显式给 --conns。
async fn task_main(
    open_id: String,
    token: String,
    targets: Vec<TaskTarget>,
    options: TaskOptions,
    events: std::sync::mpsc::Sender<BoostEvent>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let state = Arc::new(BoostState::default());
    let target_secs = options.target_secs;
    let total = targets.len();
    // 未 pin 时，连接数不能超过任务数（多出来的连接分不到任务，纯占配额）。
    // pin 时所有连接都盯同一个分组，因此不受任务数限制。
    let conns = if options.pin.is_some() {
        options.conns.max(1)
    } else {
        options.conns.clamp(1, total.max(1))
    };

    let _ = events.send(BoostEvent::Started { rooms: conns });
    let _ = events.send(BoostEvent::Log(format!(
        "任务模式：{total} 个必修任务 / {conns} 条连接，{} 秒轮换，`start` 每 {} 秒重发{}",
        options.dwell.as_secs(),
        options.start_interval.as_secs(),
        if target_secs > 0 {
            format!("，目标 {:.2}h", target_secs as f32 / 3600.0)
        } else {
            String::new()
        }
    )));

    // 每 5 秒汇报一次进度——没有它，长时间 0 记账时界面上什么都看不到。
    {
        let state = state.clone();
        let events = events.clone();
        let stopped = stopped.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.tick().await;
            let began = std::time::Instant::now();
            let mut last_warn = 0u64;
            while !stopped.load(Ordering::Relaxed) {
                ticker.tick().await;
                let credited = state.credited_secs.load(Ordering::Relaxed);
                let _ = events.send(BoostEvent::Tick {
                    credited_secs: credited,
                    live_rooms: conns,
                    reconnects: 0,
                });
                // 连了 90 秒还一分没记，说明服务端当前不给这个账号记时长。
                let elapsed = began.elapsed().as_secs();
                if credited == 0 && elapsed >= 90 && elapsed - last_warn >= 300 {
                    last_warn = elapsed;
                    let _ = events.send(BoostEvent::Log(
                        "⚠️ 已连接仍 0 记账：服务端当前没给这个账号记时长。                         常见原因：账号被限流/当日配额用尽，或 task 页前缀不被认。"
                            .to_owned(),
                    ));
                }
            }
        });
    }

    // Ctrl-C 也走同一个停止标志。
    {
        let stopped = stopped.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopped.store(true, Ordering::Relaxed);
            }
        });
    }

    // 分片：第 i 条连接拿 targets[i::conns]，互不重叠。
    let mut handles = Vec::with_capacity(conns);
    for conn in 0..conns {
        let subset: Vec<TaskTarget> = targets.iter().skip(conn).step_by(conns).cloned().collect();
        let open_id = open_id.clone();
        let token = token.clone();
        let options = options.clone();
        let state = state.clone();
        let events = events.clone();
        let stopped = stopped.clone();
        handles.push(tokio::spawn(async move {
            task_worker(
                conn, open_id, token, subset, options, state, events, stopped,
            )
            .await;
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    let credited = state.credited_secs.load(Ordering::Relaxed);
    let _ = events.send(BoostEvent::Finished {
        credited_secs: credited,
        reason: if target_secs > 0 && credited >= target_secs {
            "已达标".to_owned()
        } else {
            "已停止".to_owned()
        },
    });
}
