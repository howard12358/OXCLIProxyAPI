use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::info;

/// 对齐 CPA usage queue 语义的消息体。
///
/// Rust 当前先保证 payload 形状一致；底层仍由本地异步 producer 发射，
/// 还没有在 Rust 进程内完整复刻 CPA 的 redis 协议面。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageQueuePayload {
    pub timestamp: String,
    pub latency_ms: u64,
    pub ttft_ms: u64,
    pub source: String,
    pub auth_index: String,
    pub tokens: UsageQueueTokens,
    pub failed: bool,
    pub fail: UsageQueueFail,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub response_headers: BTreeMap<String, Vec<String>>,
    pub provider: String,
    pub executor_type: String,
    pub model: String,
    pub alias: String,
    pub endpoint: String,
    pub auth_type: String,
    pub api_key: String,
    pub request_id: String,
    pub reasoning_effort: String,
    pub service_tier: String,
}

/// usage payload 中的 token 统计集合。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageQueueTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

/// usage payload 中的失败摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageQueueFail {
    pub status_code: u16,
    pub body: String,
}

/// 异步 usage 事件生产器。
///
/// 主请求链路只做非阻塞 try-send，保证使用量事件失败不会拖慢数据面响应。
#[derive(Clone)]
pub struct UsageEventProducer {
    tx: mpsc::Sender<UsageQueuePayload>,
}

impl UsageEventProducer {
    /// 最小生产闭环：写入内存队列，再由后台任务以结构化日志吐出。
    pub fn new_log(buffer: usize) -> Self {
        let (tx, mut rx) = mpsc::channel(buffer.max(1));
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Ok(payload) = serde_json::to_string(&event) {
                    info!(target: "cliproxy_usage_queue", usage_queue = %payload);
                }
            }
        });
        Self { tx }
    }

    /// 空消费模式，保留调用路径但直接丢弃 usage 事件。
    pub fn new_noop(buffer: usize) -> Self {
        let (tx, mut rx) = mpsc::channel(buffer.max(1));
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        Self { tx }
    }

    /// 非阻塞尝试投递 usage 事件。
    pub fn try_emit(&self, event: UsageQueuePayload) -> bool {
        self.tx.try_send(event).is_ok()
    }
}

impl UsageEventProducer {
    /// 测试辅助入口：直接暴露接收端，便于断言 payload 内容。
    pub fn new_channel(buffer: usize) -> (Self, mpsc::Receiver<UsageQueuePayload>) {
        let (tx, rx) = mpsc::channel(buffer.max(1));
        (Self { tx }, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn producer_should_emit_payload_into_test_channel() {
        let (producer, mut rx) = UsageEventProducer::new_channel(4);
        assert!(producer.try_emit(UsageQueuePayload {
            timestamp: "2026-06-26T00:00:00Z".to_string(),
            latency_ms: 120,
            ttft_ms: 50,
            source: String::new(),
            auth_index: "auth-1".to_string(),
            tokens: UsageQueueTokens {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                ..UsageQueueTokens::default()
            },
            failed: false,
            fail: UsageQueueFail {
                status_code: 200,
                body: String::new(),
            },
            response_headers: BTreeMap::from([(
                "x-request-id".to_string(),
                vec!["req-1".to_string()],
            )]),
            provider: "codex".to_string(),
            executor_type: "RustResponsesExecutor".to_string(),
            model: "gpt-5-codex".to_string(),
            alias: "codex-latest".to_string(),
            endpoint: "POST /v1/responses".to_string(),
            auth_type: "oauth".to_string(),
            api_key: String::new(),
            request_id: "req-1".to_string(),
            reasoning_effort: String::new(),
            service_tier: "default".to_string(),
        }));
        let event = rx.recv().await.expect("usage payload");
        assert_eq!(event.request_id, "req-1");
        assert_eq!(event.tokens.total_tokens, 30);
    }
}
