//! config.toml loading. Every section has a `#[serde(default)]` so a partial
//! config file still boots — a missing `[health]` block just means health
//! probing is off, not a startup failure.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
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
}

// ─── server ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    /// Base URL of udirdlagaBack, e.g. https://admin.zevtabs.mn/api
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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServiceEntry {
    /// Service key name as SCM knows it, e.g. "DahuaParkingService"
    pub name: String,
    /// Friendly label used in the dashboard. Defaults to `name`.
    #[serde(default)]
    pub label: String,
    /// Report a Critical when the service is not Running.
    #[serde(default = "d_true")]
    pub must_run: bool,
}

impl Default for ServiceEntry {
    fn default() -> Self {
        Self { name: String::new(), label: String::new(), must_run: true }
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
}

fn d_probe_timeout() -> u64 { 5 }

impl Default for HttpProbe {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            expect: String::new(),
            timeout_secs: d_probe_timeout(),
            system: String::new(),
        }
    }
}

// ─── loading ─────────────────────────────────────────────────────────────────

impl Config {
    pub fn exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Where config.toml, the queue, the read offsets and the log live.
    ///
    /// Two layouts are supported, and which one applies is decided by whether
    /// a config.toml already sits next to the exe:
    ///
    /// * **Portable** — a folder holding the exes and a config.toml, used by
    ///   `cargo run` and by anyone who just copies the files onto a machine.
    ///   Everything stays in that one folder.
    /// * **Installed** — the installer puts the exes in Program Files, which
    ///   a standard user cannot write to. The desktop app runs unelevated by
    ///   design, so it would fail to save; state goes to ProgramData instead,
    ///   which the installer grants Users modify rights on and which the
    ///   LocalSystem service can equally read.
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

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("config.toml parse error: {e}"))?;

        if cfg.server.url.is_empty() {
            anyhow::bail!("config.toml: [server].url is required");
        }
        while cfg.server.url.ends_with('/') {
            cfg.server.url.pop();
        }
        if cfg.agent.name.is_empty() {
            cfg.agent.name = hostname();
        }
        for s in &mut cfg.services {
            if s.label.is_empty() {
                s.label = s.name.clone();
            }
        }
        Ok(cfg)
    }

    /// Like `load()`, but never fails — used by the GUI, which needs
    /// something to show even before config.toml exists or while it is
    /// mid-edit and temporarily invalid.
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

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string())
}
