//! config.toml loading. Every section has a `#[serde(default)]` so a partial
//! config file still boots — a missing `[health]` block just means health
//! probing is off, not a startup failure.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub eventlog: EventLogConfig,
    #[serde(default)]
    pub services: Vec<ServiceEntry>,
    #[serde(default)]
    pub logs: LogsConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub netqual: NetQualConfig,
    /// Local TCP ports that should be listening (cameras, RTSP, Dahua, the
    /// parking service itself).
    #[serde(default)]
    pub ports: Vec<PortEntry>,
    #[serde(default)]
    pub timesync: TimeSyncConfig,
    #[serde(default)]
    pub cleanup: CleanupConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
}

// ─── clock ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TimeSyncConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_time_poll")]
    pub poll_interval_secs: u64,
    /// Drift beyond this many seconds is reported, and resynced if allowed.
    #[serde(default = "d_max_drift")]
    pub max_drift_secs: i64,
    /// Run `w32tm /resync` when drift exceeds the limit.
    ///
    /// Worth knowing: this moves the clock under whatever is running, so a
    /// parking session in progress can have its duration shift. That is still
    /// better than a machine whose timestamps and JWT validation are wrong.
    #[serde(default = "d_true")]
    pub auto_resync: bool,
}

impl Default for TimeSyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: d_time_poll(),
            max_drift_secs: d_max_drift(),
            auto_resync: true,
        }
    }
}

fn d_time_poll() -> u64 { 900 }
fn d_max_drift() -> i64 { 30 }

// ─── disk cleanup ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CleanupConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// 0 = Sunday … 6 = Saturday.
    #[serde(default)]
    pub weekday: u8,
    /// Local hour to run at.
    #[serde(default = "d_cleanup_hour")]
    pub hour: u32,
    /// Nothing newer than this is ever deleted.
    #[serde(default = "d_cleanup_age")]
    pub min_age_days: u64,
    /// Also run whenever a disk crosses this used-percentage, not just weekly.
    /// 0 disables the emergency pass.
    #[serde(default = "d_cleanup_trigger")]
    pub disk_trigger_percent: f32,
    /// Rotate our own log once it passes this size.
    #[serde(default = "d_log_max_mb")]
    pub log_max_mb: u64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            weekday: 0,
            hour: d_cleanup_hour(),
            min_age_days: d_cleanup_age(),
            disk_trigger_percent: d_cleanup_trigger(),
            log_max_mb: d_log_max_mb(),
        }
    }
}

fn d_cleanup_hour() -> u32 { 3 }
fn d_cleanup_age() -> u64 { 7 }
fn d_cleanup_trigger() -> f32 { 92.0 }
fn d_log_max_mb() -> u64 { 32 }

// ─── local notification ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotifyConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_true")]
    pub popup_enabled: bool,
    #[serde(default = "d_true")]
    pub toast_enabled: bool,
    #[serde(default = "d_true")]
    pub sound_enabled: bool,
    #[serde(default = "d_notify_net_threshold")]
    pub netqual_threshold: u8,
    #[serde(default = "d_notify_disk_threshold")]
    pub disk_threshold: f32,
    #[serde(default = "d_notify_cooldown")]
    pub cooldown_mins: u64,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            popup_enabled: true,
            toast_enabled: true,
            sound_enabled: true,
            netqual_threshold: d_notify_net_threshold(),
            disk_threshold: d_notify_disk_threshold(),
            cooldown_mins: d_notify_cooldown(),
        }
    }
}

fn d_notify_net_threshold() -> u8 { 70 }
fn d_notify_disk_threshold() -> f32 { 90.0 }
fn d_notify_cooldown() -> u64 { 15 }

