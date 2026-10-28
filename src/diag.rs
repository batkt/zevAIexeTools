//! On-demand diagnostics, run from the dashboard.
//!
//! Everything here is read-only: it connects, resolves, queries and reports.
//! Nothing starts, stops, writes or deletes — that belongs to the command
//! executor, where each action is confirmed individually. Keeping the two apart
//! means a diagnostic button can never be the thing that broke a site.
//!
//! Targets come from the command, but only ever as a host/port to reach or a
//! service to look at. There is no shell interpolation anywhere below: every
//! external program is invoked with an argument list, never a command string.

use crate::config::Config;
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

/// Every diagnostic the dashboard can ask for.
pub const TURLUUD: &[&str] = &[
    "diagPing",
    "diagDns",
    "diagApi",
    "diagPort",
    "diagCamera",
    "diagGateway",
    "diagWebsocket",
    "diagService",
    "diagDisk",
    "diagEvents",
    "diagSpeed",
];

pub async fn run(kind: &str, target: &str, cfg: &Config) -> anyhow::Result<String> {
    match kind {
        "diagPing" => ping(pick(target, &server_host(cfg))).await,
        "diagDns" => dns(pick(target, &server_host(cfg))).await,
        "diagApi" => api(cfg, target).await,
        "diagPort" => port(target).await,
        "diagCamera" => camera(target).await,
        "diagGateway" => gateway().await,
        "diagWebsocket" => websocket(cfg).await,
        "diagService" => service(target),
        "diagDisk" => Ok(disk()),
        "diagEvents" => events(cfg),
        "diagSpeed" => speed().await,
        other => anyhow::bail!("Unknown diagnostic: {other}"),
    }
}

fn pick<'a>(given: &'a str, fallback: &'a str) -> &'a str {
    if given.trim().is_empty() { fallback } else { given.trim() }
}

fn server_host(cfg: &Config) -> String {
    cfg.server
        .url
        .split("://")
        .nth(1)
        .unwrap_or(&cfg.server.url)
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Runs a console tool with an argument list and captures its output.
fn tool(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(text.trim().to_string())
}

// ─── individual checks ───────────────────────────────────────────────────────

async fn ping(host: &str) -> anyhow::Result<String> {
    if host.is_empty() {
        anyhow::bail!("No host to ping");
    }
    let host = host.to_string();
    let text = tokio::task::spawn_blocking(move || tool("ping", &["-n", "4", &host])).await??;
    Ok(text)
}

async fn dns(host: &str) -> anyhow::Result<String> {
    if host.is_empty() {
        anyhow::bail!("No host to resolve");
    }
    let owned = host.to_string();
    let resolved = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        // Port 0 is fine — we only want the A/AAAA records.
        let addrs: Vec<String> = (owned.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip().to_string()).collect())
            .unwrap_or_default();
        (addrs, started.elapsed())
    })
    .await?;

    let (addrs, took) = resolved;
    if addrs.is_empty() {
        anyhow::bail!("{host}: тодорхойлж чадсангүй");
    }
    Ok(format!("{host} → {} ({}мс)", addrs.join(", "), took.as_millis()))
}

async fn api(cfg: &Config, path: &str) -> anyhow::Result<String> {
    let path = if path.trim().is_empty() { "/detector/agents" } else { path.trim() };
    let url = format!("{}{}", cfg.server.url, path);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .danger_accept_invalid_certs(true)
        .build()?;

    let started = Instant::now();
    let mut req = client.get(&url);
    if !cfg.server.token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.server.token));
    }
    let resp = req.send().await?;
    let status = resp.status();
    let date = resp
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();
    let body = resp.text().await.unwrap_or_default();

    Ok(format!(
        "GET {url}\nHTTP {status} ({}мс)\nServer date: {date}\n{}",
        started.elapsed().as_millis(),
        crate::event::truncate(body.trim(), 600)
    ))
}

/// `host:port`, or a bare port meaning localhost.
async fn port(target: &str) -> anyhow::Result<String> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("host:port шаардлагатай");
    }
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>()?),
        None => ("127.0.0.1".to_string(), target.parse::<u16>()?),
    };

    let label = format!("{host}:{port}");
    let result = tokio::task::spawn_blocking(move || {
        let addr = (host.as_str(), port).to_socket_addrs().ok()?.next()?;
        let started = Instant::now();
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .ok()
            .map(|_| started.elapsed())
    })
    .await?;

    Ok(match result {
        Some(took) => format!("{label} нээлттэй ({}мс)", took.as_millis()),
        None => format!("{label} хаалттай"),
    })
}

/// Cameras answer HTTP on 80 and RTSP on 554; both are useful to know.
async fn camera(ip: &str) -> anyhow::Result<String> {
    let ip = ip.trim();
    if ip.is_empty() {
        anyhow::bail!("Камерын IP шаардлагатай");
    }
    let mut lines = vec![format!("Камер {ip}")];
    for (p, ner) in [(80u16, "HTTP"), (554, "RTSP"), (37777, "Dahua SDK")] {
        let line = port(&format!("{ip}:{p}")).await.unwrap_or_else(|e| e.to_string());
        lines.push(format!("  {ner}: {line}"));
    }
    lines.push(ping(ip).await.unwrap_or_else(|e| e.to_string()));
    Ok(lines.join("\n"))
}

