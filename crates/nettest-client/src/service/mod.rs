//! `nettest-client service ...`: run the monitor as a system service (systemd / Windows SCM).
//!
//! The installed service reads a system-wide `monitor.toml` copied in at install time
//! (`--from`), so it never depends on a user's config directory. Everything platform-generic
//! comes from the `nettest-service` crate, shared with nettest-server.
//!
//! Invariants:
//! - `install` validates the config before touching the system and refuses one with errors.
//! - `uninstall --purge` removes only the monitor's own files; `/etc/nettest` and
//!   `%ProgramData%\nettest` are shared with nettest-server.
//! - Everything except `status` and `run` needs root/admin (handled by `run_elevated`).

mod paths;

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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
#[cfg_attr(windows, allow(unused_imports))]
pub use nettest_service::{EXIT_USAGE, Report, ServiceError, ServiceSpec};

use crate::monitor::config::{self, Resolved};
pub use paths::{ServicePaths, service_paths};

pub const SERVICE_NAME: &str = "nettest-monitor";
pub const DESCRIPTION: &str =
    "nettest endpoint monitor (ping, tcp, udp, ws, wss, sip, http probes with webhook alerts)";

pub static SPEC: ServiceSpec = ServiceSpec {
    name: SERVICE_NAME,
    display_name: "nettest monitor",
    description: DESCRIPTION,
    program: "nettest-client",
};

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
    /// Install the monitor as a system service that starts at boot (asks for admin / root)
    Install(InstallArgs),
    /// Stop and remove the service
    Uninstall {
        /// Also delete the installed monitor.toml and the log file
        #[arg(long)]
        purge: bool,
    },
    /// Start the installed service
    Start,
    /// Stop the installed service
    Stop,
    /// Restart the installed service (also re-reads monitor.toml)
    Restart,
    /// Show whether the service is installed and running, and what it monitors
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
    /// monitor.toml to install (copied to the system location). Omit to re-install with the
    /// one already installed
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// Run the service from this executable's current location instead of copying it to the
    /// system location
    #[arg(long)]
    pub no_copy: bool,
    /// Windows: account to run under (default LocalSystem), e.g. "NT AUTHORITY\NetworkService"
    #[arg(long)]
    pub account: Option<String>,
}

pub fn dispatch(args: ServiceArgs) -> ExitCode {
    if let ServiceAction::Run { config } = &args.action {
        return platform::run_as_service(config.clone());
    }
    let needs_root = args.action.needs_root();
    nettest_service::run_elevated(needs_root, args.capture_output.as_deref(), |out| {
        match args.action {
            ServiceAction::Install(a) => platform::install(a, out),
            ServiceAction::Uninstall { purge } => platform::uninstall(purge, out),
            ServiceAction::Start => platform::start(out),
            ServiceAction::Stop => platform::stop(out),
            ServiceAction::Restart => platform::restart(out),
            ServiceAction::Status => platform::status(out),
            ServiceAction::Run { .. } => unreachable!("handled above"),
        }
    })
}

// ---- TUI support ---------------------------------------------------------------------------

/// What the TUI's Service tab shows. Everything here is readable without privileges except
/// the installed config (0600, root-owned on Linux), whose read error is reported as such.
#[derive(Debug, Clone)]
pub struct ServiceStatus {
    pub elevated: bool,
    pub user: String,
    pub supported: bool,
    pub installed: bool,
    /// `active` / `inactive` / `failed` (systemd), `running` / `stopped` (SCM), `-` when not
    /// installed.
    pub state: String,
    pub paths: ServicePaths,
    /// `describe` lines of the installed config, or why it could not be read.
    pub config: Option<Result<Vec<String>, String>>,
}

/// Blocking (runs `systemctl` / opens the SCM); call from `spawn_blocking`.
pub fn status_probe() -> ServiceStatus {
    let paths = service_paths();
    let (supported, installed, state) = platform::probe(&paths);
    let config = paths.config.exists().then(|| {
        installed_config(&paths).map(|r| {
            let mut lines = Vec::new();
            for w in &r.warnings {
                lines.push(format!("warning     {w}"));
            }
            lines.extend(describe(&r, &paths));
            lines
        })
    });
    ServiceStatus {
        elevated: nettest_service::elevate::is_elevated(),
        user: std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "?".into()),
        supported,
        installed,
        state,
        paths,
        config,
    }
}

