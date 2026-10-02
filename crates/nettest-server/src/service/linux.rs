//! systemd backend: unit file + `systemctl`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use super::systemd::{UnitParams, exec_from_unit, render_unit};
use super::{
    EditArgs, InstallArgs, Report, SERVICE_NAME, ServiceError, build_edit_config,
    build_install_config, cert_fingerprint, copy_binary, describe_config, needs_low_port,
    service_paths,
};

fn require_systemd() -> Result<(), ServiceError> {
    if Path::new("/run/systemd/system").is_dir() {
        Ok(())
    } else {
        Err(ServiceError::usage(
            "systemd not detected (/run/systemd/system missing); no other init system is supported",
        ))
    }
}

fn systemctl(args: &[&str]) -> Result<String, ServiceError> {
    let out = Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ServiceError::manager(format!("could not run systemctl: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(ServiceError::manager(format!(
            "systemctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

fn active_state() -> String {
    Command::new("systemctl")
        .args(["is-active", SERVICE_NAME])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn write_unit(unit_path: &Path, text: &str) -> Result<(), ServiceError> {
    if let Some(dir) = unit_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(unit_path, text)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(unit_path, std::fs::Permissions::from_mode(0o644))?;
    Ok(())
}

fn report_state(out: &mut Report, cfg: &nettest_proto::config::ServerConfig) {
    let paths = service_paths();
    out.line(format!(
        "service     {SERVICE_NAME}.service: {}",
        active_state()
    ));
    for l in describe_config(cfg, &paths) {
        out.line(l);
    }
    if cfg.wss_port != 0 {
        match cert_fingerprint(&cfg.cert_dir) {
            Some(fp) => out.line(format!("wss SHA-256 {fp}")),
            None => out.line(
                "wss SHA-256 (certificate not generated yet; see `journalctl -u nettest-server`)",
            ),
        }
    }
    out.line(format!(
        "logs        journalctl -u {SERVICE_NAME}  (also {})",
        cfg.log_file
    ));
}

pub fn install(a: InstallArgs, out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    let paths = service_paths();
    let cfg = build_install_config(&a.flags, a.generate_token, &paths, out)?;
    let exec = if a.no_copy {
        std::env::current_exe()?
    } else {
        copy_binary(&paths.bin)?
    };
    nettest_proto::config::save_private(&paths.config, &cfg)?;
    let unit = render_unit(&UnitParams {
        exec: exec.clone(),
        config: paths.config.clone(),
        needs_low_port: needs_low_port(&cfg),
    });
    write_unit(&paths.unit, &unit)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", SERVICE_NAME])?;
    // Give the unit a moment so the state and the generated certificate are visible.
    std::thread::sleep(std::time::Duration::from_millis(700));
    out.line(format!(
        "installed   {} (exec {})",
        paths.unit.display(),
        exec.display()
    ));
    report_state(out, &cfg);
    if needs_low_port(&cfg) {
        out.line("note        a port below 1024 is in use; the unit grants CAP_NET_BIND_SERVICE");
    }
    out.line("firewall    open the listener ports inbound (firewalld/ufw/nftables) if a host firewall is active");
    Ok(())
}

pub fn edit(a: EditArgs, out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    let paths = service_paths();
    if !paths.unit.exists() {
        return Err(ServiceError::usage(format!(
            "service is not installed (no {}); run `nettest-server service install` first",
            paths.unit.display()
        )));
    }
    let cfg = build_edit_config(&a.flags, a.generate_token, &paths, out)?;
    nettest_proto::config::save_private(&paths.config, &cfg)?;
    let old = std::fs::read_to_string(&paths.unit)?;
    let exec = exec_from_unit(&old).unwrap_or_else(|| paths.bin.clone());
    let unit = render_unit(&UnitParams {
        exec,
        config: paths.config.clone(),
        needs_low_port: needs_low_port(&cfg),
    });
    if unit != old {
        write_unit(&paths.unit, &unit)?;
        systemctl(&["daemon-reload"])?;
    }
    systemctl(&["restart", SERVICE_NAME])?;
    std::thread::sleep(std::time::Duration::from_millis(700));
    out.line("updated     settings saved and service restarted");
    report_state(out, &cfg);
    Ok(())
}

pub fn uninstall(purge: bool, out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    let paths = service_paths();
    if paths.unit.exists() {
        // Tolerate a unit that is already stopped/disabled.
        let _ = Command::new("systemctl")
            .args(["disable", "--now", SERVICE_NAME])
            .status();
        std::fs::remove_file(&paths.unit)?;
        systemctl(&["daemon-reload"])?;
        out.line(format!("removed     {}", paths.unit.display()));
    } else {
        out.line("service was not installed");
    }
    if paths.bin.exists() {
        std::fs::remove_file(&paths.bin)?;
        out.line(format!("removed     {}", paths.bin.display()));
    }
    if purge {
        for dir in [
            paths.config.parent().map(Path::to_path_buf),
            Some(paths.cert_dir.clone()),
            paths.log_file.parent().map(Path::to_path_buf),
        ]
        .into_iter()
        .flatten()
        {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                out.line(format!("removed     {}", dir.display()));
            }
        }
    } else {
        out.line(format!(
            "kept        {} and {} (use --purge to delete)",
            paths.config.display(),
            paths.cert_dir.display()
        ));
    }
    Ok(())
}

pub fn start(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    systemctl(&["start", SERVICE_NAME])?;
    out.line(format!("{SERVICE_NAME}: {}", active_state()));
    Ok(())
}

pub fn stop(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    systemctl(&["stop", SERVICE_NAME])?;
    out.line(format!("{SERVICE_NAME}: {}", active_state()));
    Ok(())
}

pub fn restart(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    systemctl(&["restart", SERVICE_NAME])?;
    out.line(format!("{SERVICE_NAME}: {}", active_state()));
    Ok(())
}

pub fn status(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    let paths = service_paths();
    if !paths.unit.exists() {
        out.line(format!(
            "{SERVICE_NAME}: not installed ({} missing)",
            paths.unit.display()
        ));
        return Ok(());
    }
    // `systemctl status` exits non-zero for an inactive unit; its text is still the answer.
    let st = Command::new("systemctl")
        .args(["status", "--no-pager", "--lines=5", SERVICE_NAME])
        .output()?;
    out.line(String::from_utf8_lossy(&st.stdout).trim_end());
    match nettest_proto::config::load::<nettest_proto::config::ServerConfig>(&paths.config) {
        Ok(cfg) => report_state(out, &cfg),
        Err(e) => out.line(format!(
            "settings    not readable ({e}); run `sudo nettest-server service status`"
        )),
    }
    Ok(())
}

pub fn run_as_service(_: PathBuf) -> ExitCode {
    eprintln!(
        "'service run' is the Windows service entry point; systemd runs `nettest-server --no-tui` directly"
    );
    ExitCode::from(super::EXIT_USAGE)
}
