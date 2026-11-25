//! In-process pub/sub feeding the SSE streams (a port of `bus.ts`): one channel per session
//! with subscribers, plus the global channel for lists.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hubness_core::{GlobalStreamMsg, SessionStreamMsg};
use tokio::sync::broadcast;

/// Messages a slow subscriber may lag behind before it starts losing them. The session
/// stream's keep-alive carries the current session, so a lost status change heals itself.
const CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct Bus {
    sessions: Arc<Mutex<HashMap<String, broadcast::Sender<SessionStreamMsg>>>>,
    global: broadcast::Sender<GlobalStreamMsg>,
}

impl Default for Bus {
    fn default() -> Self {
        Self { sessions: Arc::default(), global: broadcast::channel(CAPACITY).0 }
    }
}

impl Bus {
    pub fn on_session(&self, session_id: &str) -> broadcast::Receiver<SessionStreamMsg> {
        let mut map = self.sessions.lock().expect("bus lock");
        // drop channels nobody listens to any more
        map.retain(|_, tx| tx.receiver_count() > 0);
        map.entry(session_id.to_string()).or_insert_with(|| broadcast::channel(CAPACITY).0).subscribe()
    }

    pub fn emit_session(&self, session_id: &str, msg: SessionStreamMsg) {
        if let Some(tx) = self.sessions.lock().expect("bus lock").get(session_id) {
            let _ = tx.send(msg);
        }
    }

    pub fn on_global(&self) -> broadcast::Receiver<GlobalStreamMsg> {
        self.global.subscribe()
    }

    pub fn emit_global(&self, msg: GlobalStreamMsg) {
        let _ = self.global.send(msg);
    }
}