/// One Service-tab action, mapped to the CLI it spawns. The TUI never calls the platform
/// functions in-process: they print through `Report`, and `run_elevated` would replay the TUI's
/// own argv. Spawning `nettest-client service <verb>` reuses the tested path unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Install,
    Start,
    Stop,
    Restart,
    Uninstall,
    /// `uninstall --purge`.
    Purge,
}

impl Verb {
    pub fn label(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Start => "start",
            Verb::Stop => "stop",
            Verb::Restart => "restart",
            Verb::Uninstall => "uninstall",
            Verb::Purge => "uninstall --purge",
        }
    }

    /// Destructive or disruptive: the TUI asks `y/n` first.
    pub fn needs_confirm(self) -> bool {
        matches!(self, Verb::Stop | Verb::Uninstall | Verb::Purge)
    }

    /// Arguments after the executable. `from` is the monitor.toml to install (`None` re-installs
    /// with the one already in place).
    pub fn args(self, from: Option<&Path>) -> Vec<OsString> {
        let mut v: Vec<OsString> = vec!["service".into()];
        match self {
            Verb::Install => {
                v.push("install".into());
                if let Some(p) = from {
                    v.push("--from".into());
                    v.push(p.as_os_str().to_owned());
                }
            }
            Verb::Start => v.push("start".into()),
            Verb::Stop => v.push("stop".into()),
            Verb::Restart => v.push("restart".into()),
            Verb::Uninstall => v.push("uninstall".into()),
            Verb::Purge => {
                v.push("uninstall".into());
                v.push("--purge".into());
            }
        }
        v
    }
}

/// Run `nettest-client service ...` as a child with captured output; for a process that is
/// already root / elevated. Returns the exit code and stdout + stderr, in that order.
pub fn run_child(args: &[OsString]) -> Result<(i32, String), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current executable: {e}"))?;
    let out = std::process::Command::new(&exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(1), text))
}

// ---- helpers shared by the platform backends ---------------------------------------------

/// Validate the config to install (`--from`, or the one already in place) and copy it, as
/// UTF-8 with comments intact, to the system location with private permissions.
pub(crate) fn stage_config(
    from: Option<&Path>,
    paths: &ServicePaths,
    out: &mut Report,
) -> Result<Resolved, ServiceError> {
    let source = match from {
        Some(p) => p.to_path_buf(),
        None if paths.config.exists() => paths.config.clone(),
        None => {
            return Err(ServiceError::usage(format!(
                "pass --from <monitor.toml> (no {} installed yet; `nettest-client monitor --example-config` prints a template)",
                paths.config.display()
            )));
        }
    };
    // Any common encoding is accepted (UTF-16 from a PowerShell redirection, ANSI from
    // Set-Content); the installed copy is always written as UTF-8, even when the source *is* the
    // installed file, so the service never has to convert again.
    let decoded = config::read_text(&source).map_err(ServiceError::usage)?;
    let cfg = config::parse_str(&decoded.text)
        .map_err(|e| ServiceError::usage(format!("{}: {e}", source.display())))?;
    let mut resolved = config::resolve(&cfg).map_err(|errs| {
        ServiceError::usage(format!(
            "{} has problems:\n  - {}",
            source.display(),
            errs.join("\n  - ")
        ))
    })?;
    resolved.encoding = decoded.encoding;
    if !decoded.encoding.is_utf8() {
        resolved.warnings.insert(0, config::encoding_warning(decoded.encoding));
    }
    for w in &resolved.warnings {
        out.line(format!("warning     {w}"));
    }
    let in_place = same_path(&source, &paths.config);
    if !in_place || !decoded.encoding.is_utf8() {
        nettest_service::files::write_private(&paths.config, decoded.text.as_bytes())?;
        out.line(format!(
            "config      {}{}{}",
            if in_place { String::new() } else { format!("{} -> ", source.display()) },
            paths.config.display(),
            if decoded.encoding.is_utf8() {
                String::new()
            } else {
                format!(" (converted from {} to UTF-8)", decoded.encoding)
            }
        ));
    }
    Ok(resolved)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Load the installed config for `status`.
pub(crate) fn installed_config(paths: &ServicePaths) -> Result<Resolved, String> {
    crate::monitor::cli::load_resolved(&paths.config).map_err(|e| e.join("; "))
}

#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) fn describe(resolved: &Resolved, paths: &ServicePaths) -> Vec<String> {
    let mut v = vec![format!("config      {}", paths.config.display())];
    v.extend(
        config::describe(resolved)
            .into_iter()
            .filter(|l| !l.starts_with("log file")),
    );
    v.push(format!("log file    {}", paths.log_file.display()));
    v
}

