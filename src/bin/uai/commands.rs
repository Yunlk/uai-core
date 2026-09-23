//! 各子命令的实现。每个命令只调对应的接口文件，不含业务判断。

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use uai_core::api::targets::PlanOptions;
use uai_core::api::{
    answer, assessment, bookshelf, catalog, content, course_list, group_state, progress,
    speech_eval, submit, targets, user_info,
};
use uai_core::error::{Error, Result};
use uai_core::question::{all_kind_names, kind_by_name};
use uai_core::transport::build_cookie_client;

use crate::cli::{Command, load_session, print_json, save_session, session_path};

/// 撞限流时把该组重排队尾，最多重排几次。
///
/// 超过就当失败报出来——不无限循环，也不静默丢掉。
const MAX_REQUEUE: u32 = 3;

/// 重排队尾前让这一步退让多久。
///
/// ⚠️ 曾经按「猛蹬」的测法设成 **0**（不等待、直接重排队尾再打），用来量服务端
/// 在无退避下的反应。实测结论：连打 11 组后服务端开始回 `600002 操作过于频繁`，
/// 且**退避为 0 时重排队尾只会立刻再撞一次**，等于把配额烧光。
/// 现在恢复成真等待——留出配额恢复的窗口。
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(45);

/// 执行一个命令。
pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Help => {
            println!("{}", crate::cli::HELP);
            Ok(())
        }
        Command::Login => login(),
        Command::WhoAmI => whoami(),
        Command::Bookshelf => bookshelf_cmd(),
        Command::Grade {
            course_resource_id,
            plan_only,
        } => grade_cmd(&course_resource_id, plan_only),
        Command::Progress { instance_id } => progress_cmd(&instance_id),
        Command::Catalog {
            instance_id,
            required_only,
        } => catalog_cmd(&instance_id, required_only),
        Command::Content {
            instance_id,
            group_id,
            raw,
        } => content_cmd(&instance_id, &group_id, raw),
        Command::Answer {
            instance_id,
            group_id,
        } => answer_cmd(&instance_id, &group_id),
        Command::State {
            instance_id,
            group_id,
        } => state_cmd(&instance_id, &group_id),
        Command::StateRaw {
            instance_id,
            group_id,
        } => state_raw_cmd(&instance_id, &group_id),
        Command::Snapshot {
            instance_id,
            group_id,
        } => snapshot_cmd(&instance_id, &group_id),
        Command::Targets {
            instance_id,
            class_id,
            skip_passed,
            skip_scan,
        } => targets_cmd(&instance_id, &class_id, skip_passed, skip_scan),
        Command::Submit {
            instance_id,
            group_id,
            confirm,
        } => submit_cmd(&instance_id, &group_id, confirm),
        Command::Auto {
            instance_id,
            class_id,
            units,
            dry_run,
            limit,
            redo,
            no_scan,
            jobs,
            no_readback,
            sleep_ms,
        } => auto_cmd(
            &instance_id,
            &class_id,
            AutoRun {
                dry_run,
                limit,
                redo,
                no_scan,
                jobs,
                no_readback,
                sleep_ms,
                units,
            },
        ),
        Command::Media => media_cmd(),
        Command::Eval {
            wav,
            from_group,
            text,
            kind,
            llm_sk,
            user,
            index,
        } => eval_cmd(
            &wav,
            from_group.as_ref(),
            text.as_deref(),
            &kind,
            llm_sk,
            &user,
            index,
        ),
        Command::Duration { hours, rooms } => duration_cmd(hours, rooms),
        Command::Study {
            instance_id,
            class_id,
            hours,
            dwell_min,
            page_base,
            units,
        } => study_cmd(
            &instance_id,
            &class_id,
            hours,
            dwell_min,
            page_base.as_deref(),
            &units,
        ),
        Command::Types => types_cmd(),
    }
}

/// 取环境变量里的凭据。
fn credentials() -> Result<(String, String)> {
    let user = std::env::var("UAI_USER")
        .map_err(|_| Error::invalid("缺少环境变量 UAI_USER（手机号/邮箱）"))?;
    let pass = std::env::var("UAI_PASS").map_err(|_| Error::invalid("缺少环境变量 UAI_PASS"))?;
    if user.trim().is_empty() || pass.is_empty() {
        return Err(Error::invalid("UAI_USER / UAI_PASS 不能为空"));
    }
    Ok((user, pass))
}

/// 需要登录的命令：拿会话，必要时提示先 `login`。
fn require_session() -> Result<uai_core::Session> {
    let session = load_session().ok_or_else(|| {
        Error::invalid(format!(
            "尚未登录。请先运行 `uai login`（会话缓存在 {}）",
            session_path().display()
        ))
    })?;
    if !session.is_authenticated() {
        return Err(Error::invalid("会话已失效，请重新 `uai login`"));
    }
    Ok(session)
}

/// 把 `--class` 落到**门户里的具体班级**，并核对这本教材确实挂在那个班。
///
/// ## 为什么必须先定班
///
/// 提交目标是**当前班级的必修**。同一本教材会挂在多个班，计分点分母不同
/// （实测同一本《视听说2》：一个班 `49`、另一个班 `35`），所以「交哪些组」
/// 这件事离开班级就没有确定答案。
///
/// ## 这里做的事
///
/// 1. 读门户课程列表（按班给出 `finishPointNum/totalPointNum`）；
/// 2. 用 `--class` 匹配 `classId` / 数字 `courseId` / 班级名；
/// 3. 确认该班里确实有 `<实例ID>` 这本教材——**没有就直接失败**，
///    而不是拿着一个不存在的组合继续往下发写请求。
fn resolve_class(
    session: &uai_core::Session,
    instance_id: &str,
    selector: &str,
) -> Result<course_list::CourseProgress> {
    let client = uai_core::transport::build_client()?;
    let items = course_list::fetch(&client, &session.portal_token)?;
    let needle = selector.trim();

    let in_class: Vec<&course_list::CourseProgress> = items
        .iter()
        .filter(|item| {
            item.class_id == needle
                || item.course_id.to_string() == needle
                || (!item.class_name.trim().is_empty() && item.class_name == needle)
        })
        .collect();

    if in_class.is_empty() {
        let known: Vec<String> = {
            let mut names: Vec<String> = items
                .iter()
                .map(|item| {
                    format!(
                        "{}（classId={} courseId={}）",
                        item.class_name, item.class_id, item.course_id
                    )
                })
                .collect();
            names.sort();
            names.dedup();
            names
        };
        return Err(Error::invalid(format!(
            "找不到班级 `{needle}`。当前账号的班级有：\n  {}",
            if known.is_empty() {
                "（课程列表为空）".to_owned()
            } else {
                known.join("\n  ")
            }
        )));
    }

    match in_class.iter().find(|item| item.instance_id == instance_id) {
        Some(item) => Ok((*item).clone()),
        None => {
            let has: Vec<String> = in_class
                .iter()
                .map(|item| format!("{}（{}）", item.name, item.instance_id))
                .collect();
            Err(Error::invalid(format!(
                "班级 `{}` 里没有教材 `{instance_id}`。这个班下的教材是：\n  {}",
                in_class[0].class_name,
                has.join("\n  ")
            )))
        }
    }
}

/// 拿一个可用于 ucontent 的令牌。
///
/// open_id 缺失时退回门户 JWT——目录/内容接口不校验鉴权，所以能用；
/// 但**进度/答案接口会 401**，所以那种情况下要明确警告。
fn ucontent_token(session: &uai_core::Session) -> Result<(String, bool)> {
    if session.can_touch_protected() {
        return Ok((session.annotator_token()?, true));
    }
    eprintln!(
        "警告：会话里没有 open_id（登录响应未带 ssoId），退回门户令牌。\n\
         目录/内容接口不校验鉴权，仍可读取；但进度/答案接口会 401。"
    );
    Ok((session.portal_token.clone(), false))
}

/// 登录并缓存会话。
fn login() -> Result<()> {
    let (user, pass) = credentials()?;
    let client = build_cookie_client()?;

    println!("正在登录…");
    let ticket = uai_core::api::login::login_with_password(&client, &user, &pass)?;
    let session = user_info::fetch(&client, &ticket.jwt)?;

    if session.open_id.is_empty() {
        eprintln!(
            "警告：门户用户信息里没有 ssoId，ucontent 的受保护接口将无法访问。\n\
             这通常意味着登录响应结构与预期不符。"
        );
    }

    save_session(&session)?;
    println!("登录成功：{}", session.describe());
    println!("会话已缓存到 {}", session_path().display());
    Ok(())
}

/// 打印当前会话。
fn whoami() -> Result<()> {
    match load_session() {
        Some(session) => {
            println!("{}", session.describe());
            println!(
                "  门户令牌：{}",
                if session.is_authenticated() {
                    "有"
                } else {
                    "无"
                }
            );
            println!(
                "  open_id ：{}",
                if session.can_touch_protected() {
                    "有"
                } else {
                    "无（受保护接口不可用）"
                }
            );
            println!("  缓存位置：{}", session_path().display());
        }
        None => {
            println!("尚未登录（缓存文件不存在：{}）", session_path().display());
        }
    }
    Ok(())
}

