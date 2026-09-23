//! 同课多分组并行投放：验证「同一门课的不同分组各一条连接」能不能累加。
//!
//! 用法：
//!   cargo run --offline --release --example dur_blitz -- <实例ID> <秒数> "u6,u6g93,u6g235;u6,u6g93,u6g97;..."
//!
//! ## 为什么这样才对（`dur_probe` 实测出来的）
//!
//! 服务器**靠 `start` 里的 `url` 认任务**：
//!
//! | url | `r_set` 结果 |
//! | --- | --- |
//! | 编造的 `/_pc_default/...#/…/u1/u1g1/…` | `module:"menu"`，`delta:1` ⇒ 进通用桶，**不进教材账** |
//! | 真实任务页 URL | `module:"<分组ID>"`，`delta:20` ⇒ **落进这门课的这个分组** |
//!
//! 归属键是 `(module=分组, moduleGroup=课程)`，所以每条连接必须带**自己那个
//! 分组的真实 URL 与 tag1(单元)/tag2(micro)**，否则会被丢进 `menu`。
//!
//! 若 N 个分组能并行累加，倍率就回来了，而且**精确落在一门课上**。

use futures_util::{SinkExt, StreamExt};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uai_core::Session;

const NS: &str = "/userActivities";
/// 真实任务页的前缀（照抄浏览器地址栏）。
const PAGE: &str = "https://ucontent.unipus.cn/_explorationpc_default/pc.html?cid=1111111111111111111&theme=3264FA&aitutorialId=34852&cloudCurriculaId=100001&source=cloud&courseResourceId=20000000002";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: u64 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(90);
    let spec = args
        .get(2)
        .cloned()
        .expect("缺任务清单，例：\"<实例>|u6|u6g93|u6g235;<实例2>|u5|u5g104|u5g119\"");

    // 每条连接：(课程实例, 单元, micro, 分组)。**课程可以不同**——
    // 这正是要验的：不同课程是否各有各的预算。
    let triples: Vec<(String, String, String, String)> = spec
        .split(';')
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.split('|').map(str::trim).collect();
            (parts.len() == 4).then(|| {
                (
                    parts[0].to_owned(),
                    parts[1].to_owned(),
                    parts[2].to_owned(),
                    parts[3].to_owned(),
                )
            })
        })
        .collect();
    assert!(!triples.is_empty(), "任务清单一个都没解析出来");

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let token = session.annotator_token().expect("铸令牌失败");
    let open_id = session.open_id.clone();

    let seen: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let total: Arc<Mutex<i64>> = Arc::new(Mutex::new(0));
    let started = std::time::Instant::now();

    println!("连接 {}  时长 {seconds}s", triples.len());
    for (instance, unit, micro, group) in &triples {
        println!("  {unit}/{micro}/{group}  @ {instance}");
    }
    println!();

    let mut handles = Vec::new();
    for (instance, unit, micro, group) in triples.clone() {
        let (token, open_id) = (token.clone(), open_id.clone());
        let seen = Arc::clone(&seen);
        let total = Arc::clone(&total);
        handles.push(tokio::spawn(async move {
            let url = format!(
                "wss://ucontent.unipus.cn/unipusio/?uuid={open_id}&token={token}&EIO=3&transport=websocket"
            );
            let (socket, _) = connect_async(&url).await.expect("连接失败");
            let (mut sink, mut stream) = socket.split();
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
            ticker.tick().await;
            let mut joined = false;
            let mut mine = 0i64;
            let page_url = format!("{PAGE}#/{instance}/courseware/{unit}/{micro}/{group}");

            loop {
                if started.elapsed().as_secs() >= seconds {
                    break;
                }
                tokio::select! {
                    _ = ticker.tick() => {
                        if joined {
                            let payload = serde_json::json!(["start", {
                                "module": group,
                                "moduleGroup": instance,
                                "client": "U校园pc",
                                "url": page_url,
                                "tag1": unit,
                                "tag2": micro,
                            }]);
                            let _ = sink
                                .send(Message::Text(format!("42{NS},0{payload}").into()))
                                .await;
                        }
                    }
                    message = stream.next() => {
                        let Some(Ok(message)) = message else { break };
                        let Message::Text(frame) = message else { continue };
                        let frame = frame.to_string();
                        if frame == "2" {
                            let _ = sink.send(Message::Text("3".into())).await;
                            continue;
                        }
                        if frame.starts_with('0') && !frame.starts_with("40") {
                            let _ = sink
                                .send(Message::Text(
                                    format!("40{NS}?uuid={open_id}&token={token},").into(),
                                ))
                                .await;
                            continue;
                        }
                        if frame.starts_with(&format!("40{NS}")) {
                            joined = true;
                            ticker.reset();
                            continue;
                        }
                        if frame.contains("\"r_set\"")
                            && let (Some(delta), Some(stamp)) = (
                                frame
                                    .split("\"delta\":")
                                    .nth(1)
                                    .and_then(|r| r.split(',').next())
                                    .and_then(|r| r.trim().parse::<i64>().ok()),
                                frame
                                    .split("\"msUpdate\":")
                                    .nth(1)
                                    .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
                                    .and_then(|r| r.parse::<u64>().ok()),
                            )
                            && seen.lock().unwrap().insert(stamp)
                        {
                            mine += delta;
                            *total.lock().unwrap() += delta;
                        }
                    }
                }
            }
            (group, mine)
        }));
    }

    for handle in handles {
        if let Ok((group, mine)) = handle.await {
            println!("  分组 {group:<10} 独占新增 delta {mine}s");
        }
    }

    let total = *total.lock().unwrap();
    let wall = started.elapsed().as_secs() as f64;
    println!(
        "\n合计记账 {} 秒（{:.3}h），墙钟 {:.0}s ⇒ 倍率 ≈ {:.2}×",
        total,
        total as f64 / 3600.0,
        wall,
        total as f64 / wall
    );
}