// ─── network quality ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetQualConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_netqual_poll")]
    pub poll_interval_secs: u64,
    /// Host to ping. Empty means "the host from [server].url", which is the
    /// link that actually matters to this agent.
    #[serde(default)]
    pub ping_host: String,
    #[serde(default = "d_ping_count")]
    pub ping_count: u32,
    /// Report a Warning below this score.
    #[serde(default = "d_score_threshold")]
    pub score_threshold: u32,
}

impl Default for NetQualConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: d_netqual_poll(),
            ping_host: String::new(),
            ping_count: d_ping_count(),
            score_threshold: d_score_threshold(),
        }
    }
}

fn d_netqual_poll() -> u64 { 300 }
fn d_ping_count() -> u32 { 10 }
fn d_score_threshold() -> u32 { 70 }

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct PortEntry {
    pub port: u16,
    #[serde(default)]
    pub label: String,
    /// Defaults to 127.0.0.1 — these are local listeners, not remote hosts.
    #[serde(default = "d_port_host")]
    pub host: String,
    /// Report a Critical when nothing is listening.
    #[serde(default = "d_true")]
    pub must_listen: bool,
}

impl Default for PortEntry {
    fn default() -> Self {
        Self { port: 0, label: String::new(), host: d_port_host(), must_listen: true }
    }
}

fn d_port_host() -> String { "127.0.0.1".to_string() }

// ─── self-update ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UpdateConfig {
    /// "off" — never check.
    /// "ask" — notice a new release and report it; a human approves.
    /// "auto" — install it unattended.
    ///
    /// Defaults to "ask" on purpose: a parking site runs all day, and a service
    /// restarting mid-shift without anyone expecting it is worse than being a
    /// version behind.
    #[serde(default = "d_update_mode")]
    pub mode: String,
    #[serde(default = "d_update_interval")]
    pub check_interval_hours: u64,
    /// GitHub repository releases are published to, as "owner/name".
    #[serde(default = "d_update_repo")]
    pub github_repo: String,
    /// Personal access token, needed only for a private repository.
    #[serde(default)]
    pub github_token: String,
    /// Only install inside this local-time window, as "HH:MM-HH:MM".
    /// Empty means any time. Ignored unless mode = "auto".
    #[serde(default)]
    pub window: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            mode: d_update_mode(),
            check_interval_hours: d_update_interval(),
            github_repo: d_update_repo(),
            github_token: String::new(),
            window: String::new(),
        }
    }
}

fn d_update_mode() -> String { "auto".to_string() }
fn d_update_interval() -> u64 { 6 }
pub fn d_update_repo() -> String { "batkt/zevAIexeTools".to_string() }

// ─── server ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    /// Base URL of udirdlagaBack, e.g. https://admin.zevtabs.mn/api
    #[serde(default = "d_server_url")]
    pub url: String,
    /// Bearer token sent with every request (optional; empty = no header)
    #[serde(default)]
    pub token: String,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "d_retry")]
    pub retry_count: u32,
    /// Max events per POST /detector call
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    /// How often the queue is flushed to the server
    #[serde(default = "d_send_interval")]
    pub send_interval_secs: u64,
    /// Max events kept on disk while offline before the oldest are dropped
    #[serde(default = "d_queue_max")]
    pub queue_max: usize,
}

pub fn d_server_url() -> String {
    "https://admin.zevtabs.mn/api".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: d_server_url(),
            token: String::new(),
            timeout_secs: d_timeout(),
            retry_count: d_retry(),
            batch_size: d_batch(),
            send_interval_secs: d_send_interval(),
            queue_max: d_queue_max(),
        }
    }
}

fn d_timeout() -> u64 { 20 }
fn d_retry() -> u32 { 3 }
fn d_batch() -> usize { 50 }
fn d_send_interval() -> u64 { 30 }
fn d_queue_max() -> usize { 5000 }

