use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use teloxide::error_handlers::ErrorHandler;
use teloxide::RequestError;

pub const RECONNECT_INITIAL_S: f64 = 2.0;
pub const RECONNECT_MAX_S: f64 = 30.0;
pub const RECONNECT_FACTOR: f64 = 1.8;
pub const CONFLICT_RETRY_S: f64 = 10.0;
pub const NETWORK_RETRY_BASE_S: f64 = 5.0;
pub const NETWORK_RETRY_MAX_S: f64 = 60.0;

#[derive(Default, Clone)]
pub struct ReconnectCounters {
    pub conflict_count: u32,
    pub network_count: u32,
}

pub fn looks_like_polling_conflict(err: &str) -> bool {
    let t = err.to_lowercase();
    t.contains("conflict")
        || t.contains("terminated by other getupdates request")
        || t.contains("another bot instance is running")
}

pub fn looks_like_network_error(err: &str) -> bool {
    let t = err.to_lowercase();
    t.contains("network")
        || t.contains("timed out")
        || t.contains("timeout")
        || t.contains("connection")
        || t.contains("dns")
}

pub fn plan_reconnect_delay(counters: &mut ReconnectCounters, err: &str) -> f64 {
    if looks_like_polling_conflict(err) {
        counters.conflict_count += 1;
        counters.network_count = 0;
        return CONFLICT_RETRY_S;
    }
    if looks_like_network_error(err) {
        counters.network_count += 1;
        counters.conflict_count = 0;
        let attempt = counters.network_count;
        return (NETWORK_RETRY_BASE_S * 2f64.powi(attempt as i32 - 1)).min(NETWORK_RETRY_MAX_S);
    }
    RECONNECT_INITIAL_S
}

pub fn reset_reconnect_counters(counters: &mut ReconnectCounters) {
    counters.conflict_count = 0;
    counters.network_count = 0;
}

/// Shared state updated by the teloxide update-listener error handler.
#[derive(Clone)]
pub struct PollingReconnectState {
    pub counters: Arc<Mutex<ReconnectCounters>>,
    pub last_error: Arc<Mutex<Option<String>>>,
}

impl PollingReconnectState {
    pub fn new(counters: ReconnectCounters) -> Self {
        Self {
            counters: Arc::new(Mutex::new(counters)),
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn next_delay(&self, fallback: f64) -> f64 {
        let mut c = self.counters.lock().unwrap();
        if let Some(err) = self.last_error.lock().unwrap().take() {
            plan_reconnect_delay(&mut c, &err)
        } else {
            fallback
        }
    }

    pub fn snapshot_counters(&self) -> ReconnectCounters {
        self.counters.lock().unwrap().clone()
    }
}

pub struct ListenerErrorHandler {
    state: PollingReconnectState,
}

impl ListenerErrorHandler {
    pub fn new(state: PollingReconnectState) -> Self {
        Self { state }
    }
}

impl ErrorHandler<RequestError> for ListenerErrorHandler {
    fn handle_error(
        self: Arc<Self>,
        error: RequestError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let msg = error.to_string();
        {
            let mut c = self.state.counters.lock().unwrap();
            let delay = plan_reconnect_delay(&mut c, &msg);
            tracing::warn!("telegram polling error (retry ~{delay:.1}s): {msg}");
        }
        *self.state.last_error.lock().unwrap() = Some(msg);
        Box::pin(async {})
    }
}
