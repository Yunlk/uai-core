//! # uai-core —— U校园接口核心库
//!
//! 一文件一端点的**接口层** + 一文件一类型的**题型层**，可被单独调用、单独测试。
//!
//! ## 为什么会有这个 crate
//!
//! 旧工程（`UAI-Textbook-Console`）把接口调用、缓存、事件回灌、状态机全缠在
//! GUI 代码里，且**只有 bin target、没有 lib**。后果是：为了验一个接口，
//! 必须在 `examples/` 里把鉴权 + 解密 + 拼包重新复制一遍——复制出来的那份
//! 永远会和真正的调用点分叉（真出过：example 用门户 JWT 打进度接口拿到 401，
//! 解析出「全部未完成」，整个实验结论作废）。
//!
//! 本 crate 把接口层做成 lib，CLI 只是它的一个消费者。
//!
//! ## 分层
//!
//! ```text
//! bin/uai.rs        CLI：只负责解析参数、调 lib、打印
//! api/              接口层：一文件一端点，每个文件只拼自己那个 URL
//! question/         题型层：一文件一题型，每个文件只实现自己那种题
//! transport.rs      HTTP 传输：鉴权头 + 限流退避 + 错误呈现
//! crypto.rs         两处 AES（ECB 解密 / CBC 登录加密）
//! annotator.rs      自铸 x-annotator-auth-token
//! endpoints.rs      全部路径常量的唯一出处
//! error.rs          统一错误与类别
//! session.rs        登录态（两套令牌分开存）
//! ```
//!
//! ## 两套令牌，永远别混
//!
//! - **门户 JWT**（`Authorization`）→ `uai.unipus.cn`
//! - **自铸 annotator**（`x-annotator-auth-token`）→ `ucontent.unipus.cn`
//!
//! 目录/内容接口不校验鉴权，发错头也返回 `code:0`——所以这个错误
//! **只有打进度/答案接口时才会暴露**。

pub mod annotator;
pub mod api;
pub mod crypto;
pub mod endpoints;
pub mod error;
pub mod question;
pub mod session;
pub mod transport;

pub use error::{Error, ErrorKind, Result};
pub use session::Session;
