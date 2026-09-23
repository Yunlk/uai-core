//! 统一错误类型。
//!
//! ## 为什么不是 `Result<T, String>`
//!
//! 旧工程全程用 `Result<T, String>`。搬运时如果直接换成强类型错误，
//! 两千多行里的每一处 `?` 都要重新过一遍，风险远大于收益。
//!
//! 折中做法：本类型**可以从 `String` 隐式构造**（见 `From<String>`），
//! 所以 `?` 在 `Result<T, String>` 与 `Result<T, Error>` 之间照常工作，
//! 搬运是机械的；同时 `kind()` 让 CLI 能按错误类别决定退出码，
//! 而不是把一切都糊成一句话。

use std::fmt;

/// 错误类别。CLI 用它决定退出码与是否重试。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// 网络层失败（连不上、超时、TLS）。
    Network,
    /// 服务端返回了业务错误码。
    Api,
    /// 解密失败（密文格式、密钥长度、UTF-8）。
    Decrypt,
    /// 响应结构不符合预期。
    Parse,
    /// 用户主动停止。
    Cancelled,
    /// 调用方参数有误。
    Invalid,
}

impl ErrorKind {
    /// 对应的进程退出码。
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Network => 2,
            Self::Api => 3,
            Self::Decrypt => 4,
            Self::Parse => 5,
            Self::Cancelled => 130,
            Self::Invalid => 64,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Network => "网络错误",
            Self::Api => "接口错误",
            Self::Decrypt => "解密错误",
            Self::Parse => "解析错误",
            Self::Cancelled => "已取消",
            Self::Invalid => "参数错误",
        }
    }
}

/// 库内统一错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    pub fn api(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Api, message)
    }

    pub fn decrypt(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Decrypt, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Cancelled, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// 是否值得重试（网络抖动、限流）。
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::Network)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for Error {}

/// 旧代码到处 `return Err("…".to_owned())`，靠这条 `From` 让 `?` 继续可用。
impl From<String> for Error {
    fn from(message: String) -> Self {
        // 搬运过来的字符串没有类别信息，按内容粗判一下，
        // 让 CLI 至少能把「解密失败」和「网络失败」分开。
        let kind = if message.contains("解密") || message.contains("密文") {
            ErrorKind::Decrypt
        } else if message.contains("解析") {
            ErrorKind::Parse
        } else if message.contains("请求失败") || message.contains("超时") {
            ErrorKind::Network
        } else if message.contains("已取消") {
            ErrorKind::Cancelled
        } else {
            ErrorKind::Api
        };
        Self::new(kind, message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() || error.is_connect() || error.is_request() {
            ErrorKind::Network
        } else {
            ErrorKind::Api
        };
        Self::new(kind, format!("HTTP 请求失败：{error}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::parse(format!("JSON 解析失败：{error}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
