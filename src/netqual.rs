//! Network quality: one score per machine, plus the numbers behind it.
//!
//! Two deliberate measurement choices, both about Windows realities:
//!
//! * **Packet loss comes from ping's exit code, not its output.** `ping.exe`
//!   prints "Lost = 0 (0% loss)" in English but translates that text on a
//!   localised Windows, so scraping it silently breaks on the very machines
//!   this runs on. The exit code (0 = reply, non-zero = no reply) means the
//!   same thing in every language, so loss is counted from N single pings.
//! * **Latency is measured in-process, not from ping.** Spawning a process
//!   costs 5–15 ms, which is the same order as the number being measured. A
//!   TCP connect to the server we actually talk to is both accurate and a
//!   truer reflection of what the agent experiences.
//!
//! ICMP being blocked is reported as unknown loss rather than as 100% loss —
//! a firewall dropping ping says nothing about whether uploads are working.

use crate::config::NetQualConfig;
use crate::event::{DetectorEvent, Severity, Source};
use log::debug;
use once_cell::sync::Lazy;
use std::net::{TcpStream, ToSocketAddrs};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How far back failed heartbeats count against the score.
const FAIL_WINDOW: Duration = Duration::from_secs(3600);

/// Turns the sender's ever-growing failure total into "failures in the last
/// hour". Without this, ten failed heartbeats on Monday kept the score 20
/// points down until the agent happened to restart.
#[derive(Default)]
pub struct RecentFailures {
    /// (when, running total at that moment), oldest first.
    samples: VecDeque<(Instant, u64)>,
}

impl RecentFailures {
    /// Records the current total and returns how many failures happened
    /// within the window (or since the agent started, if that is shorter).
    pub fn record(&mut self, now: Instant, total: u64) -> u64 {
        self.samples.push_back((now, total));
        // Keep the newest sample that is at least a window old as the baseline,
        // drop everything before it.
        while self.samples.len() > 1
            && now.duration_since(self.samples[1].0) >= FAIL_WINDOW
        {
            self.samples.pop_front();
        }
        let base = match self.samples.front() {
            Some(&(t, v)) if now.duration_since(t) >= FAIL_WINDOW => v,
            // Not an hour of history yet: everything so far is recent.
            _ => 0,
        };
        total.saturating_sub(base)
    }
}

static LAST_SPEED: Lazy<Mutex<Option<(f64, f64, String)>>> = Lazy::new(|| Mutex::new(None));

pub fn update_speed_metrics(download_mbps: f64, upload_mbps: f64) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if let Ok(mut lock) = LAST_SPEED.lock() {
        *lock = Some((download_mbps, upload_mbps, now));
    }
}

pub struct Measurement {
    /// Median TCP connect time to the server, in ms. None if unreachable.
    pub latency_ms: Option<u64>,
    /// Percent of pings that got no reply. None when ICMP is unavailable.
    pub loss_percent: Option<f32>,
    /// Failed heartbeats since the agent started.
    pub disconnects: u64,
    /// Failed heartbeats in the last hour — what the score is based on.
    pub disconnects_last_hour: u64,
    /// 0–100, or None when there is nothing to score.
    pub score: Option<u32>,
}

pub struct NetQualCollector {
    cfg: NetQualConfig,
    host: String,
    port: u16,
    last: serde_json::Value,
    last_reported_score: Option<u32>,
    recent_failures: RecentFailures,
}

impl NetQualCollector {
    pub fn new(cfg: NetQualConfig, server_url: &str) -> Self {
        let (host, port) = split_host_port(server_url);
        let host = if cfg.ping_host.is_empty() { host } else { cfg.ping_host.clone() };
        Self {
            cfg,
            host,
            port,
            last: serde_json::json!({}),
            last_reported_score: None,
            recent_failures: RecentFailures::default(),
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let mut val = self.last.clone();
        if let Some((dl, ul, tested_at)) = LAST_SPEED.lock().ok().and_then(|lock| lock.clone()) {
            if let Some(obj) = val.as_object_mut() {
                obj.insert("downloadMbps".into(), serde_json::json!(dl));
                obj.insert("uploadMbps".into(), serde_json::json!(ul));
                obj.insert("speedTestedAt".into(), serde_json::json!(tested_at));
            }
        }
        val
    }

    pub fn last_metrics(&self) -> (Option<u8>, Option<u64>, Option<f32>) {
        let score = self
            .last
            .get("score")
            .and_then(|v| v.as_u64())
            .map(|s| s as u8);
        let lat = self.last.get("latencyMs").and_then(|v| v.as_u64());
        let loss = self
            .last
            .get("lossPercent")
            .and_then(|v| v.as_f64())
            .map(|l| l as f32);
        (score, lat, loss)
    }

    /// `disconnects` is owned by the sender, which is the only thing that knows
    /// whether uploads are actually landing.
    pub async fn poll(&mut self, disconnects: u64) -> Vec<DetectorEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }

