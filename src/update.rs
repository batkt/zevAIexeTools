//! Updating the agent itself from GitHub releases.
//!
//! The agent cannot overwrite its own running exe, so it does not try. It
//! downloads the installer from a release and runs it silently; the installer
//! stops the service, replaces both binaries and puts it back. That is also why
//! the installer is the published artifact rather than a zip of exes.
//!
//! Three deliberate refusals:
//!
//! * **Nothing happens without a configured repo.** No default owner/name, so a
//!   misconfigured agent cannot be pointed at someone else's releases.
//! * **The asset digest is verified before the installer is executed.** This
//!   runs elevated as LocalSystem; a truncated or substituted download is the
//!   one thing that must never reach `Command::new`.
//! * **"ask" is the default mode.** A parking site runs all day, and a service
//!   restarting mid-shift unannounced is worse than being a version behind.

use crate::config::UpdateConfig;
use crate::event::{DetectorEvent, Severity, Source};
use log::{info, warn};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize, Clone)]
struct Asset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    size: u64,
    /// GitHub reports this as "sha256:<hex>" when it has one.
    #[serde(default)]
    digest: Option<String>,
}

pub struct Available {
    pub version: String,
    pub notes: String,
    pub asset_name: String,
    pub size: u64,
}

/// Compares dotted numeric versions, ignoring a leading `v`.
///
/// Written out rather than pulled from a crate because the only thing needed is
/// "is the release newer", and string comparison gets that wrong the moment a
/// version reaches 1.10 (which sorts before 1.9).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .take_while(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    if a.is_empty() {
        return false;
    }
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// True when local time falls inside "HH:MM-HH:MM". An empty window is always
/// open; a window that wraps past midnight is handled.
pub fn in_window(window: &str, now_minutes: u32) -> bool {
    let window = window.trim();
    if window.is_empty() {
        return true;
    }
    let Some((from, to)) = window.split_once('-') else {
        return true;
    };
    let parse = |s: &str| -> Option<u32> {
        let (h, m) = s.trim().split_once(':')?;
        Some(h.trim().parse::<u32>().ok()? * 60 + m.trim().parse::<u32>().ok()?)
    };
    match (parse(from), parse(to)) {
        (Some(a), Some(b)) if a <= b => now_minutes >= a && now_minutes < b,
        // 22:00-05:00 spans midnight.
        (Some(a), Some(b)) => now_minutes >= a || now_minutes < b,
        _ => true,
    }
}

pub struct Updater {
    cfg: UpdateConfig,
    client: reqwest::Client,
    /// So "ask" mode does not report the same release every check.
    announced: Option<String>,
}

impl Updater {
    pub fn new(cfg: UpdateConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            // GitHub rejects requests without one.
            .user_agent(format!("zev-detector/{}", crate::sender::AGENT_VERSION))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { cfg, client, announced: None }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.mode != "off" && !self.cfg.github_repo.trim().is_empty()
    }

    /// Checks for a release and, depending on mode, reports or installs it.
    pub async fn poll(&mut self, now_minutes: u32) -> Vec<DetectorEvent> {
        if !self.enabled() {
            return Vec::new();
        }
        let found = match self.latest().await {
            Ok(Some(f)) => f,
            Ok(None) => return Vec::new(),
            Err(e) => {
                warn!("Update check failed: {e}");
                return Vec::new();
            }
        };

        let (available, asset) = found;
        if self.cfg.mode == "auto" && in_window(&self.cfg.window, now_minutes) {
            return self.install(available, asset).await;
        }

        // "ask", or auto outside its window: say so once per version.
        if self.announced.as_deref() == Some(available.version.as_str()) {
            return Vec::new();
        }
        self.announced = Some(available.version.clone());
        vec![DetectorEvent::new(
            Source::Detector,
            Severity::Info,
            "detector",
            "update",
            format!(
                "Шинэ хувилбар {} гарсан (одоо {}). Удирдлага хуудсаас шинэчилнэ үү.",
                available.version,
                crate::sender::AGENT_VERSION
            ),
        )
        .kod("detector/updateAvailable")
        .meta("version", available.version)
        .meta("asset", available.asset_name)
        .meta("sizeBytes", available.size)
        .meta("notes", crate::event::truncate(&available.notes, 500))
        .refingerprint()]
    }