/// 考核方案 + 综合成绩。
///
/// 链路：`courseResourceId` → `getCourseResourceInfoById`（拿数字 `courseId` 与
/// `classId`）→ `/api/aca/{plan/detail, achievement/queryUserScore}`。
///
/// ⚠️ 考核接口的 `courseInstanceId` 是**数字 `courseId`**，不是 `course-v2:` 实例 ID；
/// 直接传实例 ID 会让服务端抛 `Cannot deserialize value of type long from String`。
fn grade_cmd(course_resource_id: &str, plan_only: bool) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_assessment() {
        return Err(Error::invalid(
            "考核接口需要 school 与 open_id（宿主头 u-school / u-openid）。\n\
             当前会话缺少它们 —— 会话缓存是旧格式，请重新运行 `uai login`。",
        ));
    }
    let client = uai_core::transport::build_client()?;
    let host = session.host_identity();

    println!("读取班级教材信息（courseResourceId={course_resource_id}）…");
    let info = assessment::fetch_resource_info(&client, &session.portal_token, course_resource_id)?;
    println!(
        "  班级　　：{}（classId={}）",
        info.course_name, info.class_id
    );
    println!("  教材　　：{}", info.resource_name);
    println!(
        "  courseId：{}（考核接口的 courseInstanceId）",
        info.course_id
    );
    println!("  实例 ID ：{}", info.course_instance_id);
    println!("  strategy：{}\n", info.strategy_id);

    if info.course_id <= 0 {
        return Err(Error::parse(
            "班级教材信息里没有数字 courseId，无法调用考核接口",
        ));
    }

    // 考核方案：逐教材时长标准。
    let plan = assessment::fetch_plan(
        &client,
        &session.portal_token,
        &host,
        info.course_id,
        &info.class_id,
    )?;
    println!("【考核方案】{}", plan.name);
    println!(
        "  记分周期：{} ~ {}",
        format_millis(&plan.cycle_start),
        format_millis(&plan.cycle_end)
    );
    for item in &plan.items {
        println!("  · {}（item={}，{}%）", item.name, item.item, item.percent);
        for std in &item.hours {
            println!(
                "      {} → 最低 {}h / 满分 {}h，占 {}%",
                std.resource_id, std.min_hours, std.max_hours, std.percent
            );
        }
    }
    println!("  权重合计：{}%\n", plan.total_percent());

    if plan_only {
        return Ok(());
    }

    // 综合成绩。
    let grade = assessment::fetch_grade(
        &client,
        &session.portal_token,
        &host,
        info.course_id,
        &info.class_id,
    )?;
    println!("【综合成绩】{}", grade.describe());
    for item in &grade.items {
        println!(
            "\n  ▸ {}（item={}，权重 {}%）→ {} 分",
            item.name, item.item, item.percent, item.score
        );
        println!(
            "    完成情况：{}{}",
            item.finish_process,
            if item.origin_score != 0.0 {
                format!("（加权前 {:.1} 分）", item.origin_score)
            } else {
                String::new()
            }
        );
        for book in &item.books {
            let detail = if let Some((done, total)) = book.process_ratio() {
                // ★ 这个分母就是门户认可的计分点数。
                format!("进度 {done}/{total}")
            } else if let Some(seconds) = book.duration_seconds() {
                format!(
                    "时长 {}（{} 秒）",
                    book.duration.as_deref().unwrap_or(""),
                    seconds
                )
            } else if let Some(times) = book.completed_times {
                format!("参与 {times} 次")
            } else {
                String::new()
            };
            println!(
                "      {:<28} 占 {:>3}%  得分 {:>6.1}  {}",
                book.name, book.percent, book.score, detail
            );
        }
    }

    // 把「已学时长」对上「满分标准」，直接告诉还差多少。
    if let (Some(std), Some(score_item)) =
        (plan.hours_for(&info.resource_id), grade.duration_item())
        && let Some(book) = score_item.book_named(&info.resource_name)
        && let Some(seconds) = book.duration_seconds()
    {
        let need = std.required_seconds();
        println!(
            "\n  ★ 本教材时长：已学 {}，满分标准 {}h（{}），按标准折算 {:.0}%",
            book.duration.as_deref().unwrap_or(""),
            std.max_hours,
            format_seconds(need),
            std.ratio_for(seconds) * 100.0
        );
        if seconds < need {
            println!("     还差 {}", format_seconds(need - seconds));
        }
    }

    Ok(())
}

