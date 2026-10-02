//! WebSocket accept loop, plain or TLS. TLS is terminated by tokio-rustls, the request head is
//! peeked (a real upgrade goes to tungstenite with the bytes replayed; a plain GET, a scanner or
//! silence gets the identification banner), then the stream is handed to tungstenite.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nettest_proto::transport::ws::{MaybeTls, WsTransport};
use nettest_proto::transport::{AnyTransport, Protocol, Rewind};
use tokio::net::TcpListener;

use crate::ServerCtx;
use crate::listeners::banner::{BANNER_WAIT, FirstBytes, classify, looks_like_http, reply_banner};
use crate::session::echo::handle_stream;

enum Accepted {
    Client(Box<nettest_proto::transport::ws::WsStream>),
    Scanner,
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn spawn(
    ctx: Arc<ServerCtx>,
    addr: SocketAddr,
    tls: bool,
) -> anyhow::Result<Option<SocketAddr>> {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) if super::tolerable_bind_error(&addr, &e) => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "bind {} {addr}: {e}",
                if tls { "wss" } else { "ws" }
            ));
        }
    };
    let local = listener.local_addr()?;
    let proto = if tls { Protocol::Wss } else { Protocol::Ws };
    ctx.log.info(format!("listening {proto}://{local}"));
    let acceptor = ctx.tls.clone().map(tokio_rustls::TlsAcceptor::from);

    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                r = listener.accept() => r,
            };
            let (stream, peer) = match accepted {
                Ok(x) => x,
                Err(e) => {
                    ctx.log.warn(format!("{proto} accept error: {e}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let ctx2 = ctx.clone();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let ctx3 = ctx2.clone();
                let handshake = async move {
                    let mut stream = match (tls, acceptor) {
                        (true, Some(acc)) => match acc.accept(stream).await {
                            Ok(s) => MaybeTls::ServerTls(Box::new(s)),
                            Err(e) => return Err(format!("tls handshake with {peer}: {e}")),
                        },
                        _ => MaybeTls::Plain(stream),
                    };
                    let prefix = match classify(&mut stream, BANNER_WAIT).await {
                        Ok(FirstBytes::WsUpgrade(prefix)) => prefix,
                        Ok(other) => {
                            let http = looks_like_http(other.bytes());
                            let _ = reply_banner(&mut stream, &ctx3.config, http).await;
                            return Ok(Accepted::Scanner);
                        }
                        Err(e) => return Err(format!("{proto} {peer}: {e}")),
                    };
                    tokio_tungstenite::accept_async(Rewind::new(stream, prefix))
                        .await
                        .map(|ws| Accepted::Client(Box::new(ws)))
                        .map_err(|e| format!("websocket upgrade with {peer}: {e}"))
                };
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
                    Ok(Ok(Accepted::Client(ws))) => {
                        handle_stream(AnyTransport::Ws(WsTransport::new(*ws)), ctx2, proto).await
                    }
                    Ok(Ok(Accepted::Scanner)) => {
                        ctx2.log.info(format!("[scan] {proto} {peer}: banner sent"))
                    }
                    Ok(Err(e)) => ctx2.log.warn(e),
                    Err(_) => ctx2
                        .log
                        .warn(format!("{proto} handshake with {peer} timed out")),
                }
            });
        }
    });
    Ok(Some(local))
}
