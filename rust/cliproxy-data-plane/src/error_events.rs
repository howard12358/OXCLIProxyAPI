use serde::{Deserialize, Serialize};

/// `errors` 通道输出的最小错误事件作用域。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorScope {
    Auth,
    Model,
}

/// Rust 数据平面 errors 通道的最小 payload。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEvent {
    pub timestamp: String,
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub auth_index: String,
    pub scope: ErrorScope,
    pub status_code: u16,
    pub error_code: String,
    pub message: String,
    pub retry_after_ms: u64,
    pub cooldown_until: String,
    pub quota_exceeded: bool,
    pub reason: String,
}
