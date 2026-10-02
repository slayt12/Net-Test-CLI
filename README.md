# nettest

Lightweight client/server pair for troubleshooting network paths from a terminal. Two small,
dependency-free binaries (Windows 11+ and Linux) that measure round-trip latency, loss, jitter,
connection stability and throughput over **WebSocket (ws / wss), raw TCP and UDP**. The client
can also probe devices that cannot run a server (IP phones, switches, gateways) with **ICMP ping,
TCP connect, SIP OPTIONS and HTTP(S)**, producing the same statistics, chart and reports.

```
nettest-server          runs on the far end: echoes probes, counts throughput, logs clients;
                        installs itself as a systemd unit or Windows service
nettest-client          interactive TUI with live chart, or headless for scripts and cron
```

Why another tool: `ping` needs ICMP (often filtered, needs admin for raw sockets), `iperf` does
throughput but not long-running stability, and neither tells you whether a *WebSocket* path
through a proxy or load balancer behaves. nettest speaks the same frame format on every
transport, so you can compare "tcp works, ws drops every 60 s" directly.

---

## Quick start

On the server machine:

```sh
nettest-server                      # TUI; defaults: tcp+udp 9100, ws 9101, wss 9102, bind any
nettest-server --no-tui --token s3cret --log-file server.log
nettest-server service install --token s3cret   # permanent: systemd unit / Windows service
```

On the client machine:

```sh
nettest-client                                  # interactive settings form, then live view
nettest-client ws://10.0.0.5:9101               # same, pre-filled from the URL
nettest-client --no-tui tcp://10.0.0.5:9100 -i 200ms -c 300 --csv --report
nettest-client --no-tui sip://10.0.0.77 -m soak -d 8h --csv   # an IP phone, no server needed
```

First run of the server with wss enabled generates a self-signed certificate and prints its
SHA-256 fingerprint. Give that to clients (`--fingerprint AB:CD:...`) or use `--insecure`.

Windows Defender Firewall will prompt on the first server start; allow it on the networks you
are testing. UDP needs the port open inbound on the server, like any UDP service.

---

## Client

### Interactive TUI

```
 Settings  →  s starts  →  Running (stats | live chart | log)  →  x stops  →  Summary
```

| Key | Where | Action |
|---|---|---|
| `↑/↓`, `Tab`, `j/k` | settings | move between fields |
| `Enter` | settings | edit a text field / flip a toggle |
| `Space` | settings | cycle protocol, mode, direction |
| `s`, `F5` | settings, summary | start the test |
| `w` | settings | save settings to the config file |
| `x`, `s`, `Esc` | running | stop (waits for in-flight probes) |
| `r` | running, summary | write an HTML report now |
| `1` `2` `3` | running, summary | chart window: last 60 s / 5 min / everything |
| `c` | running | clear the log panel |
| `?` | anywhere | help overlay |
| `q`, `Ctrl-C` | anywhere | quit (stops a running test first) |

The chart is RTT over time with red markers at the baseline where probes were lost. In
throughput mode it shows instantaneous Mbit/s instead.

### Headless (`--no-tui`)

Everything in the form is also a flag. Run `nettest-client --help` for the full list.

```sh
# 5 minutes of 100 ms probes over wss, CSV + JSONL + HTML report into ./runs
nettest-client --no-tui wss://edge.example.com:9102 --fingerprint 6A:52:... \
    -i 100ms -d 5m --csv --jsonl --report -o ./runs

# soak: heartbeat every 5 s for 8 hours, log every disconnect/reconnect
nettest-client --no-tui ws://10.0.0.5:9101 -m soak --heartbeat 5s -d 8h --text

# throughput, 10 s each direction
nettest-client --no-tui tcp://10.0.0.5:9100 -m throughput --tp-secs 10

# scripting: fail if loss > 1% or p95 > 80 ms, emit one JSON line
nettest-client --no-tui udp://10.0.0.5:9100 -i 50ms -c 600 --max-loss 1 --max-p95-ms 80 \
    --json-summary --quiet --no-report

# an IP phone, no server on the far end: SIP OPTIONS every 10 s overnight
nettest-client --no-tui sip://10.0.0.77 -m soak --heartbeat 10s -d 8h --csv --report
```

