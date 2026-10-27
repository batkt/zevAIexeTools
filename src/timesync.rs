//! Clock drift against the server, and putting it right.
//!
//! Drift is measured against our own API's `Date` header rather than an NTP
//! pool, because that server is the authority that actually matters here: it
//! validates the JWT the agent sends, and it stores the timestamps every event
//! is ordered by. A machine that agrees with `pool.ntp.org` but not with
//! udirdlagaBack still produces broken data.
//!
//! Network latency puts a floor on the accuracy — the `Date` header is stamped
//! before the response finishes travelling — so the threshold is deliberately
//! coarse (tens of seconds). It is looking for a clock that is *wrong*, not one
//! that is a few hundred milliseconds out.

use crate::config::TimeSyncConfig;
use crate::event::{DetectorEvent, Severity, Source};
use log::{info, warn};
use std::time::{Duration, Instant};

/// Never resync more often than this, whatever the drift says. A clock that
/// keeps drifting back has a hardware or domain problem that resyncing every
/// poll would only paper over — and every resync moves time under whatever is
/// running.
const RESYNC_COOLDOWN: Duration = Duration::from_secs(1800);

#[derive(Debug, PartialEq)]
pub enum Action {
    Nothing,
    /// Report the drift, but do not touch the clock.
    Report,
    /// Report and run w32tm.
    Resync,
}

/// Decides what to do, split out from the doing so it can be tested without a
/// clock to break.
pub fn decide(
    drift_secs: i64,
    max_drift_secs: i64,
    auto_resync: bool,
    since_last_resync: Option<Duration>,
) -> Action {
    if drift_secs.abs() <= max_drift_secs.max(1) {
        return Action::Nothing;
    }
    if !auto_resync {
        return Action::Report;
    }
    match since_last_resync {
        Some(elapsed) if elapsed < RESYNC_COOLDOWN => Action::Report,
        _ => Action::Resync,
    }
}

pub struct TimeSyncCollector {
    cfg: TimeSyncConfig,
    url: String,
    client: reqwest::Client,
    last_resync: Option<Instant>,
    /// So a permanently skewed clock does not file a report every poll.
    was_off: bool,
    last: serde_json::Value,
}

impl TimeSyncCollector {
    pub fn new(cfg: TimeSyncConfig, server_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            cfg,
            url: server_url.to_string(),
            client,
            last_resync: None,
            was_off: false,
            last: serde_json::json!({}),
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        self.last.clone()
    }

    pub async fn poll(&mut self) -> Vec<DetectorEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }
        let Some(drift) = self.measure_drift().await else {
            return Vec::new();
        };

        self.last = serde_json::json!({
            "driftSecs": drift,
            "maxDriftSecs": self.cfg.max_drift_secs,
            "autoResync": self.cfg.auto_resync,
        });

        let action = decide(
            drift,
            self.cfg.max_drift_secs,
            self.cfg.auto_resync,
            self.last_resync.map(|t| t.elapsed()),
        );

        let mut out = Vec::new();
        match action {
            Action::Nothing => {
                if self.was_off {
                    self.was_off = false;
                    out.push(
                        DetectorEvent::new(
                            Source::Health,
                            Severity::Info,
                            "windows",
                            "clock",
                            format!("Цагийн зөрүү хэвийн болов ({drift}с)"),
                        )
                        .kod("health/clock")
                        .meta("driftSecs", drift)
                        .refingerprint(),
                    );
                }
            }
            Action::Report | Action::Resync => {
                self.was_off = true;
                let resynced = if action == Action::Resync {
                    self.last_resync = Some(Instant::now());
                    match resync() {
                        Ok(()) => {
                            info!("Clock: w32tm resync issued (drift was {drift}s)");
                            Some(true)
                        }
                        Err(e) => {
                            warn!("Clock: resync failed: {e}");
                            Some(false)
                        }
                    }
                } else {
                    None
                };

                let tail = match resynced {
                    Some(true) => " — w32tm-ээр тааруулав".to_string(),
                    Some(false) => " — тааруулж чадсангүй".to_string(),
                    None => String::new(),
                };
                out.push(
                    DetectorEvent::new(
                        Source::Health,
                        Severity::Warning,
                        "windows",
                        "clock",
                        format!(
                            "Системийн цаг серверээс {drift}с зөрж байна (хязгаар {}с){tail}",
                            self.cfg.max_drift_secs
                        ),
                    )
                    .kod("health/clock")
                    .meta("driftSecs", drift)
                    .meta("resynced", resynced.unwrap_or(false))
                    .refingerprint(),
                );
            }
        }
        out
    }

    /// Positive means the local clock is ahead of the server.
    async fn measure_drift(&self) -> Option<i64> {
        let resp = self.client.get(&self.url).send().await.ok()?;
        let date = resp.headers().get(reqwest::header::DATE)?.to_str().ok()?;
        // HTTP dates are RFC 1123, which parses as RFC 2822.
        let server = chrono::DateTime::parse_from_rfc2822(date).ok()?;
        Some(chrono::Utc::now().timestamp() - server.timestamp())
    }
}

/// Asks the Windows Time service to correct the clock.
#[cfg(windows)]
fn resync() -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let run = |program: &str, args: &[&str]| -> anyhow::Result<(bool, String)> {
        let out = std::process::Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.success(), text.trim().to_string()))
    };

    // w32tm cannot do anything while the Time service is stopped, which on a
    // workgroup kiosk it very often is — so start it before giving up.
    let (ok, msg) = run("w32tm", &["/resync", "/force"])?;
    if ok {
        return Ok(());
    }
    let _ = crate::services::start("w32time");
    std::thread::sleep(Duration::from_secs(2));

    let (ok2, msg2) = run("w32tm", &["/resync", "/force"])?;
    if ok2 {
        return Ok(());
    }
    anyhow::bail!("w32tm: {msg}; after starting w32time: {msg2}")
}

#[cfg(not(windows))]
fn resync() -> anyhow::Result<()> {
    anyhow::bail!("Clock resync is Windows-only")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_drift_is_left_alone() {
        assert_eq!(decide(0, 30, true, None), Action::Nothing);
        assert_eq!(decide(29, 30, true, None), Action::Nothing);
        assert_eq!(decide(-30, 30, true, None), Action::Nothing);
    }

    #[test]
    fn drift_in_either_direction_counts() {
        // A clock running behind is exactly as broken as one running ahead.
        assert_eq!(decide(45, 30, true, None), Action::Resync);
        assert_eq!(decide(-45, 30, true, None), Action::Resync);
    }

    #[test]
    fn resync_disabled_reports_only() {
        assert_eq!(decide(600, 30, false, None), Action::Report);
    }

    #[test]
    fn cooldown_stops_a_resync_loop() {
        // A clock that drifts straight back has a hardware or domain problem;
        // resyncing every poll would hide that and keep moving time under
        // whatever is running.
        assert_eq!(decide(600, 30, true, Some(Duration::from_secs(60))), Action::Report);
        assert_eq!(decide(600, 30, true, Some(RESYNC_COOLDOWN)), Action::Resync);
    }

    #[test]
    fn a_zero_threshold_cannot_make_every_poll_resync() {
        // max_drift_secs = 0 would otherwise treat a perfectly good clock as
        // drifting; the floor of 1 second keeps that from happening.
        assert_eq!(decide(0, 0, true, None), Action::Nothing);
    }
}
