//! Windows service entry: run the host under the Service Control Manager.
//!
//! The SCM starts the binary with `--service` followed by the normal serve arguments:
//! `slipstream-host --service serve --gamestream --mgmt-bind 0.0.0.0:47990`.
//! The flag is explicit: a console launch never calls `StartServiceCtrlDispatcherW`,
//! so console behavior and accepted arguments remain unchanged.
//!
//! Lifecycle: `ServiceMain` registers the status handle, reports `START_PENDING`,
//! parses the supplied serve args through the same parser used by the console, and
//! starts `gamestream::serve` with an SCM stop watcher. Once startup has bound all
//! listeners the watcher reports `RUNNING`. `SERVICE_CONTROL_STOP` and
//! `SERVICE_CONTROL_SHUTDOWN` report `STOP_PENDING`, signal the watcher, and stop
//! live sessions plus the virtual display. The serve future unwinds with the normal
//! `HostStopping` event before `STOPPED` is reported.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};

use anyhow::Context as _;
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, NO_ERROR};
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
    SERVICE_STATUS_CURRENT_STATE, SERVICE_TABLE_ENTRYW,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

/// Service name registered with the SCM. Operator `sc create` must use this exact name.
pub(crate) const SERVICE_NAME: &str = "slipstream-host";

/// Current service state, mirrored to the SCM on every transition.
static CURRENT_STATE: AtomicU32 = AtomicU32::new(1); // SERVICE_STOPPED
/// Raw status handle: the wrapper is neither Send nor Sync, so rebuild on each use.
static STATUS_HANDLE: OnceLock<usize> = OnceLock::new();
/// Raw auto-reset Win32 event handle, shared with the serve watcher. Stored as a
/// pointer-sized integer because the windows crate handle wrapper is not `Sync`.
static STOP_EVENT: OnceLock<usize> = OnceLock::new();

fn status_handle() -> Option<SERVICE_STATUS_HANDLE> {
    STATUS_HANDLE
        .get()
        .map(|raw| SERVICE_STATUS_HANDLE(*raw as *mut core::ffi::c_void))
}

fn stop_event_handle() -> Option<HANDLE> {
    STOP_EVENT
        .get()
        .map(|raw| HANDLE(*raw as *mut core::ffi::c_void))
}

/// Dispatch only when `--service` was explicitly supplied. Blocks until the service stops.
pub fn dispatch() -> anyhow::Result<()> {
    // SAFETY: two-entry null-terminated service table; `name` is leaked for the
    // process lifetime because the SCM may read it until the dispatcher returns.
    // `service_main` is a valid LPSERVICE_MAIN_FUNCTIONW with the system ABI.
    unsafe {
        let name: &'static mut [u16] = Box::leak(
            w!("slipstream-host")
                .as_wide()
                .iter()
                .copied()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
                .into_boxed_slice(),
        );
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: windows::core::PWSTR(name.as_mut_ptr()),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW::default(),
        ];
        StartServiceCtrlDispatcherW(table.as_ptr()).context("StartServiceCtrlDispatcherW")?;
        Ok(())
    }
}

/// Install the auto-reset stop event once, before the SCM can call the handler.
fn install_stop_event() {
    // SAFETY: unnamed, non-inheritable, auto-reset event; null attributes/name are valid.
    let event = unsafe {
        CreateEventW(None, false, false, windows::core::PCWSTR::null())
            .expect("CreateEventW for service stop")
    };
    let _ = STOP_EVENT.set(event.0 as usize);
}