Exit codes:

| Code | Meaning |
|---|---|
| 0 | completed, loss ≤ `--max-loss` (default 0) and p95 ≤ `--max-p95-ms` if given |
| 1 | thresholds exceeded |
| 2 | could not connect, authentication rejected, TLS failed, or a probe could not be set up (DNS, ICMP not permitted) |
| 3 | bad arguments, unreadable config, wss without `--insecure`/`--fingerprint`, throughput with a serverless protocol |
| 4 | could not write a log file or report |
| 5 | interrupted (Ctrl-C) before a bounded run (`-c`/`-d`) completed |

Interrupting an *unbounded* run with Ctrl-C is the normal way to end it and exits 0/1 by the
thresholds.

### Protocols

| Scheme | Far end | What a probe is |
|---|---|---|
| `ws://`, `wss://` | nettest-server | a nettest frame in one WebSocket binary message (wss: TLS first) |
| `tcp://` | nettest-server | a nettest frame on a raw TCP stream |
| `udp://` | nettest-server | a nettest frame in one datagram |
| `ping://` | any host | ICMP echo request / reply |
| `connect://host:port` | any open port | a TCP handshake, timed and closed (refused is reported as such) |
| `sip://` | phone, PBX, SBC | SIP OPTIONS over UDP (default port 5060); any final response counts |
| `http://`, `https://` | any web interface | a `HEAD` request, timed to the status line (https accepts any certificate unless pinned) |

The first four are the nettest echo protocol and support every mode. The last five are
**serverless probes**: they work in `latency` and `soak` mode against devices that cannot run
software, and feed the same tracker, chart, logs and HTML report. See "Serverless probes" below.

### Test modes

| Mode | What it does | What you get |
|---|---|---|
| `latency` | sequenced probes at a fixed interval, server (or device) answers them | RTT min/avg/max, p50/p95/p99, jitter (RFC 3550), loss %, failed, late, out-of-order, duplicates |
| `soak` | long-lived connection with heartbeats; for serverless probes, continuous probes with outage detection | every disconnect/outage with reason, reconnect attempts and downtime, uptime %, longest outage, longest session |
| `throughput` | bulk upload then/or download for N seconds (nettest-server only) | Mbit/s per direction, bytes, frame counts, and for UDP the datagram loss |

All modes record the **connect-phase breakdown** (DNS, TCP connect, TLS handshake, WebSocket
upgrade, hello/auth) for every connection attempt. Most "it's slow" tickets are one of those.

Loss is declared by timeout (`max(2 s, 10 × interval)`). An echo that arrives after that is
reclassified as **late** rather than lost, so slow links do not inflate the loss count.

A probe that is answered negatively before the timeout (TCP connection refused, ICMP host
unreachable) is recorded as **failed** with the reason in the `detail` column. Failed probes count
towards loss % so thresholds and exit codes behave the same; the summary shows the split.

Latency is round-trip only. The two machines are never assumed to be clock-synchronised.

### Serverless probes

```sh
nettest-client --no-tui ping://10.0.0.77 -i 500ms -c 120            # ICMP, 120 pings
nettest-client --no-tui connect://10.0.0.77:5060 -m soak -d 4h       # is the phone's SIP port up?
nettest-client --no-tui sip://10.0.0.77 -m soak --heartbeat 10s -d 8h --csv --report
nettest-client --no-tui https://10.0.0.77/ -c 20                     # web UI, any certificate
nettest-client sip://10.0.0.77                                       # TUI, pre-filled
```

* **Privileges.** `ping` uses an unprivileged ICMP socket: on Windows the `IcmpSendEcho` API, on
  Linux a datagram ICMP socket, which the kernel allows for groups listed in
  `net.ipv4.ping_group_range` (open to all users on Fedora, Ubuntu and anything that boots with
  systemd 241+; on other distributions run as root or use `connect://` instead). The other probes
  need nothing special.
