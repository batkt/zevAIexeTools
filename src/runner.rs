//! The detector loop itself, plus the logging setup and the one-shot "test"
//! pass. Lives in the library so the service binary and the desktop app can
//! both reach it — the app's "Test run" button and the CLI's `test` command
//! are the same code path, which is the point.

use crate::config::Config;
use crate::{event, eventlog, health, logwatch, sender, services};
use log::{error, info, LevelFilter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn run_app(cfg: Config, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    info!("=== Zev Detector {} starting ===", sender::AGENT_VERSION);
    info!("Agent: {} → {}", cfg.agent.name, cfg.server.url);

    let state_dir = Config::data_dir();

    let mut evt = eventlog::EventLogCollector::new(cfg.eventlog.clone());
    let mut svc = services::ServiceCollector::new(cfg.services.clone());
    let mut logs = logwatch::LogCollector::new(cfg.logs.clone(), &state_dir);
    let mut hlth = health::HealthCollector::new(cfg.health.clone());
    let mut out = sender::Sender::new(cfg.server.clone(), cfg.agent.clone(), &state_dir);

    info!(
        "Watching: {} event log channel(s), {} service(s), {} log file target(s), {} http probe(s)",
        if cfg.eventlog.enabled { cfg.eventlog.channels.len() } else { 0 },
        cfg.services.len(),
        if cfg.logs.enabled { cfg.logs.files.len() } else { 0 },
        if cfg.health.enabled { cfg.health.http.len() } else { 0 },
    );

    // Announce ourselves so the dashboard shows the agent came up (and why, if
    // this was an unexpected restart).
    out.enqueue(vec![event::DetectorEvent::new(
        event::Source::Detector,
        event::Severity::Info,
        "detector",
        "agent",
        format!(
            "Zev Detector {} ажиллаж эхэллээ ({})",
            sender::AGENT_VERSION,
            sender::os_name()
        ),
    )
    .kod("detector/started")
    .refingerprint()]);

    let mut due_evt = Instant::now();
    let mut due_svc = Instant::now();
    let mut due_log = Instant::now();
    let mut due_health = Instant::now();
    let mut due_send = Instant::now() + Duration::from_secs(5);
    let mut due_beat = Instant::now() + Duration::from_secs(10);

    let evt_int = Duration::from_secs(cfg.eventlog.poll_interval_secs.max(5));
    let svc_int = Duration::from_secs(15);
    let log_int = Duration::from_secs(cfg.logs.poll_interval_secs.max(5));
    let health_int = Duration::from_secs(cfg.health.poll_interval_secs.max(10));
    let send_int = Duration::from_secs(cfg.server.send_interval_secs.max(5));
    let beat_int = Duration::from_secs(cfg.agent.heartbeat_interval_secs.max(30));

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();

        if cfg.eventlog.enabled && now >= due_evt {
            due_evt = now + evt_int;
            let found = evt.poll();
            if !found.is_empty() {
                info!("Event log: {} new event(s)", found.len());
                out.enqueue(found);
            }
        }

        if !cfg.services.is_empty() && now >= due_svc {
            due_svc = now + svc_int;
            let found = svc.poll();
            if !found.is_empty() {
                info!("Services: {} state change(s)", found.len());
                out.enqueue(found);
            }
        }

        if cfg.logs.enabled && now >= due_log {
            due_log = now + log_int;
            let found = logs.poll();
            if !found.is_empty() {
                info!("Log files: {} matching line(s)", found.len());
                out.enqueue(found);
            }
        }

        if cfg.health.enabled && now >= due_health {
            due_health = now + health_int;
            let found = hlth.poll().await;
            if !found.is_empty() {
                info!("Health: {} change(s)", found.len());
                out.enqueue(found);
            }
        }

        if now >= due_send {
            due_send = now + send_int;
            out.flush().await;
        }

        if now >= due_beat {
            due_beat = now + beat_int;
            out.heartbeat(svc.snapshot(), hlth.snapshot()).await;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("Stop requested — flushing queue");
    out.flush().await;
    info!("=== Zev Detector stopped ===");
    Ok(())
}

/// One pass over every enabled collector, formatted as text. Nothing is ever
/// uploaded — this is what you run on a new site after editing the config to
/// see what the machine actually produces.
pub fn collect_test_report(cfg: &Config) -> String {
    use std::fmt::Write as _;

    let state_dir = Config::data_dir();
    let mut report = String::new();
    let _ = writeln!(report, "Agent   : {}", cfg.agent.name);
    let _ = writeln!(report, "Server  : {}/detector", cfg.server.url);
    let _ = writeln!(report, "OS      : {}\n", sender::os_name());

    let mut all = Vec::new();

    if cfg.eventlog.enabled {
        let mut evt = eventlog::EventLogCollector::new(cfg.eventlog.clone());
        let found = evt.poll();
        let _ = writeln!(report, "Event log  : {} event(s)", found.len());
        all.extend(found);
    }
    if !cfg.services.is_empty() {
        let mut svc = services::ServiceCollector::new(cfg.services.clone());
        let found = svc.poll();
        let _ = writeln!(report, "Services   : {} event(s)", found.len());
        for s in svc.snapshot() {
            let _ = writeln!(report, "             {s}");
        }
        all.extend(found);
    }
    if cfg.logs.enabled {
        let mut logs = logwatch::LogCollector::new(cfg.logs.clone(), &state_dir);
        // First pass only records offsets; the second reports real lines.
        let _ = logs.poll();
        let found = logs.poll();
        let _ = writeln!(report, "Log files  : {} line(s)", found.len());
        all.extend(found);
    }
    if cfg.health.enabled {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut hlth = health::HealthCollector::new(cfg.health.clone());
        let found = rt.block_on(hlth.poll());
        let _ = writeln!(report, "Health     : {} event(s)", found.len());
        let _ = writeln!(report, "             {}", hlth.snapshot());
        all.extend(found);
    }

    let _ = writeln!(report, "\n─── collected ───────────────────────────────");
    for ev in all.iter().take(50) {
        let _ = writeln!(
            report,
            "[{}] {} / {} :: {}",
            ev.aldaanii_tuvshin, ev.system, ev.modul, ev.aldaanii_msg
        );
    }
    if all.len() > 50 {
        let _ = writeln!(report, "… and {} more", all.len() - 50);
    }
    report
}

// ─── logging ─────────────────────────────────────────────────────────────────

pub fn log_path() -> std::path::PathBuf {
    Config::data_dir().join("zev-detector.log")
}

pub fn init_logging(level: LevelFilter) {
    let path = log_path();

    let result = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                message
            ))
        })
        .level(level)
        // The HTTP stack narrates every socket and header at Debug; none of it
        // says anything about what the detector actually found.
        .level_for("hyper", LevelFilter::Warn)
        .level_for("hyper_util", LevelFilter::Warn)
        .level_for("reqwest", LevelFilter::Warn)
        .level_for("rustls", LevelFilter::Warn)
        .level_for("native_tls", LevelFilter::Warn)
        .level_for("tokio_util", LevelFilter::Warn)
        .level_for("want", LevelFilter::Warn)
        .level_for("mio", LevelFilter::Warn)
        .level_for("h2", LevelFilter::Warn)
        .chain(std::io::stdout())
        .chain(fern::log_file(&path).expect("Cannot open log file"))
        .apply();

    if result.is_ok() {
        info!("Log file: {}", path.display());
    }
}

/// A panic inside a collector would otherwise vanish into a silently dead
/// service; write it to the log so the next start reports it.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error!("PANIC: {info}");
        default(info);
    }));
}

// ─── first-run config ────────────────────────────────────────────────────────

pub const SAMPLE_CONFIG: &str = include_str!("../config.sample.toml");

pub fn write_sample_config_if_missing() {
    let path = Config::config_path();
    if path.exists() {
        return;
    }
    match std::fs::write(&path, SAMPLE_CONFIG) {
        Ok(_) => println!("→ Wrote a starter config to {}", path.display()),
        Err(e) => eprintln!("Could not write {}: {e}", path.display()),
    }
}
