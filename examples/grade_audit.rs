//! 对照实验：门户自己的「教程学习成绩」分子/分母是多少，跟 `leafs` 对不对得上。
//!
//! 用法：`cargo run --offline --example grade_audit -- <数字courseId> <classId>`
//!
//! 例：`100001 1111111111111111111`（大学英语I（三）A班）

use uai_core::Session;
use uai_core::api::assessment;
use uai_core::transport::build_client;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let course_id: i64 = args
        .get(1)
        .and_then(|raw| raw.parse().ok())
        .expect("用法：grade_audit <数字courseId> <classId>");
    let class_id = args.get(2).cloned().unwrap_or_default();

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Session = serde_json::from_str(&text).expect("会话不是 JSON");

    let client = build_client().expect("建 client");
    let host = session.host_identity();

    let grade = assessment::fetch_grade(&client, &session.portal_token, &host, course_id, &class_id)
        .expect("取综合成绩失败");

    // 逐考核项打印。
    for item in &grade.items {
        println!(
            "【item {}】{}  权重 {}%  得分 {:.2}  原文 {}",
            item.item, item.name, item.percent, item.score, item.finish_process
        );
    }

    // 进度类（item=21 教程学习成绩）的逐教材明细——分母就在这儿。
    println!("\n== 教程学习成绩（item 21）逐教材 ==");
    if let Some(item) = grade.process_item() {
        for book in &item.books {
            let ratio = book
                .process_ratio()
                .map(|(done, total)| format!("{done}/{total}"))
                .unwrap_or_else(|| "(无 process)".to_owned());
            println!(
                "  {:<45} {:<12} 得分 {:.2}  权重 {}%  原文 {:?}",
                book.name, ratio, book.score, book.percent, book.process
            );
        }
    } else {
        println!("  （没读到 item=21，打全部明细）");
        for item in &grade.items {
            for book in &item.books {
                println!("  [{}] {} => {:?}", item.item, book.name, book.process);
            }
        }
    }
}