/// 毫秒时间戳字符串 → 可读时间（`YYYY-MM-DD`）。
///
/// 只做简单换算，**不引 chrono**：这里的目的只是让人看懂周期，不是精确时间库。
fn format_millis(text: &str) -> String {
    let Ok(millis) = text.trim().parse::<i64>() else {
        return if text.trim().is_empty() {
            "未设置".to_owned()
        } else {
            text.to_owned()
        };
    };
    let days = millis / 86_400_000;
    // 1970-01-01 起的民用日期换算（Howard Hinnant 的 days_from_civil 逆运算）。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// 秒数 → `HH:MM:SS`。
fn format_seconds(total: u32) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// 门户书架。
fn bookshelf_cmd() -> Result<()> {
    let session = require_session()?;
    let client = uai_core::transport::build_client()?;
    let shelf = bookshelf::fetch(&client, &session.portal_token)?;

    println!(
        "共 {} 个教学班，{} 本教材\n",
        shelf.classes.len(),
        shelf.textbook_count()
    );
    for class in &shelf.classes {
        println!("【{}】（courseId={}）", class.name, class.course_id);
        for book in &class.textbooks {
            let activation = if !book.activation_known {
                "激活状态未知"
            } else if book.active {
                "已激活"
            } else {
                "未激活"
            };
            println!(
                "    {:<28} {:<22} {}",
                book.name, book.instance_id, activation
            );
        }
        println!();
    }
    Ok(())
}

/// 班级真实进度。
fn progress_cmd(instance_id: &str) -> Result<()> {
    let session = require_session()?;
    let client = uai_core::transport::build_client()?;
    let items = course_list::fetch(&client, &session.portal_token)?;

    // ⚠️ 同一本教材会挂在多个班里，读数**各不相同**（实测《视听说2》三班 11/49、
    // 二班 35/35）。所以这里把**所有**班级的读数都列出来，不挑一条就当答案。
    let matches: Vec<&uai_core::api::course_list::CourseProgress> = items
        .iter()
        .filter(|item| item.instance_id == instance_id)
        .collect();

    if matches.is_empty() {
        println!("门户没有给出该教材的进度：{instance_id}");
        println!("\n可用的教材：");
        for other in &items {
            println!("    {:<44} {}", other.label(), other.ratio_text());
        }
        return Ok(());
    }

    println!("教材：{}", matches[0].name);
    println!("实例：{instance_id}\n");

    if matches.len() > 1 {
        println!(
            "⚠️ 这本教材挂在 {} 个班里，**每个班的计分点总数都不同**，读数不能混用：\n",
            matches.len()
        );
    }
    for item in &matches {
        println!(
            "  {:<26} courseId={:<8} 计分点 {:>7}  完成度 {:>5.1}%{}",
            item.class_name,
            item.course_id,
            item.ratio_text(),
            item.percent(),
            if item.is_complete() {
                "  ✔ 已完成"
            } else {
                ""
            }
        );
    }
    println!("\n注：这是唯一的权威完成度来源。progress/v2 的 state 是账号级痕迹，不可用来计数。");
    if matches.len() > 1 {
        println!("    跨班比较前请先确定班级——同一实例在不同班的口径不同。");
    }
    Ok(())
}

/// 教材目录。
///
/// `--required` 时用**单元级**必修过滤（第一级粗筛）。目录里没有必修标记，
/// 所以这里只能做到单元级；任务级精筛见 `uai targets`。
fn catalog_cmd(instance_id: &str, required_only: bool) -> Result<()> {
    let session = require_session()?;
    let (token, _) = ucontent_token(&session)?;
    let client = uai_core::transport::build_client()?;
    let catalog = catalog::fetch(&client, &token, instance_id)?;

    println!(
        "{} 个单元，{} 个分组\n",
        catalog.units.len(),
        catalog.group_count()
    );
    if let Some(resource_id) = &catalog.resource_id {
        println!("资源 ID：{resource_id}");
    }

    // 拿必修单元（单元级，仅供粗看）。没有 open_id 就退化为不过滤。
    let required_units: Vec<String> = if required_only {
        if session.can_touch_protected() {
            let token = session.annotator_token()?;
            let snapshot = progress::fetch(&client, &token, instance_id, &session.open_id)?;
            let units: Vec<String> = snapshot
                .required_units()
                .iter()
                .map(|id| (*id).to_owned())
                .collect();
            println!("必修单元（单元级）：{}", units.join(", "));
            println!(
                "注：这是**单元级**清单，只作粗看。分组级必修（= 门户计分点的分母）\
                 要用 `uai targets {instance_id} --class <班级>`。\n"
            );
            units
        } else {
            eprintln!("警告：没有 open_id，无法读单元级必修；--required 将不过滤。\n");
            Vec::new()
        }
    } else {
        Vec::new()
    };

    for unit in &catalog.units {
        // --required：只留必修单元；否则全要。
        if required_only && !required_units.is_empty() && !required_units.contains(&unit.id) {
            continue;
        }
        let groups: Vec<_> = unit
            .groups
            .iter()
            .filter(|group| !required_only || group.is_submittable())
            .collect();
        if groups.is_empty() {
            continue;
        }
        println!("【{}】{}", unit.id, unit.name);
        for group in groups {
            let marks = [
                if group.is_objective() {
                    "客观题"
                } else {
                    ""
                },
                if group.is_content_only() {
                    "纯内容"
                } else {
                    ""
                },
            ]
            .iter()
            .filter(|text| !text.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
            println!(
                "    {:<8} {:<34} {:<26} {}",
                group.id, group.base, group.name, marks
            );
        }
        println!();
    }
    Ok(())
}

/// 解密题目内容：只显示题型与媒体下标，**不提交**。
fn content_cmd(instance_id: &str, group_id: &str, raw: bool) -> Result<()> {
    let session = require_session()?;
    let (token, _) = ucontent_token(&session)?;
    let client = uai_core::transport::build_client()?;
    let (plain, metas) = content::fetch_with_meta(&client, &token, instance_id, group_id)?;

    println!("{group_id}：解析出 {} 道题\n", metas.len());
    for (index, meta) in metas.iter().enumerate() {
        let kind = meta.kind();
        println!(
            "  [{}] {:<24} reply={:<22} category={:<5} 子题={:<3} 媒体下标={:?}",
            index,
            if meta.question_type.is_empty() {
                "（无）"
            } else {
                &meta.question_type
            },
            if meta.reply_type.is_empty() {
                "（无）"
            } else {
                &meta.reply_type
            },
            meta.category
                .map(|value| value.to_string())
                .unwrap_or_else(|| "无".to_owned()),
            meta.child_count,
            meta.media_indices
        );
        println!("       题型：{:<14} {}", kind.name(), kind.scoring_note());
        println!("       instanceId = {}", meta.instance_id);
        println!(
            "       会提交吗：{}   贡献项数 = {}",
            if meta.is_submittable() {
                "是"
            } else {
                "否（学习题）"
            },
            meta.is_completed_len()
        );
    }

    println!("\n内容明文长度 {} 字节（未提交任何作答）", plain.len());
    if raw {
        println!("\n──── 解密后明文 ────");
        println!("{plain}");
        println!("──── 明文结束 ────");
    }
    Ok(())
}

/// 语音评测：把 WAV 送去打分。**只评分，不写任何服务端状态。**
///
/// 这是「口语题真实分数从哪来」的答案：分数由服务端评测产生，
/// 而不是客户端编的。评测结果怎么用于提交是另一回事（见 `submit`）。
fn eval_cmd(
    wav: &std::path::Path,
    from_group: Option<&(String, String)>,
    text: Option<&str>,
    kind: &str,
    llm_sk: Option<String>,
    user: &str,
    index: Option<usize>,
) -> Result<()> {
    // ① 读 WAV 并转成协议要的 PCM。
    let bytes = std::fs::read(wav)
        .map_err(|error| Error::invalid(format!("读不到 WAV 文件 {}：{error}", wav.display())))?;
    let (pcm, sample_rate, channels) = speech_eval::to_pcm_16k_mono(&bytes)?;

    // ② 定评测类型。
    let api_name = match kind {
        "sent" | "sentence" | "en.sent.score" => speech_eval::ApiName::SentenceScore,
        "open" | "steam" | "open.steam" => speech_eval::ApiName::OpenSteam,
        other => {
            return Err(Error::invalid(format!(
                "未知评测类型 `{other}`：可用 {}",
                speech_eval::ApiName::all()
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )));
        }
    };

    // ③ 定参考文本：直接给，或从分组内容里提。
    let transcript = match (text, from_group) {
        (Some(text), _) => text.to_owned(),
        (None, Some((instance_id, group_id))) => {
            let session = require_session()?;
            let (token, _) = ucontent_token(&session)?;
            let client = uai_core::transport::build_client()?;
            let plain = content::fetch_decrypted(&client, &token, instance_id, group_id).map_err(
                |error| {
                    Error::api(format!(
                        "读取 {group_id} 内容失败（实例 ID `{instance_id}`）：{}",
                        error.message()
                    ))
                },
            )?;
            let texts = extract_speech_texts(&plain);
            if texts.is_empty() {
                return Err(Error::invalid(format!(
                    "{group_id} 里没提到可用的题目文本——用 --text 手动给"
                )));
            }

            // ⚠️ 多子题必须**只取一道**：拼接会让完整度对不上。
            // 实测 u5g341（5 个子题）拼接后 417 字 vs 音频一句 → 92 分掉到 29。
            if texts.len() > 1 {
                println!("{group_id} 有 {} 个子题，每句要单独评测：", texts.len());
                for (index, text) in texts.iter().enumerate() {
                    let preview: String = text.chars().take(64).collect();
                    println!(
                        "  [{index}] {preview}{}",
                        if text.chars().count() > 64 { "…" } else { "" }
                    );
                }
                let Some(index) = index else {
                    return Err(Error::invalid(format!(
                        "这组有 {} 个子题，必须用 --index <N> 指定评哪一个\
                         （拼成一段会让完整度对不上，实测分数会腰斩）",
                        texts.len()
                    )));
                };
                let picked = texts.get(index).ok_or_else(|| {
                    Error::invalid(format!(
                        "--index {index} 越界：这组只有 {} 个子题",
                        texts.len()
                    ))
                })?;
                println!("→ 评测第 {index} 个\n");
                picked.clone()
            } else {
                texts.into_iter().next().unwrap_or_default()
            }
        }
        (None, None) => unreachable!("cli 已保证二者必有一个"),
    };

    // ④ 身份：默认用会话里的 open_id，前缀由模块按题型加。
    let user_id = if user.is_empty() {
        load_session()
            .map(|session| session.open_id)
            .unwrap_or_default()
    } else {
        user.to_owned()
    };

    let mut request = speech_eval::EvalRequest::new(pcm, transcript, api_name).with_user(user_id);
    if let Some(key) = llm_sk {
        request = request.with_llm_sk(key);
    }

    println!(
        "音频 {}：{}Hz {}ch，{} 字节 PCM",
        wav.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        sample_rate,
        channels,
        request.pcm.len()
    );
    println!("评测类型：{}", api_name.as_str());
    if api_name.needs_llm_sk() {
        println!("⚠️ open.steam 会用 --llm-sk 那把 key 的额度（它属于优学院，非本项目）");
    }
    println!("正在评测……\n");

    let result = speech_eval::evaluate(request)?;

    // ⑤ 只打印。**不提交。**
    println!("──── 评测结果 ────");
    println!("  总分      {} / 100", result.total);
    if result.accuracy > 0.0 {
        println!("  准确度    {}", result.accuracy);
    }
    if result.completeness > 0.0 {
        println!("  完整度    {}", result.completeness);
    }
    if result.audio_time > 0.0 {
        println!("  音频时长  {} 秒", result.audio_time);
    }
    if !result.uuid.is_empty() {
        println!("  uuid      {}", result.uuid);
    }
    if !result.audio_url.is_empty() {
        println!("  音频地址  {}", result.audio_url);
        println!("            ↑ 这就是 recordDetail.audioUrl 需要的值");
    }
    if !result.asr_detail.is_empty() {
        println!("  ASR 转写  {}", result.asr_detail);
    }
    println!(
        "\n  折算：specific_scores.total = {:.2}（0~1）",
        result.score_ratio()
    );
    println!("        recordDetail.score   = {}", result.record_score());
    println!("\n⚠️ 仅评分，未提交。要不要写进成绩由你决定（见 `uai submit`）。");
    Ok(())
}

/// 从解密后的题目明文里提取「可朗读/可回答的英文文本」。
///
/// 两类题都要用：
/// - **朗读题**（`sentencerecord`）：取 `rule.text`，那是标准原文；
/// - **开放题**（`presentation` / `oral-personal-state`）：`rule.text` 是题目本身。
///
/// ⚠️ **多子题的题必须按子题分别评测。** `u5g341` 有 5 个朗读子题，
/// 每句一个 `rule.text`。若把 5 句拼成一段送去评，服务端的「完整度」会
/// 按 5 句算，而音频只读了一句——实测总分从 92 掉到 **29**。
/// 所以这里返回**逐子题的列表**，由调用方按 `--index` 选。
fn extract_speech_texts(plain: &str) -> Vec<String> {
    let mut picked: Vec<String> = Vec::new();

    // ① 优先取 `rule.text`——它才是判分依据。
    //
    // ⚠️ **必须在原始串上扫描**：先反转义会把 `\"` 变成 `"`，
    // 于是取值时把这个「真引号」当成结束符，文本被腰斩
    // （实测 "He said \"hi\" loudly." 只提取到 "He said"）。
    // 正确顺序是「按原样定位 → 再反转义取到的值」。
    for raw_value in regex_find_rule_text(plain) {
        let cleaned = strip_tags(&unescape(&raw_value));
        if cleaned.chars().any(|c| c.is_ascii_alphabetic()) && !picked.contains(&cleaned) {
            picked.push(cleaned);
        }
    }

    // ② 没有 `rule.text` 就退回所有 `<p>` 段落。
    // 标签本身是 `\u003c` 转义过的，所以这条要先反转义。
    if picked.is_empty() {
        for capture in regex_find_p(&unescape(plain)) {
            let cleaned = strip_tags(&capture);
            if cleaned.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 12
                && !picked.contains(&cleaned)
            {
                picked.push(cleaned);
            }
        }
    }

    picked
}

/// 把明文里的 `\uXXXX` 与常见转义还原成可读文本。
fn unescape(text: &str) -> String {
    text.replace("\\u003c", "<")
        .replace("\\u003e", ">")
        .replace("\\u0026", "&")
        .replace("\\\"", "\"")
        .replace("\\n", " ")
}

/// 找出所有 `"rule":{…"text":"…"}` 里的 text 值，**按原样返回**（不反转义）。
///
/// 手写扫描而不引正则库——只需要这一处，且格式已知。
///
/// ⚠️ 返回值保持转义形态，由调用方 [`unescape`]。若在这里就反转义，
/// `\"` 会变成真引号，后续扫描无从分辨它是不是结束符。
fn regex_find_rule_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let marker = "\"text\":\"";
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find("\"rule\":") {
        let start = cursor + found;
        let Some(rel) = text[start..].find(marker) else {
            break;
        };
        let value_start = start + rel + marker.len();
        let Some(end) = find_unescaped_quote(text, value_start) else {
            break;
        };
        out.push(text[value_start..end].to_owned());
        cursor = end;
    }
    out
}

/// 找出所有 `<p>…</p>` 里的内容。
fn regex_find_p(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find("<p>") {
        let start = cursor + found + 3;
        let Some(rel) = text[start..].find("</p>") else {
            break;
        };
        out.push(text[start..start + rel].to_owned());
        cursor = start + rel;
    }
    out
}

/// 找到下一个**未转义**的双引号下标。
fn find_unescaped_quote(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            // 数一下前面连续的反斜杠：偶数个说明这个引号没被转义。
            let mut backslashes = 0usize;
            let mut probe = index;
            while probe > 0 && bytes[probe - 1] == b'\\' {
                backslashes += 1;
                probe -= 1;
            }
            if backslashes.is_multiple_of(2) {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// 去掉 HTML 标签与残留转义，压平空白。
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for ch in text.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("\\u0026nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 解密标准答案。
fn answer_cmd(instance_id: &str, group_id: &str) -> Result<()> {
    let session = require_session()?;
    let (token, _) = ucontent_token(&session)?;
    let client = uai_core::transport::build_client()?;
    let answers = answer::fetch_answers(&client, &token, instance_id, group_id)?;

    if answers.is_empty() {
        println!("{group_id}：没有标准答案（口语/主观题属于此类）");
        return Ok(());
    }

    println!("{group_id}：{} 道题\n", answers.len());
    for (index, children) in answers.iter().enumerate() {
        if children.is_empty() {
            println!("  [{index}] （无标准答案——口语/主观题）");
            continue;
        }
        for (child, values) in children.iter().enumerate() {
            println!("  [{index}][{child}] {}", values.join(" | "));
        }
    }
    Ok(())
}

/// 分组状态（友好格式）。
fn state_cmd(instance_id: &str, group_id: &str) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid(
            "进度接口需要 open_id（会话里没有 ssoId）。请重新 `uai login`。",
        ));
    }
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;
    let state = group_state::fetch(&client, &token, instance_id, group_id)?;

    println!("{group_id}");
    println!("  结论　　：{}", state.verdict());
    println!(
        "  任务级必修：{}",
        if state.flow_required { "是" } else { "否" }
    );
    println!(
        "  首次通过：{}",
        state
            .first_pass_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "从未做过".to_owned())
    );
    println!(
        "  最近提交：{}",
        state
            .last_submit_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "无".to_owned())
    );
    println!("  痕迹 state：{:?}", state.state);
    println!(
        "  得分　　　：{}",
        if state.score.is_empty() {
            "（空）"
        } else {
            &state.score
        }
    );
    println!(
        "  得分率/门槛：{} / {}",
        if state.score_pct.is_empty() {
            "（空）"
        } else {
            &state.score_pct
        },
        if state.min_score_pct.is_empty() {
            "（空）"
        } else {
            &state.min_score_pct
        }
    );
    println!("\n注：state=1 是账号级痕迹，不代表班级计分点已拿到。");
    Ok(())
}

