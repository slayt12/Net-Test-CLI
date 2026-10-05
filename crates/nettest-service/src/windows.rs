//! Windows backend: Service Control Manager via the `windows-service` crate, plus the one
//! service entry point per process.
//!
//! The SCM starts the executable with the registered launch arguments and expects it to call
//! `StartServiceCtrlDispatcher` within 30 s; `run_as_service` does that. `define_windows_service!`
//! expands to an `extern "system"` trampoline that calls a handler by name, so it is invoked
//! exactly once here and the binary-specific work arrives as a boxed `ServiceBody` stored in a
//! static before the dispatcher starts. `service_main` (on the SCM's thread) registers the
//! control handler, builds a tokio runtime, runs the body, and reports Stopped with its code.

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceDependency, ServiceErrorControl,
    ServiceExitCode, ServiceInfo, ServiceStartType, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub use windows_service::service::{Service, ServiceState};

use crate::{EXIT_USAGE, ServiceError, ServiceSpec};

pub fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, ServiceError> {
    Ok(ServiceManager::local_computer(None::<&str>, access)?)
}

/// Registration record: auto-start, own process, after Tcpip, LocalSystem unless `account`.
pub fn service_info(
    spec: &ServiceSpec,
    exe: &Path,
    launch_arguments: Vec<OsString>,
    account: Option<&str>,
) -> ServiceInfo {
    ServiceInfo {
        name: OsString::from(spec.name),
        display_name: OsString::from(spec.display_name),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments,
        dependencies: vec![ServiceDependency::Service(OsString::from("Tcpip"))],
        account_name: account.map(OsString::from),
        account_password: None,
    }
}

pub fn wait_for(service: &Service, state: ServiceState) -> Result<(), ServiceError> {
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

pub fn start_service(service: &Service) -> Result<(), ServiceError> {
    if service.query_status()?.current_state == ServiceState::Running {
        return Ok(());
    }
    service.start::<&std::ffi::OsStr>(&[])?;
    wait_for(service, ServiceState::Running)
}

pub fn stop_service(service: &Service) -> Result<(), ServiceError> {
    if service.query_status()?.current_state == ServiceState::Stopped {
        return Ok(());
    }
    let _ = service.stop();
    wait_for(service, ServiceState::Stopped)
}

/// Create the service, or stop and reconfigure it when it already exists; sets the description.
/// The returned handle has query/start/stop/change-config access.
pub fn install_or_update(spec: &ServiceSpec, info: ServiceInfo) -> Result<Service, ServiceError> {
    let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;
    let service = match mgr.open_service(spec.name, access) {
        Ok(existing) => {
            stop_service(&existing)?;
            existing.change_config(&info)?;
            existing
        }
        Err(_) => mgr.create_service(&info, access)?,
    };
    service.set_description(spec.description)?;
    Ok(service)
}

pub fn open_for_control(spec: &ServiceSpec) -> Result<Service, ServiceError> {
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    mgr.open_service(
        spec.name,
        ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::QUERY_CONFIG,
    )
    .map_err(|_| {
        ServiceError::usage(format!(
            "service is not installed; run `{} service install` first",
            spec.program
        ))
    })
}

/// Current SCM state, `None` when the service is not registered (or the SCM is unreachable).
pub fn query_state(spec: &ServiceSpec) -> Option<ServiceState> {
    manager(ServiceManagerAccess::CONNECT)
        .ok()
        .and_then(|m| m.open_service(spec.name, ServiceAccess::QUERY_STATUS).ok())
        .and_then(|s| s.query_status().ok())
        .map(|st| st.current_state)
}

/// Stop and delete. `Ok(false)` when it was not installed.
pub fn delete(spec: &ServiceSpec) -> Result<bool, ServiceError> {
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    match mgr.open_service(
        spec.name,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        Ok(service) => {
            stop_service(&service)?;
            service.delete()?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

// ---- the service host --------------------------------------------------------------------

/// Handed to the body: cancelled on SCM Stop/Shutdown; `running()` reports Running once startup
/// has succeeded (until then the SCM sees StartPending).
pub struct ServiceCtl {
    pub cancel: CancellationToken,
    handle: ServiceStatusHandle,
}

impl ServiceCtl {
    pub fn running(&self) {
        set_state(
            &self.handle,
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ServiceExitCode::Win32(0),
        );
    }
}

/// The binary-specific part of a service: given the config path and the control handle, run
/// until cancelled and return the service-specific exit code (0 = clean).
pub type ServiceBody =
    Box<dyn FnOnce(PathBuf, ServiceCtl) -> Pin<Box<dyn Future<Output = u32>>> + Send>;

struct ServiceEntry {
    name: &'static str,
    config: PathBuf,
    body: ServiceBody,
}

static ENTRY: Mutex<Option<ServiceEntry>> = Mutex::new(None);

/// Hand control to the SCM. Returns only when the dispatcher is done (service stopped) or was
/// not started by the SCM at all (`EXIT_USAGE`, with `foreground_hint` in the message).
pub fn run_as_service(
    spec: &'static ServiceSpec,
    config: PathBuf,
    body: ServiceBody,
    foreground_hint: &str,
) -> ExitCode {
    if let Ok(mut e) = ENTRY.lock() {
        *e = Some(ServiceEntry {
            name: spec.name,
            config,
            body,
        });
    }
    match service_dispatcher::start(spec.name, ffi_service_main) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("not started by the Service Control Manager ({e}); {foreground_hint}");
            ExitCode::from(EXIT_USAGE)
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
    let Some(entry) = ENTRY.lock().ok().and_then(|mut e| e.take()) else {
        return;
    };
    let stop = CancellationToken::new();
    let stop2 = stop.clone();
    let handle = match service_control_handler::register(entry.name, move |ctl| match ctl {
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
                ServiceExitCode::ServiceSpecific(EXIT_USAGE as u32),
            );
            return;
        }
    };
    let ctl = ServiceCtl {
        cancel: stop,
        handle,
    };
    let code: u32 = rt.block_on((entry.body)(entry.config, ctl));
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
