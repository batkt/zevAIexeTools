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
pub const HIK_SERVICE_NAME: &str = "zevHikvision";
pub const HIK_DISPLAY_NAME: &str = "zevHikvision";
pub const DAHUA_API_PORT: u16 = 5000;
pub const DAHUA_NETSDK_PORT: u16 = 37777;
pub const MAX_RING_BUFFER_RECORDS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DahuaLogRecord {
    pub id: usize,
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParkingImageRecord {
    pub filename: String,
    pub plate: String,
    pub timestamp: String,
    pub camera_ip: String,
    pub provider: String,
    pub size_bytes: usize,
    pub relative_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_images: Vec<ParkingImageRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_image_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_image_plate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_image_time: Option<String>,
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
        if (name.eq_ignore_ascii_case("service.log") 
            || name.eq_ignore_ascii_case("hikvision-service.log")
            || entry.path.to_lowercase().contains("dahua")
            || entry.path.to_lowercase().contains("hikvision"))
            && p.exists()
        {
            return p.to_path_buf();
        }
    }

    // 2. Check SCM service exe path (Dahua or Hikvision)
    for svc in &[DAHUA_SERVICE_NAME, HIK_SERVICE_NAME, ALT_SERVICE_NAME] {
        if let Ok(exe) = services::exe_path(svc) {
            if let Some(parent) = exe.parent() {
                for log_name in &["service.log", "hikvision-service.log"] {
                    let candidate = parent.join(log_name);
                    if candidate.exists() {
                        return candidate;
                    }
                }
            }
        }
    }

    // 3. Known hardcoded path in workspace
    let primary = PathBuf::from(r"D:\zevtabs\dahuarust\dahuaExeRust\target\release\service.log");
    if primary.exists() {
        return primary;
    }
    let hik_primary = PathBuf::from(r"D:\zevtabs\hikRustExe\target\release\hikvision-service.log");
    if hik_primary.exists() {
        return hik_primary;
    }

    // 4. Secondary fallback candidates
    let fallbacks = [
        r"C:\Program Files\zevHikvision\hikvision-service.log",
        r"C:\Program Files\zevDahuaRust\service.log",
        r"C:\Program Files\DahuaParkingService\service.log",
        r"D:\zolbayar\dahua-service\target\release\service.log",
        r"C:\ProgramData\Zev Detector\service.log",
        r"C:\ProgramData\Zev Detector\hikvision-service.log",
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

/// Queries the live in-memory log buffer from dahua-service via localhost HTTP API.
pub fn fetch_live_api_logs(port: u16) -> Option<Vec<DahuaLogRecord>> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(250),
    ).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(250))).ok()?;

    let req = format!(
        "GET /api/recentLogs HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;

    let resp_str = String::from_utf8_lossy(&response);
    if let Some(body_start) = resp_str.find("\r\n\r\n") {
        let body = &resp_str[body_start + 4..];
        if let Ok(records) = serde_json::from_str::<Vec<DahuaLogRecord>>(body) {
            return Some(records);
        }
    }
    None
}

/// Queries the live recent captures from parking service via localhost HTTP API.
pub fn fetch_live_api_captures(port: u16) -> Option<Vec<ParkingImageRecord>> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(250),
    ).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(250))).ok()?;

    let req = format!(
        "GET /api/recentCaptures HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;

    let resp_str = String::from_utf8_lossy(&response);
    if let Some(body_start) = resp_str.find("\r\n\r\n") {
        let body = &resp_str[body_start + 4..];
        if let Ok(records) = serde_json::from_str::<Vec<ParkingImageRecord>>(body) {
            return Some(records);
        }
    }
    None
}

/// Returns known directories where Dahua/Hikvision cameras store captured car images.
pub fn find_image_root_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let candidates = [
        r"D:\zevtabs\parking_images",
        r"C:\ProgramData\parking_images",
        r"D:\zevtabs\dahuarust\dahuaExeRust\parking_images",
        r"D:\zevtabs\hikRustExe\parking_images",
        r"parking_images",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() && p.is_dir() {
            dirs.push(p);
        }
    }
    dirs
}

