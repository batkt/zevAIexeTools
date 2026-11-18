//! Batching, de-duplicating, offline-tolerant uploader.
//!
//! Events are merged by fingerprint before they leave the machine, so a camera
//! that fails 400 times in a minute costs one row with `davtalt: 400` instead
//! of 400 rows. If the server is unreachable the queue is written to
//! `queue.json` next to the exe and retried on the next tick — a site that
//! loses its internet link for a night still reports what happened when it
//! comes back.

use crate::config::{AgentConfig, ServerConfig};
use crate::event::DetectorEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Serialize)]
struct Payload<'a> {
    agent: &'a str,
    #[serde(rename = "agentVersion")]
    agent_version: &'a str,
    #[serde(rename = "baiguullagiinId")]
    baiguullagiin_id: &'a str,
    #[serde(rename = "baiguullagiinNer")]
    baiguullagiin_ner: &'a str,
    #[serde(rename = "barilgiinId")]
    barilgiin_id: &'a str,
    os: &'a str,
    events: &'a [DetectorEvent],
}

#[derive(Serialize)]
struct Heartbeat<'a> {
    agent: &'a str,
    #[serde(rename = "agentVersion")]
    agent_version: &'a str,
    #[serde(rename = "baiguullagiinId")]
    baiguullagiin_id: &'a str,
    #[serde(rename = "baiguullagiinNer")]
    baiguullagiin_ner: &'a str,
    #[serde(rename = "barilgiinId")]
    barilgiin_id: &'a str,
    os: &'a str,
    #[serde(rename = "uptimeSecs")]
    uptime_secs: u64,
    #[serde(rename = "queueLength")]
    queue_length: usize,
    services: Vec<serde_json::Value>,
    health: serde_json::Value,
    /// Restarts the agent performed in the last rolling hour, per service.
    /// Lets the dashboard show "healed 2×" without replaying the event stream.
    #[serde(rename = "sergeelt", skip_serializing_if = "serde_json::Value::is_null")]
    heals: serde_json::Value,
    #[serde(rename = "suljee", skip_serializing_if = "serde_json::Value::is_null")]
    network: serde_json::Value,
    #[serde(rename = "portuud", skip_serializing_if = "Vec::is_empty")]
    ports: Vec<serde_json::Value>,
    #[serde(rename = "tsag", skip_serializing_if = "serde_json::Value::is_null")]
    clock: serde_json::Value,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    dahua: serde_json::Value,
}

/// Everything the heartbeat carries besides identity. Grouped rather than
/// passed positionally — the list keeps growing, and five same-typed
/// `serde_json::Value` arguments in a row is a bug waiting to happen.
#[derive(Default, Clone)]
pub struct HeartbeatData {
    pub services: Vec<serde_json::Value>,
    pub health: serde_json::Value,
    pub heals: serde_json::Value,
    pub network: serde_json::Value,
    pub ports: Vec<serde_json::Value>,
    pub clock: serde_json::Value,
    pub dahua: serde_json::Value,
}

/// What the main loop publishes for the heartbeat task: the latest collector
/// snapshots plus the queue length, and when they were taken (so a stalled
/// main loop is visible instead of hidden behind a heartbeat that keeps going).
#[derive(Clone)]
pub struct HeartbeatSnapshot {
    pub data: HeartbeatData,
    pub queue_length: usize,
    pub taken: Instant,
}

#[derive(Serialize, Deserialize, Default)]
struct QueueFile {
    events: Vec<DetectorEvent>,
}

/// One instruction handed down in a heartbeat response.
#[derive(Debug, Clone, Deserialize)]
pub struct Tushaal {
    pub id: String,
    pub turul: String,
    #[serde(default)]
    pub tulkhuur: String,
    #[serde(default)]
    pub parametr: serde_json::Value,
}

#[derive(Deserialize, Default, Clone, Debug)]
pub struct HeartbeatResponse {
    #[serde(default)]
    pub tushaal: Vec<Tushaal>,
    #[serde(default)]
    pub tokhirgoo: Option<serde_json::Value>,
    #[serde(rename = "baiguullagiinId", default)]
    pub baiguullagiin_id: Option<String>,
    #[serde(rename = "baiguullagiinNer", default)]
    pub baiguullagiin_ner: Option<String>,
    #[serde(rename = "barilgiinId", default)]
    pub barilgiin_id: Option<String>,
}