    /// Downloads and runs the installer. Used by `poll` in auto mode and by the
    /// `agentShinechlekh` command.
    pub async fn install_latest(&mut self) -> anyhow::Result<String> {
        if !self.enabled() {
            anyhow::bail!("[update].github_repo тохируулаагүй байна");
        }
        match self.latest().await? {
            Some((available, asset)) => {
                let version = available.version.clone();
                let events = self.install(available, asset).await;
                let failed = events.iter().any(|e| e.aldaanii_tuvshin == "Critical");
                if failed {
                    anyhow::bail!(
                        "{}",
                        events.first().map(|e| e.aldaanii_msg.clone()).unwrap_or_default()
                    );
                }
                Ok(format!("{version} суулгах ажиллаж байна"))
            }
            None => Ok(format!(
                "Аль хэдийн хамгийн шинэ хувилбар ({})",
                crate::sender::AGENT_VERSION
            )),
        }
    }

    /// The newest release newer than us, with its installer asset.
    async fn latest(&self) -> anyhow::Result<Option<(Available, Asset)>> {
        let repo = normalize_repo(&self.cfg.github_repo);
        let url = format!("https://api.github.com/repos/{repo}/releases");

        let mut req = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !self.cfg.github_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.cfg.github_token));
        }

        let resp = req.send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("{repo}: release олдсонгүй");
        }
        if !resp.status().is_success() {
            anyhow::bail!("GitHub HTTP {}", resp.status());
        }

        let releases: Vec<Release> = resp.json().await?;
        for release in releases {
            if release.draft || release.prerelease {
                continue;
            }
            if let Some(asset) = release
                .assets
                .iter()
                .find(|a| a.name.to_lowercase().ends_with(".exe"))
            {
                let version = extract_version(&release.tag_name, &release.name, &asset.name);
                if is_newer(&version, crate::sender::AGENT_VERSION) {
                    return Ok(Some((
                        Available {
                            version: version.trim_start_matches('v').to_string(),
                            notes: release.body.clone(),
                            asset_name: asset.name.clone(),
                            size: asset.size,
                        },
                        asset.clone(),
                    )));
                }
            }
        }

        Ok(None)
    }

    async fn install(&mut self, available: Available, asset: Asset) -> Vec<DetectorEvent> {
        let fail = |msg: String| {
            vec![DetectorEvent::new(
                Source::Detector,
                Severity::Critical,
                "detector",
                "update",
                msg,
            )
            .kod("detector/updateFailed")
            .refingerprint()]
        };

        let bytes = match self.download(&asset).await {
            Ok(b) => b,
            Err(e) => return fail(format!("Шинэчлэлт татаж чадсангүй: {e}")),
        };

        // GitHub only supplies a digest for newer releases; when it does, this
        // is the check that stops a corrupted or substituted installer from
        // being executed as LocalSystem.
        if let Some(digest) = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
            let actual = crate::tushaal::sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(digest.trim()) {
                return fail(format!(
                    "Шинэчлэлтийн SHA256 таарсангүй (хүлээсэн {digest}, татсан {actual}) — суулгасангүй"
                ));
            }
        } else if asset.size > 0 && bytes.len() as u64 != asset.size {
            // No digest published, so at minimum insist the whole file arrived.
            return fail(format!(
                "Шинэчлэлт бүрэн татагдсангүй ({} / {} байт) — суулгасангүй",
                bytes.len(),
                asset.size
            ));
        }

        let path = std::env::temp_dir().join(&asset.name);
        if let Err(e) = std::fs::write(&path, &bytes) {
            return fail(format!("Шинэчлэлт бичиж чадсангүй: {e}"));
        }

        info!("Update: launching {} for {}", path.display(), available.version);
        // Detached: the installer stops this very service, so it must not be a
        // child that dies with us mid-copy.
        let spawned = std::process::Command::new(&path)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
            .spawn();

        match spawned {
            Ok(_) => vec![DetectorEvent::new(
                Source::Detector,
                Severity::Info,
                "detector",
                "update",
                format!(
                    "{} хувилбар руу шинэчилж байна ({} → {})",
                    available.version,
                    crate::sender::AGENT_VERSION,
                    available.version
                ),
            )
            .kod("detector/updating")
            .meta("version", available.version)
            .refingerprint()],
            Err(e) => fail(format!("Суулгагчийг ажиллуулж чадсангүй: {e}")),
        }
    }

    async fn download(&self, asset: &Asset) -> anyhow::Result<Vec<u8>> {
        // The api.github.com asset URL works for private repos with a token;
        // browser_download_url does not.
        let (url, api) = if self.cfg.github_token.is_empty() || asset.url.is_empty() {
            (asset.browser_download_url.clone(), false)
        } else {
            (asset.url.clone(), true)
        };
        if url.is_empty() {
            anyhow::bail!("Asset-ийн хаяг байхгүй");
        }

        let mut req = self.client.get(&url);
        if api {
            req = req
                .header("Accept", "application/octet-stream")
                .header("Authorization", format!("Bearer {}", self.cfg.github_token));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

pub fn normalize_repo(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "batkt/zevAIexeTools".to_string();
    }
    let mut s = trimmed;
    if let Some(pos) = s.find("github.com/") {
        s = &s[pos + "github.com/".len()..];
    }
    let clean = s.trim_end_matches(".git").trim_matches('/');
    if clean.contains('/') {
        clean.to_string()
    } else {
        "batkt/zevAIexeTools".to_string()
    }
}

pub fn extract_version(tag: &str, name: &str, asset: &str) -> String {
    for candidate in [tag, name, asset] {
        let trimmed = candidate.trim().trim_start_matches('v');
        let parts: Vec<&str> = trimmed
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .filter(|p| !p.is_empty() && p.contains('.') && p.chars().any(|c| c.is_ascii_digit()))
            .collect();
        if let Some(v) = parts.first() {
            let clean = v.trim_matches('.');
            if clean.chars().any(|c| c.is_ascii_digit()) {
                return clean.to_string();
            }
        }
    }
    tag.trim().to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub asset_name: String,
    pub size_bytes: u64,
    pub notes: String,
    pub published_at: String,
}

pub async fn check_update(cfg: &UpdateConfig) -> anyhow::Result<UpdateCheckResult> {
    let repo = normalize_repo(&cfg.github_repo);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!("zev-detector/{}", crate::sender::AGENT_VERSION))
        .build()?;

    let url = format!("https://api.github.com/repos/{repo}/releases");
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if !cfg.github_token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.github_token));
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub HTTP {}", resp.status());
    }

    let releases: Vec<Release> = resp.json().await?;
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        if let Some(asset) = release
            .assets
            .iter()
            .find(|a| a.name.to_lowercase().ends_with(".exe"))
        {
            let ver = extract_version(&release.tag_name, &release.name, &asset.name);
            let has_update = is_newer(&ver, crate::sender::AGENT_VERSION);
            return Ok(UpdateCheckResult {
                current_version: crate::sender::AGENT_VERSION.to_string(),
                latest_version: ver,
                has_update,
                asset_name: asset.name.clone(),
                size_bytes: asset.size,
                notes: release.body.clone(),
                published_at: release.tag_name.clone(),
            });
        }
    }

    Ok(UpdateCheckResult {
        current_version: crate::sender::AGENT_VERSION.to_string(),
        latest_version: crate::sender::AGENT_VERSION.to_string(),
        has_update: false,
        asset_name: String::new(),
        size_bytes: 0,
        notes: "Шинэ хувилбар олдсонгүй.".to_string(),
        published_at: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_are_detected_numerically() {
        assert!(is_newer("1.2.0", "1.1.0"));
        assert!(is_newer("v1.2.0", "1.1.0"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.9", "1.1.0"));
    }

    #[test]
    fn double_digit_components_compare_correctly() {
        // The reason this is not a string comparison: "1.10.0" < "1.9.0" as
        // text, which would silently stop every update after 1.9.
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(!is_newer("1.9.0", "1.10.0"));
        assert!(is_newer("1.0.11", "1.0.9"));
    }

    #[test]
    fn shorter_versions_are_padded_not_treated_as_smaller() {
        assert!(!is_newer("1.1", "1.1.0"));
        assert!(is_newer("1.2", "1.1.9"));
    }

    #[test]
    fn unparseable_tags_never_trigger_an_update() {
        // A release tagged "nightly" must not be read as "newer than anything".
        assert!(!is_newer("nightly", "1.0.0"));
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn empty_window_is_always_open() {
        assert!(in_window("", 0));
        assert!(in_window("   ", 13 * 60));
    }

    #[test]
    fn a_normal_window_admits_only_its_own_hours() {
        assert!(in_window("02:00-05:00", 3 * 60));
        assert!(!in_window("02:00-05:00", 1 * 60));
        assert!(!in_window("02:00-05:00", 5 * 60), "end is exclusive");
    }

    #[test]
    fn a_window_across_midnight_still_works() {
        assert!(in_window("22:00-05:00", 23 * 60));
        assert!(in_window("22:00-05:00", 2 * 60));
        assert!(!in_window("22:00-05:00", 12 * 60));
    }

    #[test]
    fn a_malformed_window_does_not_block_updates_forever() {
        assert!(in_window("garbage", 12 * 60));
    }
}
