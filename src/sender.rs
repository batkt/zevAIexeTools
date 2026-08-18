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
}

#[derive(Serialize, Deserialize, Default)]
struct QueueFile {
    events: Vec<DetectorEvent>,
}

pub struct Sender {
    server: ServerConfig,
    agent: AgentConfig,
    os: String,
    client: reqwest::Client,
    pending: Vec<DetectorEvent>,
    queue_path: PathBuf,
    started: std::time::Instant,
}

pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Sender {
    pub fn new(server: ServerConfig, agent: AgentConfig, state_dir: &std::path::Path) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(server.timeout_secs.max(5)))
            .danger_accept_invalid_certs(true)
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
            started: std::time::Instant::now(),
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

            match self.post(&url, &payload).await {
                Ok(()) => {
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

    pub async fn heartbeat(&self, services: Vec<serde_json::Value>, health: serde_json::Value) {
        let url = format!("{}/detector/heartbeat", self.server.url);
        let body = Heartbeat {
            agent: &self.agent.name,
            agent_version: AGENT_VERSION,
            baiguullagiin_id: &self.agent.baiguullagiin_id,
            baiguullagiin_ner: &self.agent.baiguullagiin_ner,
            barilgiin_id: &self.agent.barilgiin_id,
            os: &self.os,
            uptime_secs: self.started.elapsed().as_secs(),
            queue_length: self.pending.len(),
            services,
            health,
        };
        if let Err(e) = self.post(&url, &body).await {
            log::debug!("Heartbeat failed: {e}");
        }
    }

    async fn post<T: Serialize>(&self, url: &str, body: &T) -> anyhow::Result<()> {
        let mut last: Option<String> = None;

        for attempt in 0..self.server.retry_count.max(1) {
            if attempt > 0 {
                // 1s, 2s, 4s… so a server restart is ridden out without
                // hammering it.
                let backoff = 1u64 << (attempt.min(5) - 1);
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            }

            let mut req = self.client.post(url).json(body);
            if !self.server.token.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", self.server.token));
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(());
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
                Err(e) => last = Some(e.to_string()),
            }
        }
        anyhow::bail!(last.unwrap_or_else(|| "unknown error".to_string()))
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