unsafe extern "system" fn service_main(argc: u32, argv: *mut windows::core::PWSTR) {
    install_stop_event();
    // SAFETY: SCM-owned ServiceMain thread; the raw handle is rebuilt only on
    // threads that share this process and outlive the service.
    let handle = unsafe {
        match RegisterServiceCtrlHandlerExW(w!("slipstream-host"), Some(handler_ex), None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("slipstream-host service: handler registration failed: {e}");
                return;
            }
        }
    };
    let _ = STATUS_HANDLE.set(handle.0 as usize);
    report_state(SERVICE_STATUS_CURRENT_STATE(2), 0, 1, 10_000); // START_PENDING

    let args: Vec<String> = if argc > 1 && !argv.is_null() {
        // SAFETY: argv points to argc SCM-owned wide strings valid for ServiceMain.
        let raw = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
        raw[1..]
            .iter()
            .filter_map(|p| unsafe { p.to_string().ok() })
            .collect()
    } else {
        Vec::new()
    };

    // The SCM argv includes the executable path plus `--service serve ...`.
    let serve_args = match crate::service_serve_args(&args) {
        Some(rest) => rest,
        None => {
            eprintln!("slipstream-host service: expected '--service serve [options]'");
            report_state(SERVICE_STATUS_CURRENT_STATE(1), 1, 0, 0);
            return;
        }
    };
    let (mgmt_opts, native, gamestream) = match crate::parse_serve(&serve_args[1..]) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("slipstream-host service: bad serve args: {e:#}");
            report_state(SERVICE_STATUS_CURRENT_STATE(1), 1, 0, 0);
            return;
        }
    };

    let exit_code = match crate::gamestream::serve_with_stop(
        mgmt_opts,
        native,
        gamestream,
        service_stop_future(),
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("slipstream-host service: serve failed: {e:#}");
            1
        }
    };
    report_state(SERVICE_STATUS_CURRENT_STATE(1), exit_code, 0, 0); // STOPPED
}

unsafe extern "system" fn handler_ex(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report_state(SERVICE_STATUS_CURRENT_STATE(3), 0, 2, 30_000); // STOP_PENDING
            // The control handler must return immediately. The blocking waiter performs
            // the same graceful sequence as the console's SIGTERM path: stop sessions
            // deliberately and hand the display back before the serve future unwinds.
            if let Some(event) = stop_event_handle() {
                // SAFETY: `event` is the live auto-reset handle; SetEvent is callable
                // from a service control handler and never blocks.
                unsafe {
                    let _ = SetEvent(event);
                }
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => {
            report_state(
                SERVICE_STATUS_CURRENT_STATE(CURRENT_STATE.load(Ordering::SeqCst)),
                0,
                0,
                0,
            );
            NO_ERROR.0
        }
        _ => 1, // ERROR_CALL_NOT_IMPLEMENTED
    }
}

fn report_state(state: SERVICE_STATUS_CURRENT_STATE, exit_code: u32, checkpoint: u32, wait_ms: u32) {
    CURRENT_STATE.store(state.0, Ordering::SeqCst);
    if let Some(handle) = status_handle() {
        // SAFETY: handle is the live registration from ServiceMain; SERVICE_STATUS
        // is a plain C struct with no ownership requirements.
        unsafe {
            let _ = SetServiceStatus(
                handle,
                &SERVICE_STATUS {
                    dwServiceType: windows::Win32::System::Services::SERVICE_WIN32_OWN_PROCESS,
                    dwCurrentState: state,
                    dwControlsAccepted: if state.0 == 4 {
                        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
                    } else {
                        0
                    },
                    dwWin32ExitCode: exit_code,
                    dwServiceSpecificExitCode: 0,
                    dwCheckPoint: checkpoint,
                    dwWaitHint: wait_ms,
                },
            );
        }
    }
}

/// Report RUNNING once listeners are bound. Called by the serve startup hook.
pub(crate) fn report_running() {
    report_state(SERVICE_STATUS_CURRENT_STATE(4), 0, 0, 0);
}

/// Async wrapper used by the serve loop. The blocking Win32 wait runs on a
/// Tokio blocking thread so it never stalls the reactor.
pub(crate) async fn service_stop_future() {
    let _ = tokio::task::spawn_blocking(|| {
        wait_for_service_stop();
        crate::session::status::stop_all_quit();
        crate::vdisplay::restore_takeover_now();
    })
    .await;
}

/// Wait for STOP/SHUTDOWN. The wait runs only on a Tokio blocking thread.
fn wait_for_service_stop() {
    if let Some(event) = stop_event_handle() {
        // SAFETY: live auto-reset handle; the caller is a dedicated blocking thread.
        unsafe {
            let _ = WaitForSingleObject(event, windows::Win32::System::Threading::INFINITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_state_constants_match_windows_api() {
        assert_eq!(SERVICE_NAME, "slipstream-host");
        assert_eq!(windows::Win32::System::Services::SERVICE_RUNNING.0, 4);
        assert_eq!(windows::Win32::System::Services::SERVICE_START_PENDING.0, 2);
        assert_eq!(windows::Win32::System::Services::SERVICE_STOP_PENDING.0, 3);
        assert_eq!(windows::Win32::System::Services::SERVICE_STOPPED.0, 1);
    }
}
