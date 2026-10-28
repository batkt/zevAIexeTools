//! The detector loop itself, plus the logging setup and the one-shot "test"
//! pass. Lives in the library so the service binary and the desktop app can
//! both reach it — the app's "Test run" button and the CLI's `test` command
//! are the same code path, which is the point.

use crate::config::Config;
use crate::{
    cleanup, event, eventlog, heal, health, logwatch, netqual, notify, ports, sender, services, timesync,
    tushaal, update,
};
use chrono::{Datelike, Timelike};
use log::{error, info, LevelFilter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn run_app(mut cfg: Config, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    info!("=== Zev Detector {} starting ===", sender::AGENT_VERSION);
    info!("Agent: {} → {}", cfg.agent.name, cfg.server.url);

    let state_dir = Config::data_dir();

    let mut evt = eventlog::EventLogCollector::new(cfg.eventlog.clone());
    let mut svc = services::ServiceCollector::new(cfg.services.clone());
    let mut healer = heal::Healer::new(cfg.services.clone());
    let mut logs = logwatch::LogCollector::new(cfg.logs.clone(), &state_dir);
    let mut hlth = health::HealthCollector::new(cfg.health.clone());
    let mut netq = netqual::NetQualCollector::new(cfg.netqual.clone(), &cfg.server.url);
    let mut prts = ports::PortCollector::new(cfg.ports.clone());
    let mut clock = timesync::TimeSyncCollector::new(cfg.timesync.clone(), &cfg.server.url);
    let mut updater = update::Updater::new(cfg.update.clone());
    let mut notifier = notify::LocalNotifier::new(&cfg.notify, &state_dir);
    let mut out = sender::Sender::new(cfg.server.clone(), cfg.agent.clone(), &state_dir);
    let exec = tushaal::Executor::new(cfg.server.clone());

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
    // Pinging costs a second or two, so it starts after the first heartbeat
    // rather than competing with startup.
    let mut due_net = Instant::now();
    let mut due_clock = Instant::now() + Duration::from_secs(45);
    // Checking GitHub on every service start would hammer the API across a
    // fleet restarting together.
    let mut due_update = Instant::now() + Duration::from_secs(120);
    let mut due_cleanup = Instant::now() + Duration::from_secs(90);
    let mut due_notify = Instant::now() + Duration::from_secs(2);
    let mut last_cleanup_day: Option<String> = None;

    let evt_int = Duration::from_secs(cfg.eventlog.poll_interval_secs.max(5));
    let svc_int = Duration::from_secs(15);
    let log_int = Duration::from_secs(cfg.logs.poll_interval_secs.max(5));
    let health_int = Duration::from_secs(cfg.health.poll_interval_secs.max(10));
    let send_int = Duration::from_secs(cfg.server.send_interval_secs.max(5));
    let beat_int = Duration::from_secs(cfg.agent.heartbeat_interval_secs.max(30));
    let net_int = Duration::from_secs(cfg.netqual.poll_interval_secs.max(60));
    let clock_int = Duration::from_secs(cfg.timesync.poll_interval_secs.max(60));
    let update_int = Duration::from_secs(cfg.update.check_interval_hours.max(1) * 3600);
    let notify_int = Duration::from_secs(15);

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
            // Heal from the same observation the watcher just reported on,
            // so the two can never disagree about what state a service is in.
            let healed = healer.poll(svc.last_seen());
            if !healed.is_empty() {
                info!("Heal: {} action(s)", healed.len());
                out.enqueue(healed);
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
            // An endpoint that has stopped answering is the only evidence of a
            // service that is "Running" but wedged; hand it to the healer,
            // which acts on it during its own cycle under the same rate limit.
            healer.note_wedged(hlth.wedged());

            let found = prts.poll().await;
            if !found.is_empty() {
                info!("Ports: {} change(s)", found.len());
                out.enqueue(found);
            }
        }

        if cfg.netqual.enabled && now >= due_net {
            due_net = now + net_int;
            let found = netq.poll(out.disconnects()).await;
            if !found.is_empty() {
                out.enqueue(found);
            }
        }

        if cfg.timesync.enabled && now >= due_clock {
            due_clock = now + clock_int;
            let found = clock.poll().await;
            if !found.is_empty() {
                out.enqueue(found);
            }
        }

        if updater.enabled() && now >= due_update {
            due_update = now + update_int;
            let minutes = {
                let t = chrono::Local::now();
                t.hour() * 60 + t.minute()
            };
            let found = updater.poll(minutes).await;
            if !found.is_empty() {
                out.enqueue(found);
            }
        }

        // Cleanup is weekly at a set hour, plus an emergency pass whenever a
        // disk crosses the trigger. The hour is checked rather than a countdown
        // so a machine that was asleep still runs it once when it wakes.
        if cfg.cleanup.enabled && now >= due_cleanup {
            due_cleanup = now + Duration::from_secs(1800);
            let t = chrono::Local::now();
            let scheduled = t.weekday().num_days_from_sunday() as u8 == cfg.cleanup.weekday
                && t.hour() == cfg.cleanup.hour;
            let urgent = cfg.cleanup.disk_trigger_percent > 0.0
                && hlth.worst_disk_percent() >= cfg.cleanup.disk_trigger_percent;

            // Once per calendar day at most, so the emergency trigger cannot
            // loop on a disk that stays full after a sweep.
            let today = t.format("%Y-%m-%d").to_string();
            if (scheduled || urgent) && last_cleanup_day.as_deref() != Some(today.as_str()) {
                last_cleanup_day = Some(today);
                info!("Cleanup: starting ({})", if urgent { "disk trigger" } else { "scheduled" });
                let cfg_cleanup = cfg.cleanup.clone();
                let (_, found) =
                    tokio::task::spawn_blocking(move || cleanup::run(&cfg_cleanup, false))
                        .await
                        .unwrap_or_else(|_| {
                            (
                                cleanup::Report {
                                    freed_bytes: 0,
                                    files: 0,
                                    per_root: Vec::new(),
                                    errors: 0,
                                },
                                Vec::new(),
                            )
                        });
                if !found.is_empty() {
                    out.enqueue(found);
                }
            }
        }

        if cfg.notify.enabled && now >= due_notify {
            due_notify = now + notify_int;
            let (net_score, net_lat, net_loss) = netq.last_metrics();
            let (disk_pct, disk_mount, disk_free) = hlth.worst_disk_info();
            let closed_ports = prts.closed_ports();
            notifier.evaluate(
                net_score,
                net_lat,
                net_loss,
                disk_pct,
                &disk_mount,
                disk_free,
                &closed_ports,
                healer.last_limit_hit(),
            );
            crate::dahua_monitor::get_dahua_status();
        }

        if now >= due_send {
            due_send = now + send_int;
            out.flush().await;
        }

        if now >= due_beat {
            due_beat = now + beat_int;
            let resp = out
                .heartbeat(sender::HeartbeatData {
                    services: svc.snapshot(),
                    health: hlth.snapshot(),
                    heals: healer.snapshot(),
                    network: netq.snapshot(),
                    ports: prts.snapshot(),
                    clock: clock.snapshot(),
                    dahua: serde_json::to_value(crate::dahua_monitor::get_dahua_status())
                        .unwrap_or(serde_json::Value::Null),
                })
                .await;

            // Apply remote dynamic configuration from backend if present
            if let Some(tokhirgoo) = resp.tokhirgoo {
                if !tokhirgoo.is_null() && tokhirgoo.as_object().map_or(false, |o| !o.is_empty()) {
                    let old_services = cfg.services.clone();
                    let old_ports = cfg.ports.clone();
                    cfg.merge_json(&tokhirgoo);

                    if let Some(bid) = resp.barilgiin_id {
                        if !bid.is_empty() { cfg.agent.barilgiin_id = bid; }
                    }
                    if let Some(bname) = resp.baiguullagiin_ner {
                        if !bname.is_empty() { cfg.agent.baiguullagiin_ner = bname; }
                    }
                    if let Some(bgid) = resp.baiguullagiin_id {
                        if !bgid.is_empty() { cfg.agent.baiguullagiin_id = bgid; }
                    }

                    if cfg.services != old_services {
                        info!("Applying remote services update ({} service(s))", cfg.services.len());
                        svc = services::ServiceCollector::new(cfg.services.clone());
                        healer = heal::Healer::new(cfg.services.clone());
                    }
                    if cfg.ports != old_ports {
                        info!("Applying remote ports update ({} port(s))", cfg.ports.len());
                        prts = ports::PortCollector::new(cfg.ports.clone());
                    }
                    let _ = cfg.save();
                }
            }

            for t in &resp.tushaal {
                let result = exec.run(t).await;
                if result.amjilttai {
                    info!("Command {} ({}) done", t.id, t.turul);
                } else {
                    error!("Command {} ({}) failed: {}", t.id, t.turul, result.aldaa);
                }
                out.tushaal_khariu(&t.id, result.amjilttai, &result.ur_dun, &result.aldaa)
                    .await;
                if t.turul == "diagSpeed" && result.amjilttai {
                    due_beat = now;
                }
            }
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
    // One runtime for everything async below; building several would be waste.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    if cfg.health.enabled {
        let mut hlth = health::HealthCollector::new(cfg.health.clone());
        let found = rt.block_on(hlth.poll());
        let _ = writeln!(report, "Health     : {} event(s)", found.len());
        let _ = writeln!(report, "             {}", hlth.snapshot());
        all.extend(found);
    }
    if !cfg.ports.is_empty() {
        let mut prts = ports::PortCollector::new(cfg.ports.clone());
        let found = rt.block_on(prts.poll());
        let _ = writeln!(report, "Ports      : {} event(s)", found.len());
        for p in prts.snapshot() {
            let _ = writeln!(report, "             {p}");
        }
        all.extend(found);
    }
    if cfg.netqual.enabled {
        let mut netq = netqual::NetQualCollector::new(cfg.netqual.clone(), &cfg.server.url);
        let found = rt.block_on(netq.poll(0));
        let _ = writeln!(report, "Network    : {}", netq.snapshot());
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
