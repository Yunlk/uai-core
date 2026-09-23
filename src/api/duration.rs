//! 学习时长加速：Socket.IO（engine.io v3），**不走 HTTP**。
//!
//! ## 协议真相（实测，勿凭直觉改）
//!
//! ```text
//! 端点      wss://ucontent.unipus.cn/unipusiopoint/?EIO=3&transport=websocket
//! 命名空间  /userActivities
//! 计分事件  kick-v2
//! ```
//!
//! **只有 mobile 协议记账**：`client: "U校园mobile"`。
//! 桌面端 `unipusio` 的 `start` 事件记账为 **0**。
//!
//! ## 放大原理
//!
//! 服务端按 `(module, moduleGroup)` **成对独立记账**：
//!
//! | 房间数 | 实测倍率 |
//! | --- | --- |
//! | 6 | 4.20× |
//! | 16 | 11.40× |
//! | 20 | 15.79× |
//! | 60 | 26.66×（握手被随机拒绝） |
//!
//! 同一房间开多条连接会被**去重**，只算一份。所以放大靠的是**房间数**，
//! 不是连接数。安全区约 **20 房间**。
//!
//! ⚠️ **起步有坡**：`r_set` 广播约每 60 秒一批，前 60 秒记账为 0。
//! 短测（~150 秒）会看到 11.9× 的低值，长跑稳定在 14.7–16.4×。
//! **不要拿两分钟的结果判断倍率。**
//!
//! ## 两条硬约束
//!
//! 1. **心跳**：engine.io v3 服务端**不发 ping**，客户端必须每 20 秒发 `2`，
//!    否则连接在 45 秒整被断开。
//! 2. **不要重复 kick**：每 20 秒重发 `kick-v2` 会得到
//!    `kick-v2 task没有信息变更`，且**计时停止（倍率归零）**。
//!    只在握手后发一次。
//!
//! ⚠️ 回填 `timer` 无效：`delta` 由**服务端时钟**生成。
//!
//! ## ⚠️ 两条不同的时长账本，别混
//!
//! 这个模块走的是 **WebSocket 加速**这条链路，它按账号级
//! `(module, moduleGroup)` 记账，`kick-v2` 载荷里**没有课程 ID / 分组 ID**，
//! 因此**无法把时长限定到某本教材的必修项**——这是本条链路的固有限制，
//! 不是没做过滤。
//!
//! 但门户侧还有**另一套、按教材分别记账**的账本：
//! `/api/aca/achievement/queryUserScore` 会把「必修学习时长」逐教材列出来
//! （实测《视听说2》`00:31:40`），考核方案里再逐教材给出
//! `minHours` / `maxHours` 标准。**要不要拿分看的是那一套**，
//! 见 [`crate::api::assessment`]。
//!
//! ⇒ 想拿「必修学习时长」的分，用 `uai grade <班级教材ID>` 看还差多少；
//! 本模块只负责在你**已经决定刷时长**时把账号级时长刷上去。
//!
//! ## 必修的计时机制（读前端 SDK 源码得出，非猜测）
//!
//! 前端是【两层】：`general-timing-sdk.js` 包一层，底层是
//! `realtime-mobile-sdk.js`。关键事实：
//!
//! 1. **`module` / `moduleGroup` 的来源**（`parseRecordInfo`）：
//!    - `module`：调用方传入；没传就从 URL 兜底
//!      `getDefaultModuleFromUrl()` —— 取 `/app/<名字>/...` 的**第二段**，
//!      匹配不到就是 `'unipus-module'`；
//!    - `moduleGroup`：调用方传入；没传按设备兜底 ——
//!      移动端 `'app-h5'`，桌面端 `'h5'`。
//!
//!    两者**都不含课程 ID**，与上面的限制一致。
//! 2. **应用身份**：`appId` 默认 **116**，`serviceId` 默认 **`'point'`**，
//!    `usePoint` 默认 **true**。本模块的 `kick_payload` 正是照这套值写的。
//!    （宿主页也是这套：`window.generalTiming2026?.timing({... appId:116,
//!    serviceId:"point", timeout:1800 ...})`。）
//! 3. **自动停表**：底层 `timeout` 默认 60 秒；**PC 页显式传 1800 秒**
//!    （30 分钟）。无用户活动满 `timeout` 就发 `stop` 并弹框，
//!    之后「随意活动鼠标可以重新计时」。所以**长期挂机会被停表**，
//!    这解释了为什么必须持续维持连接而不是 kick 一次就完事。
//! 4. **切页面即停表**：`visibilitychange` 隐藏 → 停；可见 → 用上次
//!    `recordInfo` 重新 `start`。App 前后台同理。
//!    ⇒ 后台标签页不计时长，别指望最小化窗口攒时间。
//! 5. **状态机**：`STATE_READY → CONNECT → START → STOP / ERROR`；
//!    `kick-v2` 只在 `STATE_CONNECT` 被接受，否则回
//!    `{code:1, msg:"not connected"}`。本模块的 `run_room_once`
//!    正是「握手 → 入命名空间 → kick 一次 → 心跳」这个顺序。
//!
//! ## 教材侧的 `duration` 字段：存在，但恒为 0；**且它不参与记分**
//!
//! 单元级 `GET /course/api/v2/course_progress/{courseId}/{unitId}/{openId}/default`
//! 会返回 `rt.leafs.<groupId>.duration`（**每个分组一个累计秒数**），
//! 字段在 `task`/`text`/`video` 所有 `tab_type` 上都存在。
//!
//! 但实测《视听说1/2》《综合1》三本（均 100% 完成）**全部为 0**，
//! 八个单元、334 个 leaf 无一例外。结论：
//!
//! - 这个字段是**没启用的展示字段**，不要拿它算学分；门户用的是自己那套汇总
//!   （[`crate::api::assessment`] 里逐教材的 `duration`）。
//! - 三本教材共 5 个班级窗口里，单元级与任务级的
//!   `start_time` / `end_time`（`flowStrategy.startTime` / `endTime`）
//!   **全部是 `null`**，`state.expired` 全为 `false` —— **教材侧没有截止时间**。
//!   记分周期在**班级考核方案**里（实测 2026-08-29 ~ 2026-12-27）。
//! - ⇒ 时长走的是**账号级 `(module, moduleGroup)` 记账**；门户再按教材归集。
//!
//! ## 已接线
//!
//! `start_boost` 是常驻入口：多房间并发 + 心跳 + 断线重连 + 达标/手动停止，
//! 通过 `mpsc` 把状态报给调用方（CLI 用来打印进度）。

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

