//! Carrying out commands sent down from the dashboard.
//!
//! Commands arrive in the heartbeat response rather than over an inbound
//! connection, because these machines sit behind NAT on parking sites — there
//! is no port to open. The trade is up to one heartbeat of latency, which for
//! "restart this service" is fine.
//!
//! The binary swap is the dangerous one, so it is written to fail safe:
//!
//! * The exe path comes from the **SCM**, not from a guess. dahuarust is
//!   installed from wherever `install` was run, which is its own
//!   `target\release` — replacing an assumed Program Files path would silently
//!   do nothing.
//! * The download is checked against a SHA-256 the server supplies **before**
//!   anything is touched. A truncated download must never reach a service.
//! * The old exe is kept, and if the new one does not come up, it is put back
//!   and started again. A parking barrier that stops working because an update
//!   was half-applied is worse than one that is simply out of date.
//! * Only the exe is replaced. `config.toml`, the Dahua SDK DLLs and the log
//!   all live in the same folder and are left alone.

use crate::config::ServerConfig;
use crate::sender::Tushaal;
use crate::services;
use log::{info, warn};
use std::path::Path;
use std::time::Duration;

/// How long a service gets to stop, and to come back up, during an update.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const START_TIMEOUT: Duration = Duration::from_secs(90);

pub struct Outcome {
    pub amjilttai: bool,
    pub ur_dun: String,
    pub aldaa: String,
}

impl Outcome {
    fn ok(msg: impl Into<String>) -> Self {
        Self { amjilttai: true, ur_dun: msg.into(), aldaa: String::new() }
    }
    fn fail(msg: impl Into<String>) -> Self {
        Self { amjilttai: false, ur_dun: String::new(), aldaa: msg.into() }
    }
}

pub struct Executor {
    server: ServerConfig,
    client: reqwest::Client,
}

impl Executor {
    pub fn new(server: ServerConfig) -> Self {
        let client = reqwest::Client::builder()
            // A binary download is far slower than an event POST.
            .timeout(Duration::from_secs(600))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { server, client }
    }

    pub async fn run(&self, t: &Tushaal) -> Outcome {
        info!("Command {}: {} {}", t.id, t.turul, t.tulkhuur);
        match t.turul.as_str() {
            "uilchilgeeAsaakh" => self.simple(&t.tulkhuur, true),
            "uilchilgeeUntraakh" => self.simple(&t.tulkhuur, false),
            "uilchilgeeDakhinAchaalakh" => self.restart(&t.tulkhuur).await,
            "uilchilgeeShinechlekh" => self.update_service(t).await,
            "testAjilluulakh" => self.test(),
            "agentShinechlekh" => self.update_agent().await,
            "tsevejluulekh" => self.cleanup(t),
            // Read-only diagnostics all share one arm; they cannot change
            // anything, so they need none of the confirmation the others do.
            kind if crate::diag::TURLUUD.contains(&kind) => self.diagnose(kind, &t.tulkhuur).await,
            other => Outcome::fail(format!("Unknown command type: {other}")),
        }
    }

    async fn update_agent(&self) -> Outcome {
        let cfg = match crate::config::Config::load() {
            Ok(c) => c,
            Err(e) => return Outcome::fail(e.to_string()),
        };
        let mut updater = crate::update::Updater::new(cfg.update.clone());
        match updater.install_latest().await {
            Ok(msg) => Outcome::ok(msg),
            Err(e) => Outcome::fail(e.to_string()),
        }
    }

    /// `dry` in the command runs the scan without deleting.
    fn cleanup(&self, t: &Tushaal) -> Outcome {
        let cfg = match crate::config::Config::load() {
            Ok(c) => c,
            Err(e) => return Outcome::fail(e.to_string()),
        };
        let dry = t.parametr.get("dry").and_then(|v| v.as_bool()).unwrap_or(false);
        let (report, _) = crate::cleanup::run(&cfg.cleanup, dry);
        Outcome::ok(crate::cleanup::describe(&report, dry))
    }

    async fn diagnose(&self, kind: &str, target: &str) -> Outcome {
        let cfg = match crate::config::Config::load() {
            Ok(c) => c,
            Err(e) => return Outcome::fail(e.to_string()),
        };
        match crate::diag::run(kind, target, &cfg).await {
            Ok(text) => Outcome::ok(text),
            Err(e) => Outcome::fail(e.to_string()),
        }
    }

    fn simple(&self, name: &str, start: bool) -> Outcome {
        if name.is_empty() {
            return Outcome::fail("No service named");
        }
        let result = if start { services::start(name) } else { services::stop(name) };
        match result {
            Ok(()) => Outcome::ok(format!(
                "'{name}' {} хүсэлт илгээгдлээ",
                if start { "асаах" } else { "унтраах" }
            )),
            Err(e) => Outcome::fail(e.to_string()),
        }
    }

