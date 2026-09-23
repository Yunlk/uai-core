//! 限流探针：固定间隔连打同一个分组，看服务器多久开始回 `600002`。
//!
//! 用法：`cargo run --offline --example rate_probe -- <实例ID> <分组ID> <间隔ms> <次数>`
//!
//! 为什么要自己写：`submit_one` 撞限流会睡 20/40/60/80 秒，把测量结果搅浑。
//! 这里**只发一次、不重试、不睡觉**，拿到什么码就报什么码——服务器真实的
//! 限流窗口只能这么量。分组选一个已经交过的（幂等，重交无害）。

use std::time::Instant;
use uai_core::Session;
use uai_core::api::submit;
use uai_core::endpoints::submit_url;
use uai_core::transport::{Auth, Transport, build_client};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let instance = args.get(1).cloned().expect("用法：rate_probe <实例ID> <分组ID> <间隔ms> <次数>");
    let group = args.get(2).cloned().expect("缺分组 ID");
    let gap: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1000);
    let count: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(10);

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let token = session.annotator_token().expect("铸令牌失败");
    let client = build_client().expect("建 client");
    let transport = Transport::new(&client, Auth::Annotator(&token));

    // 先把载荷组装好（这一段要读内容 + 读答案，不计入限流测量）。
    let prepared = submit::prepare(&client, &token, &instance, &group, &session.open_id)
        .expect("组装失败");
    let body = prepared.body.clone();
    println!(
        "分组 {group}  间隔 {gap}ms  次数 {count}  载荷 {} 项\n",
        prepared.completed_count()
    );

    let start = Instant::now();
    let mut limited = 0usize;
    for index in 1..=count {
        let began = Instant::now();
        let response = transport.post_json(&submit_url(), &body);
        let code = match &response {
            Ok(value) => value.get("code").and_then(serde_json::Value::as_i64).unwrap_or(-1),
            Err(_) => -2,
        };
        let limited_here = matches!(code, 600001 | 600002);
        if limited_here {
            limited += 1;
        }
        println!(
            "  #{index:>2}  +{:>5}ms  耗时 {:>5}ms  code={code}{}",
            start.elapsed().as_millis(),
            began.elapsed().as_millis(),
            if limited_here { "  ← 限流" } else { "" }
        );
        if index < count && gap > 0 {
            std::thread::sleep(std::time::Duration::from_millis(gap));
        }
    }
    println!("\n{count} 次里被限流 {limited} 次");
}
