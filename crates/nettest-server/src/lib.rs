//! nettest-server library: everything except argument parsing and the terminal.
//!
//! Exposed as a library so integration tests can spin up a real server in-process on port 0.
//! Invariants:
//! - Every listener funnels frames through `session::echo`, so protocol behaviour is identical on
//!   TCP, WebSocket and UDP.
//! - The server never consults its own clock for latency; it echoes `client_send_ns` untouched.

pub mod listeners;
pub mod log;
pub mod session;

use std::net::SocketAddr;
use std::sync::Arc;

use nettest_proto::config::ServerConfig;
use nettest_proto::tls::fingerprint::Fingerprint;
use nettest_proto::transport::Protocol;
use tokio_util::sync::CancellationToken;

use log::ServerLog;
use session::SessionTable;

/// Shared state handed to every connection task.
pub struct ServerCtx {
    pub config: ServerConfig,
    pub sessions: SessionTable,
    pub log: ServerLog,
    pub cancel: CancellationToken,
    pub tls: Option<Arc<rustls::ServerConfig>>,
}

pub struct RunningServer {
    pub ctx: Arc<ServerCtx>,
    pub listeners: Vec<(Protocol, SocketAddr)>,
    pub fingerprint: Option<Fingerprint>,
    pub cert_path: Option<std::path::PathBuf>,
}

impl RunningServer {
    pub fn addr(&self, p: Protocol) -> Option<SocketAddr> {
        self.listeners
            .iter()
            .find(|(q, _)| *q == p)
            .map(|(_, a)| *a)
    }

    pub fn shutdown(&self) {
        self.ctx.cancel.cancel();
    }
}

/// Bind all configured listeners and spawn their accept loops.
pub async fn start(config: ServerConfig, log: ServerLog) -> anyhow::Result<RunningServer> {
    let cancel = CancellationToken::new();

    let (tls, fingerprint, cert_path) = if config.wss_port != 0 {
        let dir = if config.cert_dir.is_empty() {
            nettest_proto::config::server_cert_dir()
        } else {
            std::path::PathBuf::from(&config.cert_dir)
        };
        let host = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .filter(|h| !h.is_empty())
            .into_iter()
            .collect::<Vec<_>>();
        let id = nettest_proto::tls::server::load_or_generate(&dir, &host)?;
        if id.generated {
            log.info(format!(
                "generated self-signed certificate at {}",
                id.cert_path.display()
            ));
        }
        (Some(id.config), Some(id.fingerprint), Some(id.cert_path))
    } else {
        (None, None, None)
    };

    let ctx = Arc::new(ServerCtx {
        sessions: SessionTable::default(),
        log,
        cancel,
        tls,
        config,
    });

    let listeners = listeners::spawn_all(ctx.clone()).await?;
    Ok(RunningServer {
        ctx,
        listeners,
        fingerprint,
        cert_path,
    })
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
