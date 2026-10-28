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
    #[serde(default)]
    pub provider: String, // "hikvision" or "dahua"
    pub is_active: bool,
    pub state: String,
    pub pid: u32,
    pub exit_code: u32,
    pub api_port_5000_open: bool,
    pub netsdk_port_37777_open: bool,
    #[serde(default)]
    pub camera_port_80_open: bool,
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

/// Resolves the file path of Dahua/Hikvision service.log by picking the most recently modified candidate.
pub fn find_dahua_log_path() -> PathBuf {
    let mut candidates = Vec::new();

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
            candidates.push(p.to_path_buf());
        }
    }

    // 2. Check SCM service exe parent dirs (Dahua or Hikvision)
    for svc in &[
        "zevHikvision", "hikvision-service", "zevhikvision",
        "zevdahua", "DahuaParkingService", "ParkingService"
    ] {
        if let Ok(exe) = services::exe_path(svc) {
            if let Some(parent) = exe.parent() {
                for log_name in &["hikvision-service.log", "service.log"] {
                    let candidate = parent.join(log_name);
                    if candidate.exists() {
                        candidates.push(candidate);
                    }
                }
            }
        }
    }

    // 3. Known hardcoded paths in workspace and Program Files / ProgramData
    let hardcoded = [
        r"D:\zevtabs\hikRustExe\target\release\hikvision-service.log",
        r"D:\zevtabs\dahuarust\dahuaExeRust\target\release\service.log",
        r"C:\Program Files\zevHikvision\hikvision-service.log",
        r"C:\Program Files\zevDahuaRust\service.log",
        r"C:\Program Files\DahuaParkingService\service.log",
        r"D:\zolbayar\dahua-service\target\release\service.log",
        r"C:\ProgramData\Zev Detector\hikvision-service.log",
        r"C:\ProgramData\Zev Detector\service.log",
    ];

    for fb in hardcoded {
        let p = PathBuf::from(fb);
        if p.exists() {
            candidates.push(p);
        }
    }

    // Sort by modification timestamp descending (newest file wins)
    if !candidates.is_empty() {
        candidates.sort_by(|a, b| {
            let time_a = fs::metadata(a).and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let time_b = fs::metadata(b).and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            time_b.cmp(&time_a)
        });
        return candidates[0].clone();
    }

    // Default fallback
    PathBuf::from(r"D:\zevtabs\hikRustExe\target\release\hikvision-service.log")
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
            
            // Filename format: <HHmmss>_<plate>_<camera_ip>.jpg
            let stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let parts: Vec<&str> = stem.split('_').collect();
            let (plate, cam_ip) = if parts.len() >= 3 && parts[0].len() == 6 && parts[0].chars().all(|c| c.is_ascii_digit()) {
                (parts[1].to_string(), parts[2..].join(".").replace('_', "."))
            } else if parts.len() >= 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                (stem.to_string(), "127.0.0.1".to_string())
            };
            let provider = "hikvision".to_string();
            let rel_path = file_path.to_string_lossy().to_string();

            images.push(ParkingImageRecord {
                filename,
                plate,
                timestamp: time_str,
                camera_ip: cam_ip,
                provider,
                size_bytes,
                relative_path: rel_path,
                base64: None,
            });
        }
    }

    // 3. For all recent images, read and create base64 data URLs so any admin on any network can see them
    let mut latest_base64 = None;
    let mut latest_plate = None;
    let mut latest_time = None;

    let roots = find_image_root_dirs();
    use base64::prelude::*;

    for (idx, img) in images.iter_mut().enumerate() {
        if idx == 0 {
            latest_plate = Some(img.plate.clone());
            latest_time = Some(img.timestamp.clone());
        }

        if img.base64.is_none() {
            let cand_path = PathBuf::from(&img.relative_path);
            let path_to_read = if cand_path.exists() && cand_path.is_file() {
                Some(cand_path)
            } else {
                let mut resolved = None;
                for root in &roots {
                    let candidate = root.join(&img.filename);
                    if candidate.exists() && candidate.is_file() {
                        resolved = Some(candidate);
                        break;
                    }
                    let with_rel = root.join(&img.relative_path);
                    if with_rel.exists() && with_rel.is_file() {
                        resolved = Some(with_rel);
                        break;
                    }
                    if let Ok(entries) = fs::read_dir(root) {
                        for entry in entries.flatten() {
                            if entry.path().is_dir() {
                                let sub_cand = entry.path().join(&img.filename);
                                if sub_cand.exists() && sub_cand.is_file() {
                                    resolved = Some(sub_cand);
                                    break;
                                }
                            }
                        }
                    }
                    if resolved.is_some() {
                        break;
                    }
                }
                resolved
            };

            if let Some(p) = path_to_read {
                if let Ok(bytes) = fs::read(&p) {
                    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes.len() <= 650_000 {
                        let encoded = BASE64_STANDARD.encode(&bytes);
                        img.base64 = Some(format!("data:image/jpeg;base64,{encoded}"));
                    }
                }
            }
        }

        if idx == 0 && img.base64.is_some() {
            latest_base64 = img.base64.clone();
        }
    }

    (images, latest_base64, latest_plate, latest_time)
}