* **Stability in soak mode** is derived from the probes: `--outage-after N` (default 3)
  consecutive failures or timeouts open an outage, the next success closes it. Downtime, uptime %
  and longest outage are reported exactly as for a dropped connection.
* **What "alive" means.** `connect`: the handshake completed. `sip`: any final response
  (200, 401, 405, 486...) whose CSeq and Call-ID match the request; the status line is logged as
  the sample detail. `http`/`https`: any HTTP status line. `ping`: an echo reply with our
  sequence number.
* **Limits.** No throughput mode (there is nothing to receive bulk data). Late replies cannot
  occur (a reply after the timeout is discarded). On Linux an ICMP "destination unreachable"
  surfaces as a timeout, like `ping`; on Windows it is reported as failed. Phones that disable
  OPTIONS show 100 % loss on `sip://`; use `connect://phone:5060` instead. Some devices
  rate-limit ICMP or SIP; keep intervals at 200 ms or slower for them.
* **Frame size** applies to `ping` only (ICMP message size, 32 to 65507 bytes). `--ws-path`
  (alias `--path`) is the request path for `http`/`https`. Token, TLS "accept any" and the
  throughput fields are ignored and shown dimmed in the TUI; `--fingerprint` still pins an
  `https` certificate if given.

### Outputs

Files go to the output directory (default `.`) as `nettest-<YYYYMMDD-HHMMSS>.<ext>`:

| Flag / toggle | File | Content |
|---|---|---|
| console (default, failures only) | stderr | one line per failed probe plus events; `--all-samples` for every probe, `--quiet` for none |
| `--csv` | `.csv` | one row per sample and event: timestamp, elapsed, seq, status (ok/lost/late/ooo/dup/fail), rtt, running loss %, p50/p95/p99, jitter, detail (failure reason or SIP/HTTP status line) |
| `--text` | `.log` | same lines as the console |
| `--jsonl` | `.jsonl` | one JSON object per sample/event for log tooling |
| `--report` (default on) | `.html` | single-file report: summary tiles, RTT chart with loss markers (inline SVG), throughput, stability events, connect phases, event log. No JavaScript, no external resources, opens offline |

Each flag accepts an optional explicit path: `--csv run1.csv`.

### Settings persistence

