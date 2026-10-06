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
  equals `LossPolicy::resolve(interval, cfg.loss_timeout_ms).timeout` so the tracker's `expire()`
  and the probe future agree on "lost". A definitive negative becomes `SampleStatus::Failed` via
  `LatencyTracker::on_failed` (counted in `lost`, also in `failed`). `loss_timeout_ms = 0` keeps
  the `max(2 s, 10 × interval)` default; the monitor sets it to the target's `timeout`.

- **Shared service crate.** `crates/nettest-service` holds everything platform-generic about
  "run as a service": `elevate`, `Report`, `ServiceError`, `files` (copy / purge helpers),
  `systemd::render_unit(UnitSpec)`, `linux` systemctl wrappers and `windows` SCM helpers. The
  `define_windows_service!` macro only emits an `extern "system"` trampoline that calls a handler
  by name, so it is invoked **once**, in `nettest_service::windows`; a binary hands its async body
  in as a `ServiceBody` through `run_as_service(spec, config, body, hint)` and a static holds it
  until the SCM calls `service_main`. The server's unit text is pinned byte-for-byte by
  `systemd::tests::server_unit_is_byte_identical`.
- **One `LogsDirectory` per DynamicUser unit.** systemd re-chowns `LogsDirectory=`/`StateDirectory=`
  to the unit's throwaway UID on every start, so two `DynamicUser=yes` units must not share a
  name: the server uses `nettest`, the monitor `nettest-monitor`. Those directories are symlinks
  into `/var/{lib,log}/private/`; `std::fs::remove_dir_all` would only unlink the symlink, which
  is why `files::remove_dir_all_if_exists` also removes the private target.
- **Purge semantics.** `/etc/nettest` and `%ProgramData%\nettest` are shared by the server
  (`server.toml`, `certs/`, `server.log`) and the monitor (`monitor.toml`, `monitor.log`).
  `uninstall --purge` removes only the caller's files and the directory when it is then empty.
- **Monitor unit always grants `CAP_NET_RAW`.** Ping sockets usually work without it, but the
  raw-socket fallback needs it, and a `ping://` target added to `monitor.toml` later would
  otherwise be permanently DOWN with `ProbeError::Unsupported`.
- **webpki-roots** provides `TlsClientMode::WebPki` (cached `Arc<ClientConfig>` in
  `tls::client`) for the monitor's webhooks. It is the Mozilla bundle, not the OS store; the
  https *probe* still accepts any certificate by design. Loopback test
  `webpki_rejects_self_signed` guards against the modes being mixed up.
- **Serverless `Connected` is not a success.** `runner/probe.rs` emits `on_connected` right after
  DNS and socket setup, before any probe; the monitor supervisor (`signal_of`) ignores it for
  serverless protocols and relies on samples only. The supervisor restarts a serverless runner on
  a DOWN transition so DNS is re-resolved; nettest-protocol runners re-resolve on every `dial`.
- **Runner memory is bounded** via `Runner::set_history_limit` (events / connects kept for the
  report) and `SoakLog::with_capacity` (disconnect records; aggregates stay exact). The TUI and
  headless runs keep the unlimited default; the monitor uses 0.
- **nettest-client is lib + bin** (`src/lib.rs` re-exports every module) so
  `tests/monitor_loopback.rs` can drive the supervisor against an in-process server. It also
  made `main` a sync `fn` that builds the runtime itself (Windows SCM thread ownership).

- **monitor.toml encodings.** Windows PowerShell 5.1 writes `>` redirections as UTF-16 LE with a
  BOM and `Set-Content`/`Out-File -Encoding Default` as ANSI (Windows-1252); old Notepad saves
  ANSI too. `monitor::config::decode_text` accepts UTF-8 (BOM or not), UTF-16 LE/BE (BOM or
  BOM-less, detected by the NUL-every-second-byte pattern of ASCII text) and Windows-1252
  (fallback for invalid UTF-8, never fails), and rejects UTF-32. `Resolved::encoding` carries the
  result; a non-UTF-8 file becomes the first warning and `service install` rewrites the installed
  copy as UTF-8 even when `--from` is omitted.
- **Microsoft Teams webhooks** are Power Automate "When a Teams webhook request is received"
  triggers (Office 365 connectors are retired). Body: `{"type":"message","text":...,
  "attachments":[{"contentType":"application/vnd.microsoft.card.adaptive","content":{Adaptive
  Card}}]}`. Both fields are sent on purpose: the "Post to a channel when a webhook request is
  received" template posts each attachment and ignores `text`, the "Send webhook alerts to a
  channel" template reads `text`. The trigger answers `202 Accepted` (counted as delivered),
  limits a message to 28 KB and ~4 requests/s (429 after), and the `sig=` query parameter is the
  secret, so the URL must be sent verbatim with its query string. Sources:
  https://learn.microsoft.com/en-us/microsoftteams/platform/webhooks-and-connectors/how-to/add-incoming-webhook
  and https://learn.microsoft.com/en-us/connectors/teams/ ("Microsoft Teams - Webhook").

## Conventions

- Every non-trivial file starts with a `//!` header: purpose, why, invariants.
- Transports are an enum (`AnyTransport`), not a trait object, to avoid `async-trait`.
- The server never reads its own clock into frames; `client_send_ns` is echoed verbatim.
- Exit codes in `nettest-client/src/cli/exit.rs` are a public contract; do not renumber.
- Every listener must answer non-nettest traffic with the banner (`ServerConfig::banner_line`);
  a silent port is a regression. A service install must refuse an empty token.
- `Protocol::ALL` order is the TUI cycle order: nettest protocols first, then serverless probes.
- Monitor transitions are decided only by `monitor::health::Health` (pure, unit tested); the
  supervisor normalises runner events into `Signal`s and the delivery task never blocks probing.
- `monitor` and `service` are clap subcommands next to the positional target; a host named like
  one needs a scheme prefix (documented in README).
- **Releases:** pushing a tag `v<version>` runs `.github/workflows/release.yml`, which tests,
  builds both targets on ubuntu-22.04 (glibc 2.35 floor), runs `scripts/package.sh` and publishes
  the archives, bare binaries and `SHA256SUMS` to GitHub Releases. `package.sh` refuses when the
  built `--version` differs from the tag, so bump `workspace.package.version` in `Cargo.toml`
  (and the version strings in README.md and the admin guide) before tagging. For a tag that
  predates the workflow, use "Run workflow" in the Actions tab with the tag name. `target/` and
  `dist/` are ignored; `releases/<version>/` holds bare binaries committed as a fallback download
  (the exception in `.gitignore`). The admin guide source is `docs/nettest-admin-guide.html`; render it with the
  chromium command in its header comment, run from the repo root so the PDF lands at
  `nettest-admin-guide.pdf` next to README.md (`--generate-pdf-document-outline` gives the PDF its
  bookmarks) and keep its version line, banner text and footer in step with the version.
