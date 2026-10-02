//! `nettest-server service ...`: run the server as a system service.
//!
//! Linux uses a systemd unit, Windows the Service Control Manager; both read their settings from
//! a system-wide `server.toml` written at install time and rewritten by `edit`, so the service
//! always starts with pre-determined options and never with whatever happens to be in a user's
//! config directory.
//!
//! Invariants:
//! - A service is never installed or edited without a non-empty token.
//! - Everything except `status` and `run` needs root/admin; `dispatch` re-executes itself
//!   elevated (sudo / UAC) and otherwise never prompts.
//! - All user-visible output goes through `Report` so the Windows UAC child, whose console is
//!   invisible, can hand it back to the parent via `--capture-output`.

mod elevate;
mod paths;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod systemd;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
use unsupported as platform;

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use nettest_proto::config::ServerConfig;

use crate::cli::args::ServerFlags;
pub use paths::{ServicePaths, service_paths};

pub const SERVICE_NAME: &str = "nettest-server";
#[cfg_attr(not(windows), allow(dead_code))]
pub const DISPLAY_NAME: &str = "nettest server";
pub const DESCRIPTION: &str =
    "nettest network troubleshooting server (echo, soak and throughput for nettest-client)";

pub const EXIT_MANAGER: u8 = 2;
pub const EXIT_USAGE: u8 = 3;

#[derive(clap::Args, Debug)]
pub struct ServiceArgs {
    /// (internal) the elevated child appends its output to this file for the parent to print
    #[arg(long, hide = true, global = true)]
    pub capture_output: Option<PathBuf>,
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Install as a system service that starts at boot (asks for admin / root if needed)
    Install(InstallArgs),
    /// Change the installed service's settings and restart it
    Edit(EditArgs),
    /// Stop and remove the service
    Uninstall {
        /// Also delete the service's config, certificates and log files
        #[arg(long)]
        purge: bool,
    },
    /// Start the installed service
    Start,
    /// Stop the installed service
    Stop,
    /// Restart the installed service
    Restart,
    /// Show whether the service is installed and running, and its settings
    Status,
    /// Service Control Manager entry point (Windows); not for interactive use
    #[command(hide = true)]
    Run {
        #[arg(long)]
        config: PathBuf,
    },
}

impl ServiceAction {
    fn needs_root(&self) -> bool {
        !matches!(self, ServiceAction::Status | ServiceAction::Run { .. })
    }
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    #[command(flatten)]
    pub flags: ServerFlags,
    /// Generate a random 32-character token, store it and print it once
    #[arg(long, conflicts_with = "token")]
    pub generate_token: bool,
    /// Run the service from this executable's current location instead of copying it to the
    /// system location
    #[arg(long)]
    pub no_copy: bool,
    /// Windows: account to run under (default LocalSystem), e.g. "NT AUTHORITY\NetworkService"
    #[arg(long)]
    pub account: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct EditArgs {
    #[command(flatten)]
    pub flags: ServerFlags,
    /// Replace the token with a newly generated one and print it once
    #[arg(long, conflicts_with = "token")]
    pub generate_token: bool,
    /// Windows: change the account the service runs under
    #[arg(long)]
    pub account: Option<String>,
}

/// Failure with the exit code it maps to.
#[derive(Debug)]
pub struct ServiceError {
    pub code: u8,
    pub msg: String,
}

impl ServiceError {
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            msg: msg.into(),
        }
    }
    pub fn manager(msg: impl Into<String>) -> Self {
        Self {
            code: EXIT_MANAGER,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        ServiceError::manager(e.to_string())
    }
}

impl From<nettest_proto::config::ConfigError> for ServiceError {
    fn from(e: nettest_proto::config::ConfigError) -> Self {
        ServiceError::usage(e.to_string())
    }
}

/// Console output that is also appended to the capture file when running as the UAC child.
pub struct Report {
    capture: Option<File>,
}

impl Report {
    pub fn new(capture: Option<&Path>) -> Self {
        Self {
            capture: capture.and_then(|p| {
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(p)
                    .ok()
            }),
        }
    }

    pub fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.tee(s.as_ref());
    }

    pub fn err(&mut self, s: impl AsRef<str>) {
        eprintln!("{}", s.as_ref());
        self.tee(s.as_ref());
    }

    fn tee(&mut self, s: &str) {
        if let Some(f) = self.capture.as_mut() {
            let _ = writeln!(f, "{s}");
        }
    }
}

pub fn dispatch(args: ServiceArgs) -> ExitCode {
    if let ServiceAction::Run { config } = &args.action {
        return platform::run_as_service(config.clone());
    }
    let mut out = Report::new(args.capture_output.as_deref());
    if args.action.needs_root() && !elevate::is_elevated() {
        let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        return match elevate::reexec_elevated(&argv) {
            Ok(code) => code,
            Err(e) => {
                out.err(format!("could not obtain administrator rights: {e}"));
                ExitCode::from(EXIT_USAGE)
            }
        };
    }
    let result = match args.action {
        ServiceAction::Install(a) => platform::install(a, &mut out),
        ServiceAction::Edit(a) => platform::edit(a, &mut out),
        ServiceAction::Uninstall { purge } => platform::uninstall(purge, &mut out),
        ServiceAction::Start => platform::start(&mut out),
        ServiceAction::Stop => platform::stop(&mut out),
        ServiceAction::Restart => platform::restart(&mut out),
        ServiceAction::Status => platform::status(&mut out),
        ServiceAction::Run { .. } => unreachable!("handled above"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out.err(format!("error: {e}"));
            ExitCode::from(e.code)
        }
    }
}

// ---- helpers shared by the platform backends ---------------------------------------------

