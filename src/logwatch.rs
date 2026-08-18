//! Application log-file tailer.
//!
//! Point it at `service.log` from dahuarust (or any other project's log) and it
//! reads only what was appended since the last poll. Byte offsets are persisted
//! to `state.json` next to the exe, so a service restart resumes where it left
//! off instead of re-shipping the whole file.
//!
//! Rotation is detected by the file shrinking below the stored offset, which
//! covers both truncate-in-place and rename-and-recreate.

use crate::config::{LogFileEntry, LogsConfig};
use crate::event::{classify, DetectorEvent, Source};
use regex::Regex;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const DEFAULT_PATTERN: &str =
    r"(?i)\[(ERROR|WARN|CRITICAL|FATAL)\]|\bpanicked\b|\bpanic:|unhandled exception|\btraceback\b|\bfatal\b|\bAldaa\b";

/// Never emit more than this many events from one file in one poll — a log that
/// suddenly writes 100k error lines should produce a readable summary, not a
/// self-inflicted DoS on the dashboard.
const MAX_PER_FILE: usize = 100;
/// Hard cap on bytes read per file per poll (2 MB).
const MAX_READ: u64 = 2 * 1024 * 1024;

pub struct LogCollector {
    cfg: LogsConfig,
    offsets: HashMap<PathBuf, u64>,
    patterns: Vec<(Option<Regex>, Option<Regex>)>,
    state_path: PathBuf,
    dirty: bool,
}

impl LogCollector {
    pub fn new(cfg: LogsConfig, state_dir: &Path) -> Self {
        let patterns = cfg
            .files
            .iter()
            .map(|f| {
                let pat = if f.pattern.trim().is_empty() {
                    DEFAULT_PATTERN
                } else {
                    f.pattern.as_str()
                };
                let compiled = Regex::new(pat)
                    .map_err(|e| log::error!("Bad pattern for {}: {e}", f.path))
                    .ok();
                let ignore = if f.ignore_pattern.trim().is_empty() {
                    None
                } else {
                    Regex::new(&f.ignore_pattern)
                        .map_err(|e| log::error!("Bad ignore_pattern for {}: {e}", f.path))
                        .ok()
                };
                (compiled, ignore)
            })
            .collect();

        let state_path = state_dir.join("state.json");
        let offsets = load_state(&state_path);

        Self { cfg, offsets, patterns, state_path, dirty: false }
    }

    pub fn poll(&mut self) -> Vec<DetectorEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();

        for (idx, entry) in self.cfg.files.clone().iter().enumerate() {
            for path in resolve_paths(entry) {
                match self.read_new(&path, entry, idx) {
                    Ok(mut events) => out.append(&mut events),
                    Err(e) => log::debug!("log watch {}: {e}", path.display()),
                }
            }
        }

