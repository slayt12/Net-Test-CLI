//! Raw TCP accept loop. The first bytes decide: nettest magic -> frame handler (with those bytes
//! replayed), anything else or silence -> identification banner.

use std::net::SocketAddr;
use std::sync::Arc;

use nettest_proto::transport::tcp::TcpTransport;
use nettest_proto::transport::{AnyTransport, Protocol};
use tokio::net::TcpListener;

use crate::ServerCtx;
use crate::listeners::banner::{BANNER_WAIT, FirstBytes, classify, looks_like_http, reply_banner};
use crate::session::echo::handle_stream;

pub async fn spawn(ctx: Arc<ServerCtx>, addr: SocketAddr) -> anyhow::Result<Option<SocketAddr>> {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) if super::tolerable_bind_error(&addr, &e) => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("bind tcp {addr}: {e}")),
    };
    let local = listener.local_addr()?;
    ctx.log.info(format!("listening tcp://{local}"));
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                r = listener.accept() => r,
            };
            match accepted {
                Ok((mut stream, peer)) => {
                    let ctx2 = ctx.clone();
                    tokio::spawn(async move {
                        match classify(&mut stream, BANNER_WAIT).await {
                            Ok(FirstBytes::Nettest(prefix)) => {
                                handle_stream(
                                    AnyTransport::Tcp(TcpTransport::with_prefix(stream, &prefix)),
                                    ctx2,
                                    Protocol::Tcp,
                                )
                                .await;
                            }
                            Ok(other) => {
                                let http = looks_like_http(other.bytes());
                                let _ = reply_banner(&mut stream, &ctx2.config, http).await;
                                ctx2.log.info(format!("[scan] tcp {peer}: banner sent"));
                            }
                            Err(e) => ctx2.log.warn(format!("tcp {peer}: {e}")),
                        }
                    });
                }
                Err(e) => {
                    ctx.log.warn(format!("tcp accept error: {e}"));
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(Some(local))
}