fn is_image_file(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    ext == "jpg" || ext == "jpeg" || ext == "png"
}

/// Scans parking image files on disk or via API, and returns (recent_images, latest_base64, latest_plate, latest_time).
pub fn collect_recent_images() -> (Vec<ParkingImageRecord>, Option<String>, Option<String>, Option<String>) {
    // 1. Check if live port 5000 API provides recent captures
    let mut images = fetch_live_api_captures(DAHUA_API_PORT).unwrap_or_default();

    // 2. If API was empty, scan disk
    if images.is_empty() {
        let roots = find_image_root_dirs();
        let mut found_files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        for root in roots {
            if let Ok(entries) = fs::read_dir(&root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Ok(sub_entries) = fs::read_dir(&path) {
                            for sub in sub_entries.flatten() {
                                let sub_path = sub.path();
                                if is_image_file(&sub_path) {
                                    if let Ok(meta) = sub.metadata() {
                                        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                        found_files.push((sub_path, mtime));
                                    }
                                }
                            }
                        }
                    } else if is_image_file(&path) {
                        if let Ok(meta) = entry.metadata() {
                            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            found_files.push((path, mtime));
                        }
                    }
                }
            }
        }

        found_files.sort_by(|a, b| b.1.cmp(&a.1));
        found_files.truncate(10);

        for (file_path, mtime) in found_files {
            let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let size_bytes = fs::metadata(&file_path).map(|m| m.len() as usize).unwrap_or(0);
            let time_str = chrono::DateTime::<Local>::from(mtime).format("%Y-%m-%d %H:%M:%S").to_string();
            
            // Format: <plate>_<timestamp>_<provider>.jpg
            let stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let parts: Vec<&str> = stem.split('_').collect();
            let plate = parts.get(0).copied().unwrap_or("Машин").to_string();
            let provider = parts.get(2).copied().unwrap_or("camera").to_string();
            let rel_path = file_path.to_string_lossy().to_string();

            images.push(ParkingImageRecord {
                filename,
                plate,
                timestamp: time_str,
                camera_ip: "127.0.0.1".to_string(),
                provider,
                size_bytes,
                relative_path: rel_path,
                base64: None,
            });
        }
    }

    // 3. For the latest image, read and create base64 thumbnail
    let mut latest_base64 = None;
    let mut latest_plate = None;
    let mut latest_time = None;

    if let Some(first) = images.first() {
        latest_plate = Some(first.plate.clone());
        latest_time = Some(first.timestamp.clone());

        let cand_path = PathBuf::from(&first.relative_path);
        let path_to_read = if cand_path.exists() {
            Some(cand_path)
        } else {
            let mut resolved = None;
            for root in find_image_root_dirs() {
                let candidate = root.join(&first.filename);
                if candidate.exists() {
                    resolved = Some(candidate);
                    break;
                }
                let with_rel = root.join(&first.relative_path);
                if with_rel.exists() {
                    resolved = Some(with_rel);
                    break;
                }
            }
            resolved
        };

        if let Some(p) = path_to_read {
            if let Ok(bytes) = fs::read(&p) {
                // If it's valid JPEG SOI (0xFF, 0xD8) and size <= 350KB
                if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes.len() <= 350_000 {
                    use base64::prelude::*;
                    let encoded = BASE64_STANDARD.encode(&bytes);
                    latest_base64 = Some(format!("data:image/jpeg;base64,{encoded}"));
                }
            }
        }
    }

    (images, latest_base64, latest_plate, latest_time)
}

pub const ALT_SERVICE_NAME: &str = "ParkingService";
pub const ALT_DISPLAY_NAME: &str = "zevzogsoolrust";