        if self.dirty {
            save_state(&self.state_path, &self.offsets);
            self.dirty = false;
        }
        out
    }

    fn read_new(
        &mut self,
        path: &Path,
        entry: &LogFileEntry,
        idx: usize,
    ) -> anyhow::Result<Vec<DetectorEvent>> {
        let meta = std::fs::metadata(path)?;
        let size = meta.len();
        let known = self.offsets.get(path).copied();

        let start = match known {
            // First time we see this file: start at the end. Ingesting a 200 MB
            // historical log on install is never what anyone wants.
            None => {
                self.offsets.insert(path.to_path_buf(), size);
                self.dirty = true;
                return Ok(Vec::new());
            }
            Some(prev) if prev > size => {
                log::info!("Log rotated, restarting from 0: {}", path.display());
                0
            }
            Some(prev) => prev,
        };

        if size <= start {
            return Ok(Vec::new());
        }

        let mut file = std::fs::File::open(path)?;
        let read_from = if size - start > MAX_READ {
            log::warn!(
                "{} grew {} bytes since last poll, reading the last {MAX_READ}",
                path.display(),
                size - start
            );
            size - MAX_READ
        } else {
            start
        };
        file.seek(SeekFrom::Start(read_from))?;

        let mut reader = BufReader::new(file);
        let mut lines: Vec<String> = Vec::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            lines.push(String::from_utf8_lossy(&buf).trim_end().to_string());
        }

        self.offsets.insert(path.to_path_buf(), size);
        self.dirty = true;

        let (pattern, ignore) = self.patterns.get(idx).cloned().unwrap_or((None, None));
        let Some(pattern) = pattern else {
            return Ok(Vec::new());
        };

        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "log".to_string());
        let system = if entry.system.is_empty() {
            "app".to_string()
        } else {
            entry.system.clone()
        };
        let modul = if entry.modul.is_empty() { file_name.clone() } else { entry.modul.clone() };

        let mut out = Vec::new();
        let mut skipped = 0usize;

        for (i, line) in lines.iter().enumerate() {
            if line.trim().is_empty() || !pattern.is_match(line) {
                continue;
            }
            if ignore.as_ref().map(|r| r.is_match(line)).unwrap_or(false) {
                continue;
            }
            if out.len() >= MAX_PER_FILE {
                skipped += 1;
                continue;
            }

            // Following lines that are clearly continuation (indented, or a
            // Rust/Java stack frame) become the stack trace.
            let mut context = Vec::new();
            for follow in lines.iter().skip(i + 1).take(entry.context_lines) {
                let t = follow.trim_start();
                let is_continuation = follow.starts_with(' ')
                    || follow.starts_with('\t')
                    || t.starts_with("at ")
                    || t.starts_with("Caused by")
                    || t.starts_with("note:")
                    || t.starts_with("stack backtrace");
                if !is_continuation {
                    break;
                }
                context.push(follow.clone());
            }

            let severity = classify(line);
            let ev = DetectorEvent::new(
                Source::LogFile,
                severity,
                system.clone(),
                modul.clone(),
                strip_timestamp(line),
            )
            .kod(format!("log/{file_name}"))
            .stack(context.join("\n"))
            .meta("file", path.to_string_lossy().to_string())
            .meta("rawLine", crate::event::truncate(line, 2000))
            .refingerprint();
            out.push(ev);
        }

        if skipped > 0 {
            out.push(
                DetectorEvent::new(
                    Source::LogFile,
                    crate::event::Severity::Warning,
                    system,
                    modul,
                    format!(
                        "{file_name}: нэг удаагийн уншилтад {} нэмэлт алдааны мөр таарсныг хэтэрсэн тул алгасав",
                        skipped
                    ),
                )
                .kod(format!("log/{file_name}/overflow"))
                .meta("skipped", skipped)
                .refingerprint(),
            );
        }

        Ok(out)
    }
}

/// Removes a leading `[2026-08-17 09:12:33] [ERROR]`-style stamp so the same
/// error at two different times shares one fingerprint.
fn strip_timestamp(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let inside = &rest[..end];
            let digits = inside.chars().filter(|c| c.is_ascii_digit()).count();
            if digits >= 8 {
                return rest[end + 1..].trim_start().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Expands one config entry into concrete files: either the literal path, or
/// every file in a directory matching a `*`/`?` pattern.
fn resolve_paths(entry: &LogFileEntry) -> Vec<PathBuf> {
    let base = PathBuf::from(&entry.path);

    if entry.glob.trim().is_empty() {
        return if base.is_file() { vec![base] } else { Vec::new() };
    }

    let dir = if base.is_dir() { base } else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut found: Vec<PathBuf> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| wildcard_match(&entry.glob, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// `*` and `?` only — enough for "*.log" / "service-?.log" without pulling in a
/// glob crate for two metacharacters.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);

    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn load_state(path: &Path) -> HashMap<PathBuf, u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let parsed: HashMap<String, u64> = serde_json::from_str(&text).unwrap_or_default();
    parsed.into_iter().map(|(k, v)| (PathBuf::from(k), v)).collect()
}

fn save_state(path: &Path, offsets: &HashMap<PathBuf, u64>) {
    let plain: HashMap<String, u64> = offsets
        .iter()
        .map(|(k, v)| (k.to_string_lossy().to_string(), *v))
        .collect();
    if let Ok(text) = serde_json::to_string_pretty(&plain) {
        let _ = std::fs::write(path, text);
    }
}