/// Remove the monitor's own files only and the shared directory when nothing is left in it.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) fn purge_files(paths: &ServicePaths, out: &mut Report) -> Result<(), ServiceError> {
    use nettest_service::files::{remove_dir_all_if_exists, remove_dir_if_empty, remove_file_if_exists};
    if remove_file_if_exists(&paths.config)? {
        out.line(format!("removed     {}", paths.config.display()));
    }
    if paths.log_dir_is_own {
        if let Some(dir) = paths.log_file.parent()
            && remove_dir_all_if_exists(dir)?
        {
            out.line(format!("removed     {}", dir.display()));
        }
    } else if remove_file_if_exists(&paths.log_file)? {
        out.line(format!("removed     {}", paths.log_file.display()));
    }
    if let Some(dir) = paths.config.parent()
        && remove_dir_if_empty(dir)?
    {
        out.line(format!("removed     {}", dir.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> (PathBuf, ServicePaths) {
        let dir = std::env::temp_dir().join(format!("nettest-monsvc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = ServicePaths {
            bin: dir.join("bin"),
            config: dir.join("etc").join("monitor.toml"),
            log_file: dir.join("log").join("monitor.log"),
            unit: dir.join("unit"),
            log_dir_is_own: true,
        };
        (dir, p)
    }

    #[test]
    fn stage_validates_and_copies() {
        let (dir, paths) = temp_paths("stage");
        let mut out = Report::new(None);
        let err = stage_config(None, &paths, &mut out).unwrap_err();
        assert!(err.msg.contains("--from"));
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[[targets]]\ntarget = \"wss://h\"\n").unwrap();
        let err = stage_config(Some(&bad), &paths, &mut out).unwrap_err();
        assert_eq!(err.code, EXIT_USAGE);
        assert!(err.msg.contains("wss needs"), "{}", err.msg);
        assert!(!paths.config.exists(), "nothing written on error");
        let good = dir.join("good.toml");
        std::fs::write(&good, "# keep me\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n").unwrap();
        let r = stage_config(Some(&good), &paths, &mut out).unwrap();
        assert_eq!(r.targets.len(), 1);
        let utf16 = dir.join("utf16.toml");
        let mut bytes = vec![0xFF, 0xFE];
        for u in "# keep me\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&utf16, &bytes).unwrap();
        stage_config(Some(&utf16), &paths, &mut out).unwrap();
        assert_eq!(std::fs::read_to_string(&paths.config).unwrap(), "# keep me\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n");
        let r2 = stage_config(None, &paths, &mut out).unwrap();
        assert_eq!(r2.targets[0].name, "ping://10.0.0.1");
        // An installed copy that is somehow UTF-16 (hand-edited in place) is rewritten as UTF-8
        // on the next install without --from; an ANSI source converts too.
        std::fs::write(&paths.config, &bytes).unwrap();
        let r3 = stage_config(None, &paths, &mut out).unwrap();
        assert!(r3.warnings[0].contains("UTF-16 LE"), "{:?}", r3.warnings);
        assert_eq!(std::fs::read_to_string(&paths.config).unwrap(), "# keep me\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n");
        let ansi = dir.join("ansi.toml");
        std::fs::write(&ansi, b"# caf\xE9\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n").unwrap();
        let r4 = stage_config(Some(&ansi), &paths, &mut out).unwrap();
        assert!(r4.warnings[0].contains("Windows-1252"), "{:?}", r4.warnings);
        assert!(std::fs::read_to_string(&paths.config).unwrap().starts_with("# caf\u{E9}\n"));
        assert!(describe(&r2, &paths).iter().any(|l| l.contains("monitor.log")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn purge_keeps_neighbours() {
        let (dir, paths) = temp_paths("purge");
        std::fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        std::fs::create_dir_all(paths.log_file.parent().unwrap()).unwrap();
        std::fs::write(&paths.config, "x").unwrap();
        std::fs::write(paths.config.parent().unwrap().join("server.toml"), "y").unwrap();
        std::fs::write(&paths.log_file, "z").unwrap();
        purge_files(&paths, &mut Report::new(None)).unwrap();
        assert!(!paths.config.exists());
        assert!(paths.config.parent().unwrap().join("server.toml").exists());
        assert!(!paths.log_file.parent().unwrap().exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
