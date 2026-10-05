//! Windows backend: install / control through the shared SCM helpers, plus the service body that
//! `nettest_service::windows::run_as_service` runs on the SCM's thread. Settings are read from the
//! `--config` path registered at install time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use nettest_server::log::{LogOptions, ServerLog};
use nettest_service::files::{copy_binary, remove_binary};
use nettest_service::windows::{
    ServiceCtl, ServiceState, delete, install_or_update, open_for_control, query_state,
    service_info, start_service, stop_service,
};

use super::{
    EditArgs, InstallArgs, Report, SERVICE_NAME, SPEC, ServiceError, build_edit_config,
    build_install_config, cert_fingerprint, describe_config, purge_files, service_paths,
};

fn launch_arguments(config: &Path) -> Vec<OsString> {
    vec![
        OsString::from("service"),
        OsString::from("run"),
        OsString::from("--config"),
        config.as_os_str().to_os_string(),
    ]
}

fn report_state(
    out: &mut Report,
    cfg: &nettest_proto::config::ServerConfig,
    state: Option<ServiceState>,
) {
    let paths = service_paths();
    out.line(format!(
        "service     {SERVICE_NAME}: {}",
        state
            .map(|s| format!("{s:?}"))
            .unwrap_or_else(|| "not installed".into())
    ));
    for l in describe_config(cfg, &paths) {
        out.line(l);
    }
    if cfg.wss_port != 0 {
        match cert_fingerprint(&cfg.cert_dir) {
            Some(fp) => out.line(format!("wss SHA-256 {fp}")),
            None => out.line("wss SHA-256 (certificate not generated yet; check the log file)"),
        }
    }
}

pub fn install(a: InstallArgs, out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    let cfg = build_install_config(&a.flags, a.generate_token, &paths, out)?;
    let exe = if a.no_copy {
        std::env::current_exe()?
    } else {
        copy_binary(&paths.bin)?
    };
    nettest_proto::config::save_private(&paths.config, &cfg)?;
    std::fs::create_dir_all(&paths.cert_dir)?;
    let info = service_info(
        &SPEC,
        &exe,
        launch_arguments(&paths.config),
        a.account.as_deref(),
    );
    let service = install_or_update(&SPEC, info)?;
    start_service(&service)?;
    std::thread::sleep(Duration::from_millis(700));
    out.line(format!(
        "installed   service {SERVICE_NAME} (exec {})",
        exe.display()
    ));
    report_state(out, &cfg, Some(service.query_status()?.current_state));
    out.line("account     ".to_string() + a.account.as_deref().unwrap_or("LocalSystem"));
    out.line("firewall    allow nettest-server.exe inbound in Windows Defender Firewall for the listener ports");
    Ok(())
}

pub fn edit(a: EditArgs, out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    let service = open_for_control(&SPEC)?;
    let cfg = build_edit_config(&a.flags, a.generate_token, &paths, out)?;
    nettest_proto::config::save_private(&paths.config, &cfg)?;
    if let Some(account) = &a.account {
        let current = service.query_config()?;
        service.change_config(&service_info(
            &SPEC,
            &current.executable_path,
            launch_arguments(&paths.config),
            Some(account),
        ))?;
    }
    stop_service(&service)?;
    start_service(&service)?;
    std::thread::sleep(Duration::from_millis(700));
    out.line("updated     settings saved and service restarted");
    report_state(out, &cfg, Some(service.query_status()?.current_state));
    Ok(())
}

pub fn uninstall(purge: bool, out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    if delete(&SPEC)? {
        out.line(format!("removed     service {SERVICE_NAME}"));
    } else {
        out.line("service was not installed");
    }
    // The SCM may still hold the image for a moment after Stopped.
    match remove_binary(&paths.bin, 20) {
        Ok(true) => out.line(format!("removed     {}", paths.bin.display())),
        Ok(false) => {}
        Err(e) => out.line(e.to_string()),
    }
    if purge {
        purge_files(&paths, out)?;
    } else {
        out.line(format!(
            "kept        {} (use --purge to delete)",
            paths.config.display()
        ));
    }
    Ok(())
}

pub fn start(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control(&SPEC)?;
    start_service(&s)?;
    out.line(format!(
        "{SERVICE_NAME}: {:?}",
        s.query_status()?.current_state
    ));
    Ok(())
}

pub fn stop(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control(&SPEC)?;
    stop_service(&s)?;
    out.line(format!(
        "{SERVICE_NAME}: {:?}",
        s.query_status()?.current_state
    ));
    Ok(())
}

pub fn restart(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control(&SPEC)?;
    stop_service(&s)?;
    start_service(&s)?;
    out.line(format!(
        "{SERVICE_NAME}: {:?}",
        s.query_status()?.current_state
    ));
    Ok(())
}

pub fn status(out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    let state = query_state(&SPEC);
    match nettest_proto::config::load::<nettest_proto::config::ServerConfig>(&paths.config) {
        Ok(cfg) if paths.config.exists() => report_state(out, &cfg, state),
        _ => out.line(format!(
            "service     {SERVICE_NAME}: {}",
            state
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "not installed".into())
        )),
    }
    Ok(())
}

// ---- the service itself -------------------------------------------------------------------

pub fn run_as_service(config: PathBuf) -> ExitCode {
    nettest_service::windows::run_as_service(
        &SPEC,
        config,
        Box::new(|cfg, ctl| Box::pin(service_body(cfg, ctl))),
        "for a foreground run use `nettest-server --no-tui`",
    )
}

/// Exit codes mirror the foreground run: 3 config / log, 2 could not start.
async fn service_body(config: PathBuf, ctl: ServiceCtl) -> u32 {
    let flags = crate::cli::args::ServerFlags::default();
    let (_, cfg) = match crate::load_config(Some(&config), &flags) {
        Ok(x) => x,
        Err(_) => return 3,
    };
    let (log, _tail) = match ServerLog::start(LogOptions {
        stderr: false,
        text_file: Some(cfg.log_file.clone()),
        jsonl_file: Some(cfg.jsonl_file.clone()),
        tail_capacity: 1,
    }) {
        Ok(x) => x,
        Err(_) => return 3,
    };
    let server = match nettest_server::start(cfg.clone(), log.clone()).await {
        Ok(s) => s,
        Err(e) => {
            log.error(format!("startup failed: {e}"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            return 2;
        }
    };
    if let Some(fp) = &server.fingerprint {
        log.info(format!(
            "wss certificate fingerprint (SHA-256): {}",
            nettest_proto::tls::fingerprint::to_hex(fp)
        ));
    }
    log.info("running as a Windows service");
    ctl.running();
    tokio::select! {
        _ = ctl.cancel.cancelled() => {}
        _ = server.ctx.cancel.cancelled() => {}
    }
    server.shutdown();
    log.info("service stopping");
    tokio::time::sleep(Duration::from_millis(50)).await;
    0
}
