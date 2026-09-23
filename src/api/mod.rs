//! 接口层：**一文件一端点**。
//!
//! 每个文件只负责自己那个接口：拼 URL（路径常量来自 [`crate::endpoints`]）、
//! 发请求、把响应解析成强类型。**不掺业务判断**，也不缓存。
//!
//! | 文件 | 端点 | 域名 |
//! | --- | --- | --- |
//! | [`login`] | SSO 密码/二维码登录 | `sso.unipus.cn` |
//! | [`user_info`] | 当前用户（取 `ssoId`） | `uai.unipus.cn` |
//! | [`bookshelf`] | 门户书架 | `uai.unipus.cn` |
//! | [`course_list`] | 班级真实进度 | `uai.unipus.cn` |
//! | [`active_flag`] | 教材激活标记 | `uai.unipus.cn` |
//! | [`catalog`] | 课程目录 | `ucontent.unipus.cn` |
//! | [`content`] | 分组题目内容（加密） | `ucontent.unipus.cn` |
//! | [`answer`] | 标准答案（加密） | `ucontent.unipus.cn` |
//! | [`progress`] | 单元级 / 目录级进度 | `ucontent.unipus.cn` |
//! | [`group_state`] | 分组状态 + 作答快照 | `ucontent.unipus.cn` |
//! | [`submit`] | 提交作答（**唯一写接口**） | `ucontent.unipus.cn` |
//! | [`media`] | 录音上传 / 口语上报 | `ucontent.unipus.cn` |
//! | [`duration`] | 学习时长（WebSocket） | `ucontent.unipus.cn` |
//! | [`speech_eval`] | 语音评测（WebSocket，**只评分不提交**） | `speech.unipus.cn` |
//! | [`assessment`] | 考核方案 / 综合成绩（**要宿主头**） | `uai.unipus.cn` |
//! | [`targets`] | **必修筛选（组合层，非单一端点）** | — |
//!
//! ## 读写分开
//!
//! 只有 [`submit`]（以及 [`media`] 未接入的上传链）会写服务端。其余全部只读。
//! 把写操作集中在两个文件里，是为了「哪些代码会产生副作用」可以被一眼审完。
//!
//! ## 唯一的例外：[`targets`]
//!
//! 只有它不绑单一端点——因为「必修」这个信息**不在任何一个接口里**
//! （见 [`catalog`] 的模块文档），必须组合 [`progress`] 与 [`group_state`]。
//! 它只读，但仍然单独成文件，理由是「一个文件一职责」：
//! 职责是**筛选**，不是**调用**。

pub mod active_flag;
pub mod answer;
pub mod assessment;
pub mod bookshelf;
pub mod catalog;
pub mod class_required;
pub mod content;
pub mod course_list;
pub mod duration;
pub mod group_state;
pub mod login;
pub mod media;
pub mod progress;
pub mod speech_eval;
pub mod submit;
pub mod targets;
pub mod user_info;
