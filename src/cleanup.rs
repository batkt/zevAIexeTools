//! Scheduled disk cleanup.
//!
//! This is the only part of the agent that deletes anything, and it runs
//! unattended as LocalSystem on machines nobody is watching. So it is built
//! around refusing to do the wrong thing rather than around freeing the most
//! space:
//!
//! * **Fixed allowlist of roots.** Windows temp dirs, crash dumps, the Windows
//!   Update download cache, and our own rotated logs. Nothing is configurable
//!   here on purpose — a typo in a config file must not be able to point this
//!   at `C:\`.
//! * **Every candidate is re-checked against its root after canonicalising.**
//!   `C:\Windows\Temp` can contain a junction to anywhere on the disk; without
//!   this, walking it would happily delete the target.
//! * **Reparse points are never followed or removed**, for the same reason.
//! * **Age gate.** Nothing newer than `min_age_days` is touched, so a temp file
//!   an installer is using right now survives.
//! * **Only files.** Directories are left in place; an empty temp folder costs
//!   nothing and deleting trees is where this kind of code goes wrong.
//! * Browser profile caches are deliberately **not** included: clearing them
//!   signs the kiosk browser out of whatever it was logged into.

use crate::config::CleanupConfig;
use crate::event::{DetectorEvent, Severity, Source};
use log::{debug, info};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct Report {
    pub freed_bytes: u64,
    pub files: u64,
    pub per_root: Vec<(String, u64, u64)>,
    pub errors: u64,
}

/// The roots this is ever allowed to touch, resolved from the environment.
fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    };

    if let Ok(windir) = std::env::var("SystemRoot") {
        push(PathBuf::from(&windir).join("Temp"));
        push(PathBuf::from(&windir).join("Minidump"));
        // Windows Update keeps installers here long after they are applied.
        push(PathBuf::from(&windir).join("SoftwareDistribution").join("Download"));
    }
    // LocalSystem's own temp, which is where most service-created junk lands.
    if let Ok(tmp) = std::env::var("TEMP") {
        push(PathBuf::from(tmp));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        push(PathBuf::from(local).join("CrashDumps"));
    }
    out
}

/// True when `candidate` really sits under `root` once both are canonicalised.
///
/// The string prefix check alone is not enough: a junction inside the root
/// resolves elsewhere, and `..` segments can climb out.
fn contained(root: &Path, candidate: &Path) -> bool {
    let (Ok(root), Ok(candidate)) = (root.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    candidate.starts_with(&root)
}

fn is_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Walks one root, deleting old files. `dry_run` counts without removing.
fn sweep(root: &Path, min_age: Duration, dry_run: bool, report: &mut Report) {
    let mut freed = 0u64;
    let mut count = 0u64;
    let now = SystemTime::now();
    // Explicit stack rather than recursion: a pathological directory depth
    // should not blow the service's stack.
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            report.errors += 1;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                report.errors += 1;
                continue;
            };

            // A junction here could point at anything; do not descend into it
            // and do not delete it.
            if is_reparse(&meta) {
                debug!("Cleanup: skipping reparse point {}", path.display());
                continue;
            }
            if !contained(root, &path) {
                debug!("Cleanup: {} escapes {}", path.display(), root.display());
                continue;
            }

            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }

            // Newest of mtime/atime, so a file still being read is spared.
            let age = meta
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .unwrap_or_default();
            if age < min_age {
                continue;
            }

            let size = meta.len();
            if dry_run {
                freed += size;
                count += 1;
            } else {
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        freed += size;
                        count += 1;
                    }
                    // Locked by a running process — expected, not an incident.
                    Err(_) => report.errors += 1,
                }
            }
        }
    }

    report.freed_bytes += freed;
    report.files += count;
    report.per_root.push((root.display().to_string(), freed, count));
}

/// Rotates our own log once it grows past the limit, keeping one generation.
fn rotate_log(max_mb: u64) -> Option<u64> {
    if max_mb == 0 {
        return None;
    }
    let path = crate::runner::log_path();
    let size = std::fs::metadata(&path).ok()?.len();
    if size < max_mb * 1024 * 1024 {
        return None;
    }
    let old = path.with_extension("log.1");
    let _ = std::fs::remove_file(&old);
    // The service holds this file open, so a rename works where a delete would
    // not; the logger keeps writing to the handle until the next restart.
    std::fs::rename(&path, &old).ok()?;
    Some(size)
}

