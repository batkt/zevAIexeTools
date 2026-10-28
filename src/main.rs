//! `zev-detector.exe` — the headless agent.
//!
//! This is the binary the Service Control Manager launches, and the one the
//! desktop app shells out to (elevated) for install / start / stop. It has no
//! window and no GUI dependencies; open `zev-detector-ui.exe` for that.
//!
//! Usage:
//!   zev-detector.exe install     — install as a Windows service (run as Admin)
//!   zev-detector.exe uninstall   — remove the service
//!   zev-detector.exe start|stop  — control the installed service
//!   zev-detector.exe status      — show service state
//!   zev-detector.exe run         — run in the console (debug)
//!   zev-detector.exe test        — one collection pass, print what it found
//!   (no args)                    — entry point used by the SCM

use log::{error, LevelFilter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zev_detector::config::Config;
use zev_detector::runner::{
    collect_test_report, init_logging, install_panic_hook, run_app, write_sample_config_if_missing,
};
use zev_detector::{sender, winsvc};

fn main() {
    #[cfg(windows)]
    unsafe {
        // A DLL fault should restart the service, not park a modal dialog on a
        // headless kiosk machine forever.
        windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(
            windows_sys::Win32::System::Diagnostics::Debug::SEM_NOGPFAULTERRORBOX,
        );
    }

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("install") => winsvc::install(),
        Some("uninstall") | Some("remove") => winsvc::uninstall(),
        Some("start") => winsvc::control(true),
        Some("stop") => winsvc::control(false),
        Some("status") => winsvc::status(),
        Some("dahua-status") => print_dahua_status(),
        Some("dahua-start") => {
            if let Err(e) = zev_detector::dahua_monitor::start_dahua_service() {
                eprintln!("Failed to start DahuaParkingService: {e}");
                std::process::exit(1);
            }
            println!("DahuaParkingService start signal sent.");
        }
        Some("dahua-stop") => {
            if let Err(e) = zev_detector::dahua_monitor::stop_dahua_service() {
                eprintln!("Failed to stop DahuaParkingService: {e}");
                std::process::exit(1);
            }
            println!("DahuaParkingService stop signal sent.");
        }
        Some("dahua-restart") => {
            if let Err(e) = zev_detector::dahua_monitor::restart_dahua_service() {
                eprintln!("Failed to restart DahuaParkingService: {e}");
                std::process::exit(1);
            }
            println!("DahuaParkingService restart completed.");
        }
        Some("speed") => {
            println!("Интернэт хурд хэмжиж байна (Cloudflare CDN Edge)...");
            let rt = tokio::runtime::Runtime::new().unwrap();
            match rt.block_on(zev_detector::diag::speed()) {
                Ok(res) => println!("{res}"),
                Err(e) => eprintln!("Speedtest failed: {e}"),
            }
        }
        Some("run") => run_interactive(),
        Some("test") => run_test(),
        Some("--version") | Some("-v") => println!("zev-detector {}", sender::AGENT_VERSION),
        Some("help") | Some("--help") | Some("-h") => print_help(),
        _ => winsvc::dispatch(),
    }
}

fn print_dahua_status() {
    let s = zev_detector::dahua_monitor::get_dahua_status();
    println!("Dahua Parking Service Status:");
    println!("  Service:  {} ({})", s.service_name, s.display_name);
    println!("  Active:   {} (State: {}, PID: {})", s.is_active, s.state, s.pid);
    println!("  API 5000: {}", if s.api_port_5000_open { "OPEN" } else { "CLOSED" });
    println!("  SDK 37777: {}", if s.netsdk_port_37777_open { "OPEN" } else { "CLOSED" });
    println!("  Log path: {} (exists: {})", s.log_path, s.log_exists);
    println!("  Recent 10 records (FIFO):");
    for (i, rec) in s.recent_logs.iter().enumerate() {
        println!("    [{}] [{}] [{}] {}", i + 1, rec.timestamp, rec.level, rec.message);
    }
    println!("  Recent {} images:", s.recent_images.len());
    for (i, img) in s.recent_images.iter().enumerate() {
        println!("    [{}] {} ({}) has_base64: {}", i + 1, img.plate, img.filename, img.base64.is_some());
    }
}

fn print_help() {
    println!(
        "Zev Detector {}\n\n\
         install     Install as a Windows service (run as Administrator)\n\
         uninstall   Remove the Windows service\n\
         start       Start the installed service\n\
         stop        Stop the installed service\n\
         status      Show the service state\n\
         run         Run in this console (Ctrl+C to quit)\n\
         test        Run one collection pass and print what was found\n\
         --version   Print the version\n\n\
         The desktop app is a separate binary: zev-detector-ui.exe",
        sender::AGENT_VERSION
    );
}

fn run_interactive() {
    init_logging(LevelFilter::Debug);
    install_panic_hook();
    println!("Running in console mode — Ctrl+C to stop.");

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("✗ {e}");
            write_sample_config_if_missing();
            std::process::exit(1);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stop_ctrlc = Arc::clone(&stop);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                println!("\nCtrl+C — shutting down…");
                stop_ctrlc.store(true, Ordering::Relaxed);
            });
            if let Err(e) = run_app(cfg, stop).await {
                error!("Error: {e}");
            }
        });
}

fn run_test() {
    init_logging(LevelFilter::Info);
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("✗ {e}");
            write_sample_config_if_missing();
            std::process::exit(1);
        }
    };
    println!("{}", collect_test_report(&cfg));
    println!("\nNothing was uploaded. Use 'run' or the service to start sending.");
}
