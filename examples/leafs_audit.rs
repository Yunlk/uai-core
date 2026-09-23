//! 对照实验：`leafs.required` 从哪个接口来、合计多少个、其中几个真拿到分。
//!
//! 用法：`cargo run --offline --example leafs_audit -- <实例ID>`
//!
//! | 端点 | 拿到的 |
//! | --- | --- |
//! | `course_progress/{inst}/{open}/default` | `rt.units.<uN>.strategies.required` |
//! | `course_progress/{inst}/tasks/{open}/default?tasks=<逗号分隔 ID>` | `rt.leafs.<gid>…` |
//!
//! 目的：跟门户「教程学习成绩」的分母与分子分别对一次。

use serde_json::Value;
use uai_core::annotator;
use uai_core::api::{catalog, progress};
use uai_core::endpoints::course_progress_url;
use uai_core::transport::{Auth, Transport};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let instance = args
        .get(1)
        .map(String::as_str)
        .expect("用法：leafs_audit <实例ID>");

    let text = std::fs::read_to_string(std::env::temp_dir().join("uai-session.json"))
        .expect("读不到会话缓存，先 `uai login`");
    let session: Value = serde_json::from_str(&text).expect("会话不是 JSON");
    let open_id = session["open_id"].as_str().unwrap_or_default().to_owned();
    let token = annotator::mint(&open_id).expect("铸令牌失败");

    let client = uai_core::transport::build_client().expect("建 client");

    // ① 单元级端点。
    let transport = Transport::new(&client, Auth::Annotator(&token));
    let units_body = transport
        .get_json(&course_progress_url(instance, &open_id))
        .expect("单元级请求失败");
    println!("== 单元级端点 ==");
    println!(
        "rt 的键：{:?}",
        units_body
            .get("rt")
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );
    println!("有 leafs 吗：{}", units_body.pointer("/rt/leafs").is_some());
    let snapshot = progress::fetch(&client, &token, instance, &open_id).expect("解析失败");
    println!(
        "单元级必修：{:?}  该端点 leafs 解析出 {} 项",
        snapshot.required_units(),
        snapshot.leafs.len()
    );

    // ② 目录里的全部分组 ID。
    let catalog = catalog::fetch(&client, &token, instance).expect("目录失败");
    let ids: Vec<String> = catalog
        .units
        .iter()
        .flat_map(|unit| unit.groups.iter())
        .map(|group| group.id.clone())
        .collect();
    println!("\n目录：{} 个分组", ids.len());

    // ③ tasks 端点：一次把全部 ID 发过去。
    let leafs_body = progress::fetch_group_progress_raw(&client, &token, instance, &open_id, &ids)
        .expect("tasks 请求失败");
    println!("\n== tasks 端点 ==");
    let leafs = leafs_body
        .pointer("/rt/leafs")
        .or_else(|| leafs_body.get("leafs"))
        .and_then(Value::as_object);

    let Some(map) = leafs else {
        println!("⚠️ tasks 端点也没有 leafs。原始响应（前 3000 字符）：");
        let text = serde_json::to_string(&leafs_body).unwrap_or_default();
        println!("{}", text.chars().take(3000).collect::<String>());
        return;
    };

    let required: Vec<&String> = map
        .iter()
        .filter(|(_, value)| {
            value.pointer("/strategies/required").and_then(Value::as_bool) == Some(true)
        })
        .map(|(id, _)| id)
        .collect();
    let gated: Vec<&String> = map
        .iter()
        .filter(|(_, value)| {
            value
                .pointer("/strategies/min_score_pct")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                > 0.0
        })
        .map(|(id, _)| id)
        .collect();
    println!("leafs 共 {} 项", map.len());
    println!("★ required=true：{} 个", required.len());
    println!("★ min_score_pct>0：{} 个", gated.len());

    // ★ 关键对照：必修组里**真正拿到计分点**的有几个？
    let scored: Vec<&String> = required
        .iter()
        .filter(|id| map[id.as_str()].pointer("/score/score").and_then(Value::as_i64) == Some(1))
        .copied()
        .collect();
    let unscored: Vec<&String> = required
        .iter()
        .filter(|id| !scored.contains(id))
        .copied()
        .collect();
    println!(
        "\n★★ 必修组里 score.score==1（门户认的计分点）：{} 个",
        scored.len()
    );
    println!("★★ 必修但没拿到分：{} 个", unscored.len());
    println!("没拿到分的：{:?}", &unscored[..unscored.len().min(20)]);

    println!("\n样本（必修组前 6 个）：");
    for id in required.iter().take(6) {
        let value = &map[id.as_str()];
        println!(
            "  {id} => pass={:?} score.score={:?} mini={:?}",
            value.pointer("/state/pass"),
            value.pointer("/score/score"),
            value.pointer("/strategies/min_score_pct"),
        );
    }
}