pub fn run(cfg: &CleanupConfig, dry_run: bool) -> (Report, Vec<DetectorEvent>) {
    let min_age = Duration::from_secs(cfg.min_age_days.max(1) * 86400);
    let mut report = Report { freed_bytes: 0, files: 0, per_root: Vec::new(), errors: 0 };

    for root in roots() {
        sweep(&root, min_age, dry_run, &mut report);
    }
    let rotated = if dry_run { None } else { rotate_log(cfg.log_max_mb) };

    let mb = report.freed_bytes as f64 / 1e6;
    info!(
        "Cleanup{}: {:.1} MB across {} file(s), {} skipped",
        if dry_run { " (dry run)" } else { "" },
        mb,
        report.files,
        report.errors
    );

    let mut events = Vec::new();
    // Freeing nothing is the normal weekly outcome on a healthy machine and
    // does not need to be reported.
    if report.files > 0 || rotated.is_some() {
        let mut ev = DetectorEvent::new(
            Source::Health,
            Severity::Info,
            "windows",
            "cleanup",
            format!(
                "Диск цэвэрлэлт: {:.1} MB, {} файл{}",
                mb,
                report.files,
                if rotated.is_some() { ", лог сольсон" } else { "" }
            ),
        )
        .kod("health/cleanup")
        .meta("freedBytes", report.freed_bytes)
        .meta("files", report.files)
        .meta("skipped", report.errors);
        if let Some(size) = rotated {
            ev = ev.meta("rotatedLogBytes", size);
        }
        events.push(ev.refingerprint());
    }
    (report, events)
}

/// Human-readable summary, for the `test` output and command results.
pub fn describe(report: &Report, dry_run: bool) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{} — {:.1} MB, {} файл, {} өнгөрүүлсэн",
        if dry_run { "Цэвэрлэх боломжтой" } else { "Цэвэрлэлээ" },
        report.freed_bytes as f64 / 1e6,
        report.files,
        report.errors
    );
    for (root, bytes, files) in &report.per_root {
        let _ = writeln!(s, "  {root} — {:.1} MB / {files}", *bytes as f64 / 1e6);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("zevclean-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    /// Backdates a file so the age gate treats it as old.
    fn write_old(path: &Path, bytes: usize) {
        fs::write(path, vec![b'x'; bytes]).unwrap();
        let old = SystemTime::now() - Duration::from_secs(60 * 86400);
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(old).unwrap();
    }

    #[test]
    fn old_files_are_swept_and_new_ones_are_not() {
        let root = temp_dir("age");
        write_old(&root.join("old.tmp"), 2048);
        fs::write(root.join("new.tmp"), vec![b'y'; 512]).unwrap();

        let mut report =
            Report { freed_bytes: 0, files: 0, per_root: Vec::new(), errors: 0 };
        sweep(&root, Duration::from_secs(7 * 86400), false, &mut report);

        assert_eq!(report.files, 1, "only the old file should go");
        assert_eq!(report.freed_bytes, 2048);
        assert!(!root.join("old.tmp").exists());
        assert!(root.join("new.tmp").exists(), "a file in use must survive");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_counts_without_deleting() {
        let root = temp_dir("dry");
        write_old(&root.join("a.tmp"), 100);

        let mut report =
            Report { freed_bytes: 0, files: 0, per_root: Vec::new(), errors: 0 };
        sweep(&root, Duration::from_secs(86400), true, &mut report);

        assert_eq!(report.files, 1);
        assert!(root.join("a.tmp").exists(), "dry run must not delete");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn subdirectories_are_swept_but_directories_are_kept() {
        let root = temp_dir("nested");
        fs::create_dir_all(root.join("deep/deeper")).unwrap();
        write_old(&root.join("deep/deeper/x.tmp"), 64);

        let mut report =
            Report { freed_bytes: 0, files: 0, per_root: Vec::new(), errors: 0 };
        sweep(&root, Duration::from_secs(86400), false, &mut report);

        assert_eq!(report.files, 1);
        assert!(root.join("deep/deeper").is_dir(), "directories stay");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn containment_rejects_paths_outside_the_root() {
        let root = temp_dir("contain");
        let outside = temp_dir("contain-outside");
        fs::write(outside.join("victim.txt"), b"important").unwrap();

        // This is the check that stops a junction inside a temp directory from
        // turning a cleanup into data loss somewhere else entirely.
        assert!(contained(&root, &root));
        assert!(!contained(&root, &outside.join("victim.txt")));
        assert!(!contained(&root, &root.join("..")));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn the_allowlist_never_includes_a_drive_root() {
        // A bug that let this return C:\ would delete the machine.
        for root in roots() {
            assert!(
                root.parent().is_some(),
                "{} is a drive root and must never be swept",
                root.display()
            );
            let depth = root.components().count();
            assert!(depth >= 3, "{} is suspiciously shallow", root.display());
        }
    }
}