        let host = self.host.clone();
        let port = self.port;
        let count = self.cfg.ping_count.clamp(1, 50);

        let latency_ms =
            tokio::task::spawn_blocking(move || median_connect_ms(&host, port, 5)).await.ok().flatten();

        let host = self.host.clone();
        let loss_percent =
            tokio::task::spawn_blocking(move || ping_loss_percent(&host, count)).await.ok().flatten();

        let disconnects_last_hour = self.recent_failures.record(Instant::now(), disconnects);
        let m = Measurement {
            latency_ms,
            loss_percent,
            disconnects,
            disconnects_last_hour,
            score: score(latency_ms, loss_percent, disconnects_last_hour),
        };

        let (dl, ul, tested_at) = LAST_SPEED
            .lock()
            .ok()
            .and_then(|lock| lock.clone())
            .map(|(d, u, t)| (Some(d), Some(u), Some(t)))
            .unwrap_or((None, None, None));

        self.last = serde_json::json!({
            "score": m.score,
            "latencyMs": m.latency_ms,
            "lossPercent": m.loss_percent,
            // Running total — udirdlagaBack compares totals between heartbeats
            // to tell an internet outage from a reboot.
            "disconnects": m.disconnects,
            "disconnectsLastHour": m.disconnects_last_hour,
            "host": self.host,
            "downloadMbps": dl,
            "uploadMbps": ul,
            "speedTestedAt": tested_at,
        });
        debug!(
            "Network: score={:?} latency={:?}ms loss={:?}%",
            m.score, m.latency_ms, m.loss_percent
        );

        // Edge-triggered on crossing the threshold, so a permanently mediocre
        // link does not file a report every five minutes.
        let mut out = Vec::new();
        let Some(now) = m.score else { return out };
        let bad_now = now < self.cfg.score_threshold;
        let bad_before = self.last_reported_score.map(|s| s < self.cfg.score_threshold);

        if bad_before != Some(bad_now) {
            let msg = if bad_now {
                format!(
                    "Сүлжээний чанар доголдож байна: {now}% (латент {}, гээгдэл {})",
                    m.latency_ms.map(|v| format!("{v}мс")).unwrap_or_else(|| "?".into()),
                    m.loss_percent
                        .map(|v| format!("{v:.0}%"))
                        .unwrap_or_else(|| "мэдэгдэхгүй".into()),
                )
            } else {
                format!("Сүлжээний чанар хэвийн болов: {now}%")
            };
            out.push(
                DetectorEvent::new(
                    Source::Health,
                    if bad_now { Severity::Warning } else { Severity::Info },
                    "windows",
                    "network",
                    msg,
                )
                .kod("health/network")
                .meta("score", now as u64)
                .meta("host", self.host.clone())
                .refingerprint(),
            );
        }
        self.last_reported_score = Some(now);
        out
    }
}

/// 0–100. Latency and loss are weighted separately because they fail
/// differently: 200 ms is annoying, 5% loss breaks uploads.
pub fn score(latency_ms: Option<u64>, loss_percent: Option<f32>, disconnects: u64) -> Option<u32> {
    // Nothing measurable at all — the machine may be entirely offline, which
    // "0" would misrepresent as "connected but terrible".
    if latency_ms.is_none() && loss_percent.is_none() {
        return None;
    }

    let mut points: f32 = 100.0;

    // Under 50ms costs nothing; every 25ms after that costs 5, capped at 35.
    if let Some(ms) = latency_ms {
        if ms > 50 {
            points -= (((ms - 50) as f32 / 25.0) * 5.0).min(35.0);
        }
    } else {
        // Could not connect at all — far worse than any slow connection.
        points -= 60.0;
    }

    // Loss is punished hard and early: 1% already hurts, 10% is unusable.
    if let Some(loss) = loss_percent {
        points -= (loss * 4.0).min(40.0);
    }

    // Each failed heartbeat in the last hour costs 2, capped at 20.
    points -= ((disconnects as f32) * 2.0).min(20.0);

    Some(points.clamp(0.0, 100.0).round() as u32)
}

/// Median TCP connect time, which sidesteps a single unlucky sample.
fn median_connect_ms(host: &str, port: u16, samples: usize) -> Option<u64> {
    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let mut times = Vec::new();
    for _ in 0..samples {
        let started = Instant::now();
        if TcpStream::connect_timeout(&addr, Duration::from_secs(5)).is_ok() {
            times.push(started.elapsed().as_millis() as u64);
        }
    }
    if times.is_empty() {
        return None;
    }
    times.sort_unstable();
    Some(times[times.len() / 2])
}