/// 分组状态原始 JSON。
fn state_raw_cmd(instance_id: &str, group_id: &str) -> Result<()> {
    let session = require_session()?;
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;
    let raw =
        uai_core::transport::Transport::new(&client, uai_core::transport::Auth::Annotator(&token))
            .get_json(&uai_core::endpoints::group_state_url(instance_id, group_id))?;
    print_json(&raw);
    Ok(())
}

/// 作答快照：自动取 `lastSubmit`。
fn snapshot_cmd(instance_id: &str, group_id: &str) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid("快照接口需要 open_id，请重新 `uai login`。"));
    }
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;

    let state = group_state::fetch(&client, &token, instance_id, group_id)?;
    let Some(last_submit) = state.last_submit_at else {
        println!("{group_id} 从未提交过，没有快照可读。");
        return Ok(());
    };

    let snapshot = group_state::fetch_snapshot(
        &client,
        &token,
        instance_id,
        group_id,
        &last_submit.to_string(),
    )?;

    match group_state::submit_info(&snapshot) {
        Some(info) => {
            println!("{group_id}  lastSubmit={last_submit}\n");
            print_json(&strip_noise(info));
        }
        None => {
            println!("{group_id}  lastSubmit={last_submit}");
            println!("快照里没有 __SUBMIT_INFO__，打印整个响应：\n");
            print_json(&snapshot);
        }
    }
    Ok(())
}

/// 去掉快照里体积大又无关的字段，便于阅读。
fn strip_noise(info: &Value) -> Value {
    let mut clone = info.clone();
    if let Some(object) = clone.as_object_mut() {
        object.remove("content");
        object.remove("__CONTENT__");
    }
    json!(clone)
}

/// 组装提交载荷。**默认只预览**，`--confirm` 才真的发出去。
///
/// ## 为什么默认不发
///
/// 提交是**唯一会写服务端**的操作，而且：
///
/// 1. 会消耗限流配额（实测约每 7 组被限一次）；
/// 2. 服务端策略多为「一组只留首次记录」，发出去就改不了了。
///
/// 所以默认把载荷打出来给你看，确认没问题再加 `--confirm`。
fn submit_cmd(instance_id: &str, group_id: &str, confirm: bool) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid(
            "提交需要 open_id（会话里没有 ssoId）。请重新 `uai login`。",
        ));
    }
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;

    println!("正在组装 {group_id} 的提交载荷…");
    let prepared =
        uai_core::api::submit::prepare(&client, &token, instance_id, group_id, &session.open_id)?;
    if prepared.study_only {
        println!(
            "{group_id} 整组都是学习题：isCompleted 与 thirdPartyJudges 都是空的，\n\
             但**照样要交**——进度（媒体已播标记、isDone）就靠这次写请求记下去，submitType=2。"
        );
    }
    let submission = &prepared.submission;
    let body = &prepared.body;

    // 本地自检：两个数组不等长会被服务端拒（code 300100），提前拦下。
    // `prepare` 内部已经验过一次，这里再验一次是因为载荷最终要发出去，
    // 而 `validate` 很便宜——不省钱的地方省心。
    uai_core::api::submit::validate(body)?;

    let completed = body["isCompleted"].as_array().map(Vec::len).unwrap_or(0);
    println!(
        "\n题目 {} 道，isCompleted / thirdPartyJudges 各 {completed} 项",
        submission.questions.len()
    );
    println!("题型：{}", submission.kind_names().join(", "));
    println!("submitType = {}", submission.submit_type());
    println!("本地自检通过（两数组等长）\n");

    for (index, question) in submission.questions.iter().enumerate() {
        println!(
            "  [{index}] {:<14} 贡献 {} 项",
            question.meta.question_type,
            question.meta.is_completed_len()
        );
    }

    println!("\n载荷预览：");
    print_json(body);

    if !confirm {
        println!("\n以上**未发送**。确认无误后加 --confirm 真正提交。");
        println!("注意：提交会写服务端、消耗限流配额，且多数分组只留首次记录。");
        return Ok(());
    }

    println!("\n正在提交…");
    let outcome = uai_core::api::submit::submit(&client, &token, submission, &session.open_id)?;
    println!("{}", outcome.verdict());

    // 回读一次，让「记录是否真的留存」可见。
    if let Ok(state) = uai_core::api::group_state::fetch(&client, &token, instance_id, group_id) {
        println!("\n回读状态：{}", state.verdict());
        println!(
            "  得分：{}  得分率/门槛：{} / {}",
            if state.score.is_empty() {
                "（空）"
            } else {
                &state.score
            },
            if state.score_pct.is_empty() {
                "（空）"
            } else {
                &state.score_pct
            },
            if state.min_score_pct.is_empty() {
                "（空）"
            } else {
                &state.min_score_pct
            }
        );
    }
    Ok(())
}

/// 安装 Ctrl-C 停止开关。
///
/// 阻塞式 reqwest 没法中途 abort，所以「停止」只能是**组间检查**：
/// 收到 Ctrl-C 后把旗标置真，当前这一组跑完就不再开下一组。
/// 已经在飞的那一组**无法撤回**——提交是写操作，没有回滚。
fn install_stop_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let handle = Arc::clone(&flag);
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        runtime.block_on(async {
            if tokio::signal::ctrl_c().await.is_ok() {
                handle.store(true, Ordering::SeqCst);
                eprintln!("\n收到 Ctrl-C：跑完当前这一组就停（已提交的不会回滚）。");
            }
        });
    });
    flag
}

/// 从队列里取下一组（带它已经重排过几次）。
fn next_job(queue: &Mutex<VecDeque<(usize, u32)>>) -> Option<(usize, u32)> {
    queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
}

/// 这个错误是不是「打太快了」而不是「这一组有问题」。
///
/// ## 实测（2026-09，`--jobs 32`）
///
/// 并发一上来，服务器**在边缘就挡**：返回 `HTTP 503` + HTML，连读内容都挡
/// （44 组 0 接受、64 次业务限流）。这跟 `600002` 是一回事——速率问题，
/// 不是这一组坏了，所以值得重排而不是记成失败。
///
/// ⚠️ 旧文案把 503+HTML 说成「多半是鉴权失败被重定向到登录页」，
/// 在这个场景下是**误导**：会话是好的，是请求太密。
fn is_transient(error: &Error) -> bool {
    let message = error.message();
    message.contains("503")
        || message.contains("超时")
        || message.contains("timed out")
        || message.contains("连接")
}

