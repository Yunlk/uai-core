//! URL 探针：`start` 里的 `url` 各部分到底哪些是**必须真实**的。
//!
//! 用法：
//!   cargo run --offline --release --example url_probe -- <实例ID> <单元> <micro> <分组> <秒数> <页面前缀>
//!
//! 输出每次 `r_set` 的 `module` 与 `delta`：
//! - `module` == 分组 ID ⇒ **落进这门课的这个分组**（教材账）
//! - `module` == `"menu"` ⇒ 进了通用桶，**不进教材账**
//!
//! 靠它做 A/B：改 `micro`、改 `unit`、改 `group`、去掉查询串，看归属变不变。

use futures_util::{SinkExt, StreamExt};
use std::collections::HashSet;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uai_core::Session;

const NS: &str = "/userActivities";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let instance = args.get(1).cloned().expect("用法：url_probe <实例> <unit> <micro> <group> <秒> <前缀>");
    let unit = args.get(2).cloned().unwrap_or_else(|| "u6".to_owned());
    let micro = args.get(3).cloned().unwrap_or_else(|| "u6g93".to_owned());
    let group = args.get(4).cloned().unwrap_or_else(|| "u6g235".to_owned());
    let seconds: u64 = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(35);
    let base = args.get(6).cloned().unwrap_or_default();

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let token = session.annotator_token().expect("铸令牌失败");
    let open_id = session.open_id.clone();

    let page_url = format!("{base}#/{instance}/courseware/{unit}/{micro}/{group}");
    println!("url = {page_url}");

    let url = format!(
        "wss://ucontent.unipus.cn/unipusio/?EIO=3&transport=websocket&uuid={open_id}&token={token}"
    );
    let (socket, _) = connect_async(&url).await.expect("连接失败");
    let (mut sink, mut stream) = socket.split();

    let started = std::time::Instant::now();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut total = 0i64;
    let mut modules: Vec<String> = Vec::new();
    let mut joined = false;
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
    let mut send = tokio::time::interval(std::time::Duration::from_secs(5));
    ping.tick().await;
    send.tick().await;

    loop {
        if started.elapsed().as_secs() >= seconds {
            break;
        }
        tokio::select! {
            _ = ping.tick() => {
                let _ = sink.send(Message::Text("2".into())).await;
            }
            _ = send.tick() => {
                if joined {
                    let payload = serde_json::json!(["start", {
                        "module": group,
                        "moduleGroup": instance,
                        "client": "U校园pc",
                        "url": page_url,
                        "tag1": unit,
                        "tag2": micro,
                        "tag3": format!("{{\"microBlock\":\"{unit}/{micro}\",\"version\":\"1\",\"source\":\"ucontent\"}}"),
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
                    joined = true;
                    send.reset();
                    continue;
                }
                if let Some((delta, ms)) = uai_core::api::duration::parse_r_set(&frame)
                    && seen.insert(ms)
                {
                    total += delta as i64;
                    if let Some(module) = frame
                        .split("\"module\":\"")
                        .nth(1)
                        .and_then(|rest| rest.split('"').next())
                    {
                        modules.push(module.to_owned());
                        println!("  r_set module={module:<10} delta={delta}");
                    }
                }
            }
        }
    }

    let distinct: HashSet<&String> = modules.iter().collect();
    println!(
        "\n合计 {total} 秒 / 墙钟 {} 秒；收到的 module：{:?}",
        started.elapsed().as_secs(),
        distinct
    );
    if modules.is_empty() {
        println!("⚠️ 0 记账");
    } else if distinct.iter().all(|m| m.as_str() == "menu") {
        println!("⇒ 进了通用桶：hash 三段**没被认成真实任务**");
    } else {
        println!("⇒ 落进真实任务（教材账）");
    }
}