// ─── agent identity ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AgentConfig {
    /// Display name for this machine. Defaults to the Windows hostname.
    #[serde(default)]
    pub name: String,
    #[serde(rename = "baiguullagiinId", default)]
    pub baiguullagiin_id: String,
    #[serde(rename = "baiguullagiinNer", default)]
    pub baiguullagiin_ner: String,
    #[serde(rename = "barilgiinId", default)]
    pub barilgiin_id: String,
    #[serde(default = "d_heartbeat")]
    pub heartbeat_interval_secs: u64,
}

fn d_heartbeat() -> u64 { 60 }

// ─── windows event log ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EventLogConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_evt_poll")]
    pub poll_interval_secs: u64,
    /// Channels to query — "System", "Application", "Security", or any
    /// "Microsoft-Windows-…/Operational" channel name.
    #[serde(default = "d_channels")]
    pub channels: Vec<String>,
    /// 1=Critical 2=Error 3=Warning 4=Information. Events with Level > this
    /// are ignored, so 3 catches Critical+Error+Warning.
    #[serde(default = "d_max_level")]
    pub max_level: u32,
    /// If non-empty, only these providers are reported.
    #[serde(default)]
    pub providers: Vec<String>,
    /// Noisy event ids to drop (10016 DCOM permission spam is a classic).
    #[serde(default)]
    pub ignore_event_ids: Vec<u32>,
    /// Substrings that, if present in the message, drop the event.
    #[serde(default)]
    pub ignore_contains: Vec<String>,
    #[serde(default = "d_evt_max")]
    pub max_per_poll: usize,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: d_evt_poll(),
            channels: d_channels(),
            max_level: d_max_level(),
            providers: Vec::new(),
            ignore_event_ids: Vec::new(),
            ignore_contains: Vec::new(),
            max_per_poll: d_evt_max(),
        }
    }
}

fn d_true() -> bool { true }
fn d_evt_poll() -> u64 { 60 }
fn d_max_level() -> u32 { 3 }
fn d_evt_max() -> usize { 200 }
fn d_channels() -> Vec<String> {
    vec!["System".to_string(), "Application".to_string()]
}

// ─── windows services ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct ServiceEntry {
    /// Service key name as SCM knows it, e.g. "DahuaParkingService"
    pub name: String,
    /// Friendly label used in the dashboard. Defaults to `name`.
    #[serde(default)]
    pub label: String,
    /// Report a Critical when the service is not Running.
    #[serde(default = "d_true")]
    pub must_run: bool,

    // ─── self-healing ────────────────────────────────────────────────────
    /// Try to start the service again when it is found stopped.
    ///
    /// This is not the same as the SCM's own failure actions, which only fire
    /// when a service *crashes*. A service that exited cleanly, was stopped by
    /// something else, or that the SCM has already given up on stays down —
    /// that is the gap this closes.
    #[serde(default = "d_true")]
    pub auto_restart: bool,
    /// Ceiling on restart attempts in any rolling hour. A service that needs
    /// more than this is not going to be fixed by another restart, so the
    /// agent stops and reports instead of hammering it forever.
    #[serde(default = "d_max_restarts")]
    pub max_restarts_per_hour: u32,
    /// How long to let a start attempt settle before judging it. Must comfortably
    /// exceed the service's own startup time or every start looks like a failure.
    #[serde(default = "d_restart_grace")]
    pub restart_grace_secs: u64,
}

fn d_max_restarts() -> u32 { 3 }
fn d_restart_grace() -> u64 { 45 }

impl Default for ServiceEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            label: String::new(),
            must_run: true,
            auto_restart: true,
            max_restarts_per_hour: d_max_restarts(),
            restart_grace_secs: d_restart_grace(),
        }
    }
}

// ─── application log files ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogsConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_log_poll")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub files: Vec<LogFileEntry>,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self { enabled: true, poll_interval_secs: d_log_poll(), files: Vec::new() }
    }
}