/// **打通提交流水线**：必修筛选 → 逐组组装 → 提交 → 回读。
///
/// ## 为什么它不是 `submit` 的 `--confirm`
///
/// `submit` 是**单组**的、默认预览的调试命令。真要用起来需要的是
/// 「把**当前班级在这本教材上的必修分组**全部交掉」——那件事需要按班级
/// 圈定必修、逐个组装、逐个回读、限流退避、中途可停。这就是本命令。
///
/// ## 交什么：当前班级的必修，不是「有可判分题的组」
///
/// 口径是 `progress` 接口里的 `rt.leafs.<gid>.strategies.required`
/// ——它一次请求就给到分组级，**且正是门户「教程学习成绩」的分母**
/// （实测本班 49）。旧实现用单元级粗筛（148）＋逐组 `flowStrategy`
/// （45）：既多扫几百次请求，又漏掉 4 个 `purecontent`（门户确实计分）。
///
/// 班级必须由 `--class` 显式给出：同一本教材挂在多个班时分母不同
/// （49 vs 35），猜错就是按错的口径交。
///
/// ## 默认真的发
///
/// 与 `submit` 相反：`auto` **默认提交**，`--dry-run` 才是预览。
/// 理由是这个命令存在的全部意义就是提交；再要一次 `--confirm`
/// 只是让人多敲一次。但 `--dry-run` 会把**与真发逐字节一致**的载荷
/// 打出来（同一个 `prepare`），方便先核对。
/// `auto` 的运行参数。
///
/// 铺成 9 个参数时调用点没人看得懂哪个 true 是哪个（clippy 也会拦）。
struct AutoRun {
    dry_run: bool,
    limit: Option<usize>,
    redo: bool,
    no_scan: bool,
    jobs: usize,
    no_readback: bool,
    sleep_ms: u64,
    /// 只交这些单元（空 = 全部）。
    units: Vec<String>,
}

fn auto_cmd(instance_id: &str, class_selector: &str, run: AutoRun) -> Result<()> {
    let AutoRun {
        dry_run,
        limit,
        redo,
        no_scan,
        jobs,
        no_readback,
        sleep_ms,
        units,
    } = run;

    // 回读是 4 次往返里的最后 1 次（一组：内容 → 答案 → 提交 → 回读）。
    // 每次往返实测 70~100ms，砍掉它省 1/4 的时间；判分改取提交响应里的
    // `state.score_pct`（同样是服务端写的，且比回读更新鲜）。
    let do_readback = !no_readback;
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid(
            "提交需要 open_id（会话里没有 ssoId）。请重新 `uai login`。",
        ));
    }
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;

    // ⓪ 先定班：交的是**当前班级**的必修。
    let class = resolve_class(&session, instance_id, class_selector)?;
    println!("实例：{instance_id}");
    println!(
        "班级：{}（classId={} courseId={}）",
        class.class_name, class.class_id, class.course_id
    );
    println!(
        "门户读数：{} 计分点（完成率 {:.1}%）",
        class.ratio_text(),
        class.percent()
    );
    if dry_run {
        println!("模式：**DRY-RUN**（只组装并自检，不发任何写请求）\n");
    } else {
        println!("模式：**真实提交**（会写服务端；多数分组只留首次记录，不可回滚）\n");
    }

    // ① 目录。
    let catalog = catalog::fetch(&client, &token, instance_id)?;
    println!("目录：{} 个分组", catalog.group_count());

    // ② 必修筛选（leafs 口径）。Ctrl-C 可中断逐组扫描。
    let stop = install_stop_flag();
    let should_stop = {
        let flag = Arc::clone(&stop);
        move || flag.load(Ordering::SeqCst)
    };

    // ★ 必修口径**按班**：门户 `courseStudyStrategy/detail`（一次请求，
    //   带逐单元必需集与 `passScore`）。同一实例在不同班里的必修集可以完全不相交。
    let scope = build_class_scope(&client, &session, instance_id, &class);
    println!("正在按当前班级筛必修（门户按班策略）…");
    let plan = targets::plan(
        targets::PlanRequest {
            client: &client,
            token: &token,
            course_id: instance_id,
            open_id: &session.open_id,
            catalog: &catalog,
            scope: Some(&scope),
        },
        PlanOptions {
            // `--no-scan` 与 `--redo` 等价：不扫状态 ⇒ 已达标的也不排除。
            skip_passed: !redo && !no_scan,
        },
        &should_stop,
    )?;
    let mut plan = plan;
    // `--units u6,u7,u8`：只交这些单元。服务端有提交节奏配额（实测连打 11 组
    // 就被 `600002` 摁住），分批交比整本硬打更实际。
    if !units.is_empty() {
        let keep = |id: &String| units.iter().any(|unit| id.starts_with(&format!("{unit}g")));
        let before = plan.groups.len();
        plan.groups.retain(keep);
        plan.skipped_passed = plan.required_total.saturating_sub(plan.groups.len());
        println!(
            "只交指定单元（{}）：{before} 组 → {} 组",
            units.join(","),
            plan.groups.len()
        );
    }
    println!("{}\n", plan.describe());

    if plan.blocked_by_missing_required() {
        println!(
            "没读到 leafs.required，一组都不提交（拿不到必修口径时不动，避免误交非必修内容）。"
        );
        return Ok(());
    }
    if !plan.state_scanned {
        println!(
            "⚠️ 没有逐组扫状态：**已达标的也会重交**（服务端多为「一组只留首次记录」，\
             重交通常改写不了，只是白吃限流配额）。\n"
        );
    }

    // 班级分母与必修组数的对照：口径不一致时**当场可见**，而不是等门户对不上账。
    if plan.required_total > 0 && class.total > 0 && plan.required_total != class.total as usize {
        println!(
            "⚠️ 口径不一致：leafs.required {} 组，门户本班分母 {}。\n\
             \x20   本工具按 leafs.required 交；对不上账时先 `uai grade <班级教材ID>` 看门户口径。\n",
            plan.required_total, class.total
        );
    }

    let mut groups = plan.groups.clone();
    if let Some(limit) = limit {
        groups.truncate(limit);
        println!("--limit 生效：只处理前 {limit} 组");
    }
    if groups.is_empty() {
        println!("没有需要提交的分组。");
        return Ok(());
    }

    if dry_run {
        return auto_dry_run(
            instance_id,
            &session,
            &client,
            &token,
            &groups,
            &should_stop,
        );
    }

    // ③ 逐组提交：`jobs` 个 worker 抢一个共享队列。
    //
    //    组与组之间没有依赖（每组只是「2 次读 + 1 次写 + 1 次回读」），
    //    所以并行能直接缩短墙钟。
    //
    //    ⚠️ **撞限流不能原地睡**：`submit_one` 的退避是 20/40/60/80 秒，
    //    16 路并发时全部 worker 会被睡死——实测 50 秒只推了 6 组，比串行还慢。
    //    这里改成「重排到队尾 + 全局冷卻」，让别的组继续跑。
    println!(
        "开始提交 {} 组（并行 {}，Ctrl-C 可停）…\n",
        groups.len(),
        jobs.min(groups.len()).max(1)
    );
    let queue = Mutex::new(
        (0..groups.len())
            .map(|index| (index, 0u32))
            .collect::<VecDeque<(usize, u32)>>(),
    );
    let done = AtomicUsize::new(0);
    let out = Mutex::new(Vec::<(usize, submit::GroupOutcome)>::new());
    let failed = Mutex::new(Vec::<(usize, String)>::new());
    let print_lock = Mutex::new(());
    let total = groups.len();
    let workers = jobs.min(total).max(1);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // ⚠️ 重试次数必须**跟着这一组走**。曾经它在每次出队后被清零，
                //    于是「重排 → 出队 → 清零 → 再重排」成了热循环：实测
                //    `--jobs 2` 4 组跑了 131 秒、559 次重排，全是空转。
                while let Some((index, tries)) = next_job(&queue) {
                    if should_stop() {
                        queue
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push_back((index, tries));
                        break;
                    }
                    let group_id = &groups[index];
                    let result = submit::submit_one_once(
                        &client,
                        &token,
                        instance_id,
                        group_id,
                        &session.open_id,
                        do_readback,
                    );
                    match result {
                        Ok(outcome) if outcome.rate_limited && tries < MAX_REQUEUE => {
                            {
                                let _guard = print_lock.lock().unwrap_or_else(|e| e.into_inner());
                                println!(
                                    "      ⏳ {group_id} 被限流，重排队尾（第 {} 次）",
                                    tries + 1
                                );
                            }
                            // 冷卻：只让这一个 worker 让一步，不拖累其他组。
                            // 当前取值为 0 = 不等，直接重排队尾再打。
                            if !RATE_LIMIT_COOLDOWN.is_zero() {
                                std::thread::sleep(RATE_LIMIT_COOLDOWN);
                            }
                            queue
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push_back((index, tries + 1));
                        }
                        Ok(outcome) => {
                            let shown = done.fetch_add(1, Ordering::SeqCst) + 1;
                            {
                                let _guard = print_lock.lock().unwrap_or_else(|e| e.into_inner());
                                println!("[{shown}/{total}] {group_id} … {}", outcome.verdict());
                            }
                            out.lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((index, outcome));
                        }
                        Err(error) => {
                            // 503/超时这类**边缘限流**也算一次重试机会：它多半是
                            // 「打太快了」，而不是这一组本身有问题。
                            if is_transient(&error) && tries < MAX_REQUEUE {
                                {
                                    let _guard =
                                        print_lock.lock().unwrap_or_else(|e| e.into_inner());
                                    println!(
                                        "      ⏳ {group_id} 被挡（{}），重排队尾（第 {} 次）",
                                        error.message(),
                                        tries + 1
                                    );
                                }
                                if !RATE_LIMIT_COOLDOWN.is_zero() {
                                    std::thread::sleep(RATE_LIMIT_COOLDOWN);
                                }
                                queue
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push_back((index, tries + 1));
                            } else {
                                let shown = done.fetch_add(1, Ordering::SeqCst) + 1;
                                {
                                    let _guard =
                                        print_lock.lock().unwrap_or_else(|e| e.into_inner());
                                    println!(
                                        "[{shown}/{total}] {group_id} … 组装失败：{}",
                                        error.message()
                                    );
                                }
                                failed
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push((index, error.message().to_owned()));
                            }
                        }
                    }

                    if sleep_ms > 0 {
                        std::thread::sleep(Duration::from_millis(sleep_ms));
                    }
                }
            });
        }
    });

    // worker 是抢队列的，顺序不确定；按原始下标排回来，汇总才可复现。
    let mut collected = out.into_inner().unwrap_or_else(|e| e.into_inner());
    collected.sort_by_key(|(index, _)| *index);
    let mut failures = failed.into_inner().unwrap_or_else(|e| e.into_inner());
    failures.sort_by_key(|(index, _)| *index);

    let pending = queue.into_inner().unwrap_or_else(|e| e.into_inner());
    let mut report = submit::BatchReport {
        stopped_early: !pending.is_empty(),
        ..Default::default()
    };
    for (index, outcome) in collected {
        let group_id = &groups[index];
        if outcome.study_only {
            report.record_study_only(group_id.clone());
        }
        report.record(outcome);
    }
    for (index, reason) in failures {
        report.record_build_failure(groups[index].clone(), reason);
    }
    for (index, _tries) in pending {
        report.record_build_failure(
            groups[index].clone(),
            format!("被限流/被挡 {MAX_REQUEUE} 次仍未成功，下次重跑"),
        );
    }

    // ④ 汇总。逐组留痕，不然「哪几组没上去」无从排查。
    println!("\n──── 提交汇总 ────");
    println!("{}", report.describe());
    for (group_id, reason) in &report.build_failures {
        println!("  组装失败 {group_id}：{reason}");
    }
    let bad: Vec<&str> = report
        .outcomes
        .iter()
        .filter(|item| !item.accepted)
        .map(|item| item.group_id.as_str())
        .collect();
    if !bad.is_empty() {
        println!("  未接受：{}", bad.join(", "));
    }
    let not_passed: Vec<&str> = report
        .outcomes
        .iter()
        .filter(|item| item.accepted && !item.passed)
        .map(|item| item.group_id.as_str())
        .collect();
    if !not_passed.is_empty() {
        println!(
            "  已接受但回读未达标：{}\n\
             \x20   （客观题未达标多半是答案没匹配上——参见上面「整句填空」那个已知反例；\n\
             \x20    口语/角色扮演已经发满分包，仍未达标说明服务端另有口径；\n\
             \x20    主观题与上传题拿不到分是结构性的：前者无判分器，后者无文件）",
            not_passed.join(", ")
        );
    }

    println!(
        "\n注：服务端多为「一组只留首次记录」。重复跑同一组通常改写不了已有记录，\n\
         所以重跑前先看上面的「已接受」——那些不用再交。"
    );
    Ok(())
}