/// 打点页面 URL，`kick-v2` 要求带上。
pub const PING_PAGE: &str = "https://ucontent.unipus.cn/_explorationpc_default/pc.html";

/// 房间池的模块名。
pub const MODULES: [&str; 10] = [
    "cmgt", "ucmg", "course", "homework", "studio", "tla", "ach", "exam", "point", "cmgt2",
];

/// 房间池的客户端分组名。
pub const GROUPS: [&str; 6] = ["h5", "pc", "app", "wx", "web", "v2"];

/// 房间池容量上限（`MODULES × GROUPS`）。
pub const MAX_ROOMS: usize = MODULES.len() * GROUPS.len();

/// 实测安全房间数：再多会出现握手被随机拒绝。
pub const SAFE_ROOMS: usize = 20;

/// 单房间的经验倍率（墙钟 → 记账时长）。
///
/// 20 房间实测 15.79×，约合 0.79×/房间。
pub const PER_ROOM_RATIO: f32 = 0.79;

/// 构建房间池，取前 `rooms` 个 `(module, moduleGroup)` 组合。
///
/// 组合顺序：先遍历 group，再遍历 module。同一对只会出现一次——
/// 重复会被服务端去重，而**去重后的倍数不会增加**。
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
/// ⚠️ **公式是 `房间数 × 0.79`，没有常数项。**
/// 我曾写成 `1.0 + 0.79 × 房间数`，那不是实测口径：20 房间实测 15.79×，
/// 而 `1 + 0.79×20 = 16.8` 与之对不上。单房间本身不产生 1× 的基线，
/// 计时完全来自 `kick-v2` 的记账。
///
/// ## ⚠️ 这是**在 20 房间标定**的近似，房间少时会高估
///
/// 按实测反推单房间倍率，并不是常数：
///
/// | 房间数 | 实测倍率 | 反推单房间 |
/// | --- | --- | --- |
/// | 6 | 4.20× | 0.70 |
/// | 16 | 11.40× | 0.71 |
/// | 20 | 15.79× | **0.79** |
///
/// 单房间效率随房间数上升（`r_set` 广播按房间批量下发，房间越多摊得越匀），
/// 所以拿 0.79 去估 6 房间会得到 4.74×，比实测的 4.20× 高约 13%。
/// **本函数只适合在接近 20 房间时估值**，不要拿它外推小房间数。
pub fn estimated_ratio(rooms: usize) -> f32 {
    PER_ROOM_RATIO * rooms as f32
}