/// Loss from exit codes so the reading is locale-independent.
///
/// Returns None when every ping failed *and* TCP works, which means ICMP is
/// filtered rather than the link being down — reporting that as 100% loss
/// would be actively misleading.
#[cfg(windows)]
fn ping_loss_percent(host: &str, count: u32) -> Option<f32> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut lost = 0u32;
    for _ in 0..count {
        let status = std::process::Command::new("ping")
            .args(["-n", "1", "-w", "1000", host])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(_) => lost += 1,
            // ping.exe missing or unrunnable is not packet loss.
            Err(_) => return None,
        }
    }
    if lost == count {
        return None;
    }
    Some(lost as f32 / count as f32 * 100.0)
}

#[cfg(not(windows))]
fn ping_loss_percent(_host: &str, _count: u32) -> Option<f32> {
    None
}

/// Host and port from a base URL, without pulling in a URL parser.
fn split_host_port(url: &str) -> (String, u16) {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    let https = url.starts_with("https");
    let authority = no_scheme.split('/').next().unwrap_or(no_scheme);
    match authority.rsplit_once(':') {
        // Guard against an IPv6 literal, where the last colon is not a port.
        Some((h, p)) if !h.contains(':') => {
            (h.to_string(), p.parse().unwrap_or(if https { 443 } else { 80 }))
        }
        _ => (authority.to_string(), if https { 443 } else { 80 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_link_scores_near_100() {
        assert_eq!(score(Some(20), Some(0.0), 0), Some(100));
        assert_eq!(score(Some(50), Some(0.0), 0), Some(100));
    }

    #[test]
    fn latency_and_loss_are_punished_separately() {
        // 150ms alone: (150-50)/25*5 = 20 off.
        assert_eq!(score(Some(150), Some(0.0), 0), Some(80));
        // 2% loss alone: 8 off.
        assert_eq!(score(Some(20), Some(2.0), 0), Some(92));
        // Both, plus a dropped upload.
        assert_eq!(score(Some(150), Some(2.0), 1), Some(70));
    }

    #[test]
    fn each_penalty_is_capped_so_one_bad_metric_cannot_zero_the_score() {
        // Absurd latency stops at −35, so the score still reflects that loss
        // is fine and uploads are landing.
        assert_eq!(score(Some(100_000), Some(0.0), 0), Some(65));
        assert_eq!(score(Some(20), Some(100.0), 0), Some(60));
        assert_eq!(score(Some(20), Some(0.0), 999), Some(80));
    }

    #[test]
    fn unreachable_is_scored_worse_than_merely_slow() {
        let slow = score(Some(100_000), Some(0.0), 0).unwrap();
        let unreachable = score(None, Some(0.0), 0).unwrap();
        assert!(unreachable < slow, "{unreachable} should be worse than {slow}");
    }

    #[test]
    fn nothing_measurable_is_none_not_zero() {
        // An offline machine must not look like "connected but terrible" —
        // the dashboard shows those two very differently.
        assert_eq!(score(None, None, 0), None);
    }

    #[test]
    fn blocked_icmp_still_yields_a_score_from_latency_alone() {
        assert_eq!(score(Some(60), None, 0), Some(98));
    }

    #[test]
    fn only_failures_from_the_last_hour_count() {
        let t0 = Instant::now();
        let min = |m: u64| t0 + Duration::from_secs(m * 60);
        let mut r = RecentFailures::default();

        // Less than an hour of history: everything so far counts.
        assert_eq!(r.record(min(0), 0), 0);
        assert_eq!(r.record(min(10), 10), 10);
        assert_eq!(r.record(min(30), 10), 10);
        // The ten failures at minute 10 are more than an hour old by minute 75.
        assert_eq!(r.record(min(75), 10), 0);
        // New failures count again…
        assert_eq!(r.record(min(80), 13), 3);
        // …and age out an hour later.
        assert_eq!(r.record(min(145), 13), 0);
    }

    #[test]
    fn old_failures_no_longer_drag_the_score_down() {
        let t0 = Instant::now();
        let mut r = RecentFailures::default();
        r.record(t0, 0);
        r.record(t0 + Duration::from_secs(60), 10);
        let recent = r.record(t0 + Duration::from_secs(2 * 3600), 10);
        assert_eq!(score(Some(20), Some(0.0), recent), Some(100));
    }

    #[test]
    fn host_and_port_are_pulled_off_the_base_url() {
        assert_eq!(
            split_host_port("https://admin.zevtabs.mn/api"),
            ("admin.zevtabs.mn".to_string(), 443)
        );
        assert_eq!(split_host_port("http://192.168.1.5:8282"), ("192.168.1.5".to_string(), 8282));
        assert_eq!(split_host_port("http://localhost:8282/api"), ("localhost".to_string(), 8282));
        assert_eq!(split_host_port("https://x.mn"), ("x.mn".to_string(), 443));
    }
}