/// Settings for a fresh install: stored service config (if re-installing) + flags, with the
/// service's own cert/log locations filled in. Refuses an empty token.
pub(crate) fn build_install_config(
    flags: &ServerFlags,
    generate_token: bool,
    paths: &ServicePaths,
    out: &mut Report,
) -> Result<ServerConfig, ServiceError> {
    let base = nettest_proto::config::load::<ServerConfig>(&paths.config)?;
    let mut cfg = flags.apply(base);
    if generate_token {
        cfg.token = generate_token_string();
        out.line(format!(
            "generated token (copy it now, it is not shown again): {}",
            cfg.token
        ));
    }
    if cfg.token.trim().is_empty() {
        return Err(ServiceError::usage(
            "a service needs a secret token: pass --token <secret> or --generate-token",
        ));
    }
    if cfg.cert_dir.trim().is_empty() {
        cfg.cert_dir = paths.cert_dir.display().to_string();
    }
    if cfg.log_file.trim().is_empty() {
        cfg.log_file = paths.log_file.display().to_string();
    }
    Ok(cfg)
}

/// Settings for `edit`: the stored service config with flags applied. The token may be replaced
/// but never emptied.
pub(crate) fn build_edit_config(
    flags: &ServerFlags,
    generate_token: bool,
    paths: &ServicePaths,
    out: &mut Report,
) -> Result<ServerConfig, ServiceError> {
    if !paths.config.exists() {
        return Err(ServiceError::usage(format!(
            "service is not installed (no {}); run `nettest-server service install` first",
            paths.config.display()
        )));
    }
    let base = nettest_proto::config::load::<ServerConfig>(&paths.config)?;
    let mut cfg = flags.apply(base);
    if generate_token {
        cfg.token = generate_token_string();
        out.line(format!(
            "generated token (copy it now, it is not shown again): {}",
            cfg.token
        ));
    }
    if cfg.token.trim().is_empty() {
        return Err(ServiceError::usage(
            "the service token cannot be empty: pass --token <secret> or --generate-token",
        ));
    }
    Ok(cfg)
}

/// 32 hex characters from the OS random source.
pub(crate) fn generate_token_string() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("system random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Any listener below 1024 needs CAP_NET_BIND_SERVICE (Linux) or an elevated account.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn needs_low_port(cfg: &ServerConfig) -> bool {
    [cfg.tcp_port, cfg.udp_port, cfg.ws_port, cfg.wss_port]
        .iter()
        .any(|p| (1..1024).contains(p))
}

/// Human summary of the stored settings, token masked.
pub(crate) fn describe_config(cfg: &ServerConfig, paths: &ServicePaths) -> Vec<String> {
    let port = |p: u16| {
        if p == 0 {
            "off".to_string()
        } else {
            p.to_string()
        }
    };
    vec![
        format!("config      {}", paths.config.display()),
        format!("bind        {}", cfg.bind),
        format!(
            "listeners   tcp {}  udp {}  ws {}  wss {}",
            port(cfg.tcp_port),
            port(cfg.udp_port),
            port(cfg.ws_port),
            port(cfg.wss_port)
        ),
        format!(
            "token       {}",
            if cfg.token.is_empty() {
                "NOT SET".to_string()
            } else {
                format!("set ({} chars)", cfg.token.chars().count())
            }
        ),
        format!("cert dir    {}", cfg.cert_dir),
        format!("log file    {}", cfg.log_file),
        format!("idle        {} s", cfg.idle_timeout_secs),
        format!("banner      {}", cfg.banner_line().trim_end()),
    ]
}

/// Copy the running executable to the system location. The old file is unlinked first so a
/// binary that is currently executing (an upgrade) can be replaced.
pub(crate) fn copy_binary(dest: &Path) -> Result<PathBuf, ServiceError> {
    let src = std::env::current_exe()?;
    if same_file(&src, dest) {
        return Ok(dest.to_path_buf());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(dest);
    std::fs::copy(&src, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dest.to_path_buf())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Fingerprint of the service's wss certificate, if it has been generated yet.
pub(crate) fn cert_fingerprint(cert_dir: &str) -> Option<String> {
    let pem = std::fs::read(Path::new(cert_dir).join("cert.pem")).ok()?;
    let fp = nettest_proto::tls::fingerprint::of_pem(&pem)?;
    Some(nettest_proto::tls::fingerprint::to_hex(&fp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_generation_and_low_ports() {
        let t = generate_token_string();
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(t, generate_token_string());
        let mut cfg = ServerConfig::default();
        assert!(!needs_low_port(&cfg));
        cfg.ws_port = 443;
        assert!(needs_low_port(&cfg));
        cfg.ws_port = 0;
        assert!(!needs_low_port(&cfg));
    }

    #[test]
    fn install_requires_token() {
        let dir = std::env::temp_dir().join(format!("nettest-svc-test-{}", std::process::id()));
        let paths = ServicePaths {
            bin: dir.join("bin"),
            config: dir.join("server.toml"),
            cert_dir: dir.join("certs"),
            log_file: dir.join("server.log"),
            unit: dir.join("unit"),
        };
        let mut out = Report::new(None);
        let err =
            build_install_config(&ServerFlags::default(), false, &paths, &mut out).unwrap_err();
        assert_eq!(err.code, EXIT_USAGE);
        assert!(err.msg.contains("--generate-token"));
        let cfg = build_install_config(&ServerFlags::default(), true, &paths, &mut out).unwrap();
        assert_eq!(cfg.token.len(), 32);
        assert_eq!(cfg.cert_dir, paths.cert_dir.display().to_string());
        let flags = ServerFlags {
            token: Some(String::new()),
            ..Default::default()
        };
        assert!(build_edit_config(&flags, false, &paths, &mut out).is_err());
    }
}
