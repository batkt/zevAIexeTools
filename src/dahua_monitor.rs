//! Dahua Parking Service (`dahuarust`) live status monitor & 10-record rolling log ring buffer.
//!
//! Enforces:
//! 1. Live SCM service status (Active / Inactive, PID, exit code).
//! 2. Port probes: Port 5000 (API server) and Port 37777 (NetSDK camera).
//! 3. Strictly keeps ONLY the 10 most recent log records in a FIFO ring buffer,
//!    replacing oldest records as new ones arrive.
//! 4. Saves rolling snapshots to `dahua_status.json` and `dahua_recent_10.log`.

use crate::config::Config;
use crate::ports;
use crate::services;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

pub const DAHUA_SERVICE_NAME: &str = "DahuaParkingService";
pub const DAHUA_DISPLAY_NAME: &str = "zevDahuaRust";
pub const DAHUA_API_PORT: u16 = 5000;
pub const DAHUA_NETSDK_PORT: u16 = 37777;
pub const MAX_RING_BUFFER_RECORDS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DahuaLogRecord {
    pub id: usize,
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DahuaStatusSnapshot {
    pub service_name: String,
    pub display_name: String,
    pub is_active: bool,
    pub state: String,
    pub pid: u32,
    pub exit_code: u32,
    pub api_port_5000_open: bool,
    pub netsdk_port_37777_open: bool,
    pub log_path: String,
    pub log_exists: bool,
    pub total_records: usize,
    pub recent_logs: Vec<DahuaLogRecord>, // strictly <= 10 records
    pub updated_at: String,
}

/// Parses a single Dahua log line into a structured record.
/// Expected formats:
///   `[2026-04-08 10:38:09] [INFO] === Dahua Parking Service starting ===`
///   `[2026-04-08 10:38:25] [WARN] Холболт амжилтгүй 192.168.1.91 attempt 2: error 0x8000006b`
pub fn parse_log_line(line: &str, id: usize) -> DahuaLogRecord {
    let trimmed = line.trim();

    if trimmed.starts_with('[') {
        if let Some(first_end) = trimmed.find(']') {
            let ts = &trimmed[1..first_end];
            let rest = trimmed[first_end + 1..].trim_start();
            if rest.starts_with('[') {
                if let Some(second_end) = rest.find(']') {
                    let lvl = &rest[1..second_end];
                    let msg = rest[second_end + 1..].trim_start();
                    return DahuaLogRecord {
                        id,
                        timestamp: ts.to_string(),
                        level: lvl.to_string(),
                        message: msg.to_string(),
                        raw: trimmed.to_string(),
                    };
                }
            }
        }
    }

    // Fallback if not matching the standard Dahua format
    DahuaLogRecord {
        id,
        timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        level: "INFO".to_string(),
        message: trimmed.to_string(),
        raw: trimmed.to_string(),
    }
}

/// Resolves the file path of Dahua's service.log.
pub fn find_dahua_log_path() -> PathBuf {
    // 1. Check config.toml logs entries
    let cfg = Config::load_or_default();
    for entry in &cfg.logs.files {
        let p = Path::new(&entry.path);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if (name.eq_ignore_ascii_case("service.log") || entry.path.to_lowercase().contains("dahua"))
            && p.exists()
        {
            return p.to_path_buf();
        }
    }

    // 2. Check SCM service exe path
    if let Ok(exe) = services::exe_path(DAHUA_SERVICE_NAME) {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("service.log");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    // 3. Known hardcoded path in workspace
    let primary = PathBuf::from(r"D:\zevtabs\dahuarust\dahuaExeRust\target\release\service.log");
    if primary.exists() {
        return primary;
    }

    // 4. Secondary fallback candidates
    let fallbacks = [
        r"C:\Program Files\zevDahuaRust\service.log",
        r"C:\Program Files\DahuaParkingService\service.log",
        r"D:\zolbayar\dahua-service\target\release\service.log",
        r"C:\ProgramData\Zev Detector\service.log",
    ];

    for fb in fallbacks {
        let p = PathBuf::from(fb);
        if p.exists() {
            return p;
        }
    }

    // Default to the primary path even if it doesn't exist yet
    primary
}

/// Collects recent log records, maintaining strictly a 10-item FIFO ring buffer.
/// When new records arrive, the oldest records are popped from the front.
pub fn collect_recent_logs(path: &Path) -> (Vec<DahuaLogRecord>, usize) {
    if !path.exists() {
        return (Vec::new(), 0);
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), 0),
    };

    let all_lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    let total = all_lines.len();

    // Use a ring buffer of max 10 items
    let mut ring_buffer: VecDeque<DahuaLogRecord> = VecDeque::with_capacity(MAX_RING_BUFFER_RECORDS);

    // If the file has many lines, only scan the tail to avoid wasting CPU
    let start_idx = total.saturating_sub(100);

    for (offset, line) in all_lines[start_idx..].iter().enumerate() {
        let rec = parse_log_line(line, start_idx + offset + 1);

        if ring_buffer.len() >= MAX_RING_BUFFER_RECORDS {
            ring_buffer.pop_front(); // drop the oldest record
        }
        ring_buffer.push_back(rec); // append newest
    }

    (ring_buffer.into_iter().collect(), total)
}

pub const ALT_SERVICE_NAME: &str = "ParkingService";
pub const ALT_DISPLAY_NAME: &str = "zevzogsoolrust";

