//! Host and endpoint health.
//!
//! Two jobs: watch CPU / memory / disk against thresholds, and probe the HTTP
//! endpoints the projects expose (e.g. dahua-service's `/api/health` on :5000).
//!
//! Like the service watcher this is edge-triggered — crossing a threshold or a
//! probe flipping up/down produces one event, and a condition that persists
//! re-announces every 15 minutes.

use crate::config::HealthConfig;
use crate::event::{DetectorEvent, Severity, Source};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const REMIND: Duration = Duration::from_secs(900);

struct Flag {
    bad: bool,
    last_reported: Option<Instant>,
    since: Instant,
}

/// A probe that has failed enough times in a row that the service behind it
/// should be bounced.
#[derive(Debug, Clone)]
pub struct Wedged {
    /// SCM name of the service to restart.
    pub service: String,
    /// Probe that noticed, for the event text.
    pub probe: String,
    pub failures: u32,
}

pub struct HealthCollector {
    cfg: HealthConfig,
    flags: HashMap<String, Flag>,
    client: reqwest::Client,
    last_snapshot: serde_json::Value,
    /// Consecutive failures per probe name. Reset the moment it answers again,
    /// so one dropped request never accumulates toward a restart.
    probe_failures: HashMap<String, u32>,
    /// Filled in by each `poll()`; drained by the healer.
    wedged: Vec<Wedged>,
}

