//! Shared state of the runtime HTTP API (the module globals of `app/runtime/main.py`: `subscribers`, `rt`, `sched`
//! — plus `rt.golden`, which lives here because the Rust `Runtime` has no golden field).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;
use omuse_common::util::now_ts;
use omuse_common::Config;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent::{Publish, Runtime};
use crate::golden::Golden;
use crate::rt_api::RuntimeApi;
use crate::scheduler::Scheduler;

/// Capacity of one SSE subscriber's queue (`asyncio.Queue(maxsize=500)`).
pub const QUEUE_MAX: usize = 500;

/// The live-event fan-out (`subscribers: set[asyncio.Queue]`).
#[derive(Default)]
pub struct Hub {
    next: AtomicU64,
    subs: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
}

impl Hub {
    /// Register a subscriber: its id (for [`Hub::unsubscribe`]) and its receiving end.
    pub fn subscribe(&self) -> (u64, mpsc::Receiver<Value>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(QUEUE_MAX);
        self.subs.lock().insert(id, tx);
        (id, rx)
    }

    /// `subscribers.discard(q)`.
    pub fn unsubscribe(&self, id: u64) {
        self.subs.lock().remove(&id);
    }

    /// Number of live subscribers.
    pub fn len(&self) -> usize {
        self.subs.lock().len()
    }

    /// Whether nobody is listening.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `publish(ev)`: stamp `ts` when missing and put the event on every queue; a full queue drops it
    /// (`asyncio.QueueFull`), a closed one is forgotten.
    pub fn publish(&self, mut ev: Value) {
        if let Some(o) = ev.as_object_mut() {
            if !o.contains_key("ts") {
                o.insert("ts".into(), json!(now_ts()));
            }
        }
        let mut subs = self.subs.lock();
        subs.retain(|_, tx| match tx.try_send(ev.clone()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }
}

/// Everything a handler needs.
pub struct State {
    pub cfg: Arc<Config>,
    pub hub: Arc<Hub>,
    pub rt: Arc<Runtime>,
    pub sched: Arc<Scheduler>,
    pub golden: Arc<Golden>,
}

impl State {
    /// Build the runtime, scheduler and golden harness (nothing started yet; see `routes::serve`).
    pub fn new(cfg: Arc<Config>) -> anyhow::Result<Arc<State>> {
        let hub = Arc::new(Hub::default());
        let h = hub.clone();
        let publish: Publish = Arc::new(move |ev: Value| -> BoxFuture<'static, ()> {
            h.publish(ev);
            Box::pin(async {})
        });
        let data = cfg.runtime_data();
        std::fs::create_dir_all(&data)?;
        let rt = Runtime::new(&data, cfg.clone(), publish)?;
        let api: Arc<dyn RuntimeApi> = rt.clone();
        let sched = Arc::new(Scheduler::new(api.clone()));
        let golden = Arc::new(Golden::new(api)?);
        Ok(Arc::new(State {
            cfg,
            hub,
            rt,
            sched,
            golden,
        }))
    }

    /// `await publish(ev)`.
    pub fn publish(&self, ev: Value) {
        self.hub.publish(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_drops_on_full_and_forgets_closed() {
        let hub = Hub::default();
        let (_a, mut ra) = hub.subscribe();
        let (_b, rb) = hub.subscribe();
        drop(rb);
        for i in 0..(QUEUE_MAX + 5) {
            hub.publish(json!({"kind": "x", "i": i}));
        }
        assert_eq!(hub.len(), 1);
        let first = ra.try_recv().unwrap();
        assert_eq!(first["i"], 0);
        assert!(first["ts"].is_f64());
        let mut n = 1;
        while ra.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, QUEUE_MAX);
        hub.publish(json!({"kind": "y", "ts": 1}));
        assert_eq!(ra.try_recv().unwrap()["ts"], 1);
    }
}
