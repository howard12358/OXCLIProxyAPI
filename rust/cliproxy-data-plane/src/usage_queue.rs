use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use cliproxy_usage_events::UsageQueuePayload;
use serde_json::Value;
use tokio::sync::mpsc;

const USAGE_SUPPORT_REFRESH_PAYLOAD: &str = r#"{"support_refresh":true}"#;
const USAGE_REFRESH_PAYLOAD: &str = r#"{"refresh":true}"#;
const SUBSCRIBER_BUFFER: usize = 256;

/// RS 侧对齐 CPA redisqueue 语义的进程内 usage 队列。
///
/// CPA 的规则是“有订阅者则直接广播，不再写入内存队列”；这里保持相同行为，
/// 让 Keeper 的 SUBSCRIBE 路径和 HTTP/LPOP 兜底路径不会重复消费同一条记录。
#[derive(Clone)]
pub struct UsageQueue {
    inner: Arc<Mutex<UsageQueueInner>>,
    next_subscriber_id: Arc<AtomicU64>,
}

#[derive(Default)]
struct UsageQueueInner {
    items: VecDeque<Vec<u8>>,
    subscribers: HashMap<u64, mpsc::Sender<Vec<u8>>>,
    error_subscribers: HashMap<u64, mpsc::Sender<Vec<u8>>>,
}

impl UsageQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(UsageQueueInner::default())),
            next_subscriber_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// 写入 CPA-shaped usage payload；序列化失败时直接丢弃，避免影响主请求链路。
    pub fn enqueue(&self, payload: UsageQueuePayload) {
        let Ok(raw) = serde_json::to_vec(&payload) else {
            return;
        };
        self.enqueue_raw(raw);
    }

    pub fn enqueue_raw(&self, payload: Vec<u8>) {
        if payload.is_empty() {
            return;
        }

        let mut inner = self.inner.lock().expect("usage queue lock poisoned");
        if !inner.subscribers.is_empty() {
            inner
                .subscribers
                .retain(|_, subscriber| subscriber.try_send(payload.clone()).is_ok());
            return;
        }
        inner.items.push_back(payload);
    }

    /// 与 CPA `PopOldest` 对齐：按 FIFO 弹出，弹出即消费。
    pub fn pop_oldest(&self, count: usize) -> Vec<Vec<u8>> {
        if count == 0 {
            return Vec::new();
        }

        let mut inner = self.inner.lock().expect("usage queue lock poisoned");
        let take = count.min(inner.items.len());
        (0..take).filter_map(|_| inner.items.pop_front()).collect()
    }

    pub fn pop_oldest_json(&self, count: usize) -> Vec<Value> {
        self.pop_oldest(count)
            .into_iter()
            .map(|item| {
                serde_json::from_slice(&item)
                    .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&item).into_owned()))
            })
            .collect()
    }

    pub fn subscribe_usage(&self) -> UsageSubscription {
        self.subscribe(Some(USAGE_SUPPORT_REFRESH_PAYLOAD.as_bytes().to_vec()))
    }

    pub fn subscribe_errors(&self) -> UsageSubscription {
        self.subscribe_channel(None, QueueChannel::Errors)
    }

    pub fn notify_refresh(&self) {
        self.publish_raw(
            USAGE_REFRESH_PAYLOAD.as_bytes().to_vec(),
            QueueChannel::Usage,
        );
    }

    fn subscribe(&self, initial_payload: Option<Vec<u8>>) -> UsageSubscription {
        self.subscribe_channel(initial_payload, QueueChannel::Usage)
    }

    fn subscribe_channel(
        &self,
        initial_payload: Option<Vec<u8>>,
        channel: QueueChannel,
    ) -> UsageSubscription {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);
        if let Some(payload) = initial_payload {
            let _ = tx.try_send(payload);
        }
        let id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.inner.lock().expect("usage queue lock poisoned");
        match channel {
            QueueChannel::Usage => {
                inner.subscribers.insert(id, tx);
            }
            QueueChannel::Errors => {
                inner.error_subscribers.insert(id, tx);
            }
        }
        UsageSubscription {
            queue: self.clone(),
            id,
            channel,
            rx,
        }
    }

    fn publish_raw(&self, payload: Vec<u8>, channel: QueueChannel) {
        let mut inner = self.inner.lock().expect("usage queue lock poisoned");
        let subscribers = match channel {
            QueueChannel::Usage => &mut inner.subscribers,
            QueueChannel::Errors => &mut inner.error_subscribers,
        };
        subscribers.retain(|_, subscriber| subscriber.try_send(payload.clone()).is_ok());
    }

    fn unsubscribe(&self, id: u64, channel: QueueChannel) {
        let mut inner = self.inner.lock().expect("usage queue lock poisoned");
        match channel {
            QueueChannel::Usage => {
                inner.subscribers.remove(&id);
            }
            QueueChannel::Errors => {
                inner.error_subscribers.remove(&id);
            }
        }
    }
}

impl Default for UsageQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个 usage 订阅者句柄，Drop 时自动解除订阅。
pub struct UsageSubscription {
    queue: UsageQueue,
    id: u64,
    channel: QueueChannel,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl UsageSubscription {
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }
}

impl Drop for UsageSubscription {
    fn drop(&mut self) {
        self.queue.unsubscribe(self.id, self.channel);
    }
}

#[derive(Debug, Clone, Copy)]
enum QueueChannel {
    Usage,
    Errors,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_usage_receives_support_refresh_first() {
        let queue = UsageQueue::new();
        let mut subscription = queue.subscribe_usage();

        let payload = subscription.recv().await.expect("initial payload");

        assert_eq!(payload, br#"{"support_refresh":true}"#);
    }

    #[tokio::test]
    async fn enqueue_broadcasts_to_subscribers_without_buffering() {
        let queue = UsageQueue::new();
        let mut subscription = queue.subscribe_usage();
        let _ = subscription.recv().await.expect("initial payload");

        queue.enqueue_raw(br#"{"request_id":"req-1"}"#.to_vec());
        let payload = subscription.recv().await.expect("usage payload");

        assert_eq!(payload, br#"{"request_id":"req-1"}"#);
        assert!(queue.pop_oldest(1).is_empty());
    }

    #[test]
    fn pop_oldest_consumes_fifo_records() {
        let queue = UsageQueue::new();
        queue.enqueue_raw(br#"{"request_id":"req-1"}"#.to_vec());
        queue.enqueue_raw(br#"{"request_id":"req-2"}"#.to_vec());

        let first = queue.pop_oldest_json(1);
        let second = queue.pop_oldest_json(10);

        assert_eq!(first[0]["request_id"], "req-1");
        assert_eq!(second[0]["request_id"], "req-2");
        assert!(queue.pop_oldest(1).is_empty());
    }
}