pub struct Sender {
    server: ServerConfig,
    agent: AgentConfig,
    os: String,
    client: reqwest::Client,
    pending: Vec<DetectorEvent>,
    queue_path: PathBuf,
    started: Instant,
    /// Shared with the heartbeat task, which records its own failures.
    disconnects: Arc<AtomicU64>,
}

pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Sender {
    pub fn new(server: ServerConfig, agent: AgentConfig, state_dir: &std::path::Path) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(server.timeout_secs.max(5)))
            // Commands from the server run as LocalSystem, so the server's
            // certificate must be verified. `insecure_tls` is an escape hatch
            // for a machine with a broken root store, never the default.
            .danger_accept_invalid_certs(server.insecure_tls)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let queue_path = state_dir.join("queue.json");
        let pending = std::fs::read_to_string(&queue_path)
            .ok()
            .and_then(|t| serde_json::from_str::<QueueFile>(&t).ok())
            .map(|q| q.events)
            .unwrap_or_default();

        if !pending.is_empty() {
            log::info!("Loaded {} queued event(s) from previous run", pending.len());
        }

        Self {
            server,
            agent,
            os: os_name(),
            client,
            pending,
            queue_path,
            started: Instant::now(),
            disconnects: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn queue_len(&self) -> usize {
        self.pending.len()
    }

    /// Merge new events into the queue, collapsing repeats by fingerprint.
    pub fn enqueue(&mut self, events: Vec<DetectorEvent>) {
        if events.is_empty() {
            return;
        }
        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, e) in self.pending.iter().enumerate() {
            index.insert(e.fingerprint.clone(), i);
        }

        for ev in events {
            match index.get(&ev.fingerprint) {
                Some(&i) => {
                    let existing = &mut self.pending[i];
                    existing.davtalt = existing.davtalt.saturating_add(ev.davtalt);
                    existing.ognoo = ev.ognoo;
                    if existing.stack_trace.is_empty() {
                        existing.stack_trace = ev.stack_trace;
                    }
                }
                None => {
                    index.insert(ev.fingerprint.clone(), self.pending.len());
                    self.pending.push(ev);
                }
            }
        }

        // Bound the queue. Oldest-first drop, with one synthetic event so the
        // gap is visible in the dashboard rather than silent.
        if self.pending.len() > self.server.queue_max {
            let dropped = self.pending.len() - self.server.queue_max;
            self.pending.drain(0..dropped);
            log::warn!("Queue overflow — dropped {dropped} oldest event(s)");
            self.pending.push(
                DetectorEvent::new(
                    crate::event::Source::Detector,
                    crate::event::Severity::Warning,
                    "detector",
                    "queue",
                    format!("Дараалал дүүрсэн тул {dropped} хуучин үйл явдлыг хаяв"),
                )
                .kod("detector/queue-overflow")
                .meta("dropped", dropped)
                .refingerprint(),
            );
        }

        self.persist();
    }

    /// Push as much of the queue as the server will take. Anything left over
    /// stays queued for the next tick.
    pub async fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let url = format!("{}/detector", self.server.url);

        while !self.pending.is_empty() {
            let take = self.server.batch_size.max(1).min(self.pending.len());
            let batch: Vec<DetectorEvent> = self.pending[..take].to_vec();

            let payload = Payload {
                agent: &self.agent.name,
                agent_version: AGENT_VERSION,
                baiguullagiin_id: &self.agent.baiguullagiin_id,
                baiguullagiin_ner: &self.agent.baiguullagiin_ner,
                barilgiin_id: &self.agent.barilgiin_id,
                os: &self.os,
                events: &batch,
            };

            match post_json(&self.client, &self.server, &url, &payload, self.server.retry_count).await {
                Ok(_) => {
                    self.pending.drain(0..take);
                    self.persist();
                }
                Err(e) => {
                    log::warn!(
                        "Upload failed ({e}); {} event(s) stay queued",
                        self.pending.len()
                    );
                    self.persist();
                    return;
                }
            }
        }
    }

    /// Failed uploads and heartbeats since start — the network score's
    /// "disconnects" figure, and the only number here that reflects whether
    /// our own traffic is actually landing.
    pub fn disconnects(&self) -> u64 {
        self.disconnects.load(Ordering::Relaxed)
    }

    pub fn update_identity(&mut self, bg_id: Option<&str>, bg_ner: Option<&str>, b_id: Option<&str>) {
        if let Some(id) = bg_id {
            if !id.trim().is_empty() {
                self.agent.baiguullagiin_id = id.trim().to_string();
            }
        }
        if let Some(ner) = bg_ner {
            if !ner.trim().is_empty() {
                self.agent.baiguullagiin_ner = ner.trim().to_string();
            }
        }
        if let Some(bid) = b_id {
            if !bid.trim().is_empty() {
                self.agent.barilgiin_id = bid.trim().to_string();
            }
        }
    }

    /// A handle the heartbeat task owns. It shares the HTTP client and the
    /// failure counter with this sender but nothing else, so a slow upload,
    /// cleanup pass or command in the main loop can never delay a heartbeat
    /// (which is what made healthy machines show up as offline).
    pub fn beater(&self) -> Beater {
        Beater {
            server: self.server.clone(),
            agent: self.agent.clone(),
            os: self.os.clone(),
            client: self.client.clone(),
            started: self.started,
            disconnects: self.disconnects.clone(),
            failing_since: None,
            last_fail_log: None,
            stall_logged: false,
        }
    }

    /// Reports the outcome of one command back to the server.
    pub async fn tushaal_khariu(&self, id: &str, amjilttai: bool, ur_dun: &str, aldaa: &str) {
        let url = format!("{}/detector/tushaal/{id}/khariu", self.server.url);
        let body = serde_json::json!({
            "amjilttai": amjilttai,
            "urDun": crate::event::truncate(ur_dun, 4000),
            "aldaa": crate::event::truncate(aldaa, 2000),
        });
        if let Err(e) = post_json(&self.client, &self.server, &url, &body, self.server.retry_count).await {
            log::warn!("Could not report command {id}: {e}");
        }
    }

    fn persist(&self) {
        let file = QueueFile { events: self.pending.clone() };
        match serde_json::to_string(&file) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.queue_path, text) {
                    log::debug!("Cannot write queue file: {e}");
                }
            }
            Err(e) => log::debug!("Cannot serialize queue: {e}"),
        }
    }
}