/// 构建 `kick-v2` 载荷。
pub fn kick_payload(module: &str, group: &str, open_id: &str, now_ms: u64) -> Value {
    json!(["kick-v2", {
        "module": module,
        "moduleGroup": group,
        "client": "U校园mobile",
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
        "timer": now_ms,
    }])
}

/// 把 `kick-v2` 载荷包成 engine.io v3 报文。
pub fn kick_packet(module: &str, group: &str, open_id: &str, now_ms: u64) -> String {
    format!(
        "42{NAMESPACE},1{}",
        kick_payload(module, group, open_id, now_ms)
    )
}

/// 从 `r_set` 广播里取出 `(delta, msUpdate)`。
///
/// 报文形如 `42/userActivities,["r_set",{ … "delta": N, "msUpdate": M … }]`。
/// `msUpdate` 是去重的键（同一批可能重放）。
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

/// 拼出带 `EIO=3` 的完整 URL。
///
/// ## ★ 必须带 `uuid` 与 `token` 查询参数
///
/// 实测握手 URL 形如：
///
/// ```text
/// wss://ucontent.unipus.cn/unipusiopoint/?EIO=3&transport=websocket
///     &uuid=<open_id 的 URL 编码>&token=<annotator token 的 URL 编码>
/// ```
///
/// 我一度只写了 `?EIO=3&transport=websocket`，那样**不带身份**，
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
/// 手写而不引 `urlencoding` 之类的依赖——只需要这一处。
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
    /// 已记账秒数（服务端 `r_set` 广播累加而来）。
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
/// `target_secs` 为 `0` 表示不设目标，跑到手动停止。
///
/// ## ⚠️ 已知边界
///
/// `kick-v2` 载荷里**没有课程/分组 ID**（见 [`kick_payload`]），
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
        if let Err(error) = run_room_once(module, group, &token, &open_id, &state, &stopped).await {
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
    // 同一批 `r_set` 可能重放，按 msUpdate 去重，否则倍率会虚高。
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
                // engine.io 握手包 → 加入命名空间。
                if text.starts_with('0') {
                    let _ = sink.send(WsMessage::Text(format!("40{NAMESPACE},").into())).await;
                    continue;
                }
                // 命名空间连接成功 → **只发一次** kick-v2（重发会停表）。
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

/// 房间池校验：不重复，且长度等于 `min(请求数, 上限)`。
///
/// ⚠️ **超过上限是刻意钳制，不是错误。** `build_room_pool` 按 `MAX_ROOMS`
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

    /// `r_set` 解析：delta 与 msUpdate 都要取到。
    #[test]
    fn parse_r_set_extracts_both_fields() {
        let raw = r#"42/userActivities,["r_set",{"openId":"abc","module":"cmgt","moduleGroup":"h5","delta":42,"msUpdate":1700000000000}]"#;
        assert_eq!(parse_r_set(raw), Some((42, 1700000000000)));
    }

    /// `msUpdate` 缺失时给 0，不报错。
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

    /// ★ kick 载荷必须用 mobile 协议，桌面端协议记账为 0。
    #[test]
    fn kick_uses_mobile_client() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        assert_eq!(payload[1]["client"], json!("U校园mobile"));
    }

    /// kick 载荷不带课程 ID——这正是「无法限定教材」的原因。
    #[test]
    fn kick_payload_carries_no_course_id() {
        let payload = kick_payload("cmgt", "h5", "open", 1000);
        let object = payload[1].as_object().unwrap();
        for key in ["courseId", "groupId", "instanceId", "course_id"] {
            assert!(
                !object.contains_key(key),
                "载荷里出现了 {key} —— 若真有课程维度，『限定必修』就能实现了"
            );
        }
    }

    /// 报文前缀必须是命名空间 + engine.io v3 的 `42`。
    #[test]
    fn kick_packet_has_correct_framing() {
        let packet = kick_packet("cmgt", "h5", "open", 1000);
        assert!(packet.starts_with("42/userActivities,1["), "{packet}");
    }

    /// URL 必须带 `EIO=3`，否则 engine.io 协议版本不对。
    #[test]
    fn socket_url_specifies_engine_io_v3() {
        let url = socket_url("open", "tok");
        assert!(url.contains("EIO=3"), "{url}");
        assert!(url.contains("transport=websocket"), "{url}");
    }

    /// ★★ 回归测试：URL 必须带身份（`uuid` + `token`）。
    ///
    /// 这是真实事故：我一度只写 `?EIO=3&transport=websocket`，
    /// 那样握手**不带任何身份**，服务端不会把这批时长记到账号上——
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

    /// ★ 线性模型**只在 20 房间附近准**，房间少时会高估。
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

    /// ★ 超过上限被**钳制**到上限，不是报错——这是防护，不是缺陷。
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
// 与上面的「房间池模式」是**两条不同的账**，别混：
//
// | | 房间池（[`start_boost`]） | 真实任务（[`start_task_boost`]） |
// | --- | --- | --- |
// | 端点 | `/unipusiopoint/` | `/unipusio/` |
// | 事件 | `kick-v2`（只发一次） | `start`（每 5~10 秒重发） |
// | `client` | `U校园mobile` | `U校园pc` |
// | 归属键 | 假名 `(应用, 设备)` → 账号级 | **`(真实分组, 真实课程)`** |
// | 实测倍率 | 13.7~14.9× | **1×（账号级封顶）** |
// | 进 item 30 | ❌ 实测 1.144h 后 9 分钟无变化 | ✅ 当场落账 |
//
// ## 归属靠 `url`（实测，不是推断）
//
// 服务器从 `start` 的 `url` 里认任务：
//
// | 发出的 `url` | 回来的 `r_set` |
// | --- | --- |
// | 编造的 `/_pc_default/…#/…/u1/u1g1/…` | `module:"menu"`，`delta:1` ⇒ 通用桶，不进教材账 |
// | **真实任务页 URL** | `module:"<分组ID>"`，`delta:20` ⇒ **落进这门课的这个分组** |
//
// ## 时间不可伪造
//
// `start` 载荷里**没有时间字段**（`{module,moduleGroup,client,url,tag1,tag2,tag3}`），
// `delta` 与 `msUpdate` 都是**服务端时钟**产出。往载荷里塞 `timer/sendTimestamp/
// startTime/beginTime/delta` 不会换来任何记账。

