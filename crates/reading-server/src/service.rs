//! Running under the Windows Service Control Manager.
//!
//! A service is not simply a program Windows launches. Within about thirty
//! seconds of starting, the process must connect back to the SCM, register a
//! handler for stop and shutdown requests, and report that it is running.
//! A program that does none of that is killed with "the service did not
//! respond to the start request in a timely fashion" — which is what an
//! ordinary console binary registered with `sc.exe create` will always do,
//! however well it works from a terminal.
//!
//! So the handshake happens before anything else, and the tokio runtime is
//! built inside it rather than by `#[tokio::main]`.

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

pub const SERVICE_NAME: &str = "reading-server";

/// Windows reports this when nothing is listening for a service to attach to
/// — that is, when the program was started from a terminal.
const NOT_A_SERVICE: i32 = 1063;

define_windows_service!(ffi_service_main, service_main);

/// Hand over to the SCM if it started us.
///
/// Returns `Ok(true)` when the process ran as a service and has now finished,
/// and `Ok(false)` when there is no SCM to talk to and the caller should carry
/// on as an ordinary program.
pub fn run_if_launched_by_windows() -> anyhow::Result<bool> {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Ok(true),
        Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(NOT_A_SERVICE) => {
            Ok(false)
        }
        Err(e) => Err(anyhow::anyhow!("could not start as a service: {e}")),
    }
}

fn service_main(_arguments: Vec<OsString>) {
    // Nowhere to report a failure to at this point: the SCM is the only thing
    // listening, and if the handshake below fails there is no channel to it.
    // Whatever went wrong is written to the log file instead.
    if let Err(e) = run() {
        log_to_file(&format!("service failed: {e}"));
    }
}

fn run() -> anyhow::Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel();

    // Registered before anything slow. Until this exists Windows has no way
    // to ask the service to stop, and a service that cannot be stopped is one
    // that has to be killed.
    let status_handle = service_control_handler::register(SERVICE_NAME, move |control| {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            // Answering Interrogate is how the SCM confirms the service is
            // still alive.
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })?;

    let running = |state: ServiceState, controls: ServiceControlAccept| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: controls,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        // Only meaningful while starting or stopping; it tells the SCM how
        // long to wait before deciding the service has hung.
        wait_hint: Duration::from_secs(30),
        process_id: None,
    };

    // Connecting to Postgres and reading the configuration takes a moment,
    // and the SCM is counting. StartPending buys that time honestly.
    status_handle.set_service_status(running(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
    ))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // The listener is up by the time `serve` awaits, so reporting Running
    // once the runtime is spinning would be a lie only if startup failed --
    // and in that case the error below reports it and the service stops.
    status_handle.set_service_status(running(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    ))?;

    let outcome = runtime.block_on(crate::serve(async move {
        // A blocking receive on a runtime thread would hold one hostage for
        // the life of the service, so the wait happens off it.
        let _ = tokio::task::spawn_blocking(move || stop_rx.recv()).await;
    }));

    if let Err(e) = &outcome {
        log_to_file(&format!("stopped with an error: {e}"));
    }

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        // Reported so `sc query` shows a failure rather than a clean stop,
        // and so the recovery actions set by the installer actually fire.
        exit_code: if outcome.is_ok() {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(1)
        },
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    outcome
}

/// Write beside the binary, since a service has no console to print to.
///
/// Best effort: if even this fails there is genuinely nowhere left to say so.
fn log_to_file(message: &str) {
    use std::io::Write;

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("service.log"))
    {
        let _ = writeln!(file, "{} {message}", chrono::Utc::now().to_rfc3339());
    }
}