async fn gateway() -> anyhow::Result<String> {
    // `route print -4` is stable across Windows locales in its numeric columns,
    // which is all that is needed to find the default route's next hop.
    let text = tokio::task::spawn_blocking(|| tool("route", &["print", "-4"])).await??;
    let gw = text
        .lines()
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            (cols.len() >= 3 && cols[0] == "0.0.0.0").then(|| cols[2].to_string())
        })
        .next();

    match gw {
        Some(ip) => Ok(format!("Gateway {ip}\n{}", ping(&ip).await.unwrap_or_default())),
        None => anyhow::bail!("Default gateway олдсонгүй"),
    }
}

/// Confirms the socket.io endpoint answers a handshake, without a WS client.
async fn websocket(cfg: &Config) -> anyhow::Result<String> {
    let base = cfg.server.url.trim_end_matches("/api").trim_end_matches('/');
    let url = format!("{base}/socket.io/?EIO=4&transport=polling");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .danger_accept_invalid_certs(true)
        .build()?;
    let started = Instant::now();
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    Ok(format!(
        "GET {url}\nHTTP {status} ({}мс)\n{}",
        started.elapsed().as_millis(),
        crate::event::truncate(body.trim(), 300)
    ))
}

fn service(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("Үйлчилгээний нэр шаардлагатай");
    }
    let path = crate::services::exe_path(name)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("(зам уншиж чадсангүй: {e})"));
    // Reuse the collector's own query so the answer matches what the watcher
    // and the healer see, rather than being a second opinion.
    let mut collector =
        crate::services::ServiceCollector::new(vec![crate::config::ServiceEntry {
            name: name.to_string(),
            label: name.to_string(),
            ..Default::default()
        }]);
    collector.poll();
    let state = collector
        .last_seen()
        .get(name)
        .map(|o| format!("{} (pid {}, exit {})", o.state, o.pid, o.exit_code))
        .unwrap_or_else(|| "тодорхойгүй".to_string());

    Ok(format!("{name}\n  Төлөв: {state}\n  Зам  : {path}"))
}

fn disk() -> String {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let mut lines = Vec::new();
    for d in disks.list() {
        let total = d.total_space();
        if total == 0 {
            continue;
        }
        let free = d.available_space();
        let used = ((total - free) as f64 / total as f64 * 100.0).round();
        lines.push(format!(
            "{} — {:.1} GB чөлөөтэй / {:.1} GB ({used}% ашиглагдсан)",
            d.mount_point().to_string_lossy(),
            free as f64 / 1e9,
            total as f64 / 1e9
        ));
    }
    lines.join("\n")
}

/// Whatever the event log collector would report right now.
fn events(cfg: &Config) -> anyhow::Result<String> {
    let mut collector = crate::eventlog::EventLogCollector::new(cfg.eventlog.clone());
    let found = collector.poll();
    if found.is_empty() {
        return Ok("Шинэ бичлэг алга".to_string());
    }
    Ok(found
        .iter()
        .take(25)
        .map(|e| format!("[{}] {} :: {}", e.aldaanii_tuvshin, e.modul, e.aldaanii_msg))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Measures actual Internet Download and Upload speed using Cloudflare Speed Test Edge.
pub async fn speed() -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .danger_accept_invalid_certs(true)
        .build()?;

    // 1. Download test (5MB chunk from Cloudflare CDN)
    let dl_url = "https://speed.cloudflare.com/__down?bytes=5000000";
    let start_dl = Instant::now();
    let resp = client.get(dl_url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Download speed test HTTP error: {}", resp.status());
    }
    let bytes = resp.bytes().await?;
    let dl_duration = start_dl.elapsed();
    let dl_secs = dl_duration.as_secs_f64().max(0.001);
    let dl_mbps = (bytes.len() as f64 * 8.0) / (dl_secs * 1_000_000.0);

    // 2. Upload test (1MB payload)
    let ul_url = "https://speed.cloudflare.com/__up";
    let payload = vec![0u8; 1024 * 1024]; // 1 MB
    let start_ul = Instant::now();
    let up_resp = client.post(ul_url).body(payload.clone()).send().await?;
    let ul_duration = start_ul.elapsed();
    let ul_secs = ul_duration.as_secs_f64().max(0.001);
    let ul_mbps = if up_resp.status().is_success() {
        (payload.len() as f64 * 8.0) / (ul_secs * 1_000_000.0)
    } else {
        0.0
    };

    let dl_rounded = (dl_mbps * 100.0).round() / 100.0;
    let ul_rounded = (ul_mbps * 100.0).round() / 100.0;

    crate::netqual::update_speed_metrics(dl_rounded, ul_rounded);

    Ok(format!(
        "Хурдны шалгалт (Cloudflare CDN Edge):\n  Татах хурд (Download): {dl_rounded} Mbps ({:.2}s)\n  Илгээх хурд (Upload): {ul_rounded} Mbps ({:.2}s)\n  Цаг: {}",
        dl_secs, ul_secs, chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ))
}
