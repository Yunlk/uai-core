//! 时长归属探针：验证 `start` 里的客户端 `timer` 能不能左右服务端的 `delta`。
//!
//! 用法：
//!   cargo run --offline --release --example dur_map_probe -- <实例ID> <分组ID> [秒数] [回拨毫秒]
//!
//! 判据：
//!   回拨 = 0      → 基线，delta 应约等于真实墙钟（我们实测 1×）
//!   回拨 > 0      → 若 delta ≈ 回拨秒数 ⇒ **服务端信客户端时间**（可一次刷一小时）
//!                   若 delta 仍≈墙钟 ⇒ 服务端自算，这条路堵死
//!
//! 实测结论（2026-09-24）：
//!   · **回拨无效**：`timer` 伪造成 1 小时前，服务端仍按自己的墙钟算 `delta`；
//!     往 `/duration/` 里塞 `deltaConsistency` / `timestamp` 同样无效。
//!   · **桌面 `start` 事件本身不记账**：单发 `start` 是 0，单发 `/duration/` 也是 0；
//!     先 `start` 把模块标成 active、再 POST `delta`，才有 ≈1 秒/秒 的入账。
//!     这与 [`uai_core::api::duration`] 的结论一致 —— 计分链路是 mobile `kick-v2`，
//!     桌面 `start` 只负责在线状态。
//!   · `client` 取值：真实 PC 页面抓包（浏览器 HAR）里是 `"client":"pc"`，本探针照抄。
//!     ⚠️ **但不要据此认为换别的值就不记账**：`/duration/` 的 body 只有
//!     `{delta, version}`（**不含 `client`**），`client` 只出现在 socket 载荷里；
//!     "只改 client、其它不动"的对照实验尚未做过 —— 此处**没有结论**，别当实测引用。
//!
//! 更快的路子（倍率见 `src/api/duration.rs` 模块文档）：走 mobile `kick-v2` + 房间池，
//! 服务端按 `(module, moduleGroup)` 成对独立记账，约 20 房间 ≈ 15×。
//!
//! 归属映射：`module` = 分组 ID、`moduleGroup` = 课程实例 ID —— 两个都不能为空
//! （PC SDK 的 `Y()` 强校验）。编造 URL 会落进 `module:"menu"` 通用桶。
//!
//! ⚠️ 这是**协议研究**用的一次性探针：只发 `start`，不改任何作答数据。
//! 本项目默认不伪造时间戳（见 AGENTS.md）；此处参数显式传入才生效。

use futures_util::{SinkExt, StreamExt};
use std::collections::HashSet;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uai_core::Session;

const NS: &str = "/userActivities";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let instance = args
        .get(1)
        .cloned()
        .expect("用法：dur_map_probe <实例ID> <分组ID> [秒数] [回拨毫秒]");
    let group = args.get(2).cloned().expect("缺 <分组ID>");
    let seconds: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(40);
    let forge_ms: u64 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);

    // 单元 = 分组 ID 里 `g` 之前那段（u6g235 → u6）；micro 填单元名即可（实测）
    let unit = group.split('g').next().unwrap_or("u1").to_owned();
    let page_base = "https://ucontent.unipus.cn/_explorationpc_default/pc.html".to_owned();
    let url = format!("{page_base}#/{instance}/courseware/{unit}/{unit}/{group}");

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let token = session.annotator_token().expect("铸令牌失败");
    let open_id = session.open_id.clone();

    let now = uai_core::api::duration::now_millis();
    let timer = if forge_ms > 0 {
        now.saturating_sub(forge_ms)
    } else {
        now
    };

    println!("module={group}  moduleGroup={instance}");
    println!("url={url}");
    println!(
        "timer={timer}（{}）  跑 {seconds} 秒\n",
        if forge_ms > 0 {
            format!("回拨 {forge_ms} ms ≈ {} 分钟", forge_ms / 60000)
        } else {
            "当前时间，不回拨".to_owned()
        }
    );

    let ws = format!(
        "wss://ucontent.unipus.cn/unipusio/?EIO=3&transport=websocket&uuid={open_id}&token={token}"
    );
    let (socket, _) = connect_async(&ws).await.expect("连接失败");
    let (mut sink, mut stream) = socket.split();

    let started = std::time::Instant::now();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut total = 0i64;
    let mut joined = false;
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
    let mut send = tokio::time::interval(std::time::Duration::from_secs(5));
    ping.tick().await;
    send.tick().await;

    while started.elapsed().as_secs() < seconds {
        tokio::select! {
            _ = ping.tick() => {
                let _ = sink.send(Message::Text("2".into())).await;
            }
            _ = send.tick() => {
                if joined {
                    let payload = serde_json::json!(["start", {
                        "module": group,
                        "moduleGroup": instance,
                        "client": "pc",
                        "url": url,
                        "tag1": unit,
                        "tag2": unit,
                        "tag3": format!("{{\"microBlock\":\"{unit}/{unit}\",\"version\":\"1\",\"source\":\"ucontent\"}}"),
                        "timer": timer,
                    }]);
                    let _ = sink.send(Message::Text(format!("42{NS},0{payload}").into())).await;
                }
            }
            message = stream.next() => {
                let Some(Ok(message)) = message else { break };
                let Message::Text(frame) = message else { continue };
                let frame = frame.to_string();
                if frame == "2" { let _ = sink.send(Message::Text("3".into())).await; continue; }
                if frame.starts_with('0') && !frame.starts_with("40") {
                    let _ = sink
                        .send(Message::Text(format!("40{NS}?uuid={open_id}&token={token},").into()))
                        .await;
                    continue;
                }
                if frame.starts_with(&format!("40{NS}")) {
                    println!("{} 已入命名空间，开始发 start", started.elapsed().as_secs());
                    joined = true;
                    send.reset();
                    continue;
                }
                if let Some((delta, ms)) = uai_core::api::duration::parse_r_set(&frame)
                    && seen.insert(ms)
                {
                    total += delta as i64;
                    let module = frame
                        .split("\"module\":\"")
                        .nth(1)
                        .and_then(|rest| rest.split('"').next())
                        .unwrap_or("?");
                    println!("  r_set module={module} delta={delta} msUpdate={ms}");
                }
            }
        }
    }

    println!(
        "\n合计 delta = {total} 秒 / 墙钟 {} 秒（倍率 {:.2}×）",
        started.elapsed().as_secs(),
        total as f64 / started.elapsed().as_secs().max(1) as f64
    );
}
