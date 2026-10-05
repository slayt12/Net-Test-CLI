//! Windows backend for the monitor: install / control through the shared SCM helpers, plus the
//! service body run by `nettest_service::windows::run_as_service` on the SCM's thread.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use nettest_service::files::{copy_binary, remove_binary};
use nettest_service::windows::{
    ServiceCtl, ServiceState, delete, install_or_update, open_for_control, query_state,
    service_info, start_service, stop_service,
};

use super::{
    InstallArgs, Report, SERVICE_NAME, SPEC, ServiceError, describe, installed_config,
    purge_files, service_paths, stage_config,
};
use crate::monitor::{self, RunOptions};

fn launch_arguments(config: &Path) -> Vec<OsString> {
    vec![
        OsString::from("service"),
        OsString::from("run"),
        OsString::from("--config"),
        config.as_os_str().to_os_string(),
    ]
}

fn report_state(out: &mut Report, resolved: &super::Resolved, state: Option<ServiceState>) {
    let paths = service_paths();
    out.line(format!(
        "service     {SERVICE_NAME}: {}",
        state
            .map(|s| format!("{s:?}"))
            .unwrap_or_else(|| "not installed".into())
    ));
    for l in describe(resolved, &paths) {
        out.line(l);
    }
}

pub fn install(a: InstallArgs, out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    let resolved = stage_config(a.from.as_deref(), &paths, out)?;
    let exe = if a.no_copy {
        std::env::current_exe()?
    } else {
        copy_binary(&paths.bin)?
    };
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
    report_state(out, &resolved, Some(service.query_status()?.current_state));
    out.line("account     ".to_string() + a.account.as_deref().unwrap_or("LocalSystem"));
    out.line("edit        change the installed monitor.toml then `nettest-client service restart`");
    Ok(())
}

pub fn uninstall(purge: bool, out: &mut Report) -> Result<(), ServiceError> {
    let paths = service_paths();
    if delete(&SPEC)? {
        out.line(format!("removed     service {SERVICE_NAME}"));
    } else {
        out.line("service was not installed");
    }
    match remove_binary(&paths.bin, 20) {
        Ok(true) => out.line(format!("removed     {}", paths.bin.display())),
        Ok(false) => {}
        Err(e) => out.line(e.to_string()),
    }
    if purge {
        purge_files(&paths, out)?;
    } else {
        out.line(format!(
            "kept        {} and {} (use --purge to delete)",
            paths.config.display(),
            paths.log_file.display()
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
    let paths = service_paths();
    if let Err(e) = installed_config(&paths) {
        return Err(ServiceError::usage(format!(
            "{} is not valid, not restarting: {e}",
            paths.config.display()
        )));
    }
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
    match installed_config(&paths) {
        Ok(r) if paths.config.exists() => report_state(out, &r, state),
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
        "for a foreground run use `nettest-client monitor --config <monitor.toml>`",
    )
}

/// Exit codes mirror the foreground run: 3 config, 4 log file.
async fn service_body(config: PathBuf, ctl: ServiceCtl) -> u32 {
    let resolved = match crate::monitor::cli::load_resolved(&config) {
        Ok(r) => r,
        Err(_) => return 3,
    };
    let paths = service_paths();
    let opts = RunOptions {
        stderr: false,
        log_file: Some(resolved.log_file.clone().unwrap_or(paths.log_file)),
        jsonl_file: resolved.jsonl_file.clone(),
    };
    ctl.running();
    monitor::run(resolved, opts, ctl.cancel.clone()).await as u32
}
