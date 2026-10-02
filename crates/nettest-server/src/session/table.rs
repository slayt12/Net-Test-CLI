//! Live table of connected clients, rendered by the TUI and used for idle sweeps.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use nettest_proto::transport::Protocol;

pub type SessionId = u64;

#[derive(Debug, Clone)]
pub struct SessionStats {
    pub id: SessionId,
    pub peer: SocketAddr,
    pub protocol: Protocol,
    pub client_id: String,
    pub mode: String,
    pub connected_at: Instant,
    pub last_seen: Instant,
    pub frames_in: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub probes: u64,
    pub authed: bool,
}

#[derive(Default, Clone)]
pub struct SessionTable {
    inner: Arc<RwLock<HashMap<SessionId, SessionStats>>>,
    next_id: Arc<AtomicU64>,
    total_accepted: Arc<AtomicU64>,
}

impl SessionTable {
    pub fn open(&self, peer: SocketAddr, protocol: Protocol) -> SessionId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.total_accepted.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let stats = SessionStats {
            id,
            peer,
            protocol,
            client_id: String::new(),
            mode: String::new(),
            connected_at: now,
            last_seen: now,
            frames_in: 0,
            bytes_in: 0,
            bytes_out: 0,
            probes: 0,
            authed: false,
        };
        if let Ok(mut m) = self.inner.write() {
            m.insert(id, stats);
        }
        id
    }

    pub fn close(&self, id: SessionId) -> Option<SessionStats> {
        self.inner.write().ok()?.remove(&id)
    }

    pub fn update(&self, id: SessionId, f: impl FnOnce(&mut SessionStats)) {
        if let Ok(mut m) = self.inner.write()
            && let Some(s) = m.get_mut(&id)
        {
            f(s);
        }
    }

    pub fn snapshot(&self) -> Vec<SessionStats> {
        let mut v: Vec<SessionStats> = self
            .inner
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.id);
        v
    }

    pub fn len(&self) -> usize {
        self.inner.read().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn total_accepted(&self) -> u64 {
        self.total_accepted.load(Ordering::Relaxed)
    }
}