fn d_log_poll() -> u64 { 15 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LogFileEntry {
    /// Full path, or a directory + `glob` to pick up rotated files.
    pub path: String,
    /// Optional filename pattern when `path` is a directory, e.g. "*.log"
    #[serde(default)]
    pub glob: String,
    /// Value written into the `system` column, e.g. "dahuarust"
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub modul: String,
    /// Regex a line must match to be reported. Default catches the
    /// `[ERROR]` / `[WARN]` levels fern writes plus Rust panics.
    #[serde(default)]
    pub pattern: String,
    /// Regex that, if it matches, drops the line even when `pattern` hit.
    #[serde(default)]
    pub ignore_pattern: String,
    /// Number of following lines attached as stack trace context.
    #[serde(default = "d_context")]
    pub context_lines: usize,
}

fn d_context() -> usize { 8 }

impl Default for LogFileEntry {
    fn default() -> Self {
        Self {
            path: String::new(),
            glob: String::new(),
            system: String::new(),
            modul: String::new(),
            pattern: String::new(),
            ignore_pattern: String::new(),
            context_lines: d_context(),
        }
    }
}

// ─── health probes ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HealthConfig {
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(default = "d_health_poll")]
    pub poll_interval_secs: u64,
    #[serde(default = "d_thresh")]
    pub cpu_threshold: f32,
    #[serde(default = "d_thresh")]
    pub mem_threshold: f32,
    #[serde(default = "d_thresh")]
    pub disk_threshold: f32,
    #[serde(default)]
    pub http: Vec<HttpProbe>,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: d_health_poll(),
            cpu_threshold: d_thresh(),
            mem_threshold: d_thresh(),
            disk_threshold: d_thresh(),
            http: Vec::new(),
        }
    }
}

fn d_health_poll() -> u64 { 60 }
fn d_thresh() -> f32 { 90.0 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HttpProbe {
    pub name: String,
    pub url: String,
    /// Substring the body must contain to count as healthy (optional).
    #[serde(default)]
    pub expect: String,
    #[serde(default = "d_probe_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub system: String,

    /// SCM name of the service behind this endpoint. Set it and a persistently
    /// unanswered probe will restart that service — which catches the failure
    /// the Service Control Manager cannot see: a process that is still
    /// `Running` but wedged and no longer serving.
    #[serde(default)]
    pub service: String,
    /// Consecutive failed probes before that restart. Anything lower than a
    /// couple of cycles risks restarting on one dropped request.
    #[serde(default = "d_probe_fails")]
    pub restart_after_failures: u32,
}

fn d_probe_timeout() -> u64 { 5 }
fn d_probe_fails() -> u32 { 3 }

impl Default for HttpProbe {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            expect: String::new(),
            timeout_secs: d_probe_timeout(),
            system: String::new(),
            service: String::new(),
            restart_after_failures: d_probe_fails(),
        }
    }
}

// ─── loading & zero-configuration defaults ───────────────────────────────────