/// `unipusio` 端点（真实任务模式专用；房间池模式用的是 `unipusiopoint`）。
pub const TASK_SOCKET_BASE: &str = "wss://ucontent.unipus.cn/unipusio/";

/// 一个真实必修任务：`(课程实例, 单元, micro, 分组)`。
///
/// 四项都必须是**真的**——`url` 是服务端认任务的唯一依据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTarget {
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

    /// `tag3` 原文（照抄页面：`{microBlock, version, source}`）。
    pub fn tag3(&self) -> String {
        format!(
            "{{\"microBlock\":\"{}/{}\",\"version\":\"1\",\"source\":\"ucontent\"}}",
            self.unit, self.micro
        )
    }
}

/// 任务模式参数。
#[derive(Clone, Debug)]
pub struct TaskOptions {
    /// 真实任务页的公共前缀（含 `cid`/`cloudCurriculaId`/`courseResourceId` 等查询串，
    /// 直接从浏览器地址栏抄）。
    pub page_base: String,
    /// 目标记账秒数；`0` = 一直跑到 Ctrl-C。
    pub target_secs: u64,
    /// 每个分组停留多久（到点轮换下一个）。
    ///
    /// 真页面每 30 分钟 `timeline.stop()` 重新武装一次；我们按 4 分钟轮换，
    /// 既避开固定节奏，也不让某个任务被判定闲置。
    pub dwell: Duration,
    /// `start` 重发间隔。官方页面是 5 秒。
    pub start_interval: Duration,
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            page_base: String::new(),
            target_secs: 0,
            dwell: Duration::from_secs(240),
            start_interval: Duration::from_secs(5),
        }
    }
}

/// 启动**真实任务模式**：单连接、轮换必修任务、约 1× 记账、精确落教材账。
///
/// ## 为什么是单连接
///
/// 实测同一门课开 4 条连接只记 0.63×、跨 3 门课并行只记 0.69×——
/// **预算按账号 ~1× 封顶**，多开只会摊薄（还倒贴连接建立与爬坡损耗）。
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