    async fn restart(&self, name: &str) -> Outcome {
        if name.is_empty() {
            return Outcome::fail("No service named");
        }
        // The SCM waits run on a blocking thread, which needs an owned name.
        let owned = name.to_string();

        // Ignore a stop error: the service may already be down, which is a
        // perfectly good starting point for a restart.
        let _ = services::stop(name);
        let n = owned.clone();
        if !blocking(move || services::wait_for(&n, false, STOP_TIMEOUT)).await {
            return Outcome::fail(format!("'{name}' {}с дотор зогссонгүй", STOP_TIMEOUT.as_secs()));
        }
        if let Err(e) = services::start(name) {
            return Outcome::fail(format!("'{name}' асаахад алдаа: {e}"));
        }
        if !blocking(move || services::wait_for(&owned, true, START_TIMEOUT)).await {
            return Outcome::fail(format!("'{name}' дахин ачаалсны дараа асаагүй"));
        }
        Outcome::ok(format!("'{name}' дахин ачаалагдлаа"))
    }

    fn test(&self) -> Outcome {
        match crate::config::Config::load() {
            Ok(cfg) => Outcome::ok(crate::runner::collect_test_report(&cfg)),
            Err(e) => Outcome::fail(e.to_string()),
        }
    }

    // ─── binary swap ─────────────────────────────────────────────────────

    async fn update_service(&self, t: &Tushaal) -> Outcome {
        let name = t.tulkhuur.clone();
        if name.is_empty() {
            return Outcome::fail("No service named");
        }
        let huvilbar_id = t.parametr.get("huvilbarId").and_then(|v| v.as_str()).unwrap_or("");
        let sha_expected =
            t.parametr.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let version = t.parametr.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        if huvilbar_id.is_empty() || sha_expected.is_empty() {
            return Outcome::fail("huvilbarId ба sha256 шаардлагатай");
        }

        // Where the SCM will actually launch it from.
        let exe = match services::exe_path(&name) {
            Ok(p) => p,
            Err(e) => return Outcome::fail(format!("'{name}'-ийн зам олдсонгүй: {e}")),
        };
        let folder = match exe.parent() {
            Some(p) => p.to_path_buf(),
            None => return Outcome::fail(format!("Хачирхалтай зам: {}", exe.display())),
        };

        // Download and verify before touching anything that is running.
        let bytes = match self.download(huvilbar_id).await {
            Ok(b) => b,
            Err(e) => return Outcome::fail(format!("Татаж чадсангүй: {e}")),
        };
        let sha_actual = sha256_hex(&bytes);
        if sha_actual != sha_expected {
            return Outcome::fail(format!(
                "SHA256 таарсангүй — хүлээсэн {sha_expected}, татсан {sha_actual}. Файл эвдэрсэн байж магадгүй."
            ));
        }

        let staged = folder.join(format!("{}.shine", file_name(&exe)));
        let backup = folder.join(format!("{}.bak", file_name(&exe)));
        if let Err(e) = std::fs::write(&staged, &bytes) {
            return Outcome::fail(format!("Бичиж чадсангүй {}: {e}", staged.display()));
        }

        // Optional: the endpoint that proves the new build actually serves.
        let probe = t
            .parametr
            .get("probeUrl")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        let mut log = String::new();
        let outcome = self
            .swap(&name, &exe, &staged, &backup, version, probe, &mut log)
            .await;
        let _ = std::fs::remove_file(&staged);
        outcome
    }

