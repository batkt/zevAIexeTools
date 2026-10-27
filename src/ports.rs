//! Local TCP port watcher.
//!
//! A parking PC talks to cameras on 80/554, the Dahua SDK on 37777 and its own
//! parking service on 8080/5000. A service can be `Running` while its listener
//! never came up — a port bind that lost a race, or a firewall rule added by
//! someone else. Watching the port directly is what catches that.
//!
//! Edge-triggered like the other collectors: an open port produces no traffic,
//! and a closed one re-announces on a schedule so it cannot be forgotten.

use crate::config::PortEntry;
use crate::event::{DetectorEvent, Severity, Source};
use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const REMIND: Duration = Duration::from_secs(1800); // 30 min
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

struct Known {
    open: bool,
    since: Instant,
    last_reported: Option<Instant>,
}

pub struct PortCollector {
    entries: Vec<PortEntry>,
    known: HashMap<String, Known>,
    last: Vec<serde_json::Value>,
}

impl PortCollector {
    pub fn new(entries: Vec<PortEntry>) -> Self {
        Self { entries, known: HashMap::new(), last: Vec::new() }
    }

    pub fn snapshot(&self) -> Vec<serde_json::Value> {
        self.last.clone()
    }

    pub fn closed_ports(&self) -> Vec<(u16, String)> {
        self.last
            .iter()
            .filter(|v| {
                v.get("open").and_then(|o| o.as_bool()) == Some(false)
                    && v.get("mustListen").and_then(|m| m.as_bool()).unwrap_or(true)
            })
            .filter_map(|v| {
                let port = v.get("port").and_then(|p| p.as_u64())? as u16;
                let label = v
                    .get("label")
                    .and_then(|l| l.as_str())
                    .unwrap_or("port")
                    .to_string();
                Some((port, label))
            })
            .collect()
    }

    pub async fn poll(&mut self) -> Vec<DetectorEvent> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let entries = self.entries.clone();

        // Connects are blocking and there may be a handful of them; keep them
        // off the thread driving the detector loop.
        let checked = tokio::task::spawn_blocking(move || {
            entries
                .into_iter()
                .map(|e| {
                    let open = is_open(&e.host, e.port);
                    (e, open)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();

        let mut out = Vec::new();
        self.last.clear();

        for (entry, open) in checked {
            let label = if entry.label.is_empty() {
                format!("{}", entry.port)
            } else {
                entry.label.clone()
            };
            let key = format!("{}:{}", entry.host, entry.port);

            self.last.push(serde_json::json!({
                "port": entry.port,
                "host": entry.host,
                "label": label,
                "open": open,
                "mustListen": entry.must_listen,
            }));

            let prev = self.known.get(&key);
            let changed = prev.map(|k| k.open != open).unwrap_or(true);
            let is_first = prev.is_none();
            let due = !open
                && prev
                    .and_then(|k| k.last_reported)
                    .map(|t| t.elapsed() >= REMIND)
                    .unwrap_or(true);

            // A port that is open on the first look is the normal case.
            let report = (changed && !(is_first && open)) || due;

            if report {
                let downtime = prev.filter(|_| !open).map(|k| k.since.elapsed().as_secs()).unwrap_or(0);
                let severity = if open {
                    Severity::Info
                } else if entry.must_listen {
                    Severity::Critical
                } else {
                    Severity::Warning
                };
                let msg = if open {
                    format!("Порт {} ({label}) дахин нээгдлээ", entry.port)
                } else if downtime > 0 {
                    format!(
                        "Порт {} ({label}) хаалттай хэвээр — {}м",
                        entry.port,
                        downtime / 60
                    )
                } else {
                    format!("Порт {} ({label}) хаалттай байна", entry.port)
                };

                out.push(
                    DetectorEvent::new(Source::Health, severity, "windows", format!("port/{}", entry.port), msg)
                        .kod(format!("health/port/{}", entry.port))
                        .meta("port", entry.port as u64)
                        .meta("host", entry.host.clone())
                        .meta("label", label)
                        .meta("open", open)
                        .refingerprint(),
                );
            }

            let tracked = self.known.entry(key).or_insert(Known {
                open,
                since: Instant::now(),
                last_reported: None,
            });
            if tracked.open != open {
                tracked.open = open;
                tracked.since = Instant::now();
            }
            if report {
                tracked.last_reported = Some(Instant::now());
            }
        }

        out
    }
}

pub fn is_open(host: &str, port: u16) -> bool {
    is_open_timeout(host, port, CONNECT_TIMEOUT)
}

pub fn is_open_timeout(host: &str, port: u16, timeout: Duration) -> bool {
    match (host, port).to_socket_addrs() {
        Ok(mut addrs) => addrs.any(|a| TcpStream::connect_timeout(&a, timeout).is_ok()),
        Err(_) => false,
    }
}