/// 任务模式主循环：一条连接，轮换任务，收到 `r_set` 就累加（按 `msUpdate` 去重）。
async fn task_main(
    open_id: String,
    token: String,
    targets: Vec<TaskTarget>,
    options: TaskOptions,
    events: std::sync::mpsc::Sender<BoostEvent>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let state = std::sync::Arc::new(crate::api::duration::BoostState::default());
    let target_secs = options.target_secs;
    let _ = events.send(BoostEvent::Started { rooms: 1 });
    let _ = events.send(BoostEvent::Log(format!(
        "任务模式：{} 个必修任务，{} 秒轮换，`start` 每 {} 秒重发{}",
        targets.len(),
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
        let start = std::time::Instant::now();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.tick().await;
            while !stopped.load(Ordering::Relaxed) {
                ticker.tick().await;
                let _ = events.send(BoostEvent::Tick {
                    credited_secs: state.credited_secs.load(Ordering::Relaxed),
                    live_rooms: 1,
                    reconnects: 0,
                });
                // 连了 90 秒还一分没记，说明服务端当前不给这个账号记时长——
                // 早点喊出来，别让人以为在跑。
                if start.elapsed().as_secs() == 90
                    && state.credited_secs.load(Ordering::Relaxed) == 0
                {
                    let _ = events.send(BoostEvent::Log(
                        "⚠️ 已连接 90 秒仍 0 记账：服务端当前没给这个账号记时长。\
                         常见原因：账号被限流/当日配额用尽，或 task 页前缀不被认。"
                            .to_owned(),
                    ));
                }
            }
        });
    }

    {
        let stopped = stopped.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopped.store(true, Ordering::Relaxed);
            }
        });
    }

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
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        let mut send = tokio::time::interval(options.start_interval);
        send.tick().await;
        let mut rotate = tokio::time::interval(options.dwell);
        rotate.tick().await;

        loop {
            if stopped.load(Ordering::Relaxed) {
                let _ = sink.close().await;
                break 'outer;
            }
            if target_secs > 0 && state.credited_secs.load(Ordering::Relaxed) >= target_secs {
                let _ = sink.close().await;
                let credited = state.credited_secs.load(Ordering::Relaxed);
                let _ = events.send(BoostEvent::Finished {
                    credited_secs: credited,
                    reason: "已达标".to_owned(),
                });
                return;
            }

            tokio::select! {
                _ = ping.tick() => {
                    // engine.io v3：客户端必须主动发 `2`，否则 45 秒被断。
                    if sink.send(WsMessage::Text("2".into())).await.is_err() {
                        reconnects += 1;
                        continue 'outer;
                    }
                }
                _ = send.tick() => {
                    if joined {
                        let target = &targets[index % targets.len()];
                        let payload = serde_json::json!(["start", {
                            "module": target.group,
                            "moduleGroup": target.instance,
                            "client": "U校园pc",
                            "url": target.page_url(&options.page_base),
                            "tag1": target.unit,
                            "tag2": target.micro,
                            "tag3": target.tag3(),
                        }]);
                        let packet = format!("42{NAMESPACE},0{payload}");
                        if sink.send(WsMessage::Text(packet.into())).await.is_err() {
                            reconnects += 1;
                            continue 'outer;
                        }
                    }
                }
                _ = rotate.tick() => {
                    if joined && targets.len() > 1 {
                        // ★ 轮换前**先补一个 `stop`**。
                        //
                        // SDK 原文是 `P.emit("stop", {...T, timer: now})`，
                        // 而本库早期实现直接换 `module` 重发 `start`——
                        // 服务器一直回「task没有信息变更」，等于那个 task
                        // 一直挂着没关。实测这样跑会掉进"服务端不再记账"的状态。
                        let current = &targets[index % targets.len()];
                        let stop = serde_json::json!(["stop", {
                            "module": current.group,
                            "moduleGroup": current.instance,
                            "client": "U校园pc",
                            "url": current.page_url(&options.page_base),
                            "tag1": current.unit,
                            "tag2": current.micro,
                            "tag3": current.tag3(),
                            "timer": now_millis(),
                        }]);
                        let packet = format!("42{NAMESPACE},2{stop}");
                        if sink.send(WsMessage::Text(packet.into())).await.is_err() {
                            reconnects += 1;
                            continue 'outer;
                        }
                        index = (index + 1) % targets.len();
                        let target = &targets[index];
                        let _ = events.send(BoostEvent::Log(format!(
                            "轮换到 {}/{}/{}",
                            target.unit, target.micro, target.group
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
                    if text.starts_with('0') && !text.starts_with("40") {
                        let join = format!(
                            "40{NAMESPACE}?uuid={}&token={},",
                            urlencode(&open_id),
                            urlencode(&token)
                        );
                        let _ = sink.send(WsMessage::Text(join.into())).await;
                        continue;
                    }
                    if text.starts_with(&format!("40{NAMESPACE}")) {
                        joined = true;
                        // 立刻发一次，不等第一个周期。
                        send.reset();
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

    let credited = state.credited_secs.load(Ordering::Relaxed);
    let _ = events.send(BoostEvent::Finished {
        credited_secs: credited,
        reason: format!("已停止（重连 {reconnects} 次）"),
    });
    let _ = start;
}