impl Config {
    pub fn default_builtin() -> Self {
        Self {
            server: ServerConfig::default(),
            agent: AgentConfig {
                name: hostname(),
                baiguullagiin_id: String::new(),
                baiguullagiin_ner: String::new(),
                barilgiin_id: String::new(),
                heartbeat_interval_secs: d_heartbeat(),
            },
            eventlog: EventLogConfig::default(),
            services: vec![
                ServiceEntry {
                    name: "DahuaParkingService".to_string(),
                    label: "Dahua Parking (dahuarust)".to_string(),
                    must_run: false,
                    auto_restart: true,
                    max_restarts_per_hour: 3,
                    restart_grace_secs: 45,
                },
                ServiceEntry {
                    name: "HikvisionParkingService".to_string(),
                    label: "Hikvision Parking Service".to_string(),
                    must_run: false,
                    auto_restart: true,
                    max_restarts_per_hour: 3,
                    restart_grace_secs: 45,
                },
                ServiceEntry {
                    name: "ZevWorker".to_string(),
                    label: "Zev Worker".to_string(),
                    must_run: false,
                    auto_restart: true,
                    max_restarts_per_hour: 3,
                    restart_grace_secs: 45,
                },
            ],
            logs: LogsConfig {
                enabled: true,
                poll_interval_secs: d_log_poll(),
                files: vec![
                    LogFileEntry {
                        path: r"D:\zevtabs\dahuarust\dahuaExeRust\target\release\service.log".to_string(),
                        glob: String::new(),
                        system: "dahuarust".to_string(),
                        modul: "dahua-service".to_string(),
                        pattern: String::new(),
                        ignore_pattern: String::new(),
                        context_lines: 8,
                    },
                    LogFileEntry {
                        path: r"C:\Program Files\Zevtabs\service.log".to_string(),
                        glob: String::new(),
                        system: "dahuarust".to_string(),
                        modul: "dahua-service".to_string(),
                        pattern: String::new(),
                        ignore_pattern: String::new(),
                        context_lines: 8,
                    },
                    LogFileEntry {
                        path: r"D:\zevtabs\worker\logs".to_string(),
                        glob: "*.log".to_string(),
                        system: "worker".to_string(),
                        modul: "worker-service".to_string(),
                        pattern: String::new(),
                        ignore_pattern: String::new(),
                        context_lines: 8,
                    },
                ],
            },
            health: HealthConfig {
                enabled: true,
                poll_interval_secs: d_health_poll(),
                cpu_threshold: 90.0,
                mem_threshold: 90.0,
                disk_threshold: 90.0,
                http: vec![HttpProbe {
                    name: "dahua api".to_string(),
                    url: "http://127.0.0.1:5000/api/health".to_string(),
                    expect: "OK".to_string(),
                    timeout_secs: 5,
                    system: "dahuarust".to_string(),
                    service: "DahuaParkingService".to_string(),
                    restart_after_failures: 3,
                }],
            },
            update: UpdateConfig::default(),
            netqual: NetQualConfig::default(),
            ports: vec![
                PortEntry {
                    port: 5000,
                    label: "Parking service".to_string(),
                    host: "127.0.0.1".to_string(),
                    must_listen: false,
                },
                PortEntry {
                    port: 37777,
                    label: "Dahua SDK".to_string(),
                    host: "127.0.0.1".to_string(),
                    must_listen: false,
                },
                PortEntry {
                    port: 8000,
                    label: "Hikvision SDK".to_string(),
                    host: "127.0.0.1".to_string(),
                    must_listen: false,
                },
            ],
            timesync: TimeSyncConfig::default(),
            cleanup: CleanupConfig::default(),
            notify: NotifyConfig::default(),
        }
    }

    pub fn apply_defaults(&mut self) {
        if self.server.url.trim().is_empty() {
            self.server.url = d_server_url();
        }
        while self.server.url.ends_with('/') {
            self.server.url.pop();
        }
        if self.agent.name.trim().is_empty() {
            self.agent.name = hostname();
        }
        if self.agent.heartbeat_interval_secs == 0 {
            self.agent.heartbeat_interval_secs = d_heartbeat();
        }

        // Retain only services with non-empty name
        self.services.retain(|s| !s.name.trim().is_empty());
        for s in &mut self.services {
            if s.label.trim().is_empty() {
                s.label = s.name.clone();
            }
        }
        if self.services.is_empty() {
            self.services = Self::default_builtin().services;
        }

        // Retain only valid ports
        self.ports.retain(|p| p.port > 0);
        if self.ports.is_empty() {
            self.ports = Self::default_builtin().ports;
        }

        // Retain only log targets with non-empty path
        self.logs.files.retain(|f| !f.path.trim().is_empty());
        if self.logs.files.is_empty() {
            self.logs.files = Self::default_builtin().logs.files;
        }

        // Health probes
        self.health.http.retain(|h| !h.url.trim().is_empty());
        if self.health.http.is_empty() {
            self.health.http = Self::default_builtin().health.http;
        }

        if self.update.github_repo.trim().is_empty() {
            self.update.github_repo = d_update_repo();
        }
        if self.update.mode.trim().is_empty() {
            self.update.mode = d_update_mode();
        }
    }

