//! Self-healing: bring stopped services back up, and say so.
//!
//! Windows already restarts services that *crash*, through the SCM's failure
//! actions. What it does not cover is a service that exited cleanly, was
//! stopped by something else, or that the SCM has already exhausted its retry
//! actions on. Those stay down until a human notices — which on an unattended
//! parking PC can be a long time. This module closes that gap.
//!
//! Two rules keep it from making things worse:
//!
//! * **Never fight the SCM.** Anything mid-transition (`StartPending` and
//!   friends) is left alone, as is a service that cannot be queried at all —
//!   "not installed" is not something a restart fixes.
//! * **Give up loudly.** Restarts are capped per rolling hour. A service that
//!   needs a fourth restart in an hour has a real problem, and continuing to
//!   bounce it would bury that fact under identical events. At the cap the
//!   agent stops trying and reports once, at Critical.

use crate::config::ServiceEntry;
use crate::event::{DetectorEvent, Severity, Source};
use crate::services::Observed;
use log::{info, warn};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const HOUR: Duration = Duration::from_secs(3600);

/// What the healer is doing about one service.
enum Phase {
    /// Nothing in flight.
    Idle,
    /// A stop was issued on a wedged-but-running service; waiting for it to go
    /// down so it can be started clean.
    Stopping { issued: Instant },
    /// A start was issued; waiting to see whether it took.
    Verifying { issued: Instant },
    /// Hit the hourly cap; holding off until the window frees up.
    Exhausted,
}

struct Tracked {
    phase: Phase,
    /// Attempt timestamps inside the current rolling hour.
    attempts: VecDeque<Instant>,
    /// Set when a health probe reports the service unresponsive. The process
    /// is alive, so starting it is a no-op — it has to be stopped first.
    wedged: Option<String>,
}

impl Default for Tracked {
    fn default() -> Self {
        Self { phase: Phase::Idle, attempts: VecDeque::new(), wedged: None }
    }
}

/// How the healer actually starts a service. Injected so the decision logic —
/// the rate limiting especially — can be tested without a Service Control
/// Manager to talk to.
type Control = Box<dyn Fn(&str) -> anyhow::Result<()> + Send>;

pub struct Healer {
    entries: Vec<ServiceEntry>,
    tracked: HashMap<String, Tracked>,
    start: Control,
    stop: Control,
}

impl Healer {
    pub fn new(entries: Vec<ServiceEntry>) -> Self {
        Self::with_control(
            entries,
            Box::new(|name: &str| crate::services::start(name)),
            Box::new(|name: &str| crate::services::stop(name)),
        )
    }

    pub fn with_control(entries: Vec<ServiceEntry>, start: Control, stop: Control) -> Self {
        Self { entries, tracked: HashMap::new(), start, stop }
    }

    /// Records that a health probe has given up on a service. Nothing is done
    /// here — the next `poll()` decides, so probe-driven and SCM-driven
    /// recovery go through one state machine and one rate limiter, and a
    /// service cannot be bounced three times by each.
    pub fn note_wedged(&mut self, wedged: &[crate::health::Wedged]) {
        for w in wedged {
            // Only for services actually under management.
            if !self.entries.iter().any(|e| e.name == w.service && e.auto_restart) {
                continue;
            }
            self.tracked
                .entry(w.service.clone())
                .or_default()
                .wedged
                .get_or_insert_with(|| w.probe.clone());
        }
    }