pub const ALT_SERVICE_NAME: &str = "ParkingService";
pub const ALT_DISPLAY_NAME: &str = "zevzogsoolrust";

/// Searches running processes on the system to detect interactive or console-mode parking services.
pub fn find_running_parking_process() -> Option<(u32, String, String, String)> {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let candidates: &[(&str, &str, &str)] = &[
        ("hikvision-service", "Hikvision Parking Service", "hikvision"),
        ("zevhikvision", "Hikvision Parking Service", "hikvision"),
        ("dahua-service", "Dahua Parking Service", "dahua"),
        ("dahuarust", "Dahua Parking Service", "dahua"),
        ("zevdahua", "Dahua Parking Service", "dahua"),
        ("ParkingService", "Parking Service", "parking"),
    ];

    for (pid, proc) in sys.processes() {
        let raw_name = proc.name().to_string_lossy().to_string();
        let lower = raw_name.to_lowercase();
        for (target, disp, prov) in candidates {
            let t_lower = target.to_lowercase();
            if lower == t_lower || lower == format!("{t_lower}.exe") || lower.starts_with(&t_lower) {
                return Some((pid.as_u32(), raw_name, disp.to_string(), prov.to_string()));
            }
        }
    }
    None
}

/// Inspects Dahua / Hikvision parking service status, checks ports,
/// extracts recent logs directly from live exe API or fallback log file, and saves the snapshot to disk.
pub fn get_dahua_status() -> DahuaStatusSnapshot {
    let mut active_svc_name = DAHUA_SERVICE_NAME.to_string();
    let mut active_disp_name = DAHUA_DISPLAY_NAME.to_string();
    let mut provider = "dahua".to_string();

    let mut observed = services::Observed {
        state: "NotInstalled".to_string(),
        pid: 0,
        exit_code: 0,
    };

    let scm_candidates: &[(&str, &str, &str)] = &[
        ("zevHikvision", "Hikvision Parking Service", "hikvision"),
        ("hikvision-service", "Hikvision Parking Service", "hikvision"),
        ("zevhikvision", "Hikvision Parking Service", "hikvision"),
        ("zevdahua", "Dahua Parking Service", "dahua"),
        ("DahuaParkingService", "Dahua Parking Service", "dahua"),
        ("ParkingService", "Parking Service", "parking"),
    ];

    // 1. Check running processes first (catches interactive / console mode)
    if let Some((pid, proc_name, disp, prov)) = find_running_parking_process() {
        active_svc_name = proc_name;
        active_disp_name = disp;
        provider = prov;
        observed = services::Observed {
            state: "Running (Interactive)".to_string(),
            pid,
            exit_code: 0,
        };
    }

    // 2. If no running process was found, check SCM services for running ones
    if !observed.is_running() {
        for (svc_name, disp_name, prov) in scm_candidates {
            if let Ok(obs) = services::query(svc_name) {
                if obs.is_running() {
                    active_svc_name = svc_name.to_string();
                    active_disp_name = disp_name.to_string();
                    provider = prov.to_string();
                    observed = obs;
                    break;
                }
            }
        }
    }

    // 3. If still not running, check if any SCM service is at least installed (even if stopped)
    if observed.state == "NotInstalled" {
        for (svc_name, disp_name, prov) in scm_candidates {
            if let Ok(obs) = services::query(svc_name) {
                if obs.state != "NotInstalled" {
                    active_svc_name = svc_name.to_string();
                    active_disp_name = disp_name.to_string();
                    provider = prov.to_string();
                    observed = obs;
                    break;
                }
            }
        }
    }

    let local_timeout = std::time::Duration::from_millis(150);
    let api_port_5000_open = ports::is_open_timeout("127.0.0.1", DAHUA_API_PORT, local_timeout);
    let netsdk_port_37777_open = ports::is_open_timeout("127.0.0.1", DAHUA_NETSDK_PORT, local_timeout);
    let camera_port_80_open = ports::is_open_timeout("192.168.1.64", 80, local_timeout)
        || ports::is_open_timeout("127.0.0.1", 80, local_timeout);

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

                // If live logs mention Hikvision, confirm provider
                if ring.iter().any(|r| r.message.to_lowercase().contains("hikvision") || r.raw.to_lowercase().contains("hikvision")) {
                    provider = "hikvision".to_string();
                    if !active_disp_name.contains("Hikvision") {
                        active_disp_name = "Hikvision Parking Service".to_string();
                    }
                }

                (ring, total, format!("http://127.0.0.1:{DAHUA_API_PORT}/api/recentLogs (Live Exe API)"), true, true)
            } else {
                let log_path_buf = find_dahua_log_path();
                let exists = log_path_buf.exists();
                let (logs, total) = collect_recent_logs(&log_path_buf);
                if log_path_buf.to_string_lossy().to_lowercase().contains("hikvision") {
                    provider = "hikvision".to_string();
                    if !active_disp_name.contains("Hikvision") {
                        active_disp_name = "Hikvision Parking Service".to_string();
                    }
                }
                (logs, total, log_path_buf.display().to_string(), exists, false)
            }
        } else {
            let log_path_buf = find_dahua_log_path();
            let exists = log_path_buf.exists();
            let (logs, total) = collect_recent_logs(&log_path_buf);
            if log_path_buf.to_string_lossy().to_lowercase().contains("hikvision") {
                provider = "hikvision".to_string();
                if !active_disp_name.contains("Hikvision") {
                    active_disp_name = "Hikvision Parking Service".to_string();
                }
            }
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
        provider,
        is_active,
        state: effective_state,
        pid: observed.pid,
        exit_code: observed.exit_code,
        api_port_5000_open,
        netsdk_port_37777_open,
        camera_port_80_open,
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
        "# Parking Service Recent 10 Records ({}) (Updated: {})\n# Status: {} | PID: {}\n\n",
        snapshot.display_name, snapshot.updated_at, snapshot.state, snapshot.pid
    ));
    for rec in &snapshot.recent_logs {
        text_content.push_str(&format!(
            "[{}] [{}] {}\n",
            rec.timestamp, rec.level, rec.message
        ));
    }
    let _ = fs::write(text_file, text_content);
}

fn resolve_active_service_name() -> &'static str {
    for s in &["zevHikvision", "hikvision-service", "zevhikvision", "zevdahua", DAHUA_SERVICE_NAME, ALT_SERVICE_NAME] {
        if let Ok(obs) = services::query(s) {
            if obs.state != "NotInstalled" {
                return s;
            }
        }
    }
    DAHUA_SERVICE_NAME
}

/// Commands to start/stop the Dahua/Hikvision service directly via SCM.
pub fn start_dahua_service() -> anyhow::Result<()> {
    services::start(resolve_active_service_name())
}

pub fn stop_dahua_service() -> anyhow::Result<()> {
    services::stop(resolve_active_service_name())
}

pub fn restart_dahua_service() -> anyhow::Result<()> {
    let name = resolve_active_service_name();
    let _ = services::stop(name);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    services::start(name)
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
