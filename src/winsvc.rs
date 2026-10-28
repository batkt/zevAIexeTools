//! Windows Service Control Manager plumbing: running as a service, and the
//! install / uninstall / start / stop / status commands that manage it.
//!
//! `query()` is deliberately separate from `status()`: the desktop app polls
//! it several times a minute and must work without Administrator rights,
//! while everything that *changes* the service still requires elevation.

use crate::config::Config;
use crate::{SvcStatus, SERVICE_DESC, SERVICE_DISPLAY, SERVICE_NAME};

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::runner::{init_logging, install_panic_hook, run_app};
    use log::{error, LevelFilter};
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
            ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    define_windows_service!(ffi_service_main, service_main);

    pub fn dispatch() {
        if let Err(e) = service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
            eprintln!("Service dispatcher failed: {e}");
            eprintln!("Run 'zev-detector.exe run' for console mode, or 'install' first.");
            eprintln!("To open the desktop app, run zev-detector-ui.exe.");
            std::process::exit(1);
        }
    }

    /// Non-elevated status read — safe to call from the desktop app.
    pub fn query() -> SvcStatus {
        let manager =
            match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) {
                Ok(m) => m,
                Err(_) => return SvcStatus::not_installed(),
            };
        match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
            Ok(svc) => match svc.query_status() {
                Ok(s) => SvcStatus {
                    installed: true,
                    state: Some(format!("{:?}", s.current_state)),
                    pid: s.process_id,
                },
                Err(_) => SvcStatus { installed: true, state: None, pid: None },
            },
            Err(_) => SvcStatus::not_installed(),
        }
    }

    fn service_main(args: Vec<OsString>) {
        if let Err(e) = run_service(args) {
            error!("Service fatal error: {e}");
        }
    }

    fn run_service(_args: Vec<OsString>) -> anyhow::Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_handler = Arc::clone(&stop);

        let event_handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    stop_handler.store(true, Ordering::Relaxed);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        let set = |state: ServiceState, accept: ServiceControlAccept, exit: ServiceExitCode| {
            ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: exit,
                checkpoint: 0,
                wait_hint: Duration::from_secs(15),
                process_id: None,
            }
        };

        status_handle.set_service_status(set(
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
        ))?;

        init_logging(LevelFilter::Info);
        install_panic_hook();

        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                error!("Config load failed: {e}");
                status_handle.set_service_status(set(
                    ServiceState::Stopped,
                    ServiceControlAccept::empty(),
                    ServiceExitCode::ServiceSpecific(1),
                ))?;
                return Ok(());
            }
        };

        status_handle.set_service_status(set(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            ServiceExitCode::Win32(0),
        ))?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime");

        rt.block_on(async {
            if let Err(e) = run_app(cfg, stop).await {
                error!("App error: {e}");
            }
        });

        status_handle.set_service_status(set(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
        ))?;
        Ok(())
    }

    pub fn install() {
        let manager = match ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        ) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("✗ Cannot open Service Control Manager: {e}");
                eprintln!("  Run this from an Administrator command prompt.");
                std::process::exit(1);
            }
        };

        let exe = std::env::current_exe().expect("Cannot resolve exe path");

        // Deliberately does NOT write a starter config.toml. The desktop app
        // treats "no config.toml" as "never set up" and opens its wizard; a
        // sample config dropped here would look configured, skip the wizard,
        // and leave the service posting to the sample URL.
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.clone(),
            launch_arguments: vec![],
            dependencies: vec![],
            // LocalSystem — needed to read the Security channel and query
            // other services' status.
            account_name: None,
            account_password: None,
        };

        let created = manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START);
        let svc = match created {
            Ok(s) => {
                s.set_description(SERVICE_DESC).ok();
                let _ = std::process::Command::new("sc")
                    .args([
                        "failure",
                        SERVICE_NAME,
                        "reset=",
                        "60",
                        "actions=",
                        "restart/5000/restart/10000/restart/30000",
                    ])
                    .output();

                println!("✓ Service '{SERVICE_NAME}' registered ({SERVICE_DISPLAY}).");
                println!("  Auto-restart on failure: 5s / 10s / 30s");
                s
            }
            Err(e) => {
                println!("! Service already registered or create notice ({e}), opening existing...");
                let exe_str = exe.to_string_lossy();
                let _ = std::process::Command::new("sc")
                    .args([
                        "config",
                        SERVICE_NAME,
                        &format!("binPath= \"{}\"", exe_str),
                    ])
                    .output();
                match manager.open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START) {
                    Ok(s) => s,
                    Err(oe) => {
                        eprintln!("✗ Cannot open service: {oe}");
                        std::process::exit(1);
                    }
                }
            }
        };

        match svc.start::<&str>(&[]) {
            Ok(_) => println!("✓ Service '{SERVICE_NAME}' started successfully."),
            Err(e) => {
                let _ = std::process::Command::new("net").args(["start", SERVICE_NAME]).output();
                println!("  Service start triggered ({e}).");
            }
        }
        println!("✓ Zev Detector is now running in zero-configuration mode.");
    }

    pub fn uninstall() {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .expect("Cannot open Service Control Manager — run as Administrator");
        let svc =
            match manager.open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("✗ Service not found: {e}");
                    std::process::exit(1);
                }
            };
        let _ = svc.stop();
        std::thread::sleep(Duration::from_secs(2));
        match svc.delete() {
            Ok(_) => println!("✓ Service '{SERVICE_NAME}' removed."),
            Err(e) => {
                eprintln!("✗ Uninstall failed: {e}");
                std::process::exit(1);
            }
        }
    }

    pub fn control(start: bool) {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .expect("Cannot open Service Control Manager — run as Administrator");
        let access = if start { ServiceAccess::START } else { ServiceAccess::STOP };
        let svc = manager
            .open_service(SERVICE_NAME, access)
            .expect("Service not found — install it first");

        if start {
            match svc.start::<&str>(&[]) {
                Ok(_) => println!("✓ Started."),
                Err(e) => {
                    eprintln!("✗ Start failed: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            match svc.stop() {
                Ok(_) => println!("✓ Stop requested."),
                Err(e) => {
                    eprintln!("✗ Stop failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    pub fn status() {
        match query() {
            SvcStatus { installed: false, .. } => println!("{SERVICE_NAME}: not installed"),
            SvcStatus { state, pid, .. } => println!(
                "{SERVICE_NAME}: {} (pid {:?})",
                state.unwrap_or_else(|| "unknown".into()),
                pid
            ),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn dispatch() {
        eprintln!("zev-detector is a Windows service; use 'run' on this platform.");
        std::process::exit(1);
    }
    pub fn query() -> SvcStatus {
        SvcStatus::not_installed()
    }
    pub fn install() {
        eprintln!("Service install is Windows-only.");
    }
    pub fn uninstall() {
        eprintln!("Service uninstall is Windows-only.");
    }
    pub fn control(_start: bool) {
        eprintln!("Service control is Windows-only.");
    }
    pub fn status() {
        eprintln!("Service status is Windows-only.");
    }
}

pub use imp::{control, dispatch, install, query, status, uninstall};