    /// Decides what to do about each watched service, given the states the
    /// service collector just observed. Returns the events to report.
    pub fn poll(&mut self, seen: &HashMap<String, Observed>) -> Vec<DetectorEvent> {
        let mut out = Vec::new();

        for entry in self.entries.clone() {
            if !entry.auto_restart {
                continue;
            }
            let Some(observed) = seen.get(&entry.name) else {
                continue;
            };
            // Mid-transition, or not queryable at all — either way, hands off.
            if observed.is_transitional() || observed.is_unknown() {
                continue;
            }

            let grace = Duration::from_secs(entry.restart_grace_secs.max(10));
            let tracked = self.tracked.entry(entry.name.clone()).or_default();
            prune(&mut tracked.attempts);

            match tracked.phase {
                // ─── waiting to see whether our last start worked ─────────
                Phase::Verifying { issued } => {
                    if observed.is_running() {
                        tracked.phase = Phase::Idle;
                        info!("Heal: '{}' is back up", entry.label);
                        out.push(
                            DetectorEvent::new(
                                Source::Service,
                                Severity::Info,
                                "windows",
                                entry.name.clone(),
                                format!(
                                    "Үйлчилгээ '{}' автоматаар сэргэлээ ({} дэх оролдлого)",
                                    entry.label,
                                    tracked.attempts.len()
                                ),
                            )
                            .kod("heal/restarted")
                            .meta("serviceName", entry.name.clone())
                            .meta("label", entry.label.clone())
                            .meta("attemptsThisHour", tracked.attempts.len() as u64)
                            .refingerprint(),
                        );
                    } else if issued.elapsed() >= grace {
                        tracked.phase = Phase::Idle;
                        warn!("Heal: '{}' did not come up within grace", entry.label);
                        out.push(
                            DetectorEvent::new(
                                Source::Service,
                                Severity::Critical,
                                "windows",
                                entry.name.clone(),
                                format!(
                                    "Үйлчилгээ '{}' автоматаар асаах оролдлого бүтэлгүйтлээ ({}с хүлээсэн, төлөв: {})",
                                    entry.label,
                                    grace.as_secs(),
                                    observed.state
                                ),
                            )
                            .kod("heal/failed")
                            .meta("serviceName", entry.name.clone())
                            .meta("label", entry.label.clone())
                            .meta("state", observed.state.clone())
                            .meta("attemptsThisHour", tracked.attempts.len() as u64)
                            .refingerprint(),
                        );
                    }
                }

                // ─── capped out; wait for the window to free up ───────────
                Phase::Exhausted => {
                    if observed.is_running() {
                        // Someone (or the SCM) fixed it — resume normal duty.
                        tracked.phase = Phase::Idle;
                    } else if (tracked.attempts.len() as u32) < entry.max_restarts_per_hour {
                        tracked.phase = Phase::Idle;
                    }
                }

                // ─── stop issued on a wedged service; wait for it to die ──
                Phase::Stopping { issued } => {
                    if !observed.is_running() {
                        match (self.start)(&entry.name) {
                            Ok(()) => {
                                tracked.phase = Phase::Verifying { issued: Instant::now() };
                            }
                            Err(e) => {
                                tracked.phase = Phase::Idle;
                                out.push(heal_error(&entry, &e));
                            }
                        }
                    } else if issued.elapsed() >= grace {
                        // It ignored the stop. Escalating to a kill is not
                        // something an unattended agent should decide.
                        tracked.phase = Phase::Idle;
                        out.push(
                            DetectorEvent::new(
                                Source::Service,
                                Severity::Critical,
                                "windows",
                                entry.name.clone(),
                                format!(
                                    "Үйлчилгээ '{}' зогсох хүсэлтэд хариу өгсөнгүй ({}с). Гар аргаар шалгах шаардлагатай.",
                                    entry.label,
                                    grace.as_secs()
                                ),
                            )
                            .kod("heal/stuck")
                            .meta("serviceName", entry.name.clone())
                            .meta("label", entry.label.clone())
                            .refingerprint(),
                        );
                    }
                }

                // ─── idle: act if it is down, or alive but not serving ────
                Phase::Idle => {
                    let wedged = tracked.wedged.clone();
                    if observed.is_running() && wedged.is_none() {
                        continue;
                    }
                    if (tracked.attempts.len() as u32) >= entry.max_restarts_per_hour {
                        tracked.phase = Phase::Exhausted;
                        warn!(
                            "Heal: '{}' hit the restart cap ({}/hour)",
                            entry.label, entry.max_restarts_per_hour
                        );
                        out.push(
                            DetectorEvent::new(
                                Source::Service,
                                Severity::Critical,
                                "windows",
                                entry.name.clone(),
                                format!(
                                    "Үйлчилгээ '{}' нэг цагт {} удаа сэргээхийг оролдсон ч болсонгүй. Гар аргаар шалгах шаардлагатай.",
                                    entry.label, entry.max_restarts_per_hour
                                ),
                            )
                            .kod("heal/limit")
                            .meta("serviceName", entry.name.clone())
                            .meta("label", entry.label.clone())
                            .meta("maxPerHour", entry.max_restarts_per_hour as u64)
                            .refingerprint(),
                        );
                        continue;
                    }

                    tracked.attempts.push_back(Instant::now());
                    let attempt_no = tracked.attempts.len();
                    tracked.wedged = None;

                    // Down → just start it. Alive but unresponsive → a start
                    // would be a no-op, so it has to go down first.
                    let (action, result) = match &wedged {
                        Some(_) => ("stop", (self.stop)(&entry.name)),
                        None => ("start", (self.start)(&entry.name)),
                    };

                    match result {
                        Ok(()) => {
                            tracked.phase = if wedged.is_some() {
                                Phase::Stopping { issued: Instant::now() }
                            } else {
                                Phase::Verifying { issued: Instant::now() }
                            };
                            info!("Heal: issued {action} for '{}'", entry.label);

                            let msg = match &wedged {
                                Some(probe) => format!(
                                    "Үйлчилгээ '{}' ажиллаж байгаа ч '{}' шалгалтад хариу өгөхгүй тул дахин ачаалж байна ({}/{} оролдлого)",
                                    entry.label, probe, attempt_no, entry.max_restarts_per_hour
                                ),
                                None => format!(
                                    "Үйлчилгээ '{}' зогссон тул автоматаар асааж байна ({}/{} оролдлого)",
                                    entry.label, attempt_no, entry.max_restarts_per_hour
                                ),
                            };

                            let mut ev = DetectorEvent::new(
                                Source::Service,
                                Severity::Warning,
                                "windows",
                                entry.name.clone(),
                                msg,
                            )
                            .kod("heal/restarting")
                            .meta("serviceName", entry.name.clone())
                            .meta("label", entry.label.clone())
                            .meta("attempt", attempt_no as u64)
                            .meta("maxPerHour", entry.max_restarts_per_hour as u64);
                            if let Some(probe) = &wedged {
                                ev = ev.meta("reason", "probe").meta("probe", probe.clone());
                            }
                            out.push(ev.refingerprint());
                        }
                        Err(e) => {
                            tracked.phase = Phase::Idle;
                            warn!("Heal: could not {action} '{}': {e}", entry.label);
                            out.push(heal_error(&entry, &e));
                        }
                    }
                }
            }
        }

        out
    }