/// Inspects DahuaParkingService / ParkingService status, checks ports 5000 and 37777,
/// extracts the 10 most recent logs, and saves the snapshot to disk.
pub fn get_dahua_status() -> DahuaStatusSnapshot {
    let mut active_svc_name = DAHUA_SERVICE_NAME.to_string();
    let mut active_disp_name = DAHUA_DISPLAY_NAME.to_string();

    // 1. Try DahuaParkingService first
    let mut observed = services::query(DAHUA_SERVICE_NAME).unwrap_or(services::Observed {
        state: "NotInstalled".to_string(),
        pid: 0,
        exit_code: 0,
    });

    // 2. If not running, check ParkingService
    if !observed.is_running() {
        if let Ok(alt_obs) = services::query(ALT_SERVICE_NAME) {
            if alt_obs.is_running() {
                active_svc_name = ALT_SERVICE_NAME.to_string();
                active_disp_name = ALT_DISPLAY_NAME.to_string();
                observed = alt_obs;
            }
        }
    }

    let api_port_5000_open = ports::is_open("127.0.0.1", DAHUA_API_PORT);
    let netsdk_port_37777_open = ports::is_open("127.0.0.1", DAHUA_NETSDK_PORT);

    // If SCM says stopped or not installed, but Port 5000 is listening, it is active in standalone/console mode
    let is_active = observed.is_running() || api_port_5000_open;
    let effective_state = if observed.is_running() {
        observed.state
    } else if api_port_5000_open {
        "Running (Port 5000 Active)".to_string()
    } else {
        observed.state
    };

    let log_path_buf = find_dahua_log_path();
    let log_exists = log_path_buf.exists();
    let (recent_logs, total_records) = collect_recent_logs(&log_path_buf);

    let snapshot = DahuaStatusSnapshot {
        service_name: active_svc_name,
        display_name: active_disp_name,
        is_active,
        state: effective_state,
        pid: observed.pid,
        exit_code: observed.exit_code,
        api_port_5000_open,
        netsdk_port_37777_open,
        log_path: log_path_buf.display().to_string(),
        log_exists,
        total_records,
        recent_logs,
        updated_at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };

    // Save snapshot files to ProgramData / data_dir
    save_snapshot_to_disk(&snapshot);

    snapshot
}

/// Persists the 10-record rolling snapshot to disk.
fn save_snapshot_to_disk(snapshot: &DahuaStatusSnapshot) {
    let data_dir = Config::data_dir();
    let _ = fs::create_dir_all(&data_dir);

    // 1. Save JSON snapshot
    let json_file = data_dir.join("dahua_status.json");
    if let Ok(serialized) = serde_json::to_string_pretty(snapshot) {
        let _ = fs::write(json_file, serialized);
    }

    // 2. Save text log of strictly the latest 10 records
    let text_file = data_dir.join("dahua_recent_10.log");
    let mut text_content = String::new();
    text_content.push_str(&format!(
        "# Dahua Parking Service Recent 10 Records (Updated: {})\n# Status: {} | PID: {}\n\n",
        snapshot.updated_at, snapshot.state, snapshot.pid
    ));
    for rec in &snapshot.recent_logs {
        text_content.push_str(&format!(
            "[{}] [{}] {}\n",
            rec.timestamp, rec.level, rec.message
        ));
    }
    let _ = fs::write(text_file, text_content);
}

/// Commands to start/stop the Dahua service directly via SCM.
pub fn start_dahua_service() -> anyhow::Result<()> {
    services::start(DAHUA_SERVICE_NAME)
}

pub fn stop_dahua_service() -> anyhow::Result<()> {
    services::stop(DAHUA_SERVICE_NAME)
}

pub fn restart_dahua_service() -> anyhow::Result<()> {
    let _ = services::stop(DAHUA_SERVICE_NAME);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    services::start(DAHUA_SERVICE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dahua_standard_log_line() {
        let line = "[2026-04-08 10:38:09] [INFO] API server listening on 0.0.0.0:5000";
        let rec = parse_log_line(line, 1);
        assert_eq!(rec.id, 1);
        assert_eq!(rec.timestamp, "2026-04-08 10:38:09");
        assert_eq!(rec.level, "INFO");
        assert_eq!(rec.message, "API server listening on 0.0.0.0:5000");
    }

    #[test]
    fn test_parse_dahua_warn_log_line() {
        let line = "[2026-04-08 10:38:25] [WARN] Холболт амжилтгүй 192.168.1.91 attempt 2: error 0x8000006b";
        let rec = parse_log_line(line, 42);
        assert_eq!(rec.id, 42);
        assert_eq!(rec.timestamp, "2026-04-08 10:38:25");
        assert_eq!(rec.level, "WARN");
        assert_eq!(rec.message, "Холболт амжилтгүй 192.168.1.91 attempt 2: error 0x8000006b");
    }

    #[test]
    fn test_fifo_ring_buffer_exact_ten_records() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let test_log = temp_dir.join("test_dahua_service.log");

        let mut f = fs::File::create(&test_log).unwrap();
        // Write 25 records
        for i in 1..=25 {
            writeln!(f, "[2026-04-08 10:38:{:02}] [INFO] Record number {}", i, i).unwrap();
        }
        f.flush().unwrap();

        let (logs, total) = collect_recent_logs(&test_log);
        assert_eq!(total, 25);
        assert_eq!(logs.len(), 10, "Ring buffer must strictly hold 10 records");
        // Oldest records 1..15 should have been popped; logs should be 16..25
        assert_eq!(logs.first().unwrap().message, "Record number 16");
        assert_eq!(logs.last().unwrap().message, "Record number 25");

        let _ = fs::remove_file(test_log);
    }
}
