//! 参数解析与会话缓存。

use serde_json::Value;
use std::path::PathBuf;
use uai_core::error::{Error, Result};
use uai_core::session::Session;

/// 解析出的命令。
#[derive(Debug)]
pub enum Command {
    Help,
    /// 登录并缓存会话。
    Login,
    /// 打印当前会话。
    WhoAmI,
    /// 门户书架。
    Bookshelf,
    /// 考核方案 + 综合成绩（`/api/aca/*`）。
    Grade {
        /// 班级教材 ID（`courseResourceId`，如 20000000001）。
        course_resource_id: String,
        /// 只打印考核方案，不查成绩。
        plan_only: bool,
    },
    /// 班级真实进度。
    Progress {
        instance_id: String,
    },
    /// 教材目录。
    Catalog {
        instance_id: String,
        required_only: bool,
    },
    /// 解密题目内容。
    Content {
        instance_id: String,
        group_id: String,
        /// 连同解密后的明文一起打印（排查音频/媒体字段用）。
        raw: bool,
    },
    /// 解密标准答案。
    Answer {
        instance_id: String,
        group_id: String,
    },
    /// 分组状态。
    State {
        instance_id: String,
        group_id: String,
    },
    /// 作答快照。
    Snapshot {
        instance_id: String,
        group_id: String,
    },
    /// 分组状态原始 JSON（排查用）。
    StateRaw {
        instance_id: String,
        group_id: String,
    },
    /// 必修筛选（`leafs.required` 口径，即班级计分点的分母）。
    Targets {
        instance_id: String,
        /// **当前班级**：`classId`、数字 `courseId` 或班级名。必填。
        class_id: String,
        /// 是否再排除已达标的分组。
        skip_passed: bool,
        /// 不逐组扫状态（连已达标的也不排除）。
        skip_scan: bool,
    },
    /// 组装提交载荷（**默认只预览，不发送**）。
    Submit {
        instance_id: String,
        group_id: String,
        /// 真正发送。不加这个旗标只打印载荷。
        confirm: bool,
    },
    /// **打通提交流水线**：必修筛选 → 逐组组装 → 提交 → 回读。
    ///
    /// 与 [`Command::Submit`] 的差别是它**默认真的发**（`--dry-run` 才预览），
    /// 且一次处理**当前班级在本教材上的全部必修分组**。
    Auto {
        instance_id: String,
        /// **当前班级**：`classId`、数字 `courseId` 或班级名。必填。
        ///
        /// 同一本教材会挂在多个班里，计分点分母不同（实测同一本《视听说2》
        /// 一个班 49、另一个班 35），所以班级不能猜。
        class_id: String,
        /// 只交这些单元（如 `u6,u7,u8`）；空 = 全部必修单元。
        ///
        /// 服务端对提交有**节奏配额**（实测连打 11 组后全回 `600002`），
        /// 所以分批交比整本硬打更实际。
        units: Vec<String>,
        /// 只组装并校验，不发任何写请求。
        dry_run: bool,
        /// 最多处理多少组（`None` = 不限）。
        limit: Option<usize>,
        /// 重做模式：不排除已达标的分组。
        redo: bool,
        /// 不逐组扫状态（等价于 `--redo`：必修组全交，省掉每组一次请求）。
        no_scan: bool,
        /// 并行处理多少个分组。
        ///
        /// 一组要 4 次串行请求（内容 → 答案 → 提交 → 回读），实测约 600ms；
        /// 串行跑一本教材要几分钟，并行是唯一的数量级改进。
        jobs: usize,
        /// 不做回读（省掉每组最后一次 GET，约快 1/4）。
        ///
        /// 判分改取提交响应里的 `state.score_pct`——同样是服务端写的值。
        no_readback: bool,
        /// 每组之间额外等待的毫秒数（对限流温柔一点）。
        sleep_ms: u64,
    },
    /// 媒体上传链状态与口语上报自检（只读，不发上传）。
    Media,
    /// 语音评测：把一段 WAV 送去优学院评测服务打分（**只评分，不提交**）。
    Eval {
        /// 16kHz 单声道 16-bit WAV 文件路径。
        wav: PathBuf,
        /// 从分组内容里自动提取题目文本：`(实例ID, 分组ID)`。
        from_group: Option<(String, String)>,
        /// 直接给的参考文本。与 `from_group` 二选一。
        text: Option<String>,
        /// 评测类型：`sent`（朗读，默认）或 `open`（开放题，要 `--llm-sk`）。
        kind: String,
        /// 开放题需要的 LLM 凭证。
        ///
        /// ⚠️ 不内置默认值：那把 key 来自前端 bundle、属于优学院，
        /// 是否使用由使用者决定。
        llm_sk: Option<String>,
        /// 调用者身份（发给服务端，开放题会加 `gd-uai-edu-` 前缀）。
        user: String,
        /// 多子题时评哪一个（`--from` 用）。
        index: Option<usize>,
    },
    /// 学习时长加速（常驻，直到达标或 Ctrl-C）。
    ///
    /// ## ⚠️ 这条链**只进积分账，不进 item 30（必修学习时长）**
    ///
    /// 它按 `(应用名, 设备名)` 假键对记账、实测 13.7~14.9×，但 2026-09 实测
    /// 刷了 1.144h 后等 9 分钟，门户「必修学习时长」纹丝不动。
    /// 要刷必修学习时长请用 [`Command::Study`]。
    Duration {
        hours: f64,
        rooms: usize,
    },
    /// **真实必修任务时长投放**：单连接、轮换必修任务、约 1×、精确落教材账。
    ///
    /// 与 [`Command::Duration`] 是两条不同的链：这条用 `/unipusio` + `start`，
    /// 归属键是 `(真实分组, 真实课程)`，**能进 item 30**。
    ///
    /// ## 倍率就是 1×，没有捷径
    ///
    /// 实测：同课 4 连接 0.63×、跨 3 门课并行 0.69×——预算按账号 ~1× 封顶，
    /// 多开只会摊薄。时间戳也不可伪造（`start` 载荷里没有时间字段，
    /// `delta` 由服务端时钟产出）。
    Study {
        instance_id: String,
        /// 当前班级（`classId` / 数字 `courseId` / 班级名）。必填：任务页前缀要用它。
        class_id: String,
        /// 目标记账小时数；`0` = 一直跑到 Ctrl-C。
        hours: f64,
        /// 每个必修任务停留多少分钟（到点轮换下一个）。
        dwell_min: f64,
        /// 真实任务页前缀（含 `cid`/`cloudCurriculaId`/`courseResourceId` 等查询串）。
        /// 不给就用实测那套默认值。
        page_base: Option<String>,
        /// 只投这些单元（如 `u6,u7`）；不给就投任务树里的全部必修。
        units: Vec<String>,
    },
    /// 列出全部题型。
    Types,
}