The client remembers its last settings in `client.toml` under the per-user config directory
(`%APPDATA%\nettest\` on Windows, `~/.config/nettest/` on Linux). Save with `w` in the TUI or
`--save-config` headless. Precedence: flag > `NETTEST_*` environment variable > config file >
default. `--config <path>` points at a different file.

---

## Server

```
nettest-server [--bind any|<ip>] [--tcp-port 9100] [--udp-port 9100] [--ws-port 9101]
               [--wss-port 9102] [--token <secret>] [--log-file f] [--jsonl f]
               [--cert-dir dir] [--idle-timeout 60] [--banner text] [--no-tui]
               [--config f] [--save-config]
nettest-server service install|edit|uninstall|start|stop|restart|status [...]
```

* Any port set to `0` disables that listener. `--bind any` (default) binds both `0.0.0.0` and
  `[::]` so IPv4 and IPv6 clients work on every OS.
* `--token` makes the server drop clients that do not present the same token in their hello.
  Unauthenticated UDP peers are silently ignored, so the server cannot be used as a reflector.
  A foreground server without a token logs a warning; a *service* refuses to install without one.
* **Scanner identification.** Anything that connects to a listener without speaking the nettest
  protocol gets a readable answer, so security scanners can identify the service: on the TCP, ws
  and wss ports an HTTP request receives `HTTP/1.1 200` with a `text/plain` body, any other bytes
  (or silence for 3 s) receive the bare line, and a non-nettest UDP datagram receives the same
  line (at most one reply per source IP every 2 s and 50 per second overall, so the server cannot
  amplify traffic). The text is `nettest by Slaytons Technology Services (nettest-server x.y.z)`;
  `--banner` changes the first part. Real clients are unaffected: the first bytes are classified
  and replayed into the protocol handler. Each banner reply is logged as `[scan] <proto> <peer>`.
* The TUI shows listeners, the wss fingerprint, and a live client table (protocol, peer,
  client id, mode, age, idle time, probes, bytes). `c` clears the log, `w` saves the effective
  settings, `q` quits. `--no-tui` logs to stderr instead; `--log-file` / `--jsonl` append to
  files in both modes.
* Certificates: `cert.pem` / `key.pem` in `--cert-dir` (default `<config dir>/nettest/server/`).
  Delete them to regenerate. Drop in your own PEM pair if you prefer a CA-signed cert.
* Sessions idle longer than `--idle-timeout` seconds are closed (stream) or expired (UDP).

Server exit codes: 0 clean shutdown, 1 TUI error, 2 could not bind, 3 bad arguments/config.

### Running the server as a service

```sh
nettest-server service install --token s3cret                 # all four listeners, default ports
nettest-server service install --generate-token --ws-port 9100 --tcp-port 0 --udp-port 0 --wss-port 0
nettest-server service edit --idle-timeout 120 --wss-port 9102   # change settings, restart
nettest-server service status
nettest-server service stop | start | restart
nettest-server service uninstall [--purge]                     # --purge also deletes config, certs, logs
```

* **Token required.** `install` exits 3 unless a token is given (`--token`, `NETTEST_TOKEN`, or
  `--generate-token`, which prints a random 32-character token once). `edit` can replace the
  token but never clear it.
* **Elevation is automatic.** Every subcommand except `status` needs root/administrator. On
  Linux the command re-runs itself through `sudo` (then `pkexec`, `doas`), so the password prompt
  and all output appear inline. On Windows it triggers the UAC prompt and shows the elevated
  run's output afterwards. Exit code 3 if the prompt is declined.
* **Settings are stored system-wide** and are the only thing the service reads; per-user
  `client.toml`/`server.toml` files are not involved. `install` accepts every server flag;
  anything not given keeps its default (or the previously installed value when re-installing).

| | Linux (systemd) | Windows (SCM) |
|---|---|---|
| executable | `/usr/local/bin/nettest-server` (copied; `--no-copy` keeps the current path) | `%ProgramFiles%\nettest\nettest-server.exe` |
| settings | `/etc/nettest/server.toml`, mode 0600 | `%ProgramData%\nettest\server.toml` |
| certificates | `/var/lib/nettest` | `%ProgramData%\nettest\certs` |
| log | `/var/log/nettest/server.log` plus `journalctl -u nettest-server` | `%ProgramData%\nettest\server.log` |
| unit / name | `/etc/systemd/system/nettest-server.service` | service `nettest-server`, display "nettest server" |
| account | `DynamicUser=yes` (no fixed user, no shell), `ProtectSystem=strict`, `NoNewPrivileges` | `LocalSystem` by default; `--account "NT AUTHORITY\NetworkService"` to change (grant it write access to `%ProgramData%\nettest`) |

Linux notes: the unit passes the config as a systemd *credential* (`LoadCredential=`), which is
how a root-only 0600 file reaches a dynamic user; this needs systemd 247 (2020) or newer. Ports
below 1024 add `AmbientCapabilities=CAP_NET_BIND_SERVICE` automatically. `systemctl stop` sends
SIGTERM, which the server handles like Ctrl-C. Windows notes: the service is registered with
`service run --config <path>` as its command line and depends on `Tcpip`; the firewall rule is
not created automatically. `%ProgramData%` is readable by local users, so keep the token out of
other places.

---

## Wire format

One 32-byte little-endian header, then `payload_len` bytes, identical on TCP (header is the
length delimiter), WebSocket (one binary message per frame) and UDP (one datagram per frame).

```
off size field
0   4    magic "NTP1"
4   1    version = 1
5   1    kind   Hello HelloAck Probe Echo Heartbeat HeartbeatAck TpStart TpData TpEnd TpResult Error Bye
6   2    flags  (TpStart bit0: 1 = server streams to client)
8   8    seq
16  8    client_send_ns   client monotonic clock, echoed untouched; RTT = now - this
24  4    payload_len
28  4    reserved
```

Hello / HelloAck / TpStart / TpResult carry a small JSON body; Probe carries filler; Error
carries a UTF-8 reason.

---

## Building

Requires Rust 1.85+ (edition 2024). No C toolchain beyond what Rust already needs; TLS is
pure Rust (rustls + ring, no OpenSSL). Windows service management and ICMP use `windows-service`
and `windows-sys` (pure Rust bindings to system DLLs); Linux ICMP uses `socket2`.

```sh
cargo build --release                                   # Linux binaries in target/release/
cargo build --release --target x86_64-pc-windows-gnu    # from Linux, needs mingw-w64 gcc
cargo test --workspace                                  # unit + in-process loopback tests
```

Windows cross-compile: `rustup target add x86_64-pc-windows-gnu` and a `x86_64-w64-mingw32-gcc`
on PATH (`.cargo/config.toml` names it as the linker). The resulting `.exe` files import only
Windows system DLLs. Building natively on Windows with the MSVC toolchain also works.

Fully static Linux binaries (no glibc version dependency) are possible with
`rustup target add x86_64-unknown-linux-musl` and `cargo build --release --target
x86_64-unknown-linux-musl`; ring's C sources need a working `cc` for that target.

Release profile is tuned for size (`opt-level = "s"`, fat LTO, `panic = "abort"`, stripped):
roughly 2.5 MB per binary.

### Application icon

`assets/icon.svg` is the source: the Lucide "cable" glyph on the slaytons.net theme blue
(`#007aff`). `cargo run -p xtask -- icons` renders it to `assets/icon-<size>.png` (16 to 512 px)
and a multi-size `assets/icon.ico`. The Windows executables embed that `.ico` plus product
metadata through `build.rs` (winresource), which needs `x86_64-w64-mingw32-windres` and `-ar`
when cross-compiling; on Linux targets the build script is a no-op. Re-run the xtask after
editing the SVG; the generated files are committed so normal builds never need resvg.

---

## Layout

```
crates/nettest-proto     shared: frame format + codec, transports (tcp/udp/ws/wss), TLS helpers,
                         serverless probes (icmp/tcp connect/sip/http), stats (latency/
                         throughput/soak), sinks (console/csv/text/jsonl), HTML report, config
crates/nettest-client    runner (latency, soak, throughput, serverless probe loop, reconnect),
                         headless driver, TUI
crates/nettest-server    listeners (with scanner banner), session table, shared echo handler,
                         log pipeline, TUI, service/ (systemd + Windows SCM install, elevation),
                         tests/loopback.rs (real server + real client per protocol, banner tests)
assets/                  icon.svg source, rendered PNGs and icon.ico
xtask/                   developer tasks (icon rendering); excluded from default builds
```

---

## Troubleshooting with it

* **TCP fine, ws fails to connect** – look at the connect-phase breakdown: a TCP time but no
  WS-upgrade time means a proxy or load balancer is refusing the upgrade.
* **Periodic disconnects at a fixed interval** in soak mode usually mean a NAT or firewall idle
  timeout; lower `--heartbeat` below that interval to confirm.
* **Loss on UDP but not TCP** is real packet loss; TCP hides it as latency spikes, which the
  p99 and jitter columns will show.
* **High DNS time** on every reconnect points at the resolver, not the path. Test with the IP.
* **Throughput much lower in one direction** often indicates asymmetric shaping; compare
  upload and download results from the same run.
* **A phone keeps dropping registration**: `sip://phone -m soak --heartbeat 10s` overnight. An
  outage in the log with "connection refused" or timeouts at the same time as the drop puts the
  problem on the network or the phone, not the PBX; `connect://pbx:5060` from the same place
  tests the other end of that call leg.
* **`ping://` says ICMP is not permitted** (Linux): the user's group is outside
  `net.ipv4.ping_group_range`. Run as root, add the group, or use `connect://host:port`.
* **A vulnerability scan reports an unknown service** on 9100-9102: that is nettest; connect
  with a browser or `curl` to see the identification banner. Set `--banner` if your scanner
  needs specific text.
