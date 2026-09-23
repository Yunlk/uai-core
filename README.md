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
- 学习时长上报（两种协议，见 Roadmap）

**不做**

- 不碰考试类任务（尚未实现，见 Roadmap）
- 不伪造时间戳或分数
- 不内置任何账号、Cookie 或代理

## 快速开始

```powershell
$env:UAI_USER='<手机号>'; $env:UAI_PASS='<密码>'   # 凭据只走环境变量

cargo build --release --offline
cargo run --offline -- login                      # 登录并缓存会话
cargo run --offline -- whoami
cargo run --offline -- bookshelf                  # 班级 → 教材
cargo run --offline -- progress  <实例ID>          # 按班列出完成度
cargo run --offline -- grade     <班级教材ID>       # 考核方案 + 成绩
cargo run --offline -- targets   <实例ID> --class <班级>
cargo run --offline -- content   <实例ID> <分组ID>
cargo run --offline -- answer    <实例ID> <分组ID>
cargo run --offline -- state     <实例ID> <分组ID>
cargo run --offline -- submit    <实例ID> <分组ID> [--confirm]
cargo run --offline -- auto      <实例ID> --class <班级> [--dry-run] [--redo] [--units u6,u7,u8]
cargo run --offline -- study     <实例ID> --class <班级> [--hours N]
cargo run --offline -- types
```

`submit` **默认只打印将发送的载荷、不发写请求**；真正写服务端需要 `--confirm`。
`auto` 是流水线版本，默认真的提交，请先跑 `--dry-run`。

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

- **提交有配额。** 密集连打会触发平台的频率限制；建议每组之间留出足够间隔
  （实测秒级间隔会很快被限），被限后**务必真正等待**再重试。
- **成绩与进度的刷新有延迟**，且不同页面/接口的新鲜度不同。需要尽快看到结果时，
  去平台页面上查看往往比反复请求接口更快。
- **作答记录通常只保留首次**。重复提交一般改写不了已有记录，重跑前先看哪些已达标。
- 所有**写操作集中在少数几处**，便于审阅；其余代码只读。

## 项目结构

```text
src/
  api/        接口层：一文件一端点，只负责拼请求与解析响应
  question/   题型层：把平台返回的题目还原成可作答结构
  transport/  网络与速率控制
  session.rs  会话与凭据
src/bin/uai/  命令行工具（薄壳）
examples/     诊断用小程序（审计、探测）
```

分层原则：**接口层不做业务判断，题型层不做网络请求**。这样「哪些代码会写服务端」
可以被一眼审完。

## 开发与验证

```powershell
cargo test --offline                 # 319 个测试，不联网
cargo clippy --offline --all-targets # 0 warning
rustfmt --edition 2024 <files>
```

所有测试均离线运行，不依赖真实账号。

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