/// 解析命令行参数。
pub fn parse(args: &[String]) -> Result<Command> {
    let Some(name) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };
    let rest = &args[1..];

    match name {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "login" => Ok(Command::Login),
        "whoami" => Ok(Command::WhoAmI),
        "bookshelf" => Ok(Command::Bookshelf),
        "grade" => Ok(Command::Grade {
            course_resource_id: one(rest, "grade")?,
            plan_only: rest.iter().any(|flag| flag == "--plan-only"),
        }),
        "types" => Ok(Command::Types),
        "submit" => {
            let (instance_id, group_id) = two(rest, "submit")?;
            Ok(Command::Submit {
                instance_id,
                group_id,
                // 默认预览。写操作必须显式确认——见 help 里的说明。
                confirm: rest.iter().any(|flag| flag == "--confirm"),
            })
        }
        "media" => Ok(Command::Media),
        "auto" => {
            let instance_id = one(rest, "auto")?;
            let limit = match flag_value(rest, "--limit") {
                Some(raw) => Some(
                    raw.parse::<usize>()
                        .map_err(|_| Error::invalid(format!("--limit 不是数字：{raw}")))?,
                ),
                None => None,
            };
            if limit == Some(0) {
                return Err(Error::invalid("--limit 0 等于什么都不做，去掉它或给个正数"));
            }
            let sleep_ms = match flag_value(rest, "--sleep-ms") {
                Some(raw) => raw
                    .parse::<u64>()
                    .map_err(|_| Error::invalid(format!("--sleep-ms 不是数字：{raw}")))?,
                None => 0,
            };
            // 默认 4：一组 4 次串行请求、每次约 150ms，串行是主要瓶颈。
            // 再高会明显撞限流（实测约每 7 组一次），得不偿失。
            let jobs = match flag_value(rest, "--jobs") {
                Some(raw) => {
                    let parsed = raw
                        .parse::<usize>()
                        .map_err(|_| Error::invalid(format!("--jobs 不是数字：{raw}")))?;
                    if parsed == 0 {
                        return Err(Error::invalid("--jobs 0 等于不干活，给个正数（1 = 串行）"));
                    }
                    parsed.min(MAX_JOBS)
                }
                None => DEFAULT_JOBS,
            };
            Ok(Command::Auto {
                instance_id,
                class_id: required_class(rest)?,
                units: flag_value(rest, "--units")
                    .map(|raw| {
                        raw.split(',')
                            .map(str::trim)
                            .filter(|part| !part.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                dry_run: rest.iter().any(|flag| flag == "--dry-run"),
                limit,
                redo: rest.iter().any(|flag| flag == "--redo"),
                no_scan: rest.iter().any(|flag| flag == "--no-scan"),
                jobs,
                no_readback: rest.iter().any(|flag| flag == "--no-readback"),
                sleep_ms,
            })
        }
        "eval" => {
            // uai eval <wav> --text "…"            [--kind sent|open] [--llm-sk K] [--user U]
            // uai eval <wav> --from <实例ID> <分组ID> [--kind open] [--llm-sk K]
            let wav = positional(rest, 0)
                .map(PathBuf::from)
                .ok_or_else(|| Error::invalid("`eval` 缺少参数：16kHz 单声道 WAV 路径"))?;

            let flag_value = |name: &str| -> Option<String> {
                rest.iter()
                    .position(|arg| arg == name)
                    .and_then(|index| rest.get(index + 1))
                    .cloned()
            };

            // --from 走「从分组内容里自动提取题目文本」这条路。
            //
            // ⚠️ 不能用 `positional` 取这两个值：WAV 路径本身就是第 0 个位置
            // 参数，会把 `a.wav` 当成实例 ID。必须取 `--from` **紧跟**的两个。
            let from_group = if let Some(index) = rest.iter().position(|arg| arg == "--from") {
                let pick = |offset: usize| -> Result<String> {
                    rest.get(index + offset)
                        .filter(|value| !value.starts_with("--"))
                        .cloned()
                        .ok_or_else(|| {
                            Error::invalid("`eval --from` 需要两个参数：<实例ID> <分组ID>")
                        })
                };
                Some((pick(1)?, pick(2)?))
            } else {
                None
            };

            let text = flag_value("--text");
            if from_group.is_none() && text.is_none() {
                return Err(Error::invalid(
                    "`eval` 需要参考文本：`--text \"…\"`，或用 `--from <实例ID> <分组ID>` 自动提取",
                ));
            }

            Ok(Command::Eval {
                wav,
                from_group,
                text,
                kind: flag_value("--kind").unwrap_or_else(|| "sent".to_owned()),
                llm_sk: flag_value("--llm-sk"),
                user: flag_value("--user").unwrap_or_default(),
                index: match flag_value("--index") {
                    Some(raw) => Some(
                        raw.parse::<usize>()
                            .map_err(|_| Error::invalid(format!("--index 不是数字：{raw}")))?,
                    ),
                    None => None,
                },
            })
        }
        "duration" => {
            // `uai duration [小时] [--rooms N]`，默认 1 小时 / 20 房间。
            let hours = positional(rest, 0)
                .map(|text| {
                    text.parse::<f64>()
                        .map_err(|_| Error::invalid(format!("小时数不是数字：{text}")))
                })
                .transpose()?
                .unwrap_or(1.0);
            if !(0.0..=24.0).contains(&hours) {
                return Err(Error::invalid("小时数应在 0 ~ 24 之间"));
            }
            let rooms = match rest.iter().position(|arg| arg == "--rooms") {
                Some(index) => rest
                    .get(index + 1)
                    .ok_or_else(|| Error::invalid("--rooms 后面要跟房间数"))?
                    .parse::<usize>()
                    .map_err(|_| Error::invalid("--rooms 不是数字"))?,
                None => uai_core::api::duration::SAFE_ROOMS,
            };
            // ⚠️ 超过 SAFE_ROOMS 要**明确警告**，不能静默接受。
            //
            // `build_room_pool` 会按 MAX_ROOMS(60) 钳制，而 20 以上实测
            // 握手会被服务端随机拒绝——静默接受等于让用户拿到一个
            // 「看着在跑、实际大半房间连不上」的结果。
            if rooms > uai_core::api::duration::SAFE_ROOMS {
                eprintln!(
                    "警告：{} 个房间超过实测安全值 {}。\
                     更多房间的握手会被服务端随机拒绝，实际在线数可能远低于请求数。",
                    rooms,
                    uai_core::api::duration::SAFE_ROOMS
                );
            }
            Ok(Command::Duration { hours, rooms })
        }
        "study" => {
            let instance_id = one(rest, "study")?;
            // 目标时长走 `--hours`（不要用位置参数：`--class 100001` 的值会被
            // 位置参数解析器当成时长吃掉，实测踩过）。
            let hours = match flag_value(rest, "--hours") {
                Some(raw) => raw
                    .parse::<f64>()
                    .map_err(|_| Error::invalid(format!("--hours 不是数字：{raw}")))?,
                None => 0.0,
            };
            if hours < 0.0 {
                return Err(Error::invalid("--hours 不能为负（0 = 不限，Ctrl-C 停）"));
            }
            let dwell_min = match flag_value(rest, "--dwell-min") {
                Some(raw) => raw
                    .parse::<f64>()
                    .map_err(|_| Error::invalid(format!("--dwell-min 不是数字：{raw}")))?,
                None => 4.0,
            };
            if dwell_min <= 0.0 {
                return Err(Error::invalid("--dwell-min 必须为正（单位：分钟）"));
            }
            let units = flag_value(rest, "--units")
                .map(|raw| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|part| !part.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(Command::Study {
                instance_id,
                class_id: required_class(rest)?,
                hours,
                dwell_min,
                page_base: flag_value(rest, "--page-base"),
                units,
            })
        }
        "targets" => Ok(Command::Targets {
            instance_id: one(rest, "targets")?,
            class_id: required_class(rest)?,
            skip_passed: rest.iter().any(|flag| flag == "--skip-passed"),
            skip_scan: rest.iter().any(|flag| flag == "--no-scan"),
        }),
        "progress" => Ok(Command::Progress {
            instance_id: one(rest, "progress")?,
        }),
        "catalog" => Ok(Command::Catalog {
            instance_id: one(rest, "catalog")?,
            required_only: rest.iter().any(|flag| flag == "--required"),
        }),
        "content" => Ok(Command::Content {
            instance_id: one(rest, "content")?,
            group_id: two(rest, "content")?.1,
            raw: rest.iter().any(|flag| flag == "--raw"),
        }),
        "answer" => Ok(Command::Answer {
            instance_id: one(rest, "answer")?,
            group_id: two(rest, "answer")?.1,
        }),
        "state" => Ok(Command::State {
            instance_id: one(rest, "state")?,
            group_id: two(rest, "state")?.1,
        }),
        "state-raw" => Ok(Command::StateRaw {
            instance_id: one(rest, "state-raw")?,
            group_id: two(rest, "state-raw")?.1,
        }),
        "snapshot" => Ok(Command::Snapshot {
            instance_id: one(rest, "snapshot")?,
            group_id: two(rest, "snapshot")?.1,
        }),
        other => Err(Error::invalid(format!(
            "未知命令：{other}\n运行 `uai help` 查看用法"
        ))),
    }
}

/// 取位置参数 `index`（跳过以 `--` 开头的旗标）。
fn positional(args: &[String], index: usize) -> Option<&String> {
    args.iter().filter(|arg| !arg.starts_with("--")).nth(index)
}

/// 取 `--flag value` 形式的值。旗标不存在返回 `None`。
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn one(args: &[String], command: &str) -> Result<String> {
    let Some(value) = positional(args, 0) else {
        return Err(Error::invalid(format!(
            "`{command}` 缺少参数：实例 ID（course-v2:…）"
        )));
    };
    Ok(value.clone())
}

fn two(args: &[String], command: &str) -> Result<(String, String)> {
    let first = one(args, command)?;
    let Some(second) = positional(args, 1) else {
        return Err(Error::invalid(format!(
            "`{command}` 缺少参数：分组 ID（如 u1g2）"
        )));
    };
    Ok((first, second.clone()))
}

/// `auto` 默认并行度。
///
/// ## 实测是 1，不是 4（2026-09）
///
/// 服务端把提交路径锁成了串行，并发只会撞墙：
///
/// | `--jobs` | 4 组耗时 | 接受 | 503 |
/// | --- | --- | --- | --- |
/// | **1** | **2.49s** | **4/4** | 0 |
/// | 2 | 131.3s | 2 | 6 |
/// | 4 | 6.93s | 1 | 8 |
/// | 8 | 4.77s | 1 | 8 |
/// | 32 | 44 组全 503 | 0 | — |
///
/// 所以默认串行。真要并发就自己 `--jobs N`，但先读上面那张表。
pub const DEFAULT_JOBS: usize = 1;

/// 并行度上限。再高只会撞限流（实测约每 7 组一次 600002）。
pub const MAX_JOBS: usize = 32;

/// 取必填的 `--class`。
///
/// ## 为什么不让它可选
///
/// 提交目标是**当前班级的必修**，而同一本教材会挂在多个班里、计分点分母
/// 不同（实测同一本《视听说2》：一个班 49，另一个班 35）。默认到某个班
/// 就等于替用户猜了一次口径，猜错的方向是「交错东西」。
///
/// 取值可以是 `classId`（字符串）、数字 `courseId`，或班级名。
fn required_class(args: &[String]) -> Result<String> {
    match flag_value(args, "--class") {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(Error::invalid(
            "缺少 `--class <classId|数字courseId|班级名>`：\
             提交目标是**当前班级的必修**，同一教材挂在多个班时口径不同，必须显式指定\
             （用 `uai bookshelf` 看班级，或 `uai progress <实例ID>` 看按班的读数）",
        )),
    }
}

/// 帮助文本。
pub const HELP: &str = "\
U校园接口调试 CLI

用法：
  uai <命令> [参数]

命令：
  login                                 登录并缓存会话
  whoami                                查看当前会话
  bookshelf                             门户书架（班级 → 教材）
  grade    <班级教材ID> [--plan-only]     考核方案 + 综合成绩（/api/aca/*）
                                           --plan-only  只打考核方案，不查成绩
  progress <实例ID>                      班级真实进度（唯一权威来源）
  catalog  <实例ID> [--required]         教材目录；--required 只列必修
  targets  <实例ID> --class <班级> [--skip-passed] [--no-scan]
                                        必修筛选：口径是 leafs.required
                                        （= 门户「教程学习成绩」的分母，本班实测 49）
                                          --class <班级> 必填：classId / 数字 courseId / 班级名
                                          --skip-passed  排除已达标的分组
                                          --no-scan      不逐组扫状态（连已达标的也不排除）
  content  <实例ID> <分组ID>             解密题目内容（只看类型，不提交）
  answer   <实例ID> <分组ID>             解密标准答案
  state    <实例ID> <分组ID>             分组状态
  state-raw <实例ID> <分组ID>            分组状态原始 JSON（排查用）
  snapshot <实例ID> <分组ID>             作答快照（自动取 lastSubmit）
  submit   <实例ID> <分组ID> [--confirm]
                                        组装提交载荷；**默认只预览不发送**
                                          --confirm  真正提交（会写服务端！）
  auto     <实例ID> --class <班级> [--dry-run] [--limit N] [--redo] [--no-scan] [--sleep-ms N]
                                        **打通提交流水线**：按当前班级的必修筛选
                                        → 逐组组装 → 提交 → 回读
                                          默认**真的提交**；--dry-run 只预览
                                          --class <班级> 必填：classId / 数字 courseId / 班级名
                                          --limit N    最多处理 N 组
                                          --redo       连已达标的分组也重交
                                                       ★ 想顶门户完成度就得用它：默认的
                                                         「已达标」是账号级痕迹，比门户
                                                         班级口径宽（实测 42/44 vs 6/44）
                                          --no-scan    不扫状态（等同 --redo，省每组一次请求）
                                          --sleep-ms N 每组之间等待 N 毫秒
                                        Ctrl-C 可中途停止，已提交的不会回滚
  media                                 媒体上传链状态与口语上报自检
  eval     <WAV> --text \"…\"             语音评测：送一段 WAV 去打分（**只评分，不提交**）
           <WAV> --from <实例ID> <分组ID>   参考文本从该组内容里自动提取
             --kind sent|open                sent=朗读题（默认，不要 key）
                                             open=开放题 presentation（要 --llm-sk）
             --llm-sk <KEY>                 open 用的 LLM 凭证。**不内置默认值**：
                                            那把 key 来自前端 bundle、属于优学院，
                                            是否使用由你决定
           WAV 必须是 16kHz 单声道 16-bit
  duration [小时] [--rooms N]            学习时长加速（常驻，默认 1 小时 / 20 房间）
                                          ⚠️ 只进积分账，**不进必修学习时长**
  study    <实例ID> --class <班级> [--hours N] [--dwell-min M] [--units u6,u7]
                                         **真实必修任务时长投放**（进必修学习时长）
                                           单连接、轮换必修任务、约 1×（无倍率）
                                           --hours N      目标记账小时数，0/缺省 = 不限
                                           --dwell-min M  每个任务停留几分钟后轮换（默认 4）
                                           --units u6,u7  只投这些单元（默认全部必修单元）
                                           --page-base   真实任务页前缀，不信用默认值时可覆盖
                                         Ctrl-C 停止；时间戳不可伪造，账只按真实时间走
  types                                 列出全部题型及其评分说明
  help                                  显示本帮助

凭据：
  从环境变量读取，不要写进命令行：
    $env:UAI_USER='手机号'
    $env:UAI_PASS='密码'

说明：
  **会写服务端的只有 `submit --confirm` 与 `auto`（不加 --dry-run）。**
  其余子命令全部只读；`eval` 只向评测服务要分数，不写任何服务端状态。

  auto 的两级必修筛选缺一不可：目录里**没有**必修标记，必修散在
  `progress`（单元级）与 `progress/v2`（任务级）里。只用单元级会多交 3.3 倍。
";

/// 会话缓存路径。
pub fn session_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push("uai-session.json");
    path
}

/// 读取缓存的会话。
pub fn load_session() -> Option<Session> {
    let text = std::fs::read_to_string(session_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// 写入会话缓存。
pub fn save_session(session: &Session) -> Result<()> {
    let path = session_path();
    let text = serde_json::to_string_pretty(session)
        .map_err(|error| Error::parse(format!("序列化会话失败：{error}")))?;
    std::fs::write(&path, text)
        .map_err(|error| Error::invalid(format!("写入会话缓存失败：{error}")))?;
    Ok(())
}

/// 打印一个 JSON 值（缩进）。
pub fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => println!("（无法格式化 JSON：{error}）"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn no_args_shows_help() {
        assert!(matches!(parse(&[]).unwrap(), Command::Help));
    }

    #[test]
    fn parses_two_positional_args() {
        let command = parse(&args(&["content", "course-v2:x", "u1g2"])).unwrap();
        match command {
            Command::Content {
                instance_id,
                group_id,
                raw,
            } => {
                assert_eq!(instance_id, "course-v2:x");
                assert_eq!(group_id, "u1g2");
                assert!(!raw, "默认不打印明文");
            }
            _ => panic!("应解析成 content 命令"),
        }
    }

    /// 旗标不能挤掉位置参数。
    #[test]
    fn flags_do_not_consume_positional_slots() {
        let command = parse(&args(&["catalog", "--required", "course-v2:x"])).unwrap();
        match command {
            Command::Catalog {
                instance_id,
                required_only,
            } => {
                assert_eq!(instance_id, "course-v2:x");
                assert!(required_only, "--required 应被识别");
            }
            _ => panic!("应解析成 catalog 命令"),
        }
    }

    /// 缺少参数要给出可读错误，不能 panic。
    #[test]
    fn missing_args_report_readable_errors() {
        let error = parse(&args(&["content", "course-v2:x"])).unwrap_err();
        assert!(error.message().contains("分组 ID"), "{error}");
        let error = parse(&args(&["progress"])).unwrap_err();
        assert!(error.message().contains("实例 ID"), "{error}");
    }

    /// 未知命令要提示看帮助。
    #[test]
    fn unknown_command_is_rejected() {
        let error = parse(&args(&["submit-everything"])).unwrap_err();
        assert!(error.message().contains("未知命令"), "{error}");
    }

    /// 帮助文本必须覆盖全部命令名。
    #[test]
    fn help_lists_every_command() {
        for name in [
            "login",
            "whoami",
            "bookshelf",
            "grade",
            "progress",
            "catalog",
            "content",
            "answer",
            "state",
            "snapshot",
            "types",
            "eval",
            "media",
            "duration",
            "targets",
            "submit",
            "auto",
        ] {
            assert!(HELP.contains(name), "帮助里缺少 {name}");
        }
    }

    /// ★ `auto` 默认**真的提交**（与 `submit` 的预览默认相反）。
    #[test]
    fn auto_defaults_to_real_submission() {
        match parse(&args(&["auto", "course-v2:x", "--class", "100001"])).unwrap() {
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
            } => {
                assert_eq!(instance_id, "course-v2:x");
                assert_eq!(class_id, "100001");
                assert!(units.is_empty(), "没给 --units 就该是空");
                assert!(!dry_run, "auto 的意义就是提交；不 --dry-run 就得真发");
                assert_eq!(limit, None);
                assert!(!redo, "默认排除已达标的分组");
                assert!(!no_scan, "默认要扫状态");
                // ★ 默认串行：实测并发只会撞服务端限流（jobs=2 反而慢 50 倍）。
                assert_eq!(jobs, DEFAULT_JOBS);
                assert_eq!(DEFAULT_JOBS, 1, "并发已被实测否掉，别再改回 >1");
                assert!(!no_readback, "默认回读，`--no-readback` 才省那一次往返");
                assert_eq!(sleep_ms, 0);
            }
            _ => panic!("应解析成 auto 命令"),
        }
    }

    /// ★ 缺 `--class` 必须报错——提交目标是**当前班级**的必修，不能猜。
    #[test]
    fn auto_requires_class() {
        let error = parse(&args(&["auto", "course-v2:x"])).unwrap_err();
        assert!(error.message().contains("--class"), "{error}");
        assert!(error.message().contains("班级"), "{error}");
    }

    /// `targets` 同样必填 `--class`。
    #[test]
    fn targets_requires_class() {
        assert!(
            parse(&args(&["targets", "course-v2:x"]))
                .unwrap_err()
                .message()
                .contains("--class")
        );
        assert!(parse(&args(&["targets", "course-v2:x", "--class", "某班"])).is_ok());
    }

    /// `auto` 的全部旗标都要认。
    #[test]
    fn auto_parses_every_flag() {
        match parse(&args(&[
            "auto",
            "--dry-run",
            "course-v2:x",
            "--class",
            "1111111111111111111",
            "--limit",
            "10",
            "--redo",
            "--no-scan",
            "--jobs",
            "3",
            "--no-readback",
            "--sleep-ms",
            "1500",
        ]))
        .unwrap()
        {
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
            } => {
                assert_eq!(instance_id, "course-v2:x", "旗标不能挤掉位置参数");
                assert_eq!(class_id, "1111111111111111111");
                assert!(units.is_empty(), "这条没给 --units");
                assert!(dry_run);
                assert_eq!(limit, Some(10));
                assert!(redo);
                assert!(no_scan);
                assert_eq!(jobs, 3);
                assert!(no_readback);
                assert_eq!(sleep_ms, 1500);
            }
            _ => panic!("应解析成 auto 命令"),
        }
    }

    /// `auto` 缺实例 ID 要报可读错误。
    #[test]
    fn auto_requires_instance_id() {
        let error = parse(&args(&["auto"])).unwrap_err();
        assert!(error.message().contains("实例 ID"), "{error}");
        assert!(parse(&args(&["auto", "--dry-run"])).is_err());
    }

    /// ★ `--limit 0` 必须被拒——否则命令「成功」却什么都没做，最难排查。
    #[test]
    fn auto_rejects_zero_limit() {
        let error = parse(&args(&["auto", "course-v2:x", "--limit", "0"])).unwrap_err();
        assert!(error.message().contains("--limit 0"), "{error}");
    }

    /// `--limit` / `--sleep-ms` 不是数字要报错，不能静默当 0。
    #[test]
    fn auto_rejects_non_numeric_values() {
        assert!(parse(&args(&["auto", "course-v2:x", "--limit", "abc"])).is_err());
        assert!(parse(&args(&["auto", "course-v2:x", "--sleep-ms", "soon"])).is_err());
    }

    /// `eval` 用 `--text` 时不需要登录，也不该带 llmSk。
    #[test]
    fn eval_parses_text_form() {
        match parse(&args(&[
            "eval", "a.wav", "--text", "hello", "--kind", "sent",
        ]))
        .unwrap()
        {
            Command::Eval {
                wav,
                from_group,
                text,
                kind,
                llm_sk,
                ..
            } => {
                assert_eq!(wav, PathBuf::from("a.wav"));
                assert!(from_group.is_none());
                assert_eq!(text.as_deref(), Some("hello"));
                assert_eq!(kind, "sent");
                assert!(llm_sk.is_none(), "默认绝不能内置那把 key");
            }
            _ => panic!("应解析成 eval 命令"),
        }
    }

    /// `eval --from` 走「从分组内容提取文本」那条路。
    #[test]
    fn eval_parses_from_group_form() {
        match parse(&args(&[
            "eval",
            "a.wav",
            "--from",
            "course-v2:x",
            "u5g295",
            "--kind",
            "open",
            "--llm-sk",
            "sk-x",
        ]))
        .unwrap()
        {
            Command::Eval {
                from_group,
                text,
                kind,
                llm_sk,
                ..
            } => {
                assert_eq!(
                    from_group,
                    Some(("course-v2:x".to_owned(), "u5g295".to_owned()))
                );
                assert!(text.is_none());
                assert_eq!(kind, "open");
                assert_eq!(llm_sk.as_deref(), Some("sk-x"));
            }
            _ => panic!("应解析成 eval 命令"),
        }
    }

    /// `eval` 既没 `--text` 也没 `--from` 时要报可读错误。
    #[test]
    fn eval_requires_a_text_source() {
        let error = parse(&args(&["eval", "a.wav"])).unwrap_err();
        assert!(error.message().contains("--text"), "{error}");
        assert!(error.message().contains("--from"), "{error}");
    }

    /// `eval` 缺 WAV 路径要报可读错误。
    #[test]
    fn eval_requires_wav_path() {
        let error = parse(&args(&["eval"])).unwrap_err();
        assert!(error.message().contains("WAV"), "{error}");
    }
    /// ★ `submit` 默认必须是预览 —— 写操作不能默认生效。
    #[test]
    fn submit_defaults_to_preview() {
        match parse(&args(&["submit", "course-v2:x", "u1g2"])).unwrap() {
            Command::Submit { confirm, .. } => {
                assert!(!confirm, "不加 --confirm 绝不能真的提交");
            }
            _ => panic!("应解析成 submit 命令"),
        }
    }

    /// `--confirm` 才开启真提交。
    #[test]
    fn submit_confirm_flag_enables_sending() {
        match parse(&args(&["submit", "course-v2:x", "u1g2", "--confirm"])).unwrap() {
            Command::Submit { confirm, .. } => assert!(confirm),
            _ => panic!("应解析成 submit 命令"),
        }
    }

    /// `submit` 缺参数要报可读错误，不能默认拿到空 ID。
    #[test]
    fn submit_requires_both_ids() {
        assert!(parse(&args(&["submit", "course-v2:x"])).is_err());
        assert!(parse(&args(&["submit"])).is_err());
    }

    /// `duration` 默认 1 小时 / 安全房间数。
    #[test]
    fn duration_defaults_are_safe() {
        match parse(&args(&["duration"])).unwrap() {
            Command::Duration { hours, rooms } => {
                assert_eq!(hours, 1.0);
                assert_eq!(rooms, uai_core::api::duration::SAFE_ROOMS);
            }
            _ => panic!("应解析成 duration 命令"),
        }
    }

    /// `duration` 能解析小时数与 `--rooms`。
    #[test]
    fn duration_parses_hours_and_rooms() {
        match parse(&args(&["duration", "2.5", "--rooms", "6"])).unwrap() {
            Command::Duration { hours, rooms } => {
                assert_eq!(hours, 2.5);
                assert_eq!(rooms, 6);
            }
            _ => panic!("应解析成 duration 命令"),
        }
    }

    /// `duration` 拒绝非法小时数。
    #[test]
    fn duration_rejects_bad_hours() {
        assert!(parse(&args(&["duration", "abc"])).is_err());
        assert!(parse(&args(&["duration", "99"])).is_err());
        assert!(parse(&args(&["duration", "-1"])).is_err());
    }

    /// `--rooms` 后面没值要报错，不能默认成 0 或安全值。
    #[test]
    fn duration_requires_rooms_value() {
        assert!(parse(&args(&["duration", "1", "--rooms"])).is_err());
        assert!(parse(&args(&["duration", "1", "--rooms", "abc"])).is_err());
    }

    /// `media` 不需要参数。
    #[test]
    fn media_takes_no_args() {
        assert!(matches!(parse(&args(&["media"])).unwrap(), Command::Media));
    }

    /// `grade` 需要班级教材 ID，`--plan-only` 不影响位置参数。
    #[test]
    fn grade_parses_id_and_flag() {
        match parse(&args(&["grade", "20000000001"])).unwrap() {
            Command::Grade {
                course_resource_id,
                plan_only,
            } => {
                assert_eq!(course_resource_id, "20000000001");
                assert!(!plan_only);
            }
            _ => panic!("应解析成 grade 命令"),
        }
        match parse(&args(&["grade", "--plan-only", "20000000001"])).unwrap() {
            Command::Grade { plan_only, .. } => assert!(plan_only),
            _ => panic!("应解析成 grade 命令"),
        }
        assert!(parse(&args(&["grade"])).is_err(), "缺 ID 要报错");
    }
}
