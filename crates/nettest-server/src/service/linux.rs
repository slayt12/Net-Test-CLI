//! systemd backend: unit file + `systemctl` (helpers in `nettest_service::linux`).

use std::path::PathBuf;
use std::process::ExitCode;

use nettest_service::files::{copy_binary, remove_binary};
use nettest_service::linux::{
    active_state, install_unit, remove_unit, require_systemd, status_text, systemctl, write_unit,
};

use super::systemd::{UnitParams, exec_from_unit, render_unit};
use super::{
    EditArgs, InstallArgs, Report, SERVICE_NAME, ServiceError, build_edit_config,
    build_install_config, cert_fingerprint, describe_config, needs_low_port, purge_files,
    service_paths,
};

fn report_state(out: &mut Report, cfg: &nettest_proto::config::ServerConfig) {
    let paths = service_paths();
    out.line(format!(
        "service     {SERVICE_NAME}.service: {}",
        active_state(SERVICE_NAME)
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
    install_unit(&paths.unit, &unit, SERVICE_NAME)?;
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
    if remove_unit(&paths.unit, SERVICE_NAME)? {
        out.line(format!("removed     {}", paths.unit.display()));
    } else {
        out.line("service was not installed");
    }
    if remove_binary(&paths.bin, 1)? {
        out.line(format!("removed     {}", paths.bin.display()));
    }
    if purge {
        purge_files(&paths, out)?;
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
    out.line(format!("{SERVICE_NAME}: {}", active_state(SERVICE_NAME)));
    Ok(())
}

pub fn stop(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    systemctl(&["stop", SERVICE_NAME])?;
    out.line(format!("{SERVICE_NAME}: {}", active_state(SERVICE_NAME)));
    Ok(())
}

pub fn restart(out: &mut Report) -> Result<(), ServiceError> {
    require_systemd()?;
    systemctl(&["restart", SERVICE_NAME])?;
    out.line(format!("{SERVICE_NAME}: {}", active_state(SERVICE_NAME)));
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
    out.line(status_text(SERVICE_NAME)?);
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
