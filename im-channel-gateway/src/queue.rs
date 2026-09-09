use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

pub type QueueKey = (String, String, i32);

type ConsumerTask = Arc<
    dyn Fn(QueueKey, mpsc::Receiver<serde_json::Value>) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
struct QueueState {
    sender: mpsc::Sender<serde_json::Value>,
}

pub struct UnifiedQueue {
    queues: Arc<Mutex<HashMap<QueueKey, QueueState>>>,
    capacity: usize,
}

impl UnifiedQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            capacity,
        }
    }

    pub async fn enqueue(
        &self,
        channel_id: &str,
        session_id: &str,
        priority: i32,
        payload: serde_json::Value,
        consumer: ConsumerTask,
    ) {
        let key = (channel_id.to_string(), session_id.to_string(), priority);
        let mut guard = self.queues.lock().await;

        if !guard.contains_key(&key) {
            let (tx, rx) = mpsc::channel(self.capacity);
            let task = consumer.clone();
            let k = key.clone();
            tokio::spawn(async move {
                task(k, rx).await;
            });
            guard.insert(key.clone(), QueueState { sender: tx });
        }

        if let Some(state) = guard.get(&key) {
            let _ = state.sender.send(payload).await;
        }
    }
}

pub const PRIORITY_NORMAL: i32 = 20;

pub fn priority_for_query(query: &str) -> i32 {
    let q = query.trim().to_lowercase();
    if q == "/stop" {
        return 0;
    }
    if q.starts_with("/daemon ")
        || q == "/status"
        || q == "/restart"
        || q.starts_with("/approve")
        || q.starts_with("/deny")
        || q.starts_with("/answer")
    {
        return 10;
    }
    PRIORITY_NORMAL
}

pub fn session_key(channel: &str, sender_id: &str) -> String {
    format!("{channel}:{sender_id}")
}

/// Build a queue consumer that type-erases the async handler future.
pub fn make_consumer<F>(f: F) -> ConsumerTask
where
    F: Fn(QueueKey, mpsc::Receiver<serde_json::Value>) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_response_commands_use_high_priority() {
        assert_eq!(priority_for_query("/approve req_1"), 10);
        assert_eq!(priority_for_query("/deny req_1"), 10);
        assert_eq!(priority_for_query("/answer req_1 yes"), 10);
    }
}