/// `auto --dry-run`：只组装并本地自检，**一个写请求都不发**。
///
/// 走的是与真提交同一个 [`submit::prepare`]，所以这里看到的载荷
/// 就是去掉 `--dry-run` 后会原样发出去的那份。
fn auto_dry_run(
    instance_id: &str,
    session: &uai_core::Session,
    client: &reqwest::blocking::Client,
    token: &str,
    groups: &[String],
    should_stop: &impl Fn() -> bool,
) -> Result<()> {
    println!("DRY-RUN：组装 {} 组（不发写请求）…\n", groups.len());
    let mut prepared = 0usize;
    let mut study_only = 0usize;
    let mut failed = 0usize;

    for (index, group_id) in groups.iter().enumerate() {
        if should_stop() {
            println!("\n被中止。");
            break;
        }
        match submit::prepare(client, token, instance_id, group_id, &session.open_id) {
            Ok(item) => {
                prepared += 1;
                if item.study_only {
                    study_only += 1;
                    println!(
                        "  [{}/{}] {:<10} ✔ 可提交（纯学习题，submitType=2 只记进度）：{} 题，题型 {}",
                        index + 1,
                        groups.len(),
                        group_id,
                        item.question_count(),
                        item.submission.kind_names().join(",")
                    );
                } else {
                    println!(
                        "  [{}/{}] {:<10} ✔ 可提交：{} 题 / {} 项，题型 {}",
                        index + 1,
                        groups.len(),
                        group_id,
                        item.question_count(),
                        item.completed_count(),
                        item.submission.kind_names().join(",")
                    );
                }
            }
            Err(error) => {
                failed += 1;
                println!(
                    "  [{}/{}] {:<10} ✗ 组装失败：{}",
                    index + 1,
                    groups.len(),
                    group_id,
                    error.message()
                );
            }
        }
    }

    println!(
        "\nDRY-RUN 汇总：可提交 {prepared} 组（其中纯学习题 {study_only} 组），组装失败 {failed} 组"
    );
    println!("以上**未发送任何写请求**。去掉 --dry-run 即真正提交。");
    Ok(())
}

/// 媒体：报告上传链状态，并对「参数校验」做一次离线自检。
///
/// **不发任何请求**——上传链未接入，没有可发的合法载荷。
/// 这个命令存在的意义是把「未接入」这件事变成**可执行、可核对**的，
/// 而不是只写在文档里。
fn media_cmd() -> Result<()> {
    println!(
        "媒体上传链状态：{}\n",
        if uai_core::api::media::UPLOAD_WIRED {
            "已接入"
        } else {
            "**未接入**"
        }
    );

    println!("完整链路（录音题拿真实满分的唯一正当路径）：");
    // 每一步单独标状态：库里有实现 ≠ 实测通过，这两件事必须分开写，
    // 否则「有代码」会被读成「能用」。
    for (index, (state, step)) in [
        (
            "未实测",
            "GET  /media/user_resource/cms/token      取七牛上传 token",
        ),
        (
            "未实现",
            "上传音频到七牛                            得到真实 URL",
        ),
        (
            "未实测",
            "POST /media/user_resource/fetch          登记资源",
        ),
        ("未实现", "把 URL 填进 record.url / recordDetail.audioUrl"),
        (
            "未实测",
            "POST /course/api/v3/voiceEvaluation      由服务端评定",
        ),
    ]
    .iter()
    .enumerate()
    {
        println!("  {}. [{}] {}", index + 1, state, step);
    }
    println!(
        "\n  未实测 = 库里有函数，但**没有用真实账号跑通过**；\n\
         \x20 未实现 = 连函数都还没写。"
    );

    println!(
        "\n关于「直接发满分包」：\n\
         \x20 全 bundle 只有一处给 recordDetail.score 赋值，值取自**本地 state**\n\
         \x20 （`e.score = e.recordDetail?.score || 0`）——填多少服务端就记多少。\n\
         \x20 所以口语/录音题与角色扮演**默认就发满分包**（full_marks_record），\n\
         \x20 这是本库当前的行为；上面那条上传链是「想要真实 audioUrl」时才需要的另一条路。"
    );

    println!("\n参数自检（离线，不发请求）：");
    let client = uai_core::transport::build_client()?;
    let empty = uai_core::api::media::VoiceEvaluation::default();
    match uai_core::api::media::report_voice_evaluation(&client, "unused", &empty) {
        Ok(_) => println!("  ✗ 空 block_id 竟然通过了校验（不应该）"),
        Err(error) => println!("  ✔ 空 block_id 被本地拦下：{}", error.message()),
    }
    Ok(())
}

/// 账号级必修集（`leafs` 端点）——**降级路径**，不按班。
fn fetch_account_level_required(
    client: &reqwest::blocking::Client,
    token: &str,
    instance_id: &str,
    session: &uai_core::Session,
    catalog: &catalog::Catalog,
) -> Result<Vec<String>> {
    let mut all: Vec<String> = catalog
        .units
        .iter()
        .flat_map(|unit| unit.groups.iter())
        .map(|group| group.id.clone())
        .collect();
    all.sort();
    all.dedup();
    let leafs = progress::fetch_leafs(client, token, instance_id, &session.open_id, &all)?;
    Ok(leafs
        .values()
        .filter(|leaf| leaf.required)
        .map(|leaf| leaf.id.clone())
        .collect())
}

/// 组装**按班**取必修的上下文。
///
/// 五项键来自两处，缺一不可：
///
/// | 字段 | 来源 |
/// | --- | --- |
/// | `courseResourceId` / `resourceId` / `strategyId` | 门户课程列表的**教材节点** |
/// | `courseInstanceId` | 同上（`instanceId`） |
/// | `courseId`（如 `34852`） | `/api/aca/plan/detail` 的 `value.courseId` |
///
/// 拿不到 `courseId` 时 `is_usable()` 为假，调用方会自动退回账号级口径。
fn build_class_scope(
    client: &reqwest::blocking::Client,
    session: &uai_core::Session,
    instance_id: &str,
    class: &course_list::CourseProgress,
) -> uai_core::api::class_required::ClassScope {
    let host = session.host_identity();
    // `courseId` 只能从考核方案里取；失败不致命（退化成账号级口径）。
    let course_id = assessment::fetch_plan(
        client,
        &session.portal_token,
        &host,
        class.course_id,
        &class.class_id,
    )
    .map(|plan| plan.course_id)
    .unwrap_or_default();
    uai_core::api::class_required::ClassScope {
        portal_token: session.portal_token.clone(),
        host,
        course_resource_id: class.course_resource_id,
        resource_id: class.resource_id.clone(),
        instance_id: instance_id.to_owned(),
        strategy_id: class.strategy_id,
        course_id,
    }
}

