//! Windows service watcher.
//!
//! Edge-triggered: a service that is simply running produces no traffic. A
//! transition (Running → Stopped, Stopped → Running, or a non-zero exit code)
//! produces one event, and a service that stays down re-announces itself every
//! `REMIND_SECS` so it does not scroll off the dashboard and get forgotten.

use crate::config::ServiceEntry;
use crate::event::{DetectorEvent, Severity, Source};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const REMIND: Duration = Duration::from_secs(900); // 15 min

struct Known {
    state: String,
    since: Instant,
    last_reported: Option<Instant>,
}

pub struct ServiceCollector {
    entries: Vec<ServiceEntry>,
    known: HashMap<String, Known>,
    /// Whatever the last `poll()` actually saw, keyed by service name. The
    /// healer reads this instead of querying the SCM again, so both are
    /// deciding from exactly the same observation.
    last_seen: HashMap<String, Observed>,
}

impl ServiceCollector {
    pub fn new(entries: Vec<ServiceEntry>) -> Self {
        Self { entries, known: HashMap::new(), last_seen: HashMap::new() }
    }

    pub fn last_seen(&self) -> &HashMap<String, Observed> {
        &self.last_seen
    }

    pub fn poll(&mut self) -> Vec<DetectorEvent> {
        let mut out = Vec::new();

        for entry in self.entries.clone() {
            let observed = platform::query(&entry.name);
            let (state, pid, exit_code) = match observed {
                Ok(s) => (s.state, s.pid, s.exit_code),
                Err(e) => (format!("Unknown ({e})"), 0, 0),
            };
            self.last_seen.insert(
                entry.name.clone(),
                Observed { state: state.clone(), pid, exit_code },
            );

            let running = state == "Running";
            let prev = self.known.get(&entry.name);
            let changed = prev.map(|k| k.state != state).unwrap_or(true);
            let is_first = prev.is_none();

            let due_reminder = !running
                && prev
                    .and_then(|k| k.last_reported)
                    .map(|t| t.elapsed() >= REMIND)
                    .unwrap_or(true);

            // A first sighting of a healthy service is the normal case at
            // startup — record it, say nothing.
            let should_report = (changed && !(is_first && running)) || due_reminder;

            if should_report {
                let severity = if running {
                    Severity::Info
                } else if entry.must_run {
                    Severity::Critical
                } else {
                    Severity::Warning
                };

                let downtime = prev
                    .filter(|_| !running)
                    .map(|k| k.since.elapsed().as_secs())
                    .unwrap_or(0);

                let msg = if running {
                    format!("Үйлчилгээ '{}' ажиллаж эхэллээ", entry.label)
                } else if due_reminder && !changed {
                    format!(
                        "Үйлчилгээ '{}' зогссон хэвээр байна ({state}), {}м",
                        entry.label,
                        downtime / 60
                    )
                } else {
                    format!("Үйлчилгээ '{}' зогссон: {state}", entry.label)
                };

                let mut ev = DetectorEvent::new(
                    Source::Service,
                    severity,
                    "windows",
                    entry.name.clone(),
                    msg,
                )
                .kod(format!("service/{}", state))
                .meta("serviceName", entry.name.clone())
                .meta("label", entry.label.clone())
                .meta("state", state.clone())
                .meta("pid", pid)
                .meta("mustRun", entry.must_run);

                if exit_code != 0 {
                    ev = ev.meta("exitCode", exit_code);
                }
                if downtime > 0 {
                    ev = ev.meta("downtimeSecs", downtime);
                }
                out.push(ev.refingerprint());
            }

            let entry_known = self.known.entry(entry.name.clone()).or_insert(Known {
                state: state.clone(),
                since: Instant::now(),
                last_reported: None,
            });
            if entry_known.state != state {
                entry_known.state = state;
                entry_known.since = Instant::now();
            }
            if should_report {
                entry_known.last_reported = Some(Instant::now());
            }
        }

        out
    }

