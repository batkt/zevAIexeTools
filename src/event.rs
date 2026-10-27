//! The one shape every collector produces and the sender ships.
//!
//! Field names mirror the existing `aldaa` collection so the Detector page and
//! the Алдаа баригч page stay readable side by side, with `turul` /
//! `khereglegch`-style extras added for the machine-agent case.

use serde::{Deserialize, Serialize};

/// Where an event came from. Kept as a string in the payload so the backend
/// never has to be redeployed to accept a new collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    EventLog,
    Service,
    LogFile,
    Health,
    Detector,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::EventLog => "eventlog",
            Source::Service => "service",
            Source::LogFile => "log",
            Source::Health => "health",
            Source::Detector => "detector",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::Warning => "Warning",
            Severity::Info => "Info",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorEvent {
    /// "eventlog" | "service" | "log" | "health" | "detector"
    pub turul: String,
    /// Which project the event belongs to: "windows", "dahuarust", "worker"…
    pub system: String,
    /// Sub-component: service name, log file name, probe name…
    pub modul: String,
    #[serde(rename = "aldaaniiMsg")]
    pub aldaanii_msg: String,
    #[serde(rename = "aldaaniiKod")]
    pub aldaanii_kod: String,
    #[serde(rename = "aldaaniiTuvshin")]
    pub aldaanii_tuvshin: String,
    #[serde(rename = "stackTrace", skip_serializing_if = "String::is_empty")]
    pub stack_trace: String,
    /// RFC3339, local time with offset.
    pub ognoo: String,
    /// Stable hash of the normalized message — the backend groups on this so
    /// one flapping camera does not become 4 000 rows.
    pub fingerprint: String,
    pub davtalt: u32,
    /// Anything collector-specific: event id, provider, exit code, cpu%…
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub nemelt: serde_json::Map<String, serde_json::Value>,
}

impl DetectorEvent {
    pub fn new(
        source: Source,
        severity: Severity,
        system: impl Into<String>,
        modul: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        let system = system.into();
        let modul = modul.into();
        let msg: String = msg.into();
        let fingerprint = fingerprint(&[source.as_str(), &system, &modul, &normalize(&msg)]);

        Self {
            turul: source.as_str().to_string(),
            system,
            modul,
            aldaanii_msg: truncate(&msg, 4000),
            aldaanii_kod: String::new(),
            aldaanii_tuvshin: severity.as_str().to_string(),
            stack_trace: String::new(),
            ognoo: chrono::Local::now().to_rfc3339(),
            fingerprint,
            davtalt: 1,
            nemelt: serde_json::Map::new(),
        }
    }

    pub fn kod(mut self, kod: impl Into<String>) -> Self {
        self.aldaanii_kod = kod.into();
        self
    }

    pub fn stack(mut self, stack: impl Into<String>) -> Self {
        self.stack_trace = truncate(&stack.into(), 8000);
        self
    }

    pub fn at(mut self, ognoo: impl Into<String>) -> Self {
        let v = ognoo.into();
        if !v.is_empty() {
            self.ognoo = v;
        }
        self
    }

    pub fn meta(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.nemelt.insert(key.to_string(), value.into());
        self
    }

    /// Re-derive the fingerprint after the event id / code has been attached, so
    /// two different Event IDs with a similar message stay separate rows.
    pub fn refingerprint(mut self) -> Self {
        self.fingerprint = fingerprint(&[
            &self.turul,
            &self.system,
            &self.modul,
            &self.aldaanii_kod,
            &normalize(&self.aldaanii_msg),
        ]);
        self
    }
}

/// Strip the parts of a message that change every occurrence (timestamps, ids,
/// ip:port pairs, hex handles, paths with digits) so repeats collapse onto one
/// fingerprint instead of flooding the dashboard.
pub fn normalize(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut in_num = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !in_num {
                out.push('#');
                in_num = true;
            }
            continue;
        }
        in_num = false;
        // Collapse whitespace runs.
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        out.push(c.to_ascii_lowercase());
    }
    truncate(out.trim(), 500)
}

/// FNV-1a 64. Not cryptographic — it only has to be stable across restarts and
/// across machines, which a hash of the normalized text gives us for free.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hash ^= 0x1f;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for b in part.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Keyword classifier used by the log-file and event-log collectors when the
/// source does not hand us a level of its own.
pub fn classify(msg: &str) -> Severity {
    let l = msg.to_lowercase();
    const CRITICAL: [&str; 14] = [
        "critical",
        "fatal",
        "panic",
        "unhandled exception",
        "stack overflow",
        "access violation",
        "500 internal",
        "database connection",
        "connection refused",
        "out of memory",
        "disk full",
        "terminated unexpectedly",
        "failed to start",
        "crash",
    ];
    const WARNING: [&str; 10] = [
        "warn",
        "warning",
        "timeout",
        "timed out",
        "retry",
        "retrying",
        "slow query",
        "failed to load",
        "unreachable",
        "reconnect",
    ];

    if CRITICAL.iter().any(|k| l.contains(k)) {
        return Severity::Critical;
    }
    if l.contains("error") || l.contains("[err") || l.contains("aldaa") {
        return Severity::Critical;
    }
    if WARNING.iter().any(|k| l.contains(k)) {
        return Severity::Warning;
    }
    Severity::Info
}
