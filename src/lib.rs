//! Zev Detector — Windows activity / error detector for the Zevtabs stack.
//!
//! Watches four things on the machine it runs on:
//!   1. Windows Event Log (System / Application / any channel) — Critical,
//!      Error and Warning events, with the publisher's real message text.
//!   2. Windows services (DahuaParkingService and friends) — start, stop,
//!      crash and exit codes, via the Service Control Manager.
//!   3. Application log files — dahuarust's `service.log` and any other log,
//!      tailed incrementally with rotation handling.
//!   4. Host and endpoint health — CPU / RAM / disk thresholds and HTTP probes.
//!
//! Everything it finds is de-duplicated, classified Critical / Warning / Info,
//! queued to disk if the network is down, and posted to udirdlagaBack's
//! `/detector` endpoint where the Zevtabs "Detector" page renders it.
//!
//! This crate builds two binaries on top of these modules:
//!
//!   * `zev-detector.exe`     — the headless agent: the Windows service
//!                              itself plus its install/start/stop CLI.
//!   * `zev-detector-ui.exe`  — the desktop app people actually open, which
//!                              edits config.toml and drives the service.
//!
//! Keeping them separate means the service binary the SCM launches stays a
//! plain console program with no window, no WebView and no GUI dependencies,
//! exactly as it was before the desktop app existed.

pub mod config;
pub mod elevate;
pub mod event;
pub mod eventlog;
pub mod health;
pub mod logwatch;
pub mod runner;
pub mod sender;
pub mod services;
pub mod winsvc;

pub const SERVICE_NAME: &str = "ZevDetector";
pub const SERVICE_DISPLAY: &str = "Zev Detector";
pub const SERVICE_DESC: &str =
    "Windows event log, service, application log and health detector for Zevtabs";

/// Service state as the unelevated desktop app is allowed to read it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SvcStatus {
    pub installed: bool,
    pub state: Option<String>,
    pub pid: Option<u32>,
}

impl SvcStatus {
    pub fn not_installed() -> Self {
        Self { installed: false, state: None, pid: None }
    }
}