    /// Current state of every watched service, for the heartbeat payload.
    pub fn snapshot(&self) -> Vec<serde_json::Value> {
        self.entries
            .iter()
            .map(|e| {
                let state = self
                    .known
                    .get(&e.name)
                    .map(|k| k.state.clone())
                    .unwrap_or_else(|| "Unknown".to_string());
                serde_json::json!({
                    "name": e.name,
                    "label": e.label,
                    "state": state,
                    "mustRun": e.must_run,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Observed {
    pub state: String,
    pub pid: u32,
    pub exit_code: u32,
}

impl Observed {
    pub fn is_running(&self) -> bool {
        self.state == "Running"
    }

    /// True while the SCM is mid-transition. Acting on these would mean
    /// fighting a start that is already happening.
    pub fn is_transitional(&self) -> bool {
        matches!(
            self.state.as_str(),
            "StartPending" | "StopPending" | "ContinuePending" | "PausePending"
        )
    }

    /// True when the service could not be queried at all — usually "not
    /// installed" or a rights problem, neither of which a restart fixes.
    pub fn is_unknown(&self) -> bool {
        self.state.starts_with("Unknown")
    }
}

/// Asks the SCM to start a service. The agent runs as LocalSystem, so this
/// needs no elevation dance — unlike the desktop app, which has to shell out.
pub fn start(name: &str) -> anyhow::Result<()> {
    platform::start(name)
}

/// Asks the SCM to stop a service. Returns as soon as the request is accepted;
/// the stop itself is observed on a later poll rather than waited for, so the
/// detector loop is never blocked by a service that takes its time shutting down.
pub fn stop(name: &str) -> anyhow::Result<()> {
    platform::stop(name)
}

/// The exe the SCM will actually launch for this service.
///
/// Asked of the SCM rather than assumed, because the installed location is
/// wherever `install` happened to be run from — for dahuarust that is its own
/// `target\release`, not a Program Files path. Replacing the wrong file would
/// leave the real service untouched and look like a silently failed update.
pub fn exe_path(name: &str) -> anyhow::Result<std::path::PathBuf> {
    platform::exe_path(name)
}

/// Blocks until the service reaches (or leaves) the Running state, or the
/// deadline passes. Used only by the updater, which genuinely must serialise:
/// the exe cannot be replaced until the process holding it has exited.
pub fn wait_for(name: &str, running: bool, timeout: Duration) -> anyhow::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        match platform::query(name) {
            Ok(observed) if observed.is_running() == running => return Ok(true),
            // "Not installed" will never change by waiting.
            Err(e) if running => return Err(e),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(windows)]
mod platform {
    use super::Observed;
    use windows_service::service::{ServiceAccess, ServiceExitCode, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    pub fn start(name: &str) -> anyhow::Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(name, ServiceAccess::START)?;
        service.start::<&str>(&[])?;
        Ok(())
    }

    pub fn stop(name: &str) -> anyhow::Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(name, ServiceAccess::STOP)?;
        service.stop()?;
        Ok(())
    }

    pub fn exe_path(name: &str) -> anyhow::Result<std::path::PathBuf> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(name, ServiceAccess::QUERY_CONFIG)?;
        Ok(service.query_config()?.executable_path)
    }

    pub fn query(name: &str) -> anyhow::Result<Observed> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(name, ServiceAccess::QUERY_STATUS)?;
        let status = service.query_status()?;

        let state = match status.current_state {
            ServiceState::Stopped => "Stopped",
            ServiceState::StartPending => "StartPending",
            ServiceState::StopPending => "StopPending",
            ServiceState::Running => "Running",
            ServiceState::ContinuePending => "ContinuePending",
            ServiceState::PausePending => "PausePending",
            ServiceState::Paused => "Paused",
        }
        .to_string();

        let exit_code = match status.exit_code {
            ServiceExitCode::Win32(c) => c,
            ServiceExitCode::ServiceSpecific(c) => c,
        };

        Ok(Observed { state, pid: status.process_id.unwrap_or(0), exit_code })
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Observed;

    pub fn query(_name: &str) -> anyhow::Result<Observed> {
        anyhow::bail!("service control manager is only available on Windows")
    }

    pub fn start(_name: &str) -> anyhow::Result<()> {
        anyhow::bail!("service control manager is only available on Windows")
    }

    pub fn stop(_name: &str) -> anyhow::Result<()> {
        anyhow::bail!("service control manager is only available on Windows")
    }

    pub fn exe_path(_name: &str) -> anyhow::Result<std::path::PathBuf> {
        anyhow::bail!("service control manager is only available on Windows")
    }
}