/// POST with retries (1s, 2s, 4s… between attempts) so a server restart is
/// ridden out without hammering it. The error carries the full source chain —
/// reqwest's own Display hides the part that says *why* (DNS, refused,
/// certificate…).
async fn post_json<T: Serialize>(
    client: &reqwest::Client,
    server: &ServerConfig,
    url: &str,
    body: &T,
    attempts: u32,
) -> anyhow::Result<String> {
    let mut last: Option<String> = None;

    for attempt in 0..attempts.max(1) {
        if attempt > 0 {
            let backoff = 1u64 << (attempt.min(5) - 1);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
        }

        let mut req = client.post(url).json(body);
        if !server.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", server.token));
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp.text().await.unwrap_or_default());
                }
                // 4xx is our bug, not a network blip — retrying will not fix
                // a malformed payload, so fail fast and keep the queue.
                let text = resp.text().await.unwrap_or_default();
                let msg = format!("HTTP {status}: {}", crate::event::truncate(&text, 300));
                if status.is_client_error() {
                    anyhow::bail!(msg);
                }
                last = Some(msg);
            }
            Err(e) => last = Some(error_chain(&e)),
        }
    }
    anyhow::bail!(last.unwrap_or_else(|| "unknown error".to_string()))
}

/// "outer: cause: root cause" — the whole chain on one line.
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let t = s.to_string();
        if !out.contains(&t) {
            out.push_str(": ");
            out.push_str(&t);
        }
        src = s.source();
    }
    out
}

fn is_certificate_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("certificate") || m.contains("cert_") || (m.contains("tls") && m.contains("trust"))
}

/// How long the main loop may go without publishing a snapshot before the
/// heartbeat task says so in the log.
const STALL_WARN: Duration = Duration::from_secs(600);
/// While the server stays unreachable, repeat the warning this often.
const FAIL_REPEAT: Duration = Duration::from_secs(600);

/// Sends heartbeats from its own task. See [`Sender::beater`].
pub struct Beater {
    server: ServerConfig,
    agent: AgentConfig,
    os: String,
    client: reqwest::Client,
    started: Instant,
    disconnects: Arc<AtomicU64>,
    failing_since: Option<Instant>,
    last_fail_log: Option<Instant>,
    stall_logged: bool,
}