    /// Restart counts per service in the current rolling hour, for the
    /// heartbeat — the dashboard can then show "healed 2× today" without
    /// having to reconstruct it from the event stream.
    pub fn snapshot(&self) -> serde_json::Value {
        let map: serde_json::Map<String, serde_json::Value> = self
            .tracked
            .iter()
            .filter(|(_, t)| !t.attempts.is_empty())
            .map(|(name, t)| (name.clone(), serde_json::json!(t.attempts.len())))
            .collect();
        serde_json::Value::Object(map)
    }
}

fn heal_error(entry: &ServiceEntry, e: &anyhow::Error) -> DetectorEvent {
    DetectorEvent::new(
        Source::Service,
        Severity::Critical,
        "windows",
        entry.name.clone(),
        format!("Үйлчилгээ '{}' автоматаар сэргээх боломжгүй: {e}", entry.label),
    )
    .kod("heal/error")
    .meta("serviceName", entry.name.clone())
    .meta("label", entry.label.clone())
    .meta("error", e.to_string())
    .refingerprint()
}

/// Drops attempts that have aged out of the rolling hour.
fn prune(attempts: &mut VecDeque<Instant>) {
    while attempts.front().map(|t| t.elapsed() >= HOUR).unwrap_or(false) {
        attempts.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const SVC: &str = "TestSvc";

    fn entry(max_per_hour: u32, auto_restart: bool) -> ServiceEntry {
        ServiceEntry {
            name: SVC.into(),
            label: "Test Service".into(),
            must_run: true,
            auto_restart,
            max_restarts_per_hour: max_per_hour,
            restart_grace_secs: 45,
        }
    }

    fn seen(state: &str) -> HashMap<String, Observed> {
        let mut map = HashMap::new();
        map.insert(
            SVC.to_string(),
            Observed { state: state.into(), pid: 0, exit_code: 0 },
        );
        map
    }

    /// A control that always succeeds, counting how many times it was asked.
    fn counting() -> (Control, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        (
            Box::new(move |_name: &str| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            calls,
        )
    }

    /// Healer wired to counting start/stop, returning both counters.
    fn healer(entry: ServiceEntry) -> (Healer, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (starter, starts) = counting();
        let (stopper, stops) = counting();
        (Healer::with_control(vec![entry], starter, stopper), starts, stops)
    }

    fn wedged(probe: &str) -> Vec<crate::health::Wedged> {
        vec![crate::health::Wedged {
            service: SVC.into(),
            probe: probe.into(),
            failures: 3,
        }]
    }

    fn kods(events: &[DetectorEvent]) -> Vec<&str> {
        events.iter().map(|e| e.aldaanii_kod.as_str()).collect()
    }

    #[test]
    fn stopped_service_is_started_once_and_announced() {
        let (mut healer, starts, _) = healer(entry(3, true));

        let events = healer.poll(&seen("Stopped"));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(kods(&events), ["heal/restarting"]);
    }

    #[test]
    fn does_not_start_again_while_waiting_to_see_if_it_worked() {
        let (mut healer, calls, _) = healer(entry(3, true));

        healer.poll(&seen("Stopped"));
        // Still down a moment later — the start we issued is simply not
        // finished yet. Issuing another would stack starts on the SCM.
        let events = healer.poll(&seen("Stopped"));

        assert_eq!(calls.load(Ordering::SeqCst), 1, "must not re-issue during grace");
        assert!(events.is_empty(), "nothing new to report yet, got {:?}", kods(&events));
    }

    #[test]
    fn recovery_is_reported_once_then_goes_quiet() {
        let (mut healer, _, _) = healer(entry(3, true));

        healer.poll(&seen("Stopped"));
        assert_eq!(kods(&healer.poll(&seen("Running"))), ["heal/restarted"]);
        // A healthy service must not keep generating traffic.
        assert!(healer.poll(&seen("Running")).is_empty());
    }

    #[test]
    fn flapping_service_stops_being_restarted_at_the_cap() {
        let (mut healer, calls, _) = healer(entry(3, true));

        // Three full down→up cycles: each one should earn a restart.
        for _ in 0..3 {
            healer.poll(&seen("Stopped"));
            healer.poll(&seen("Running"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // The fourth failure inside the same hour is not another restart —
        // it is a Critical saying a human is needed.
        let events = healer.poll(&seen("Stopped"));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "cap must hold");
        assert_eq!(kods(&events), ["heal/limit"]);

        // And it must not repeat that Critical on every subsequent poll.
        assert!(healer.poll(&seen("Stopped")).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_failing_start_is_reported_and_does_not_wedge_the_healer() {
        let mut healer = Healer::with_control(
            vec![entry(2, true)],
            Box::new(|_| anyhow::bail!("access denied")),
            Box::new(|_| Ok(())),
        );

        assert_eq!(kods(&healer.poll(&seen("Stopped"))), ["heal/error"]);
        // Back to Idle, so the next cycle tries again — up to the cap.
        assert_eq!(kods(&healer.poll(&seen("Stopped"))), ["heal/error"]);
        assert_eq!(kods(&healer.poll(&seen("Stopped"))), ["heal/limit"]);
    }

    #[test]
    fn auto_restart_off_means_hands_off() {
        let (mut healer, calls, _) = healer(entry(3, false));

        assert!(healer.poll(&seen("Stopped")).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn transitional_and_unqueryable_states_are_left_alone() {
        for state in ["StartPending", "StopPending", "Unknown (not installed)"] {
            let (mut healer, calls, _) = healer(entry(3, true));

            let events = healer.poll(&seen(state));
            assert_eq!(calls.load(Ordering::SeqCst), 0, "{state} must not be touched");
            assert!(events.is_empty(), "{state} produced {:?}", kods(&events));
        }
    }

    // ─── probe-driven healing ────────────────────────────────────────────

    #[test]
    fn a_running_but_unresponsive_service_is_stopped_then_started() {
        let (mut healer, starts, stops) = healer(entry(3, true));

        healer.note_wedged(&wedged("dahua api"));

        // Still "Running" as far as the SCM is concerned — a start would do
        // nothing, so the stop has to come first.
        let events = healer.poll(&seen("Running"));
        assert_eq!(stops.load(Ordering::SeqCst), 1, "must stop it");
        assert_eq!(starts.load(Ordering::SeqCst), 0, "must not start a running service");
        assert_eq!(kods(&events), ["heal/restarting"]);

        // Once it is actually down, start it.
        assert!(healer.poll(&seen("Stopped")).is_empty());
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        assert_eq!(kods(&healer.poll(&seen("Running"))), ["heal/restarted"]);
    }

    #[test]
    fn probe_restarts_share_the_same_hourly_cap_as_crash_restarts() {
        let (mut healer, _, _) = healer(entry(2, true));

        // Two crash-driven restarts use up the budget.
        for _ in 0..2 {
            healer.poll(&seen("Stopped"));
            healer.poll(&seen("Running"));
        }

        // A probe failure must not buy two more attempts.
        healer.note_wedged(&wedged("dahua api"));
        assert_eq!(kods(&healer.poll(&seen("Running"))), ["heal/limit"]);
    }

    #[test]
    fn probe_report_for_an_unmanaged_service_is_ignored() {
        let (mut healer, starts, stops) = healer(entry(3, true));

        healer.note_wedged(&[crate::health::Wedged {
            service: "SomeOtherService".into(),
            probe: "other".into(),
            failures: 5,
        }]);

        assert!(healer.poll(&seen("Running")).is_empty());
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
    }
}