/// **真实必修任务时长投放**（`uai study`）。
///
/// ## 它和 `duration` 是两条不同的账
///
/// | | `duration`（房间池） | `study`（本命令） |
/// | --- | --- | --- |
/// | 端点 / 事件 | `/unipusiopoint/` + `kick-v2` | `/unipusio/` + `start` |
/// | 归属键 | 假名 `(应用, 设备)` | **`(真实分组, 真实课程)`** |
/// | 倍率 | 13.7~14.9× | **1×（账号级封顶）** |
/// | 进 item 30 | ❌ 实测 1.144h 后 9 分钟无变化 | ✅ 当场落账 |
///
/// ## URL 怎么造（实测规则，够全自动）
///
/// ```text
/// {前缀}#/{实例ID}/courseware/{单元}/{micro}/{分组}
/// ```
///
/// | 段 | 要求 | 依据 |
/// | --- | --- | --- |
/// | `<实例ID>` | 真实 | — |
/// | `<分组>` | **真实且属于该课程** | 假 unit 实测 0 记账 |
/// | `<单元>` | 必须与分组匹配 | 同上 |
/// | `<micro>` | **随便填**（假名 / 单元名 / 空都行） | 三跑实测均落进目标分组 |
/// | 查询串 | 不能整个省，但 `courseResourceId` 可省 | 空查询串实测 0 记账 |
///
/// 所以分组清单与单元来自 ucontent 目录、必修性来自 `leafs`，
/// **不需要门户任务树**。
///
/// ## 倍率没有捷径
///
/// 同课 4 连接 0.63×、跨 3 门课并行 0.69× —— 预算按账号 ~1× 封顶。
/// 所以这里**只开一条连接**，靠轮换必修任务覆盖整本书。
fn study_cmd(
    instance_id: &str,
    class_selector: &str,
    hours: f64,
    dwell_min: f64,
    page_base: Option<&str>,
    units: &[String],
) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_assessment() {
        return Err(Error::invalid(
            "任务模式需要门户会话（school + openId）。请重新 `uai login`。",
        ));
    }
    let client = uai_core::transport::build_client()?;
    let class = resolve_class(&session, instance_id, class_selector)?;

    // ① 任务页前缀：真实 URL 是服务端认任务的唯一依据。
    let page_base = match page_base {
        Some(text) if !text.trim().is_empty() => text.to_owned(),
        _ => {
            // 「班级教材ID」不用抄地址栏：门户课程列表里**教材节点自己的 `id`**
            // 就是它（实测《基础篇综合教程2》= 20000000002）。
            let resource_id = course_list::fetch(&client, &session.portal_token)
                .map(|items| {
                    items
                        .iter()
                        .find(|item| item.instance_id == instance_id)
                        .map(|item| item.course_resource_id)
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            if resource_id <= 0 {
                println!(
                    "⚠️ 没从门户课程列表拿到「班级教材ID」(courseResourceId)，前缀里省掉它。\n\
                     \x20  若跑几分钟仍 0 记账，用 `--page-base` 直接粘浏览器地址栏里 `pc.html?…` 那一段。"
                );
                format!(
                    "https://ucontent.unipus.cn/_explorationpc_default/pc.html?cid={}&cloudCurriculaId={}&source=cloud",
                    class.class_id, class.course_id
                )
            } else {
                format!(
                    "https://ucontent.unipus.cn/_explorationpc_default/pc.html?cid={}&cloudCurriculaId={}&source=cloud&courseResourceId={resource_id}",
                    class.class_id, class.course_id
                )
            }
        }
    };

    // ② 必修分组 → 真实 `(单元, micro, 分组)`。
    //
    // ★ **不需要门户任务树**（实测 `micro` 随便填照样记账）：
    //   分组与归属单元来自 ucontent 目录（`catalog`），
    //   「哪些必修」来自 `leafs`（tasks 端点，一次请求），
    //   `micro` 直接填单元名——实测 `u6/u6/u6g235` 一样落进 `u6g235`。
    let token = session.annotator_token()?;
    let catalog = catalog::fetch(&client, &token, instance_id)?;
    let mut all_ids: Vec<String> = catalog
        .units
        .iter()
        .flat_map(|unit| unit.groups.iter())
        .map(|group| group.id.clone())
        .collect();
    all_ids.sort();
    all_ids.dedup();
    let leafs = progress::fetch_leafs(&client, &token, instance_id, &session.open_id, &all_ids)?;

    let mut targets: Vec<uai_core::api::duration::TaskTarget> = Vec::new();
    for unit in &catalog.units {
        if !units.is_empty() && !units.contains(&unit.id) {
            continue;
        }
        for group in &unit.groups {
            let required = leafs
                .get(&group.id)
                .map(|leaf| leaf.required)
                .unwrap_or(false);
            if required {
                targets.push(uai_core::api::duration::TaskTarget {
                    instance: instance_id.to_owned(),
                    unit: unit.id.clone(),
                    // `micro` 只是 URL 里的过场段：实测填单元名即可。
                    micro: unit.id.clone(),
                    group: group.id.clone(),
                });
            }
        }
    }
    if targets.is_empty() {
        return Err(Error::invalid(
            "没找到必修分组（先用 `uai targets <实例ID> --class <班级>` 看看）",
        ));
    }

    println!("实例：{instance_id}");
    println!(
        "班级：{}（classId={} courseId={}）",
        class.class_name, class.class_id, class.course_id
    );
    println!(
        "必修任务：{} 个（单元 {}）",
        targets.len(),
        if units.is_empty() {
            catalog
                .units
                .iter()
                .map(|unit| unit.id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        } else {
            units.join(",")
        }
    );
    println!("任务页前缀：{page_base}");
    println!(
        "参数：目标 {}，每任务停留 {:.1} 分钟轮换，单连接（实测 1× 封顶，多开只会摊薄）\n",
        if hours > 0.0 {
            format!("{hours:.2}h")
        } else {
            "不限（Ctrl-C 停）".to_owned()
        },
        dwell_min
    );

    let (tx, rx) = std::sync::mpsc::channel();
    let options = uai_core::api::duration::TaskOptions {
        page_base,
        target_secs: (hours * 3600.0) as u64,
        dwell: Duration::from_secs_f64(dwell_min * 60.0),
        ..Default::default()
    };
    let _stopped = uai_core::api::duration::start_task_boost(
        session.open_id.clone(),
        token,
        targets,
        options,
        tx,
    )?;

    for event in rx {
        match event {
            uai_core::api::duration::BoostEvent::Started { .. } => {
                println!("已连接（真实任务模式）");
            }
            uai_core::api::duration::BoostEvent::Tick { credited_secs, .. } => {
                print!(
                    "\r记账 {:>6.2}h / {}   ",
                    credited_secs as f64 / 3600.0,
                    if hours > 0.0 {
                        format!("{hours:.2}h")
                    } else {
                        "不限".to_owned()
                    }
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            uai_core::api::duration::BoostEvent::Log(text) => {
                println!("\n{text}");
            }
            uai_core::api::duration::BoostEvent::Finished {
                credited_secs,
                reason,
            } => {
                println!(
                    "\n结束（{reason}）：共记账 {:.3}h",
                    credited_secs as f64 / 3600.0
                );
                break;
            }
        }
    }
    Ok(())
}
///
/// ⚠️ 这是**唯一会长时间占用终端**的命令（默认 1 小时）。
/// 每 5 秒打一次状态；`Ctrl-C` 会优雅停下（关闭所有连接后退出）。
fn duration_cmd(hours: f64, rooms: usize) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid(
            "时长加速需要 open_id（会话里没有 ssoId）。请重新 `uai login`。",
        ));
    }
    let token = session.annotator_token()?;
    let target_secs = (hours * 3600.0) as u64;

    uai_core::api::duration::validate_room_pool(rooms)?;
    println!(
        "准备启动时长加速：目标 {hours:.2}h，{} 个房间，预计倍率 ≈{:.1}×",
        rooms.min(uai_core::api::duration::MAX_ROOMS),
        uai_core::api::duration::estimated_ratio(rooms.min(uai_core::api::duration::MAX_ROOMS))
    );
    println!("注意：倍率有约 60 秒起步坡，前 60 秒记账为 0 属正常。");
    println!("按 Ctrl-C 停止。\n");

    let (tx, rx) = std::sync::mpsc::channel();
    // Ctrl-C 由 `start_boost` 内部接管（tokio::signal），所以 CLI 不需要
    // 这个停止开关。仍然接收它：库的调用方可以据此**程序化**停止，
    // CLI 只是不用而已。
    let _stopped = uai_core::api::duration::start_boost(
        session.open_id.clone(),
        token,
        target_secs,
        rooms,
        tx,
    )?;

    for event in rx {
        match event {
            uai_core::api::duration::BoostEvent::Started { rooms } => {
                println!("已连接 {rooms} 个房间");
            }
            uai_core::api::duration::BoostEvent::Tick {
                credited_secs,
                live_rooms,
                reconnects,
            } => {
                print!(
                    "\r记账 {:>6.2}h  在线 {:>2}  重连 {:>2}   ",
                    credited_secs as f64 / 3600.0,
                    live_rooms,
                    reconnects
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            uai_core::api::duration::BoostEvent::Log(text) => {
                println!("\n{text}");
            }
            uai_core::api::duration::BoostEvent::Finished {
                credited_secs,
                reason,
            } => {
                println!(
                    "\n结束（{reason}）：共记账 {:.2}h",
                    credited_secs as f64 / 3600.0
                );
                break;
            }
        }
    }
    Ok(())
}

/// 必修筛选：口径是 `leafs.required`（= 门户「教程学习成绩」的分母）。
///
/// ⚠️ 旧版本这里是「单元级粗筛 + 逐组 `flowStrategy.required` 精筛」，
/// 两个毛病：多扫几百次请求；漏掉 4 个 `purecontent`（门户确实计分）。
/// 现在必修一次请求就拿全，逐组扫只用来说明「哪些已达标」。
fn targets_cmd(
    instance_id: &str,
    class_selector: &str,
    skip_passed: bool,
    skip_scan: bool,
) -> Result<()> {
    let session = require_session()?;
    if !session.can_touch_protected() {
        return Err(Error::invalid(
            "必修筛选需要 open_id（会话里没有 ssoId）。请重新 `uai login`。",
        ));
    }
    let token = session.annotator_token()?;
    let client = uai_core::transport::build_client()?;

    let class = resolve_class(&session, instance_id, class_selector)?;
    println!("实例：{instance_id}");
    println!(
        "班级：{}（classId={} courseId={}）",
        class.class_name, class.class_id, class.course_id
    );
    println!(
        "门户读数：{} 计分点（完成率 {:.1}%）\n",
        class.ratio_text(),
        class.percent()
    );

    let catalog = catalog::fetch(&client, &token, instance_id)?;
    let snapshot = progress::fetch(&client, &token, instance_id, &session.open_id)?;

    let units = snapshot.required_units();
    println!("目录 {} 个分组", catalog.group_count());
    println!(
        "单元级必修：{}",
        if units.is_empty() {
            "（没读到）".to_owned()
        } else {
            units.join(", ")
        }
    );

    // 必修集：**按班**（门户 `chapters/unitTaskSituation`，键是 (班,书) 绑定的
    // `courseResourceId`）。拿不到才退回账号级的 `leafs`——那个不随班级变，
    // 同一实例在不同班里可能给出完全不相交的集合。
    let scope = build_class_scope(&client, &session, instance_id, &class);
    let (mut required, source) = if scope.is_usable() {
        match uai_core::api::class_required::fetch_strategy(&client, &scope) {
            Ok(strategy) => {
                let ids = strategy.required_tasks();
                (ids, format!("按班策略（{}）", strategy.class_name))
            }
            Err(error) => {
                println!(
                    "⚠️ 按班策略取数失败（{}），降级到账号级 leafs",
                    error.message()
                );
                (
                    fetch_account_level_required(&client, &token, instance_id, &session, &catalog)?,
                    "账号级 leafs（**不按班**）".to_owned(),
                )
            }
        }
    } else {
        println!(
            "⚠️ 拿不到完整的按班五件套（courseResourceId/resourceId/strategyId/实例/courseId），\
             降级到账号级 leafs（**不按班**，可能多列或漏列）"
        );
        (
            fetch_account_level_required(&client, &token, instance_id, &session, &catalog)?,
            "账号级 leafs（**不按班**）".to_owned(),
        )
    };
    required.sort();
    required.dedup();
    if required.is_empty() {
        println!(
            "\n**没读到 `leafs.required`** —— 一个分组都不交。\n\
             这是刻意的：拿不到必修口径时宁可不动，也不去提交非必修内容。"
        );
        return Ok(());
    }
    println!(
        "必修集（{source}）→ **{} 组**（门户「教程学习成绩」的分母就是这个口径）",
        required.len()
    );
    if class.total > 0 && required.len() != class.total as usize {
        println!(
            "⚠️ 门户口径不一致：本班分母 {}，leafs.required {}。先 `uai grade` 核对再交。",
            class.total,
            required.len()
        );
    }

    // 列出来。目录里查不到的分组单独标出来，别静默吞掉。
    for id in &required {
        match catalog
            .units
            .iter()
            .flat_map(|unit| unit.groups.iter())
            .find(|group| group.id == *id)
        {
            Some(group) => println!("    {:<8} {:<38} {}", group.id, group.base, group.name),
            None => println!("    {id:<8} （目录里没有这个分组）"),
        }
    }

    // 扫状态只为了「排除已达标」；不排除就没必要扫（省每组一次请求）。
    if !skip_passed || skip_scan {
        println!(
            "\n（未扫状态：上面是 **全部必修组**，已达标的也在里面。\n\
             \x20 加 `--skip-passed` 才逐组扫状态并排除已达标。）"
        );
        println!("预计提交阶段：{} 次请求", required.len() * 4);
        return Ok(());
    }

    println!(
        "\n逐组扫状态以排除已达标（需 {} 次请求，约 {:.0} 秒）…",
        required.len(),
        required.len() as f64 * 0.088
    );
    let (kept, requests) =
        targets::filter_passed(&client, &token, instance_id, &required, skip_passed, || {
            false
        })?;

    println!("\n待提交 → {} 组（发出 {requests} 次状态请求）", kept.len());
    println!(
        "  {}",
        if skip_passed {
            format!(
                "已排除达标 {} 组",
                required.len().saturating_sub(kept.len())
            )
        } else {
            "未排除达标分组（重做模式）".to_owned()
        }
    );
    for id in &kept {
        println!("    {id}");
    }

    println!("\n预计提交阶段：{} 次请求", kept.len() * 4);
    Ok(())
}

/// 列出全部题型。
fn types_cmd() -> Result<()> {
    let names = all_kind_names();
    println!("共 {} 种题型（注册表顺序即匹配优先级）\n", names.len());
    for (index, name) in names.iter().enumerate() {
        let kind =
            kind_by_name(name).ok_or_else(|| Error::invalid(format!("题型 {name} 查不到实现")))?;
        println!("{:>3}. {:<14} {}", index + 1, name, kind.scoring_note());
    }
    println!(
        "\n匹配顺序说明：`oral-state` 必须排在 `oral` 之前——\n\
         它的 reply_type 也是 record，顺序反了「只取最后一个子题」的规则会静默失效。"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 多子题必须**逐个**返回，不能拼成一段。
    ///
    /// 实测 `u5g341` 有 5 个朗读子题：拼接后是 417 字，而音频只读一句，
    /// 完整度按 5 句算 → 总分从 92 掉到 **29**。这个测试钉住「逐子题」。
    #[test]
    fn extracts_each_subquestion_separately() {
        let plain = r#"[{"content":"{\"replyType\":\"record\",\"children\":[\
{\"rule\":{\"keywords\":[],\"words\":[],\"text\":\"<p>First sentence here.</p>\"},\"config\":{\"recordTime\":9000}},\
{\"rule\":{\"keywords\":[],\"words\":[],\"text\":\"<p>Second sentence here.</p>\"},\"config\":{\"recordTime\":9000}}]}"#;

        let texts = extract_speech_texts(plain);
        assert_eq!(texts.len(), 2, "两个子题要返回两条：{texts:?}");
        assert_eq!(texts[0], "First sentence here.");
        assert_eq!(texts[1], "Second sentence here.");
    }

    /// 明文里的 `\u003c` 等转义要先还原再剥标签。
    #[test]
    fn unescapes_and_strips_tags() {
        let plain = r#"{"rule":{"text":"\u003cp\u003eHello \u0026nbsp;world\u003c/p\u003e"}}"#;
        let texts = extract_speech_texts(plain);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Hello world", "转义与 &nbsp; 都要处理掉");
    }

    /// 没有 `rule.text` 时退回 `<p>` 段落，且过滤掉太短的中文/噪声。
    #[test]
    fn falls_back_to_paragraphs() {
        let plain = r#"{"contents":[{"text":"<p>This is a long enough English sentence.</p>"},
                                     {"text":"<p>短</p>"}]}"#;
        let texts = extract_speech_texts(plain);
        assert_eq!(texts.len(), 1, "太短的段落要被过滤：{texts:?}");
        assert!(texts[0].starts_with("This is a long"));
    }

    /// 重复文本要去重。
    #[test]
    fn deduplicates_repeated_text() {
        let plain = r#"{"a":{"rule":{"text":"Same sentence."}},
                          "b":{"rule":{"text":"Same sentence."}}}"#;
        let texts = extract_speech_texts(plain);
        assert_eq!(texts.len(), 1, "重复的只留一条：{texts:?}");
    }

    /// 什么文本都提不出来时返回空，由调用方报可读错误。
    #[test]
    fn empty_when_nothing_usable() {
        assert!(extract_speech_texts("{}").is_empty());
        assert!(extract_speech_texts(r#"{"rule":{"text":""}}"#).is_empty());
    }

    /// 未转义的引号要能正确终止取值，别把后面的 JSON 也吃进去。
    #[test]
    fn stops_at_unescaped_quote() {
        let plain = r#"{"rule":{"text":"Stop here."},"other":"should not appear"}"#;
        let texts = extract_speech_texts(plain);
        assert_eq!(texts, vec!["Stop here.".to_owned()]);
    }

    /// 转义引号（`\"`）不算结束——朗读题文本里可能出现引号。
    #[test]
    fn escaped_quote_does_not_terminate() {
        let plain = r#"{"rule":{"text":"He said \"hi\" loudly."}}"#;
        let texts = extract_speech_texts(plain);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("hi"), "{texts:?}");
    }
}
