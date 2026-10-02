# nettest – notes for agents and maintainers

See README.md for usage. This file records non-obvious facts verified during development so
nobody has to rediscover them.

## Known Gotchas

- **rustls crypto provider must be `ring`.** rustls 0.23, tokio-rustls 0.26 and rcgen 0.14
  default to `aws-lc-rs`, which needs cmake + nasm and does not cross-compile cleanly with
  mingw. All three are declared with `default-features = false, features = ["ring", ...]` in the
  workspace `Cargo.toml`. Build every `ClientConfig`/`ServerConfig` through
  `builder_with_provider(nettest_proto::tls::provider())`; a bare `builder()` panics if two
  providers end up in the binary. (Verified against rustls-0.23.43 source.)
- **Do not enable tokio-tungstenite's `rustls-tls-*` features.** TLS is done with tokio-rustls
  first and the `TlsStream` is handed to `client_async` / `accept_async`. This keeps one crypto
  provider and lets `dial()` time TCP, TLS and the WS upgrade separately. tungstenite 0.30
  `Message::Binary` holds `bytes::Bytes`.
- **ratatui 0.30 pairs with crossterm 0.29.** The direct `crossterm` dependency exists only to
  enable the `event-stream` feature for async input; versions unify with ratatui's re-export.
  Filter key events to `KeyEventKind::Press`; Windows also delivers Release events.
- **hdrhistogram needs `default-features = false`** or it pulls flate2/nom/base64.
- **rustls-pki-types PEM parsing** is `rustls_pki_types::pem::PemObject::from_pem_slice` (trait
  must be in scope), no extra crate.
- **Windows cross build** needs only `.cargo/config.toml` naming `x86_64-w64-mingw32-gcc` as the
  linker; Rust's windows-gnu target bundles its own CRT objects. Resulting binaries import only
  Windows system DLLs (checked with `x86_64-w64-mingw32-objdump -p`).
- **Dual-stack binding:** `--bind any` binds `0.0.0.0` and `[::]` separately. On Linux the v6
  bind may fail with EADDRINUSE after v4 succeeded (v4-mapped dual-stack); that is tolerated,
  see `listeners::tolerable_bind_error`.
- **UDP throughput** must yield between datagrams or the sender floods the local socket buffer
  and the kernel drops packets locally, which would be misreported as path loss.
- **Integration tests** each need their own cert directory: they run concurrently in one
  process and would otherwise race on rcgen output (`tests/loopback.rs` uses a counter).
- **Loss semantics:** a probe is `Lost` after `max(2 s, 10 × interval)`; an echo arriving later
  flips it to `Late` (lost −1, late +1). Keep that symmetric if you change either path.

- **winresource build scripts** must check `CARGO_CFG_TARGET_OS` at runtime, not `cfg!`,
  because build.rs runs on the host. For the gnu cross target it shells out to
  `x86_64-w64-mingw32-windres` / `-ar` (prefix chosen from `TARGET`). The icon path in
  `build.rs` is relative to the crate directory. `xtask` is not in `default-members`, so
  `cargo build` / `cargo test` skip resvg.

- **Windows service/elevation bindings:** `windows-service 0.8` (depends on windows-sys 0.61)
  in `[target.'cfg(windows)'.dependencies]` only; it compiles under the mingw cross target. The
  windows-sys features needed: server `Win32_Foundation, Win32_Security, Win32_System_Threading,
  Win32_System_Registry` (SHELLEXECUTEINFOW has an HKEY field), `Win32_UI_Shell,
  Win32_UI_WindowsAndMessaging`; proto `Win32_NetworkManagement_IpHelper, Win32_Networking_WinSock,
  Win32_System_IO`. Only the `Status` field of `ICMP_ECHO_REPLY` is read (fixed offset) so the
  32/64-bit layout difference does not matter.
- **`main.rs` of the server is a sync `fn main`.** The SCM calls the service entry on its own
  thread and that thread must own the tokio runtime, so `#[tokio::main]` was removed; foreground
  runs build the runtime explicitly.
- **systemd `DynamicUser=yes` does not chown `ConfigurationDirectory=`**, so a root-only 0600
  config would be unreadable. The unit passes it as `LoadCredential=server.toml:/etc/nettest/
  server.toml` and starts with `--config %d/server.toml` (systemd >= 247).
- **Linux ping sockets (`SOCK_DGRAM`/`IPPROTO_ICMP`) rewrite the ICMP identifier**; replies are
  matched on the 64-bit sequence embedded in the echo data, never on the id. Raw sockets (root
  fallback) deliver the IPv4 header in front of the ICMP message; ping sockets and IPv6 do not.
  Destination-unreachable is not delivered without `IP_RECVERR`, so unreachable = timeout on
  Linux (Windows reports it as failed).
- **Banner classification happens before tungstenite sees the stream.** The peeked bytes are
  replayed through `transport::Rewind` (ws) or `TcpTransport::with_prefix` (tcp, via
  `FramedParts.read_buf`). `BANNER_WAIT` is 3 s because nmap's null probe waits 6 s and a real
  client sends Hello immediately. UDP banner replies are rate limited (`BannerLimiter`), valid
  frames from unknown peers stay silently dropped.
- **Serverless probes** spawn one future per sequence number (`JoinSet`); the per-probe timeout
  equals `LossPolicy::for_interval(interval).timeout` so the tracker's `expire()` and the probe
  future agree on "lost". A definitive negative becomes `SampleStatus::Failed` via
  `LatencyTracker::on_failed` (counted in `lost`, also in `failed`).

## Conventions

- Every non-trivial file starts with a `//!` header: purpose, why, invariants.
- Transports are an enum (`AnyTransport`), not a trait object, to avoid `async-trait`.
- The server never reads its own clock into frames; `client_send_ns` is echoed verbatim.
- Exit codes in `nettest-client/src/cli/exit.rs` are a public contract; do not renumber.
- Every listener must answer non-nettest traffic with the banner (`ServerConfig::banner_line`);
  a silent port is a regression. A service install must refuse an empty token.
- `Protocol::ALL` order is the TUI cycle order: nettest protocols first, then serverless probes.
- **Releases:** `releases/<version>/` holds the four shipped binaries plus `SHA256SUMS` and is
  the only build output committed (`.gitignore` excludes `target/`). To cut a release: bump
  `workspace.package.version` in `Cargo.toml`, rebuild both targets in release mode, refresh the
  directory and checksums, update the version strings in README.md and the admin guide, then tag
  `v<version>`. The admin guide HTML source is not in the repo (see memory notes).
