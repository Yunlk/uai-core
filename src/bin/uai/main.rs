//! `uai` —— 接口层的命令行入口。
//!
//! 每个子命令对应**一个接口**，都可以单独跑、单独验。这是与旧工程最关键的
//! 差别：以前验一个接口要在 `examples/` 里把鉴权 + 解密 + 拼包复制一遍。
//!
//! ```text
//! uai login                     登录并缓存会话
//! uai whoami                    查看当前会话
//! uai bookshelf                 门户书架（班级 → 教材）
//! uai progress <实例ID>          班级真实进度（唯一权威）
//! uai catalog <实例ID> [--required]   教材目录
//! uai content <实例ID> <分组ID>   解密题目内容
//! uai answer  <实例ID> <分组ID>   解密标准答案
//! uai state   <实例ID> <分组ID>   分组状态
//! uai snapshot <实例ID> <分组ID>  作答快照（自动取 lastSubmit）
//! uai types                     列出全部题型
//! ```
//!
//! ## 凭据
//!
//! 用户名密码从环境变量 `UAI_USER` / `UAI_PASS` 读，**不经命令行参数**
//! （命令行参数会进 shell 历史和进程列表）。
//! 会话缓存在 `%TEMP%/uai-session.json`，权限按当前用户。

mod cli;
mod commands;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 先解析参数成命令，再交给执行层。解析失败（未知命令、缺参数）
    // 与接口错误走同一个出口，退出码与格式因此保持一致。
    let result = cli::parse(&args).and_then(commands::run);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 错误按类别给退出码，并写明类别——脚本能据此区分
            // 网络问题、解密失败与参数问题。
            eprintln!("错误[{}]：{}", error.kind().label(), error.message());
            ExitCode::from(error.kind().exit_code() as u8)
        }
    }
}
