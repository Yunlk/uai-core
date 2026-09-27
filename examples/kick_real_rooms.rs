//! **真键房间探针**：用 `kick-v2` 的倍率链条，但把房间键换成**真实必修任务**。
//!
//! 动机：`duration` 模块的房间池用的是假键（`MODULES` × `GROUPS`，平台应用名与分组名），
//! 所以时长只进账号级积分账，进不了教材账（见 `src/api/duration.rs` 模块文档）。
//! 而记账规则是「按 `(module, moduleGroup)` 成对独立记账」——
//! 那么把键换成 `(真实必修分组, 真实课程实例)`，是否既能放大、又能落到逐题时长？
//!
//! 用法：
//!   cargo run --offline --release --example kick_real_rooms -- <实例ID> <分组1,分组2,...> [秒数]
//!
//! 判据（跑完用 TLA `unitTaskSituation` 逐题对拍）：
//!   每题时长涨 ≈ 秒数 × 倍率  ⇒ 真键可用，**倍率与落账两全**
//!   逐题时长纹丝不动          ⇒ 服务端不认这套键，这条链只能刷账号级
//!
//! ⚠️ 只发 `kick-v2` 计时事件，不碰任何作答数据；账号 ID 由命令行传入。
//! ⚠️ 与 lib 内的加速器同源，属于**写入**行为：跑之前想清楚要不要在真实账号上跑。

use futures_util::{SinkExt, StreamExt};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
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
        .expect("用法：kick_real_rooms <实例ID> <分组1,分组2> [秒数]");
    let groups: Vec<String> = args
        .get(2)
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let seconds: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(300);
    assert!(!groups.is_empty(), "至少要给一个分组 ID");
    assert!(groups.len() <= 20, "房间数上限 20（再多握手会被随机拒绝）");

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let token = session.annotator_token().expect("铸令牌失败");
    let open_id = session.open_id.clone();

    println!("实例：{instance}");
    println!("真键房间 {} 个：{}", groups.len(), groups.join(", "));
    println!(
        "跑 {seconds} 秒；预计倍率 ≈{:.1}×（若真键有效）\n",
        groups.len() as f32 * 0.79
    );

    let credited: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let seen_ms: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut handles = Vec::new();
    for group in groups.clone() {
        let (token, open_id, instance) = (token.clone(), open_id.clone(), instance.clone());
        let credited = credited.clone();
        let seen_ms = seen_ms.clone();
        handles.push(tokio::spawn(async move {
            let ws = uai_core::api::duration::socket_url(&open_id, &token);
            let Ok((socket, _)) = connect_async(ws).await else { return };
            let (mut sink, mut stream) = socket.split();
            let mut joined = false;
            let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
            ping.tick().await;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => { let _ = sink.close().await; return; }
                    _ = ping.tick() => { if sink.send(Message::Text("2".into())).await.is_err() { return; } }
                    message = stream.next() => {
                        let Some(Ok(message)) = message else { return };
                        let Message::Text(frame) = message else { continue };
                        let frame = frame.to_string();
                        if frame == "2" { let _ = sink.send(Message::Text("3".into())).await; continue; }
                        if frame.starts_with('0') {
                            let _ = sink.send(Message::Text(format!("40{NS},").into())).await;
                            continue;
                        }
                        if frame.starts_with(&format!("40{NS}")) && !joined {
                            joined = true;
                            // **真键**：module = 真实必修分组 ID，moduleGroup = 真实课程实例
                            let payload = uai_core::api::duration::kick_payload(
                                &group, &instance, &open_id, uai_core::api::duration::now_millis(),
                            );
                            if sink.send(Message::Text(format!("42{NS},1{payload}").into())).await.is_err() { return; }
                            continue;
                        }
                        if let Some((delta, ms)) = uai_core::api::duration::parse_r_set(&frame) {
                            let fresh = seen_ms.lock().unwrap().insert(ms);
                            if fresh {
                                let module = frame.split("\"module\":\"").nth(1)
                                    .and_then(|rest| rest.split('"').next()).unwrap_or("?").to_owned();
                                *credited.lock().unwrap().entry(module.clone()).or_default() += delta;
                                println!("  r_set module={module} +{delta}s");
                            }
                        }
                    }
                }
            }
        }));
    }

    let started = std::time::Instant::now();
    for handle in handles {
        let _ = handle.await;
    }
    let elapsed = started.elapsed().as_secs().max(1);
    let map = credited.lock().unwrap().clone();
    let total: u64 = map.values().sum();
    println!("\n===== 真键房间结果 =====");
    let mut rows: Vec<_> = map.into_iter().collect();
    rows.sort_by_key(|row| Reverse(row.1));
    for (module, secs) in &rows {
        println!("  {module}  +{secs}s");
    }
    println!(
        "合计 {total}s / 墙钟 {elapsed}s = {:.2}×",
        total as f64 / elapsed as f64
    );
    println!("（下一步：用 TLA unitTaskSituation 查上面这些分组 ID 的逐题时长是否真的涨了）");
}