impl HealthCollector {
    pub fn new(cfg: HealthConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            cfg,
            flags: HashMap::new(),
            client,
            last_snapshot: serde_json::json!({}),
            probe_failures: HashMap::new(),
            wedged: Vec::new(),
        }
    }

    /// Services whose endpoint has been unanswered long enough to warrant a
    /// restart, as of the last `poll()`.
    pub fn wedged(&self) -> &[Wedged] {
        &self.wedged
    }

    /// Fullest disk seen on the last poll, as a used percentage. Drives the
    /// cleanup trigger, so it reads from the snapshot rather than re-scanning.
    pub fn worst_disk_percent(&self) -> f32 {
        self.last_snapshot
            .get("disks")
            .and_then(|d| d.as_array())
            .map(|disks| {
                disks
                    .iter()
                    .filter_map(|d| d.get("usedPercent").and_then(|v| v.as_f64()))
                    .fold(0.0f64, f64::max) as f32
            })
            .unwrap_or(0.0)
    }

    pub fn worst_disk_info(&self) -> (f32, String, f32) {
        let mut worst_pct = 0.0f32;
        let mut worst_mount = String::new();
        let mut worst_free = 0.0f32;

        if let Some(disks) = self.last_snapshot.get("disks").and_then(|d| d.as_array()) {
            for d in disks {
                let pct = d.get("usedPercent").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                if pct >= worst_pct {
                    worst_pct = pct;
                    worst_mount = d.get("mount").and_then(|m| m.as_str()).unwrap_or("").to_string();
                    worst_free = d.get("freeGb").and_then(|f| f.as_f64()).unwrap_or(0.0) as f32;
                }
            }
        }
        (worst_pct, worst_mount, worst_free)
    }

    /// Returns Some(event) when the key's state changed or the reminder is due.
    fn edge(
        &mut self,
        key: &str,
        bad: bool,
        make: impl FnOnce(u64) -> DetectorEvent,
    ) -> Option<DetectorEvent> {
        let now = Instant::now();
        let prev = self.flags.get(key);
        let changed = prev.map(|f| f.bad != bad).unwrap_or(bad);
        let due = bad
            && prev
                .and_then(|f| f.last_reported)
                .map(|t| t.elapsed() >= REMIND)
                .unwrap_or(true);

        let report = changed || due;
        let duration = prev.map(|f| f.since.elapsed().as_secs()).unwrap_or(0);

        let entry = self.flags.entry(key.to_string()).or_insert(Flag {
            bad,
            last_reported: None,
            since: now,
        });
        if entry.bad != bad {
            entry.bad = bad;
            entry.since = now;
        }
        if report {
            entry.last_reported = Some(now);
            return Some(make(duration));
        }
        None
    }

    pub async fn poll(&mut self) -> Vec<DetectorEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }
        let mut out = self.poll_system();
        out.extend(self.poll_http().await);
        out
    }

    pub fn snapshot(&self) -> serde_json::Value {
        self.last_snapshot.clone()
    }

    fn poll_system(&mut self) -> Vec<DetectorEvent> {
        use sysinfo::{Disks, System};

        let mut sys = System::new();
        sys.refresh_cpu_usage();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        let cpu = sys.global_cpu_usage();
        let total_mem = sys.total_memory() as f64;
        let used_mem = sys.used_memory() as f64;
        let mem_pct = if total_mem > 0.0 {
            (used_mem / total_mem * 100.0) as f32
        } else {
            0.0
        };

        let mut out = Vec::new();
        let mut disk_report = Vec::new();

        // Copied out of `self.cfg` first: `edge` takes `&mut self`, so the
        // closure cannot keep borrowing the config through `self`.
        let cpu_threshold = self.cfg.cpu_threshold;
        let mem_threshold = self.cfg.mem_threshold;

        if let Some(ev) = self.edge("cpu", cpu >= cpu_threshold, move |secs| {
            let sev = if cpu >= cpu_threshold { Severity::Warning } else { Severity::Info };
            let msg = if cpu >= cpu_threshold {
                format!("CPU ачаалал өндөр: {cpu:.0}% ({}м үргэлжилж байна)", secs / 60)
            } else {
                format!("CPU ачаалал хэвийн болов: {cpu:.0}%")
            };
            DetectorEvent::new(Source::Health, sev, "windows", "cpu", msg)
                .kod("health/cpu")
                .meta("cpuPercent", cpu)
                .refingerprint()
        }) {
            out.push(ev);
        }

        if let Some(ev) = self.edge("memory", mem_pct >= mem_threshold, move |secs| {
            let sev = if mem_pct >= mem_threshold { Severity::Warning } else { Severity::Info };
            let msg = if mem_pct >= mem_threshold {
                format!(
                    "Санах ой дүүрч байна: {mem_pct:.0}% ({:.1}/{:.1} GB, {}м)",
                    used_mem / 1e9,
                    total_mem / 1e9,
                    secs / 60
                )
            } else {
                format!("Санах ой хэвийн болов: {mem_pct:.0}%")
            };
            DetectorEvent::new(Source::Health, sev, "windows", "memory", msg)
                .kod("health/memory")
                .meta("memoryPercent", mem_pct)
                .refingerprint()
        }) {
            out.push(ev);
        }

        let disks = Disks::new_with_refreshed_list();
        for disk in disks.list() {
            let total = disk.total_space();
            if total == 0 {
                continue;
            }
            let used = total.saturating_sub(disk.available_space());
            let pct = (used as f64 / total as f64 * 100.0) as f32;
            let mount = disk.mount_point().to_string_lossy().to_string();
            let free_gb = disk.available_space() as f64 / 1e9;

            disk_report.push(serde_json::json!({
                "mount": mount,
                "usedPercent": (pct * 10.0).round() / 10.0,
                "freeGb": (free_gb * 10.0).round() / 10.0,
            }));

            let key = format!("disk:{mount}");
            let over = pct >= self.cfg.disk_threshold;
            let threshold = self.cfg.disk_threshold;
            let mount_for_msg = mount.clone();
            if let Some(ev) = self.edge(&key, over, move |secs| {
                let sev = if over { Severity::Critical } else { Severity::Info };
                let msg = if over {
                    format!(
                        "Диск {mount_for_msg} дүүрч байна: {pct:.0}% ашиглагдсан, {free_gb:.1} GB чөлөөтэй ({}м)",
                        secs / 60
                    )
                } else {
                    format!("Диск {mount_for_msg} хэвийн болов: {pct:.0}%")
                };
                DetectorEvent::new(Source::Health, sev, "windows", "disk", msg)
                    .kod(format!("health/disk/{mount_for_msg}"))
                    .meta("mount", mount_for_msg.clone())
                    .meta("usedPercent", pct)
                    .meta("freeGb", free_gb)
                    .meta("threshold", threshold)
                    .refingerprint()
            }) {
                out.push(ev);
            }
        }

        self.last_snapshot = serde_json::json!({
            "cpuPercent": (cpu * 10.0).round() / 10.0,
            "memoryPercent": (mem_pct * 10.0).round() / 10.0,
            "totalMemoryGb": (total_mem / 1e9 * 10.0).round() / 10.0,
            "disks": disk_report,
            "uptimeSecs": System::uptime(),
        });

        out
    }

    async fn poll_http(&mut self) -> Vec<DetectorEvent> {
        let probes = self.cfg.http.clone();
        let mut out = Vec::new();
        self.wedged.clear();

        for probe in probes {
            let started = Instant::now();
            let result = self
                .client
                .get(&probe.url)
                .timeout(Duration::from_secs(probe.timeout_secs.max(1)))
                .send()
                .await;

            let (bad, detail, status) = match result {
                Err(e) => (true, format!("холбогдож чадсангүй: {e}"), 0u16),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    if !(200..400).contains(&status) {
                        (true, format!("HTTP {status}"), status)
                    } else if !probe.expect.is_empty() && !body.contains(&probe.expect) {
                        (
                            true,
                            format!(
                                "хариунд '{}' алга (HTTP {status}): {}",
                                probe.expect,
                                crate::event::truncate(body.trim(), 200)
                            ),
                            status,
                        )
                    } else {
                        (false, format!("HTTP {status}"), status)
                    }
                }
            };

            // A service can sit at `Running` in the SCM while its listener is
            // wedged; consecutive unanswered probes are the only evidence of
            // that, so they are counted separately from the reporting edge.
            let streak = if bad {
                let n = self.probe_failures.entry(probe.name.clone()).or_insert(0);
                *n += 1;
                *n
            } else {
                self.probe_failures.remove(&probe.name);
                0
            };
            if bad
                && !probe.service.is_empty()
                && streak >= probe.restart_after_failures.max(1)
            {
                self.wedged.push(Wedged {
                    service: probe.service.clone(),
                    probe: probe.name.clone(),
                    failures: streak,
                });
            }

            let ms = started.elapsed().as_millis() as u64;
            let key = format!("http:{}", probe.name);
            let system = if probe.system.is_empty() { "windows".to_string() } else { probe.system.clone() };
            let name = probe.name.clone();
            let url = probe.url.clone();

            if let Some(ev) = self.edge(&key, bad, move |secs| {
                let sev = if bad { Severity::Critical } else { Severity::Info };
                let msg = if bad {
                    format!("'{name}' үйлчилгээ хариу өгөхгүй байна — {detail} ({}м)", secs / 60)
                } else {
                    format!("'{name}' үйлчилгээ сэргэлээ — {detail}")
                };
                DetectorEvent::new(Source::Health, sev, system, name.clone(), msg)
                    .kod(format!("health/http/{name}"))
                    .meta("url", url)
                    .meta("httpStatus", status)
                    .meta("responseMs", ms)
                    .refingerprint()
            }) {
                out.push(ev);
            }
        }

        out
    }
}
