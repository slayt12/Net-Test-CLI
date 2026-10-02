//! Windows backend: Service Control Manager via the `windows-service` crate, plus the service
//! entry point itself (`service run`).
//!
//! The SCM starts the executable with the registered launch arguments and expects it to call
//! `StartServiceCtrlDispatcher` within 30 s; `run_as_service` does that, then `service_main`
//! (on the SCM's thread) builds a tokio runtime, starts the listeners, reports Running and waits
//! for Stop/Shutdown. Settings are read from the `--config` path registered at install time.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Duration;

use nettest_server::log::{LogOptions, ServerLog};
use tokio_util::sync::CancellationToken;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceDependency, ServiceErrorControl,
    ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use super::{
    DESCRIPTION, DISPLAY_NAME, EditArgs, InstallArgs, Report, SERVICE_NAME, ServiceError,
    build_edit_config, build_install_config, cert_fingerprint, copy_binary, describe_config,
    service_paths,
};

impl From<windows_service::Error> for ServiceError {
    fn from(e: windows_service::Error) -> Self {
        ServiceError::manager(e.to_string())
    }
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, ServiceError> {
    Ok(ServiceManager::local_computer(None::<&str>, access)?)
}

fn service_info(
    exe: &std::path::Path,
    config: &std::path::Path,
    account: Option<&str>,
) -> ServiceInfo {
    ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: vec![
            OsString::from("service"),
            OsString::from("run"),
            OsString::from("--config"),
            config.as_os_str().to_os_string(),
        ],
        dependencies: vec![ServiceDependency::Service(OsString::from("Tcpip"))],
        account_name: account.map(OsString::from),
        account_password: None,
    }
}

fn wait_for(
    service: &windows_service::service::Service,
    state: ServiceState,
) -> Result<(), ServiceError> {
    for _ in 0..40 {
        if service.query_status()?.current_state == state {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(ServiceError::manager(format!(
        "service did not reach state {state:?} within 10 s"
    )))
}

fn start_service(service: &windows_service::service::Service) -> Result<(), ServiceError> {
    if service.query_status()?.current_state == ServiceState::Running {
        return Ok(());
    }
    service.start::<&std::ffi::OsStr>(&[])?;
    wait_for(service, ServiceState::Running)
}

fn stop_service(service: &windows_service::service::Service) -> Result<(), ServiceError> {
    if service.query_status()?.current_state == ServiceState::Stopped {
        return Ok(());
    }
    let _ = service.stop();
    wait_for(service, ServiceState::Stopped)
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
    let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let info = service_info(&exe, &paths.config, a.account.as_deref());
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;
    let service = match mgr.open_service(SERVICE_NAME, access) {
        Ok(existing) => {
            stop_service(&existing)?;
            existing.change_config(&info)?;
            existing
        }
        Err(_) => mgr.create_service(&info, access)?,
    };
    service.set_description(DESCRIPTION)?;
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
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::QUERY_CONFIG;
    let service = mgr.open_service(SERVICE_NAME, access).map_err(|_| {
        ServiceError::usage("service is not installed; run `nettest-server service install` first")
    })?;
    let cfg = build_edit_config(&a.flags, a.generate_token, &paths, out)?;
    nettest_proto::config::save_private(&paths.config, &cfg)?;
    if let Some(account) = &a.account {
        let current = service.query_config()?;
        service.change_config(&service_info(
            &current.executable_path,
            &paths.config,
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
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    match mgr.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        Ok(service) => {
            stop_service(&service)?;
            service.delete()?;
            out.line(format!("removed     service {SERVICE_NAME}"));
        }
        Err(_) => out.line("service was not installed"),
    }
    if paths.bin.exists() {
        // The SCM may still hold the image for a moment after Stopped.
        let mut last = None;
        for _ in 0..20 {
            match std::fs::remove_file(&paths.bin) {
                Ok(()) => {
                    last = None;
                    break;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
        match last {
            None => out.line(format!("removed     {}", paths.bin.display())),
            Some(e) => out.line(format!("could not remove {}: {e}", paths.bin.display())),
        }
    }
    if purge {
        if let Some(dir) = paths.config.parent()
            && dir.exists()
        {
            std::fs::remove_dir_all(dir)?;
            out.line(format!("removed     {}", dir.display()));
        }
    } else {
        out.line(format!(
            "kept        {} (use --purge to delete)",
            paths.config.display()
        ));
    }
    Ok(())
}

fn open_for_control() -> Result<windows_service::service::Service, ServiceError> {
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    mgr.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP,
    )
    .map_err(|_| ServiceError::usage("service is not installed"))
}

pub fn start(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control()?;
    start_service(&s)?;
    out.line(format!(
        "{SERVICE_NAME}: {:?}",
        s.query_status()?.current_state
    ));
    Ok(())
}

pub fn stop(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control()?;
    stop_service(&s)?;
    out.line(format!(
        "{SERVICE_NAME}: {:?}",
        s.query_status()?.current_state
    ));
    Ok(())
}

pub fn restart(out: &mut Report) -> Result<(), ServiceError> {
    let s = open_for_control()?;
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
    let state = manager(ServiceManagerAccess::CONNECT)
        .ok()
        .and_then(|m| {
            m.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
                .ok()
        })
        .and_then(|s| s.query_status().ok())
        .map(|st| st.current_state);
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

static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn run_as_service(config: PathBuf) -> ExitCode {
    let _ = CONFIG_PATH.set(config);
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "not started by the Service Control Manager ({e}); for a foreground run use `nettest-server --no-tui`"
            );
            ExitCode::from(super::EXIT_USAGE)
        }
    }
}

define_windows_service!(ffi_service_main, service_main);

fn set_state(
    handle: &ServiceStatusHandle,
    state: ServiceState,
    accept: ServiceControlAccept,
    exit: ServiceExitCode,
) {
    let _ = handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: exit,
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    });
}

fn service_main(_argv: Vec<OsString>) {
    let stop = CancellationToken::new();
    let stop2 = stop.clone();
    let handle = match service_control_handler::register(SERVICE_NAME, move |ctl| match ctl {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop2.cancel();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    }) {
        Ok(h) => h,
        Err(_) => return,
    };
    set_state(
        &handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        ServiceExitCode::Win32(0),
    );

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => {
            set_state(
                &handle,
                ServiceState::Stopped,
                ServiceControlAccept::empty(),
                ServiceExitCode::ServiceSpecific(3),
            );
            return;
        }
    };
    let code: u32 = rt.block_on(async {
        let flags = crate::cli::args::ServerFlags::default();
        let (_, cfg) = match crate::load_config(CONFIG_PATH.get().map(|p| p.as_path()), &flags) {
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
        set_state(
            &handle,
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ServiceExitCode::Win32(0),
        );
        tokio::select! {
            _ = stop.cancelled() => {}
            _ = server.ctx.cancel.cancelled() => {}
        }
        server.shutdown();
        log.info("service stopping");
        tokio::time::sleep(Duration::from_millis(50)).await;
        0
    });
    drop(rt);
    let exit = if code == 0 {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(code)
    };
    set_state(
        &handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit,
    );
}