    /// Stop → replace → start, restoring the old exe if the new one fails.
    #[allow(clippy::too_many_arguments)]
    async fn swap(
        &self,
        name: &str,
        exe: &Path,
        staged: &Path,
        backup: &Path,
        version: &str,
        probe: Option<String>,
        log: &mut String,
    ) -> Outcome {
        use std::fmt::Write as _;
        let owned = name.to_string();

        let _ = services::stop(name);
        let n = owned.clone();
        if !blocking(move || services::wait_for(&n, false, STOP_TIMEOUT)).await {
            return Outcome::fail(format!(
                "'{name}' {}с дотор зогссонгүй — файлыг сольсонгүй",
                STOP_TIMEOUT.as_secs()
            ));
        }
        let _ = writeln!(log, "'{name}' зогслоо");

        // The exe is only replaceable now that nothing holds it open. Keep the
        // old one: it is the only way back if the new build is broken.
        let _ = std::fs::remove_file(backup);
        if let Err(e) = std::fs::rename(exe, backup) {
            let _ = services::start(name);
            return Outcome::fail(format!("Хуучин файлыг нөөцлөж чадсангүй: {e}"));
        }
        if let Err(e) = std::fs::rename(staged, exe) {
            // Put the original back before anything else.
            let _ = std::fs::rename(backup, exe);
            let _ = services::start(name);
            return Outcome::fail(format!("Шинэ файлыг тавьж чадсангүй: {e}"));
        }
        let _ = writeln!(log, "exe {version} болж солигдлоо");

        if let Err(e) = services::start(name) {
            let _ = writeln!(log, "Асаахад алдаа: {e}");
            return self.rollback(name, exe, backup, log, &e.to_string()).await;
        }
        let n = owned.clone();
        if !blocking(move || services::wait_for(&n, true, START_TIMEOUT)).await {
            return self
                .rollback(name, exe, backup, log, "шинэ хувилбар асаагүй")
                .await;
        }
        let _ = writeln!(log, "'{name}' асаалаа");

        // "Running" is not the same as working — a service can start and then
        // fail to bind its port. If a health endpoint was supplied, that is the
        // only real proof the new build is good, and the last chance to roll
        // back before the backup is discarded.
        if let Some(url) = probe {
            if !self.probe_ok(&url).await {
                let _ = writeln!(log, "{url} хариу өгсөнгүй");
                return self
                    .rollback(name, exe, backup, log, "шинэ хувилбар эрүүл мэндийн шалгалтад тэнцсэнгүй")
                    .await;
            }
            let _ = writeln!(log, "{url} хариу өглөө");
        }

        let _ = std::fs::remove_file(backup);
        Outcome::ok(format!("{log}'{name}' {version} хувилбар руу шинэчлэгдлээ"))
    }

    async fn rollback(
        &self,
        name: &str,
        exe: &Path,
        backup: &Path,
        log: &mut String,
        why: &str,
    ) -> Outcome {
        use std::fmt::Write as _;
        warn!("Update of '{name}' failed ({why}) — rolling back");

        let owned = name.to_string();
        let _ = services::stop(name);
        let n = owned.clone();
        blocking(move || services::wait_for(&n, false, STOP_TIMEOUT)).await;

        if backup.exists() {
            let _ = std::fs::remove_file(exe);
            if let Err(e) = std::fs::rename(backup, exe) {
                // This is the bad one: no working exe in place.
                return Outcome::fail(format!(
                    "{log}ЭМЗЭГ: {why}, буцаах ч боломжгүй ({e}). '{name}' ажиллахгүй байна — гар аргаар сэргээнэ үү."
                ));
            }
            let _ = writeln!(log, "хуучин exe буцаагдлаа");
        }

        let started = services::start(name).is_ok()
            && blocking(move || services::wait_for(&owned, true, START_TIMEOUT)).await;
        let _ = writeln!(
            log,
            "{}",
            if started { "хуучин хувилбар дээр сэргэлээ" } else { "хуучин хувилбар ч асаагүй" }
        );

        Outcome::fail(format!("{log}Шинэчлэлт бүтэлгүйтлээ: {why}"))
    }

    async fn download(&self, huvilbar_id: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/detector/huvilbar/tatakh/{huvilbar_id}", self.server.url);
        let mut req = self.client.get(&url);
        if !self.server.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.server.token));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    async fn probe_ok(&self, url: &str) -> bool {
        for _ in 0..5 {
            if let Ok(r) = self.client.get(url).timeout(Duration::from_secs(5)).send().await {
                if r.status().is_success() {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        false
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "service.exe".into())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    // Small, dependency-free SHA-256. Pulling in a crate for one hash used
    // once per update is not worth the supply-chain surface.
    let mut h = Sha256::new();
    h.update(bytes);
    h.finish()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Runs a blocking SCM wait without stalling the async runtime.
async fn blocking<F>(f: F) -> bool
where
    F: FnOnce() -> anyhow::Result<bool> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map(|r| r.unwrap_or(false)).unwrap_or(false)
}

// ─── SHA-256 ─────────────────────────────────────────────────────────────────

struct Sha256 {
    state: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: Vec::new(),
            len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let block: [u8; 64] = self.buf[..64].try_into().unwrap();
            self.compress(&block);
            self.buf.drain(..64);
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bit_len = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        let tail = self.buf.split_off(0);
        for chunk in tail.chunks(64) {
            if chunk.len() == 56 {
                let mut block = [0u8; 64];
                block[..56].copy_from_slice(chunk);
                block[56..].copy_from_slice(&bit_len.to_be_bytes());
                self.compress(&block);
            } else {
                self.compress(chunk.try_into().unwrap());
            }
        }
        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g; g = f; f = e;
            e = d.wrapping_add(t1);
            d = c; c = b; b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_matches_known_vectors() {
        // A wrong hash here would either reject every good download or, far
        // worse, wave a corrupt one through onto a running service.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Spans several blocks and exercises the length padding.
        assert_eq!(
            sha256_hex(&b"a".repeat(1000)),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
        // 56 bytes: the exact boundary where padding needs an extra block.
        assert_eq!(
            sha256_hex(&b"a".repeat(56)),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
    }
}