/// Inspects DahuaParkingService / ParkingService status, checks ports 5000 and 37777,
/// extracts recent logs directly from live exe API or fallback log file, and saves the snapshot to disk.
pub fn get_dahua_status() -> DahuaStatusSnapshot {
    let mut active_svc_name = DAHUA_SERVICE_NAME.to_string();
    let mut active_disp_name = DAHUA_DISPLAY_NAME.to_string();

    // 1. Try DahuaParkingService first
    let mut observed = services::query(DAHUA_SERVICE_NAME).unwrap_or(services::Observed {
        state: "NotInstalled".to_string(),
        pid: 0,
        exit_code: 0,
    });

    // 2. If not running, check zevHikvision
    if !observed.is_running() {
        if let Ok(hik_obs) = services::query(HIK_SERVICE_NAME) {
            if hik_obs.is_running() {
                active_svc_name = HIK_SERVICE_NAME.to_string();
                active_disp_name = HIK_DISPLAY_NAME.to_string();
                observed = hik_obs;
            }
        }
    }

    // 3. If still not running, check ParkingService
    if !observed.is_running() {
        if let Ok(alt_obs) = services::query(ALT_SERVICE_NAME) {
            if alt_obs.is_running() {
                active_svc_name = ALT_SERVICE_NAME.to_string();
                active_disp_name = ALT_DISPLAY_NAME.to_string();
                observed = alt_obs;
            }
        }
    }

    let local_timeout = std::time::Duration::from_millis(150);
    let api_port_5000_open = ports::is_open_timeout("127.0.0.1", DAHUA_API_PORT, local_timeout);
    let netsdk_port_37777_open = ports::is_open_timeout("127.0.0.1", DAHUA_NETSDK_PORT, local_timeout);

    // Pull directly from live exe API if Port 5000 is open
    let (recent_logs, total_records, log_path_display, log_exists, live_api_active) =
        if api_port_5000_open {
            if let Some(api_logs) = fetch_live_api_logs(DAHUA_API_PORT) {
                let total = api_logs.len();
                let ring: Vec<DahuaLogRecord> = api_logs
                    .into_iter()
                    .rev()
                    .take(MAX_RING_BUFFER_RECORDS)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                (ring, total, format!("http://127.0.0.1:{DAHUA_API_PORT}/api/recentLogs (Live Exe API)"), true, true)
            } else {
                let log_path_buf = find_dahua_log_path();
                let exists = log_path_buf.exists();
                let (logs, total) = collect_recent_logs(&log_path_buf);
                (logs, total, log_path_buf.display().to_string(), exists, false)
            }
        } else {
            let log_path_buf = find_dahua_log_path();
            let exists = log_path_buf.exists();
            let (logs, total) = collect_recent_logs(&log_path_buf);
            (logs, total, log_path_buf.display().to_string(), exists, false)
        };

    // If SCM says stopped or not installed, but Port 5000 is listening, it is active in standalone/console mode
    let is_active = observed.is_running() || api_port_5000_open;
    let effective_state = if observed.is_running() {
        if live_api_active {
            format!("{} (Live API Connected)", observed.state)
        } else {
            observed.state
        }
    } else if api_port_5000_open {
        if live_api_active {
            "Running (Live Exe API Connected)".to_string()
        } else {
            "Running (Port 5000 Active)".to_string()
        }
    } else {
        observed.state
    };

    let (recent_images, latest_image_base64, latest_image_plate, latest_image_time) = collect_recent_images();

    let snapshot = DahuaStatusSnapshot {
        service_name: active_svc_name,
        display_name: active_disp_name,
        is_active,
        state: effective_state,
        pid: observed.pid,
        exit_code: observed.exit_code,
        api_port_5000_open,
        netsdk_port_37777_open,
        log_path: log_path_display,
        log_exists,
        total_records,
        recent_logs,
        recent_images,
        latest_image_base64,
        latest_image_plate,
        latest_image_time,
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

    #[test]
    fn test_fetch_live_api_logs_returns_none_when_unreachable() {
        let res = fetch_live_api_logs(59999);
        assert!(res.is_none());
    }
}
