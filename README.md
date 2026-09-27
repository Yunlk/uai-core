<h1 align="center">uai-core</h1>

<p align="center"><em>U 校园学习平台的 Rust 接口客户端与协议研究工具集</em></p>

<p align="center">
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-Noncommercial-d73a49"></a>
  <img alt="rust" src="https://img.shields.io/badge/rust-edition%202024-dea584?logo=rust&logoColor=white">
  <img alt="tests" src="https://img.shields.io/badge/tests-319%20passing-2ea44f">
  <img alt="clippy" src="https://img.shields.io/badge/clippy-0%20warnings-2ea44f">
  <img alt="build" src="https://img.shields.io/badge/build-offline%20friendly-6f42c1">
  <a href="https://github.com/Yunlk/uai-core/actions/workflows/release.yml">
    <img alt="release" src="https://github.com/Yunlk/uai-core/actions/workflows/release.yml/badge.svg">
  </a>
</p>

> ## 免责声明
>
> 本项目是**个人技术研究**的产物，用于学习 HTTP / WebSocket 协议、前端逆向与
> Rust 工程实践，**仅供学习与交流**。
>
> - 使用者应自行确保其使用方式符合所在平台的服务条款、所在学校的规章制度以及
>   当地法律法规。**使用本工具产生的一切后果由使用者自行承担。**
> - 本项目**不鼓励、也不支持**任何违反考试纪律、学术诚信或平台规则的行为。
> - 作者不对因使用本项目导致的**账号受限、成绩处理、数据丢失**或其它任何损失
>   承担责任。
> - 仓库**不含任何账号凭据**，也不收集、不上传使用者数据；凭据仅由使用者在本机
>   通过环境变量提供，会话缓存也只写在本机临时目录。
> - 平台接口随时可能变更，本项目**不保证长期可用**。
>
> 如果你不同意以上任何一条，请立即停止使用并删除本项目。

---

## 概述

U 校园（Unipus）在线学习平台的接口客户端，Rust 实现，分为**库**（`uai-core`）与
**命令行工具**（`uai`）。

它做的事情很具体：把平台上前端做的事（读目录、取题、取答案、组装作答、提交）
用可审计的代码复刻一遍，并把「读」与「写」严格分开。

## 能做什么 / 不做什么

**能**

- 登录并缓存会话（凭据只来自环境变量）
- 读取班级、教材、目录、题目、标准答案、作答状态与成绩
- 把**当前班级**的必修任务按要求组装并提交，支持限速与断点式分批
- 语音评测（只评分，不上传）
- 学习时长上报（两条协议：积分账 / 必修时长账，见下方命令详解）

**不做**

- 不碰考试类任务（尚未实现，见 Roadmap）
- 不伪造时间戳或分数
- 不内置任何账号、Cookie 或代理

## 安装与构建

**从源码构建**（需要 Rust 1.87+，edition 2024）：

```powershell
git clone https://github.com/Yunlk/uai-core.git
cd uai-core
cargo build --release --offline     # 本机 cargo 缓存齐全时用 --offline
# 产物：target/release/uai.exe（Windows）/ target/release/uai（Linux、macOS）
```