    pub fn merge_json(&mut self, val: &serde_json::Value) {
        if let Some(obj) = val.as_object() {
            if let Some(services) = obj.get("services") {
                if let Ok(s) = serde_json::from_value::<Vec<ServiceEntry>>(services.clone()) {
                    self.services = s;
                }
            }
            if let Some(ports) = obj.get("ports") {
                if let Ok(p) = serde_json::from_value::<Vec<PortEntry>>(ports.clone()) {
                    self.ports = p;
                }
            }
            if let Some(health) = obj.get("health") {
                if let Ok(h) = serde_json::from_value::<HealthConfig>(health.clone()) {
                    self.health = h;
                }
            }
            if let Some(netqual) = obj.get("netqual") {
                if let Ok(n) = serde_json::from_value::<NetQualConfig>(netqual.clone()) {
                    self.netqual = n;
                }
            }
            if let Some(notify) = obj.get("notify") {
                if let Ok(nt) = serde_json::from_value::<NotifyConfig>(notify.clone()) {
                    self.notify = nt;
                }
            }
            if let Some(update) = obj.get("update") {
                if let Ok(u) = serde_json::from_value::<UpdateConfig>(update.clone()) {
                    self.update = u;
                }
            }
            if let Some(eventlog) = obj.get("eventlog") {
                if let Ok(el) = serde_json::from_value::<EventLogConfig>(eventlog.clone()) {
                    self.eventlog = el;
                }
            }
            if let Some(cleanup) = obj.get("cleanup") {
                if let Ok(c) = serde_json::from_value::<CleanupConfig>(cleanup.clone()) {
                    self.cleanup = c;
                }
            }
            if let Some(timesync) = obj.get("timesync") {
                if let Ok(ts) = serde_json::from_value::<TimeSyncConfig>(timesync.clone()) {
                    self.timesync = ts;
                }
            }
            if let Some(agent) = obj.get("agent") {
                if let Some(b_id) = agent.get("barilgiinId").and_then(|v| v.as_str()) {
                    self.agent.barilgiin_id = b_id.to_string();
                }
                if let Some(b_ner) = agent.get("baiguullagiinNer").and_then(|v| v.as_str()) {
                    self.agent.baiguullagiin_ner = b_ner.to_string();
                }
                if let Some(bg_id) = agent.get("baiguullagiinId").and_then(|v| v.as_str()) {
                    self.agent.baiguullagiin_id = bg_id.to_string();
                }
            }
            self.apply_defaults();
        }
    }

    pub fn exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn data_dir() -> PathBuf {
        let exe = Self::exe_dir();
        if exe.join("config.toml").exists() {
            return exe;
        }
        match std::env::var("ProgramData") {
            Ok(p) if !p.is_empty() => {
                let dir = PathBuf::from(p).join("Zev Detector");
                let _ = std::fs::create_dir_all(&dir);
                dir
            }
            _ => exe,
        }
    }

    pub fn config_path() -> PathBuf {
        Self::data_dir().join("config.toml")
    }

    /// Loads config. If no config.toml exists, or if reading it encounters an error,
    /// it automatically boots using full built-in defaults without failing.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        let mut cfg = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(text) => match toml::from_str(&text) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("config.toml parse error ({e}), using default built-in configuration");
                        Self::default_builtin()
                    }
                },
                Err(e) => {
                    log::warn!("Cannot read {}: {e}, using default built-in configuration", path.display());
                    Self::default_builtin()
                }
            }
        } else {
            Self::default_builtin()
        };

        cfg.apply_defaults();
        Ok(cfg)
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_default()
    }

    /// Writes this config back to config.toml, pretty-printed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path();
        let text = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Cannot serialize config: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&path, text)
            .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::default_builtin()
    }
}

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string())
}
