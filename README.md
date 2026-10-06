# nettest

**Network path troubleshooting from a terminal.** Two small, dependency-free binaries for
Windows 11 and Linux that measure round-trip latency, loss, jitter, connection stability and
throughput over WebSocket, TCP and UDP, and probe devices that cannot run software (IP phones,
switches, gateways) with ping, TCP connect, SIP OPTIONS and HTTP. The client also runs as a
**monitoring service** that watches any number of endpoints and sends ntfy, Slack, Discord or Teams
alerts when one goes down or comes back.

[![Release](https://img.shields.io/github/v/release/slayt12/Net-Test-CLI?label=release)](https://github.com/slayt12/Net-Test-CLI/releases)
[![Release build](https://github.com/slayt12/Net-Test-CLI/actions/workflows/release.yml/badge.svg)](https://github.com/slayt12/Net-Test-CLI/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%2011%20%7C%20Linux-lightgrey)
![Rust](https://img.shields.io/badge/rust-2024%20edition-orange)

**Contents:** [Download](#download) · [Quick start](#quick-start) · [Client](#client) ·
[Monitoring as a service](#monitoring-as-a-service) · [Server](#server) ·
[Server as a service](#running-the-server-as-a-service) ·
[Wire format](#wire-format) · [Building](#building) · [Layout](#layout) ·
[Troubleshooting](#troubleshooting-with-it) · [License](#license-and-support)

Full manual with a walkthrough of every screen, a field-by-field settings reference and
troubleshooting recipes: [nettest-admin-guide.pdf](nettest-admin-guide.pdf).

---

## Download

Ready-to-run packages for every version are on the
**[Releases page](https://github.com/slayt12/Net-Test-CLI/releases)**. Nothing to install: unpack,
copy the file where you need it, run it. Current version **1.1.2**.

| Asset | Contents |
|---|---|
| `nettest-<version>-windows-x86_64.zip` | `nettest-client.exe`, `nettest-server.exe`, the admin guide (PDF), README, LICENSE |
| `nettest-<version>-linux-x86_64.tar.gz` | `nettest-client`, `nettest-server`, the admin guide (PDF), README, LICENSE |
| `nettest-client-windows-x86_64.exe`, `nettest-server-windows-x86_64.exe` | single executables for Windows 11 x86-64 (import only system DLLs) |
| `nettest-client-linux-x86_64`, `nettest-server-linux-x86_64` | single executables for Linux x86-64 (`chmod +x` after download) |
| `SHA256SUMS` | checksums of every asset |

```sh
# Linux: unpack the archive, or grab one binary
curl -LO https://github.com/slayt12/Net-Test-CLI/releases/latest/download/nettest-client-linux-x86_64
chmod +x nettest-client-linux-x86_64 && ./nettest-client-linux-x86_64 --version

# Windows (PowerShell)
Invoke-WebRequest https://github.com/slayt12/Net-Test-CLI/releases/latest/download/nettest-client-windows-x86_64.exe -OutFile nettest-client.exe
.\nettest-client.exe --version
```

Verify with `sha256sum -c SHA256SUMS` (Linux) or `Get-FileHash` (Windows). The executables are
not code-signed, so Windows SmartScreen may show "Windows protected your PC" on first run; choose
*More info* then *Run anyway*. Windows Defender Firewall asks once when the server first listens.
Release Linux binaries are built on Ubuntu 22.04 and run on glibc 2.35 or newer (Ubuntu 22.04+,
Debian 12+, RHEL 9+, Fedora 36+); older systems can build the static musl variant, see
[Building](#building).

Releases are produced by the `release` GitHub Actions workflow from the tagged source, so every
asset is reproducible from the tag. The bare binaries for each version are also committed under
[releases/](releases/) as a fallback when the Releases page is unreachable.

---

## What it is

Lightweight client/server pair for troubleshooting network paths from a terminal. The server
end echoes probes over **WebSocket (ws / wss), raw TCP and UDP**; the client can also probe
devices that cannot run a server with **ICMP ping, TCP connect, SIP OPTIONS and HTTP(S)**,
producing the same statistics, live chart and reports.

```
nettest-server          runs on the far end: echoes probes, counts throughput, logs clients;
                        installs itself as a systemd unit or Windows service
nettest-client          interactive TUI with live chart, or headless for scripts and cron;
                        `monitor` watches endpoints and alerts via ntfy / Slack / Discord / Teams,
                        installable as a systemd unit or Windows service
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
nettest-client monitor --example-config > monitor.toml           # then edit targets + webhooks
nettest-client service install --from monitor.toml              # alert on DOWN / UP, forever
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

### Monitoring as a service

`nettest-client monitor` watches any number of endpoints for as long as it runs and sends a
webhook when one goes **DOWN** or comes back **UP**. It uses the same probes as the tests above
(ping, connect, sip, http/https against any device; ws/wss/tcp/udp against a nettest-server), so
anything you can test you can monitor. Installed as a service it starts at boot and survives
reboots, targets that are down at start, and webhook endpoints that are temporarily unreachable.

```sh
nettest-client monitor --example-config > monitor.toml     # annotated template, edit it
nettest-client monitor --config monitor.toml --check       # validate, show what would be watched
nettest-client monitor --config monitor.toml --test-notify # send a test message to every notifier
nettest-client monitor --config monitor.toml               # run in the foreground (Ctrl-C / SIGTERM stops)
nettest-client service install --from monitor.toml         # install + start the service (asks for root / UAC)
nettest-client service status | restart | stop | start
nettest-client service uninstall [--purge]                 # --purge also removes monitor.toml and the log
```

A minimal `monitor.toml` (save it as UTF-8, UTF-16 or Windows-1252/ANSI, whatever your editor or
PowerShell produced: the encoding is detected, `--check` prints it, and `service install` stores
the installed copy as UTF-8):

```toml
[[targets]]
name = "gateway"
target = "ping://10.0.0.1"          # any target form the CLI accepts
interval = "5s"
timeout = "2s"
failures_before_down = 3            # DOWN after 3 consecutive failures (≈ 15 s here)
successes_before_up = 1             # UP on the first success
remind_every = "1h"                 # re-send the DOWN alert hourly while still down ("0" = never)

[[targets]]
name = "pbx"
target = "sip://pbx.example.com"

[[targets]]
name = "edge nettest"
target = "wss://edge.example.com:9102"
token = "s3cret"
fingerprint = "6A:52:..."           # or insecure = true

[[notify]]
kind = "ntfy"                       # ntfy | slack | discord | teams
url = "https://ntfy.sh/my-secret-topic"
# token = "tk_..."                  # ntfy access token
priority = "high"                   # ntfy priority of DOWN alerts; UP alerts use "default"

[[notify]]
kind = "slack"
url = "https://hooks.slack.com/services/T000/B000/XXXX"

[[notify]]
kind = "discord"
url = "https://discord.com/api/webhooks/123456/abcdef"

[[notify]]
kind = "teams"                      # Microsoft Teams via a Workflows webhook (see below)
url = "https://<env>.environment.api.powerplatform.com/powerautomate/automations/direct/cu/.../invoke?api-version=1&sp=...&sv=1.0&sig=..."
```

* **Alert rule.** Every probe (or, for ws/wss/tcp/udp, every echo, disconnect and failed
  reconnect) is a success or a failure. A target becomes DOWN after `failures_before_down`
  consecutive failures and UP again after `successes_before_up` consecutive successes; shorter
  blips never alert. `timeout` is also the loss timeout, so with `interval = "5s"` and
  `timeout = "2s"` a dead host is reported after about 15 s. A target that is already down when
  the monitor starts is reported DOWN after the same number of failures.
* **Notifiers.** All configured notifiers receive every alert. ntfy gets a text message with
  `Title`, `Priority` and `Tags` headers (and `Authorization: Bearer` when `token` is set);
  Slack and Discord get their JSON webhook payload; Teams gets an Adaptive Card (title coloured
  red for DOWN, green for UP) plus a plain-text copy. Delivery runs in its own task with three
  attempts (2 s, 5 s, `Retry-After` on 429); a webhook that fails is logged, never retried
  forever, and never delays probing. `notify_on_start = true` in `[monitor]` sends a message at
  startup so you know the chain works; `--test-notify` does the same on demand.
* **Microsoft Teams.** The old Office 365 connector webhooks are retired; use a Workflows
  webhook instead: in Teams open the channel's `...` menu, choose **Workflows**, pick **Post to
  a channel when a webhook request is received** (or **Send webhook alerts to a channel**), and
  copy the URL it shows into `url`. Leave the trigger open to "Anyone" (nettest sends no
  token; the `sig=` part of the URL is the secret, so treat the URL like a password). The
  payload carries both an Adaptive Card and a `text` field, so either template posts the alert,
  and the trigger's `202 Accepted` counts as delivered.
* **TLS.** Webhook HTTPS is verified against the Mozilla root bundle (public ntfy.sh, Slack,
  Discord and Teams just work). For a self-hosted ntfy with a self-signed certificate set `insecure = true`
  or pin it with `fingerprint = "<sha256>"` on that notifier. Plain `http://` is accepted for a
  LAN ntfy. Monitored `wss://` targets still need `insecure` or `fingerprint`, like the CLI.
* **Log.** Every transition, every delivery result and (every `summary_every`, default 1 h) a
  per-target status line with loss % and p95 go to the log; in the foreground to stderr, as a
  service to the file below (`--log-file` or `[monitor].log_file`).
* **Editing.** The installed file is the one the service reads; after changing it run
  `nettest-client service restart` (which validates it first) or `service install --from` again.
  A device literally named `monitor` or `service` needs a scheme on the command line
  (`nettest-client ping://monitor`) so it is not taken for the subcommand.

| | Linux (systemd) | Windows (SCM) |
|---|---|---|
| executable | `/usr/local/bin/nettest-client` (copied; `--no-copy` keeps the current path) | `%ProgramFiles%\nettest\nettest-client.exe` |
| settings | `/etc/nettest/monitor.toml`, mode 0600 (may hold tokens) | `%ProgramData%\nettest\monitor.toml` |
| log | `/var/log/nettest-monitor/monitor.log` plus `journalctl -u nettest-monitor` | `%ProgramData%\nettest\monitor.log` |
| unit / name | `/etc/systemd/system/nettest-monitor.service` | service `nettest-monitor`, display "nettest monitor" |
| account | `DynamicUser=yes`, `ProtectSystem=strict`, `NoNewPrivileges`, `CAP_NET_RAW` for ping | `LocalSystem` by default; `--account` to change |

The service shares `/etc/nettest` and `%ProgramData%\nettest` with nettest-server but no file
names, so both can be installed on one host; `uninstall --purge` on either removes only its own
files. Elevation, the credential-based config delivery and the Windows registration work exactly
as for the server (see below). Exit codes of `monitor`: 0 stopped by signal, 2 `--test-notify`
could not deliver everywhere, 3 invalid or missing `monitor.toml`, 4 log file not writable.

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
  amplify traffic). The text is `nettest by Slaytons Technology Services (nettest-server 1.1.2)`;
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
nettest-server service uninstall [--purge]                     # --purge also deletes the server's config, certs, logs
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
SIGTERM, which the server handles like Ctrl-C. `uninstall --purge` removes only the server's own
files (`server.toml`, the certificates, `server.log`); a client monitor installed on the same
host keeps its `monitor.toml`. Windows notes: the service is registered with
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
and `windows-sys` (pure Rust bindings to system DLLs); Linux ICMP uses `socket2`. The monitor's
HTTPS webhooks are verified against the Mozilla root bundle shipped in `webpki-roots` (no system
certificate store is consulted, so behaviour is identical on every host); bump that crate to
refresh the roots.

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

The version is set once in the workspace `Cargo.toml` and reported by `--version`, in the
server banner and in the HTML report. To publish a release: bump that version, commit, tag
`v<version>` and push the tag. The `release` workflow (`.github/workflows/release.yml`) tests,
builds both platforms, runs `scripts/package.sh` and uploads the archives, bare binaries and
`SHA256SUMS` to the Releases page. The same script produces identical files locally into `dist/`
for a manual `gh release create`. `target/` and `dist/` are never committed.

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
                         throughput/soak), sinks (console/csv/text/jsonl), HTML report, config,
                         line log, minimal HTTP POST client (webhooks)
crates/nettest-service   shared service plumbing: elevation (sudo / UAC), systemd unit rendering
                         and systemctl, Windows SCM helpers and the service entry point
crates/nettest-client    runner (latency, soak, throughput, serverless probe loop, reconnect),
                         headless driver, TUI, monitor/ (config, health state machine, per-target
                         supervisor, ntfy/Slack/Discord/Teams delivery), service/ (monitor as a service),
                         tests/monitor_loopback.rs (DOWN/UP transitions against a real server)
crates/nettest-server    listeners (with scanner banner), session table, shared echo handler,
                         TUI, service/ (server-specific install logic on top of nettest-service),
                         tests/loopback.rs (real server + real client per protocol, banner tests)
assets/                  icon.svg source, rendered PNGs and icon.ico
releases/<version>/      bare binaries committed per tagged version as a fallback download
scripts/package.sh       builds the Releases assets (archives, binaries, SHA256SUMS, notes) into dist/
.github/workflows/       release.yml: tag push -> test, build, package, publish to GitHub Releases
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

---

## License and support

MIT, see [LICENSE](LICENSE). Copyright Slayton's Technology Services.

Bugs and feature requests: [github.com/slayt12/Net-Test-CLI/issues](https://github.com/slayt12/Net-Test-CLI/issues).
Include the output of `--version`, the exact command line, the OS on both ends, and the CSV or
HTML report from the run if you have one.