**直接下载二进制**：见 [Releases](https://github.com/Yunlk/uai-core/releases)。
`latest` 是随 `main` 滚动的预览版，`v0.x.y` 是打了 tag 的稳定版 (当前 `v0.1.2`)，
三平台产物：`uai-windows-x86_64.exe` / `uai-linux-x86_64` / `uai-macos-arm64`。

**凭据**只从环境变量读，**不要写进命令行**（会进 shell 历史）：

```powershell
$env:UAI_USER='<手机号/邮箱>'
$env:UAI_PASS='<密码>'
```

会话缓存在系统临时目录的 `uai-session.json`（Windows 为 `%TEMP%`），
包含门户令牌与 `open_id`。**不要提交、不要外传**；换账号直接重新 `login`。

> 下文的示例统一写 `uai`。从源码跑就把 `uai` 换成
> `cargo run --offline --release --`，例如 `cargo run --offline --release -- bookshelf`。

## 命令详解

### 读：会话与书架

#### `uai login`

手机号/密码登录，换到门户 SSO 令牌与 `open_id`，写入临时目录的会话缓存。
缺少环境变量、或平台要求验证码时报错退出，不会静默失败。

```powershell
uai login
```

#### `uai whoami`

只看当前会话状态，不发任何业务请求。用它确认「我登的是谁、令牌在不在」。

```powershell
uai whoami
# 示例用户（门户令牌 已获取，open_id 已获取）
#   门户令牌：有
#   open_id ：有
#   缓存位置：C:\Users\<你>\AppData\Local\Temp\uai-session.json
```

#### `uai bookshelf`

门户书架：**班级 → 教材**，同时给出每本教材的实例 ID（`course-v2:…`）与激活状态。
实例 ID 是后面几乎所有命令的第一个参数。

```powershell
uai bookshelf
```

> 实例 ID 长这样：`course-v2:<hash>+<教材代号>+<版本日期>`。
> **同一个实例可以挂在多个班级下**，而必修集合按班级定——所以选靶时必须带班级。

#### `uai grade <班级教材ID> [--plan-only]`

考核方案（`/api/aca/plan/detail`）+ 综合成绩（`/api/aca/achievement/queryUserScore`）。
「班级教材ID」是门户课程列表里**教材节点自己的 id**（纯数字），不是 `course-v2:…`。

```powershell
uai grade 20000000001              # 方案 + 成绩
uai grade 20000000001 --plan-only  # 只打方案，不查成绩（少一次请求）
```

输出里有三样东西值得看：

- **记分周期**：什么时候截止。教材侧**没有**截止时间，周期在班级的考核方案里。
- **逐项权重**：常见是「必修学习时长」+「教程学习成绩」各占一半。
- **逐教材缺口**：每本教材的时长、满分标准（`minHours` / `maxHours`）、还差多少。

#### `uai progress <实例ID>`

班级真实进度的**唯一权威来源**：单元级 `leafs`（含 `required` 必修标记）
与任务级 `progress/v2`。必修标记只在这里和 `targets` 里能看到，目录里没有。

```powershell
uai progress course-v2:example0000+book_x+2024_01_31
```

#### `uai catalog <实例ID> [--required]`

教材目录（单元 → 分组）。`--required` 只列必修的那些。

```powershell
uai catalog course-v2:example0000+book_x+2024_01_31
uai catalog course-v2:example0000+book_x+2024_01_31 --required
```

#### `uai targets <实例ID> --class <班级> [--skip-passed] [--no-scan]`

**按班级**筛出必修分组。口径是 `leafs.required`，也就是门户「教程学习成绩」的
分母。`--class` 必填，取值可以是 `classId`、数字 `courseId`，或班级名。

```powershell
uai targets course-v2:example0000+book_x+2024_01_31 --class 20000000002
uai targets course-v2:example0000+book_x+2024_01_31 --class 20000000002 --skip-passed
```

| 参数 | 作用 |
| --- | --- |
| `--class` | **必填**。不猜班级，因为必修按班级定 |
| `--skip-passed` | 排除已达标的分组（每组多一次状态请求） |
| `--no-scan` | 不逐组扫状态，连已达标的也列出来 |

### 读：题目与状态

这一组都是**只读**：拿内容、拿答案、看状态、看快照，都不写服务端。

| 命令 | 作用 |
| --- | --- |
| `uai content <实例ID> <分组ID>` | 解密题目内容，只看题型与题干 |
| `uai answer <实例ID> <分组ID>` | 解密标准答案 |
| `uai state <实例ID> <分组ID>` | 分组状态（是否达标、`first_pass` 时间） |
| `uai state-raw <实例ID> <分组ID>` | 上面那条的原始 JSON，排查用 |
| `uai snapshot <实例ID> <分组ID>` | 作答快照，自动取 `lastSubmit` |

```powershell
uai content course-v2:example0000+book_x+2024_01_31 u1g10
uai answer  course-v2:example0000+book_x+2024_01_31 u1g10
uai state-raw course-v2:example0000+book_x+2024_01_31 u1g10
```

#### `uai types`

列出全部题型及其评分说明，不需要登录。

```powershell
uai types
```

### 写：提交（会改服务端）

> **全仓库会写服务端的命令只有两个半**：`submit --confirm`、`auto`（不加 `--dry-run`），
> 以及两条时长链 `duration` / `study`。其余全部只读。

#### `uai submit <实例ID> <分组ID> [--confirm]`

只处理**一个**分组：组装提交载荷 → 默认**只打印、不发送**。确认载荷没问题了再加
`--confirm` 真发。先跑 `snapshot` / `content` 看清要交什么。

```powershell
uai submit course-v2:example0000+book_x+2024_01_31 u1g10              # 预览
uai submit course-v2:example0000+book_x+2024_01_31 u1g10 --confirm    # 真提交
```

#### `uai auto <实例ID> --class <班级> [选项]`

提交流水线：按**当前班级**的必修筛选 → 逐组组装 → 提交 → 回读。
**默认真的提交**，所以第一次务必先 `--dry-run`。

```powershell
uai auto course-v2:example0000+book_x+2024_01_31 --class 20000000002 --dry-run
uai auto course-v2:example0000+book_x+2024_01_31 --class 20000000002 --limit 5 --sleep-ms 25000
```

| 参数 | 作用 |
| --- | --- |
| `--class <班级>` | **必填**。`classId` / 数字 `courseId` / 班级名 |
| `--dry-run` | 只预览，不发请求。**第一次一定用它** |
| `--limit N` | 最多处理 N 组（`0` 会被拒绝：等于什么都没做） |
| `--redo` | 连已达标的分组也重交 |
| `--no-scan` | 不扫状态（等同 `--redo`，省掉每组一次请求） |
| `--units u6,u7` | 只交这些单元（缺省 = 全部必修单元） |
| `--jobs N` | 并发处理 N 组（1~32，**默认 1 = 串行**）。实测并发会互相拖住、更易撞限流，默认串行 |
| `--no-readback` | 不做回读，判分取提交响应里的 `state.score_pct`，省一次 GET |
| `--sleep-ms N` | 每组之间等待 N 毫秒（**默认 0**）。批量时请显式放大 |

两个坑：

- **筛选要两级**：目录里没有必修标记，必修散在单元级 `progress` 与任务级
  `progress/v2` 里。只用单元级会多交约 3.3 倍。
- **「已达标」是账号级痕迹**，比门户的班级口径宽（实测 42/44 vs 6/44）。
  想顶门户完成度就得加 `--redo`。

平台对提交有**节奏配额**（实测连打 11 组后全回 `600002`），所以分批交比整本硬打更实际：

```powershell
uai auto <实例ID> --class 20000000002 --units u6 --sleep-ms 25000
uai auto <实例ID> --class 20000000002 --units u7 --sleep-ms 25000
```

`Ctrl-C` 可中途停止，已提交的不会回滚。

#### `uai media`

媒体上传链状态与口语上报自检，只读。

```powershell
uai media
```

### 语音评测

#### `uai eval <WAV> --text "…" | --from <实例ID> <分组ID> [--kind …] [--llm-sk …]`

把一段 WAV 送去评测服务打分。**只评分，不提交任何作答**，所以用了也不涨进度。
WAV 必须是 **16 kHz 单声道 16-bit**。

```powershell
uai eval take.wav --text "The quick brown fox."                 # 朗读题
uai eval take.wav --from course-v2:example0000+book_x+2024_01_31 u1g10
uai eval take.wav --from course-v2:example0000+book_x+2024_01_31 u1g10 --kind open --llm-sk <KEY>
```

| 参数 | 作用 |
| --- | --- |
| `--text "…"` | 参考文本，直接给。不需要登录 |
| `--from <实例ID> <分组ID>` | 参考文本从该组内容里自动提取 |
| `--kind sent` | 朗读题，默认，不需要 key |
| `--kind open` | 开放题（presentation），**需要** `--llm-sk` |
| `--llm-sk <KEY>` | `open` 用的 LLM 凭证。**不内置默认值**：那把 key 来自前端 bundle、属于优学院，是否使用由你决定 |

### 学习时长：两条链，两本账

这是本项目最容易踩坑的地方。平台上有**两条互不相通的时长链**：

| | `duration`（房间池） | `study`（真实任务） |
| --- | --- | --- |
| 端点 | `wss://ucontent.unipus.cn/unipusiopoint/` | `wss://ucontent.unipus.cn/unipusio/` |
| 事件 | `kick-v2`（只发一次） | `start`（每 5 秒重发） |
| 归属键 | 假键 `(应用, 设备)` → **账号级** | **真实 `(分组, 课程)`** |
| 实测倍率 | 13.7~14.9× | **约 1×**（账号级封顶） |
| 进「必修学习时长」 | ❌ 不进 | ✅ 进 |

#### `uai duration [小时] [--rooms N]`

常驻的多房间加速，默认 1 小时 / 20 房间。倍率靠**房间数**（约 0.79×/房间），
同房间多开连接会被去重。

```powershell
uai duration                       # 1 小时，20 房间
uai duration 0.5 --rooms 6         # 半小时，6 房间（≈4.2×）
```

> ⚠️ **只进积分账，不进必修学习时长。** 要刷必修时长请用 `study`。
> 实测：刷了 1.144h 后等 9 分钟，门户的「必修学习时长」纹丝不动。
>
> 另外：起步有坡（`r_set` 每约 60 秒一批，前 60 秒记账为 0），
> **别拿两分钟的结果判断倍率**；`timer` / `sendTimestamp` 之类的字段是服务端时钟产出的，
> 回填无效。

#### `uai study <实例ID> --class <班级> [--hours N] [--dwell-min M] [--units u6,u7] [--page-base …]`

**真实必修任务时长投放**：单连接、按 `--dwell-min` 轮换必修任务、约 1× 记账，
精确落到教材账。这是唯一能推进门户「必修学习时长」的命令。

```powershell
uai study course-v2:example0000+book_x+2024_01_31 --class 20000000002 --hours 2
uai study course-v2:example0000+book_x+2024_01_31 --class 20000000002 --units u6,u7
uai study course-v2:example0000+book_x+2024_01_31 --class 20000000002 \
    --page-base 'https://ucontent.unipus.cn/_explorationpc_default/pc.html?cid=<…>&cloudCurriculaId=<…>&courseResourceId=<…>'
```

| 参数 | 作用 |
| --- | --- |
| `--class <班级>` | **必填**，任务页前缀要用它 |
| `--hours N` | 目标记账小时数；`0` 或缺省 = 不限，`Ctrl-C` 停 |
| `--dwell-min M` | 每个任务停留几分钟后轮换（默认 4） |
| `--units u6,u7` | 只投这些单元（默认全部必修单元） |
| `--page-base` | 真实任务页前缀（含 `cid` / `cloudCurriculaId` / `courseResourceId`）。不信用默认值时可覆盖 |

> ⚠️ **倍率就是 1×，没有捷径。** 实测同课 4 连接只记 0.63×、跨 3 门课并行 0.69×——
> 预算按账号约 1× 封顶，**多开只会摊薄**。
> 时间戳也不可伪造：`start` 载荷里没有时间字段，`delta` 由服务端时钟产出。
> 20 小时的必修时长，就是 20 小时的墙钟时间。

### 其它

```powershell
uai help      # 完整帮助（与本节内容同源）
uai types     # 题型清单
```

## 三个核心口径

同一个平台上，**「必修」「已完成」「计分点」是三本互不相等的账**。理解这一点，
才能正确判断一次提交到底有没有用：

| 口径 | 含义 | 归属 |
| --- | --- | --- |
| **必修** | 本班要求完成哪些任务 | **按班级**。同一本教材挂在不同班级时，必修集合可能完全不同 |
| **已完成** | 该任务是否做过 | 按**教材实例**，跨班级共享 |
| **计分点** | 该任务**得分是否达标** | **按班级**，由平台的考核方案计算 |

三条推论，都是实测结论：

1. **选靶必须指定班级**，不能猜。课程目录里没有必修标记，必修口径要按班级、
   用平台侧的策略接口取。
2. **「交过」不等于「得分」**。占位式、无判分的作答可以完成任务，但不会产生计分点。
3. **判定结果要看平台，不看客户端回读**。客户端的回读只说明服务端收下了请求。

## 使用注意

- **提交有配额。** 密集连打会触发平台的频率限制；`auto` 批量时请显式给足
  `--sleep-ms`（实测秒级间隔会很快被限），被限后**务必真正等待**再重试。
- **成绩与进度的刷新有延迟**，且不同页面/接口的新鲜度不同。需要尽快看到结果时，
  去平台页面上查看往往比反复请求接口更快。
- **作答记录通常只保留首次**。重复提交一般改写不了已有记录，重跑前先看哪些已达标。
- 所有**写操作集中在少数几处**，便于审阅；其余代码只读。

## 项目结构

```text
src/
  api/        接口层：一文件一端点，只负责拼请求与解析响应
  question/   题型层：把平台返回的题目还原成可作答结构
  transport.rs 网络与速率控制
  session.rs  会话与凭据
src/bin/uai/  命令行工具（薄壳，只有参数解析与输出）
examples/     诊断用小程序（审计、探测），不被 lib 依赖
```

分层原则：**接口层不做业务判断，题型层不做网络请求**。这样「哪些代码会写服务端」
可以被一眼审完。

## 开发与验证

```powershell
cargo test --offline                    # 319 个测试，全离线
cargo clippy --offline --all-targets -- -D warnings   # 0 warning（CI 口径）
rustfmt --edition 2024 <changed files>
cargo build --release --offline
```

- 本地一律加 `--offline`（本机 cargo 缓存齐全）；**CI 上去掉它、改用 `--locked`**，
  因为 runner 是干净环境。
- 测试**不依赖**真实账号、真实网络或时间敏感数据，全部用固定夹具。
- 工作约定与边界见 [`AGENTS.md`](AGENTS.md)。

## Roadmap

### 1. 考试类任务

目前只覆盖普通学习任务与作业，**考试尚未实现**。已知考试走的不是普通提交链路，
且**通常只有一次作答机会、不可回滚**。计划先做只读探针摸清流程，再考虑实现；
在那之前不会提供任何写入支持。

### 2. 更快、更准的时长上报

现有两条链都不理想：一条倍率高但**只累计到积分体系、不计入要求的学习时长**；
另一条计入学习时长但**只有约 1× 的速率**，且依赖真实任务路径。

还需要解决：

- **速率**：并行多开实测只会摊薄（同一账号存在总量上限）
- **规律**：平台的记账是**间歇性放行**的，窗口规律尚未摸清，需要长跑采样
- **校验**：服务端广播的增量是**账号级**的，不等于单条连接的实际贡献，
  真值只能以平台的时长账为准

### 3. 图形界面

命令行已跑通全链路，但对外不易用。目标是在其之上提供一个界面：

**选班级 → 查看必修与缺口 → 一键提交（自带节流与退避）→ 查看两本账的进账**

必须原样保留命令行的几条硬规则：班级必填、按班选靶、提交限速、
以及「交过 ≠ 得分」的读法。

## 许可

本项目采用**非商业使用许可**（见 [`LICENSE`](LICENSE)），要点：

- ✅ **允许随意使用、修改、改写、再分发**（学习 / 研究 / 教学 / 个人用途）
- ✅ 允许创作并发布衍生作品
- ❌ **禁止任何商业性使用**（销售、付费服务、SaaS、有偿代做等）
- ⚠️ 衍生作品须采用同等条款（同样允许改写、同样禁止商用），并保留本许可与声明

需要商业授权请先联系版权持有人取得**书面许可**。本软件按"现状"提供，
不附带任何担保。