impl Beater {
    /// Sends one heartbeat. `None` when the server could not be reached.
    /// At most 2 attempts, so even the worst case (2 × timeout + 1 s) stays
    /// inside the 60 s interval and the next beat is never pushed back.
    pub async fn send(&mut self, snap: &HeartbeatSnapshot) -> Option<HeartbeatResponse> {
        let age = snap.taken.elapsed();
        if age >= STALL_WARN {
            if !self.stall_logged {
                log::warn!(
                    "Main loop has not refreshed its data for {}s — collectors may be stuck; heartbeat keeps sending the last snapshot",
                    age.as_secs()
                );
                self.stall_logged = true;
            }
        } else if self.stall_logged {
            log::info!("Main loop is refreshing data again");
            self.stall_logged = false;
        }

        let d = &snap.data;
        let body = Heartbeat {
            agent: &self.agent.name,
            agent_version: AGENT_VERSION,
            baiguullagiin_id: &self.agent.baiguullagiin_id,
            baiguullagiin_ner: &self.agent.baiguullagiin_ner,
            barilgiin_id: &self.agent.barilgiin_id,
            os: &self.os,
            uptime_secs: self.started.elapsed().as_secs(),
            queue_length: snap.queue_length,
            services: d.services.clone(),
            health: d.health.clone(),
            heals: d.heals.clone(),
            network: d.network.clone(),
            ports: d.ports.clone(),
            clock: d.clock.clone(),
            dahua: d.dahua.clone(),
        };
        let url = format!("{}/detector/heartbeat", self.server.url);

        match post_json(&self.client, &self.server, &url, &body, 2).await {
            Ok(text) => {
                if let Some(since) = self.failing_since.take() {
                    log::info!("Heartbeat restored after {}s offline", since.elapsed().as_secs());
                }
                self.last_fail_log = None;
                let khariu = serde_json::from_str::<HeartbeatResponse>(&text).unwrap_or_default();
                self.update_identity(&khariu);
                Some(khariu)
            }
            Err(e) => {
                self.disconnects.fetch_add(1, Ordering::Relaxed);
                let msg = format!("{e:#}");
                let now = Instant::now();
                let since = *self.failing_since.get_or_insert(now);
                let due = self
                    .last_fail_log
                    .map_or(true, |t| now.duration_since(t) >= FAIL_REPEAT);
                if due {
                    self.last_fail_log = Some(now);
                    let dur = now.duration_since(since).as_secs();
                    if is_certificate_error(&msg) {
                        log::error!(
                            "Heartbeat failed: server certificate rejected ({msg}). \
                             Check the machine's clock and root certificates; as a last resort set \
                             [server] insecure_tls = true in config.toml"
                        );
                    } else if dur == 0 {
                        log::warn!("Heartbeat failed — server unreachable: {msg}");
                    } else {
                        log::warn!("Heartbeat still failing after {dur}s: {msg}");
                    }
                } else {
                    log::debug!("Heartbeat failed: {msg}");
                }
                None
            }
        }
    }

    fn update_identity(&mut self, r: &HeartbeatResponse) {
        for (src, dst) in [
            (&r.baiguullagiin_id, &mut self.agent.baiguullagiin_id),
            (&r.baiguullagiin_ner, &mut self.agent.baiguullagiin_ner),
            (&r.barilgiin_id, &mut self.agent.barilgiin_id),
        ] {
            if let Some(v) = src {
                if !v.trim().is_empty() {
                    *dst = v.trim().to_string();
                }
            }
        }
    }
}

#[cfg(test)]
mod beater_tests {
    use super::is_certificate_error;

    #[test]
    fn spots_certificate_errors() {
        assert!(is_certificate_error(
            "error sending request: client error (Connect): The certificate chain was issued by an authority that is not trusted"
        ));
        assert!(!is_certificate_error("error sending request: connection refused"));
    }
}

pub fn os_name() -> String {
    #[cfg(windows)]
    {
        let name = sysinfo::System::long_os_version()
            .unwrap_or_else(|| "Windows".to_string());
        let kernel = sysinfo::System::kernel_version().unwrap_or_default();
        if kernel.is_empty() {
            name
        } else {
            format!("{name} (build {kernel})")
        }
    }
    #[cfg(not(windows))]
    {
        sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string())
    }
}
