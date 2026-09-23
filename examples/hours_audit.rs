//! 时长对账：本班「必修学习时长」的**标准**与**已学**逐教材摊开。
//!
//! 用法：`cargo run --offline --release --example hours_audit -- <数字courseId> <classId>`
//!
//! 例：`100001 1111111111111111111`（大学英语I（三）A班）
//!
//! 两部分数据来源不同，必须分开看：
//!
//! | 来源 | 给什么 |
//! | --- | --- |
//! | `plan/detail` | 每本教材的 `minHours` / `maxHours`（**标准**） |
//! | `achievement/queryUserScore` | 每本教材已学时长与该项得分（**已学**） |

use uai_core::Session;
use uai_core::api::assessment;
use uai_core::transport::build_client;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let course_id: i64 = args
        .get(1)
        .and_then(|raw| raw.parse().ok())
        .expect("用法：hours_audit <数字courseId> <classId>");
    let class_id = args.get(2).cloned().unwrap_or_default();

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");
    let client = build_client().expect("建 client");
    let host = session.host_identity();

    // ① 方案：每本教材的时长标准。
    let plan = assessment::fetch_plan(&client, &session.portal_token, &host, course_id, &class_id)
        .expect("取考核方案失败");
    println!("方案：{}", plan.name);
    println!("记分周期：{} ~ {}", plan.cycle_start, plan.cycle_end);
    println!("总权重：{}%\n", plan.total_percent());

    if let Some(item) = plan.duration_item() {
        println!("【item {}】{}  权重 {}%", item.item, item.name, item.percent);
        println!(
            "  {:<52} {:>8} {:>8} {:>8}",
            "教材（资源 ID）", "min小时", "max小时", "权重%"
        );
        for hours in &item.hours {
            println!(
                "  {:<52} {:>8.1} {:>8.1} {:>8.1}",
                hours.resource_id, hours.min_hours, hours.max_hours, hours.percent
            );
        }
    } else {
        println!("⚠️ 方案里没有 item=30（必修学习时长）");
    }

    // ② 成绩：每本教材已学多久、拿了多少分。
    let grade = assessment::fetch_grade(&client, &session.portal_token, &host, course_id, &class_id)
        .expect("取综合成绩失败");

    println!("\n== 已学时长（逐教材）==");
    if let Some(item) = grade.duration_item() {
        println!(
            "【item {}】{}  权重 {}%  本项得分 {:.2}  原文 {}",
            item.item, item.name, item.percent, item.score, item.finish_process
        );
        println!(
            "  {:<46} {:>10} {:>8} {:>8} {:>10}",
            "教材", "已学", "得分", "权重%", "满分需时"
        );
        for book in &item.books {
            let learned = book.duration.clone().unwrap_or_else(|| "-".to_owned());
            let seconds = book.duration_seconds().unwrap_or(0);
            // 标准按「名字里含 resource_id 的关键词」对不上，这里只打已学；
            // 标准看上面那张表（资源 ID）。
            println!(
                "  {:<46} {:>10} {:>8.2} {:>8.1} {:>10}",
                book.name,
                learned,
                book.score,
                book.percent,
                format!("{}s≈{:.2}h", seconds, f64::from(seconds) / 3600.0)
            );
        }
    } else {
        println!("⚠️ 综合成绩里没有 item=30");
    }

    println!("\n提示：刷时长走 WebSocket（`uai duration`），那条链是**账号级**记账；");
    println!("      门户再按教材归集。刷完回来看这张表里「已学」有没有涨。");
}
