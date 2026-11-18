//! Finds IP cameras on the parking PC's local network(s).
//!
//! Used by the installer ("pick your camera") and `zev-detector discover`.
//! Several methods run at the same time, because no single one finds
//! everything on the brands we deploy:
//!
//!   * Dahua `DHDiscover.search` (UDP 37810) — Dahua and its OEM LPR cameras
//!   * Hikvision SADP (UDP 37020)            — Hikvision cameras
//!   * ONVIF WS-Discovery (UDP 3702)         — most other brands (ZKTeco,
//!                                             Tiger Wong, …) when ONVIF is on
//!   * TCP scan of each local /24 on camera ports (554 RTSP, 37777 Dahua SDK,
//!     8000 Hikvision SDK, 4370 ZKTeco) — catches cameras whose discovery is
//!     disabled or blocked by the firewall
//!
//! Each hit is then fingerprinted from its open ports and web page to name the
//! vendor. Nothing is ever logged in to — discovery only reads what devices
//! announce publicly. Total run time is a few seconds.

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Camera {
    pub ip: String,
    /// Dahua | Hikvision | ZKTeco | TigerWong | ONVIF | Unknown
    pub vendor: String,
    pub model: String,
    pub mac: String,
    pub ports: Vec<u16>,
    /// Which methods saw it: dahua, sadp, onvif, scan
    pub sources: Vec<String>,
}

impl Camera {
    /// The port worth watching for "is this camera reachable".
    pub fn monitor_port(&self) -> u16 {
        for p in [80, 554, 37777, 8000, 4370] {
            if self.ports.contains(&p) {
                return p;
            }
        }
        80
    }
}

/// Ports that, when open, make a host a camera candidate. 80 alone is not
/// enough — every router and printer has it.
const CAMERA_PORTS: [u16; 4] = [554, 37777, 8000, 4370];
const SCAN_PORTS: [u16; 5] = [554, 80, 37777, 8000, 4370];
const PROBE_WINDOW: Duration = Duration::from_millis(3500);

#[derive(Default)]
struct Hit {
    vendor: String,
    model: String,
    mac: String,
    ports: Vec<u16>,
    sources: Vec<String>,
}

type Found = Arc<Mutex<BTreeMap<Ipv4Addr, Hit>>>;

fn record(found: &Found, ip: Ipv4Addr, source: &str, vendor: &str, model: &str, mac: &str, ports: &[u16]) {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_broadcast() {
        return;
    }
    let mut map = found.lock().unwrap();
    let h = map.entry(ip).or_default();
    if !h.sources.iter().any(|s| s == source) {
        h.sources.push(source.to_string());
    }
    if h.vendor.is_empty() && !vendor.is_empty() {
        h.vendor = vendor.to_string();
    }
    if h.model.is_empty() && !model.trim().is_empty() {
        h.model = model.trim().to_string();
    }
    if h.mac.is_empty() && !mac.trim().is_empty() {
        h.mac = mac.trim().to_uppercase().replace('-', ":");
    }
    for p in ports {
        if !h.ports.contains(p) {
            h.ports.push(*p);
        }
    }
}

/// Private IPv4 addresses of this machine's adapters.
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut v: Vec<Ipv4Addr> = Vec::new();

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(out) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-NetIPAddress -AddressFamily IPv4 | Select-Object -ExpandProperty IPAddress",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Ok(ip) = line.trim().parse::<Ipv4Addr>() {
                    v.push(ip);
                }
            }
        }
    }

    // Fallback: the address the default route would use.
    if v.is_empty() {
        if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
            if s.connect("8.8.8.8:80").is_ok() {
                if let Ok(SocketAddr::V4(a)) = s.local_addr() {
                    v.push(*a.ip());
                }
            }
        }
    }

    v.retain(|ip| ip.is_private());
    v.sort();
    v.dedup();
    v
}

fn uuid_like() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let h = format!("{:032x}", n ^ 0x5a17_c0de_9e37_79b9_7f4a_7c15_f39c_c060u128);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Sends `probe` from every local interface to `group:port`, and listens both
/// on the sending sockets (unicast replies) and on `port` itself (devices that
/// answer to the multicast group, as Dahua and Hikvision do).
fn multicast_probe(locals: &[Ipv4Addr], group: Ipv4Addr, port: u16, probe: &[u8], mut on_reply: impl FnMut(Ipv4Addr, &[u8])) {
    let mut socks: Vec<UdpSocket> = Vec::new();

    if let Ok(listener) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)) {
        for l in locals {
            let _ = listener.join_multicast_v4(&group, l);
        }
        socks.push(listener);
    }

    for l in locals {
        let Ok(s) = UdpSocket::bind(SocketAddrV4::new(*l, 0)) else { continue };
        let _ = s.set_multicast_ttl_v4(2);
        let _ = s.set_broadcast(true);
        let _ = s.send_to(probe, SocketAddrV4::new(group, port));
        // Some firmware only answers broadcast, not the multicast group.
        let o = l.octets();
        let _ = s.send_to(probe, SocketAddrV4::new(Ipv4Addr::new(o[0], o[1], o[2], 255), port));
        socks.push(s);
    }

    for s in &socks {
        let _ = s.set_read_timeout(Some(Duration::from_millis(60)));
    }

    let deadline = Instant::now() + PROBE_WINDOW;
    let mut buf = vec![0u8; 16 * 1024];
    while Instant::now() < deadline {
        for s in &socks {
            if let Ok((n, SocketAddr::V4(from))) = s.recv_from(&mut buf) {
                // Our own probe echoed back by the multicast loopback.
                if n == probe.len() && &buf[..n] == probe {
                    continue;
                }
                on_reply(*from.ip(), &buf[..n]);
            }
        }
    }
}

fn xml_tag(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    text.find(&open)
        .and_then(|a| {
            let start = a + open.len();
            text[start..].find(&close).map(|b| text[start..start + b].trim().to_string())
        })
        .unwrap_or_default()
}

fn dahua(locals: &[Ipv4Addr], found: &Found) {
    let body = br#"{"method":"DHDiscover.search","params":{"mac":"","uni":1}}"#;
    let len = (body.len() as u32).to_le_bytes();
    let mut pkt: Vec<u8> = vec![0x20, 0, 0, 0];
    pkt.extend_from_slice(b"DHIP");
    pkt.extend_from_slice(&[0; 8]); // session id, request id
    pkt.extend_from_slice(&len);
    pkt.extend_from_slice(&[0; 4]);
    pkt.extend_from_slice(&len);
    pkt.extend_from_slice(&[0; 4]);
    pkt.extend_from_slice(body);

    multicast_probe(locals, Ipv4Addr::new(239, 255, 255, 251), 37810, &pkt, |from, data| {
        let Some(start) = data.iter().position(|&b| b == b'{') else { return };
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(trim_nul(&data[start..])) else { return };
        if json.get("method").and_then(|m| m.as_str()) == Some("DHDiscover.search") && json.get("params").map_or(false, |p| p.get("deviceInfo").is_none()) {
            return; // another probe, not a device
        }
        let info = json.pointer("/params/deviceInfo").unwrap_or(&json);
        let ip = info
            .pointer("/IPv4Address/IPAddress")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
            .unwrap_or(from);
        let s = |k: &str| info.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let vendor_raw = s("Vendor");
        let vendor = vendor_name(&vendor_raw).unwrap_or("Dahua");
        let model = [s("DeviceType"), s("DeviceClass")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
        record(found, ip, "dahua", vendor, &model, &s("mac"), &[]);
    });
}

fn trim_nul(b: &[u8]) -> &[u8] {
    let end = b.iter().rposition(|&c| c == b'}').map(|i| i + 1).unwrap_or(b.len());
    &b[..end]
}

fn sadp(locals: &[Ipv4Addr], found: &Found) {
    let probe = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Probe><Uuid>{}</Uuid><Types>inquiry</Types></Probe>",
        uuid_like().to_uppercase()
    );
    multicast_probe(locals, Ipv4Addr::new(239, 255, 255, 250), 37020, probe.as_bytes(), |from, data| {
        let text = String::from_utf8_lossy(data);
        if !text.contains("<ProbeMatch") {
            return;
        }
        let ip = xml_tag(&text, "IPv4Address").parse::<Ipv4Addr>().unwrap_or(from);
        let model = [xml_tag(&text, "DeviceDescription"), xml_tag(&text, "DeviceType")]
            .into_iter()
            .find(|x| !x.is_empty())
            .unwrap_or_default();
        let mut ports = Vec::new();
        if let Ok(p) = xml_tag(&text, "CommandPort").parse::<u16>() {
            ports.push(p);
        }
        record(found, ip, "sadp", "Hikvision", &model, &xml_tag(&text, "MAC"), &ports);
    });
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn onvif(locals: &[Ipv4Addr], found: &Found) {
    let probe = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><e:Header><w:MessageID>uuid:{}</w:MessageID><w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action e:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header><e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>"#,
        uuid_like()
    );
    let scope_re = regex::Regex::new(r"onvif://www\.onvif\.org/(name|hardware|mfr|manufacturer|MAC)/([^\s<]+)").unwrap();
    let xaddr_re = regex::Regex::new(r"https?://(\d+\.\d+\.\d+\.\d+)").unwrap();

    // WS-Discovery replies are unicast to the sender, so no fixed-port listener
    // is needed — but binding 3702 is harmless and catches multicast replies.
    multicast_probe(locals, Ipv4Addr::new(239, 255, 255, 250), 3702, probe.as_bytes(), |from, data| {
        let text = String::from_utf8_lossy(data);
        if !text.contains("ProbeMatch") {
            return;
        }
        let ip = xaddr_re
            .captures(&text)
            .and_then(|c| c[1].parse::<Ipv4Addr>().ok())
            .unwrap_or(from);
        let (mut name, mut hardware, mut mfr, mut mac) = (String::new(), String::new(), String::new(), String::new());
        for c in scope_re.captures_iter(&text) {
            let v = url_decode(&c[2]);
            match &c[1] {
                "name" => name = v,
                "hardware" => hardware = v,
                "MAC" => mac = v,
                _ => mfr = v,
            }
        }
        let vendor = vendor_name(&format!("{mfr} {name} {hardware}")).unwrap_or("ONVIF");
        let model = if hardware.is_empty() { name } else { hardware };
        record(found, ip, "onvif", vendor, &model, &mac, &[]);
    });
}

fn vendor_name(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    if t.contains("dahua") || t.contains("dh-") || t.contains("dh_") {
        Some("Dahua")
    } else if t.contains("hikvision") || t.contains("hik-") || t.contains("hikvision") || t.contains("ds-2") {
        Some("Hikvision")
    } else if t.contains("zkteco") || t.contains("zk teco") || t.contains("zkbio") {
        Some("ZKTeco")
    } else if t.contains("tigerwong") || t.contains("tiger wong") || t.contains("tiger-wong") || t.contains("tigerw") {
        Some("TigerWong")
    } else {
        None
    }
}

fn port_scan(locals: &[Ipv4Addr], found: &Found) {
    let mut targets: Vec<Ipv4Addr> = Vec::new();
    for l in locals {
        let o = l.octets();
        for h in 1..=254u8 {
            let ip = Ipv4Addr::new(o[0], o[1], o[2], h);
            if !locals.contains(&ip) {
                targets.push(ip);
            }
        }
    }
    targets.sort();
    targets.dedup();

    let queue = Arc::new(Mutex::new(targets));
    let workers: Vec<_> = (0..96)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let found = Arc::clone(found);
            std::thread::spawn(move || loop {
                let Some(ip) = queue.lock().unwrap().pop() else { break };
                let open: Vec<u16> = SCAN_PORTS
                    .iter()
                    .copied()
                    .filter(|p| tcp_open(ip, *p, Duration::from_millis(300)))
                    .collect();
                if open.iter().any(|p| CAMERA_PORTS.contains(p)) {
                    let vendor = if open.contains(&37777) {
                        "Dahua"
                    } else if open.contains(&8000) {
                        "Hikvision"
                    } else if open.contains(&4370) {
                        "ZKTeco"
                    } else {
                        ""
                    };
                    record(&found, ip, "scan", vendor, "", "", &open);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

fn tcp_open(ip: Ipv4Addr, port: u16, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&SocketAddr::new(ip.into(), port), timeout).is_ok()
}

/// First few KB of `GET /` — headers and title are enough to name the vendor.
fn http_banner(ip: Ipv4Addr) -> String {
    let Ok(mut s) = TcpStream::connect_timeout(&SocketAddr::new(ip.into(), 80), Duration::from_millis(800)) else {
        return String::new();
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(800)));
    let req = format!("GET / HTTP/1.0\r\nHost: {ip}\r\nUser-Agent: zev-detector\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return String::new();
    }
    let mut buf = vec![0u8; 8192];
    let mut got = 0;
    while got < buf.len() {
        match s.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    String::from_utf8_lossy(&buf[..got]).to_string()
}

/// Runs every method in parallel and returns the cameras found, sorted by IP.
pub fn discover() -> Vec<Camera> {
    let locals = local_ipv4s();
    let found: Found = Arc::new(Mutex::new(BTreeMap::new()));
    if locals.is_empty() {
        return Vec::new();
    }

    let jobs: Vec<std::thread::JoinHandle<()>> = vec![
        {
            let (l, f) = (locals.clone(), Arc::clone(&found));
            std::thread::spawn(move || dahua(&l, &f))
        },
        {
            let (l, f) = (locals.clone(), Arc::clone(&found));
            std::thread::spawn(move || sadp(&l, &f))
        },
        {
            let (l, f) = (locals.clone(), Arc::clone(&found));
            std::thread::spawn(move || onvif(&l, &f))
        },
        {
            let (l, f) = (locals.clone(), Arc::clone(&found));
            std::thread::spawn(move || port_scan(&l, &f))
        },
    ];
    for j in jobs {
        let _ = j.join();
    }

    // Our own adapters can answer our own probes (e.g. a PC running ONVIF NVR software).
    let hits: Vec<(Ipv4Addr, Hit)> = std::mem::take(&mut *found.lock().unwrap())
        .into_iter()
        .filter(|(ip, _)| !locals.contains(ip))
        .collect();

    // Fingerprint in parallel: fill in ports for multicast-only hits, then the
    // web banner decides the vendor where nothing else did.
    let handles: Vec<_> = hits
        .into_iter()
        .map(|(ip, mut h)| {
            std::thread::spawn(move || {
                if !h.sources.iter().any(|s| s == "scan") {
                    for p in SCAN_PORTS {
                        if !h.ports.contains(&p) && tcp_open(ip, p, Duration::from_millis(400)) {
                            h.ports.push(p);
                        }
                    }
                }
                let generic = h.vendor.is_empty() || h.vendor == "ONVIF";
                if generic && h.ports.contains(&80) {
                    if let Some(v) = vendor_name(&http_banner(ip)) {
                        h.vendor = v.to_string();
                    }
                }
                if h.vendor.is_empty() {
                    h.vendor = "Unknown".to_string();
                }
                h.ports.sort();
                Camera {
                    ip: ip.to_string(),
                    vendor: h.vendor,
                    model: h.model,
                    mac: h.mac,
                    ports: h.ports,
                    sources: h.sources,
                }
            })
        })
        .collect();

    let mut cams: Vec<Camera> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    cams.sort_by_key(|c| c.ip.parse::<Ipv4Addr>().map(u32::from).unwrap_or(0));
    cams
}

/// One line per camera, tab-separated and ASCII-only, for the installer
/// (Inno Setup reads files in the ANSI code page and has no JSON parser):
/// `ip<TAB>vendor<TAB>model<TAB>mac<TAB>monitor_port<TAB>ports`
pub fn to_tsv(cams: &[Camera]) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { ' ' })
            .collect::<String>()
            .trim()
            .chars()
            .take(48)
            .collect()
    };
    cams.iter()
        .map(|c| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\r\n",
                c.ip,
                clean(&c.vendor),
                clean(&c.model),
                clean(&c.mac),
                c.monitor_port(),
                c.ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_names() {
        assert_eq!(vendor_name("Dahua ITC413"), Some("Dahua"));
        assert_eq!(vendor_name("HIKVISION DS-2CD"), Some("Hikvision"));
        assert_eq!(vendor_name("ZKTeco LPR"), Some("ZKTeco"));
        assert_eq!(vendor_name("TigerWong TW-LPR"), Some("TigerWong"));
        assert_eq!(vendor_name("Generic"), None);
    }

    #[test]
    fn xml_and_url() {
        assert_eq!(xml_tag("<a><IPv4Address>192.168.1.64</IPv4Address></a>", "IPv4Address"), "192.168.1.64");
        assert_eq!(url_decode("IPC%20Camera"), "IPC Camera");
    }

    #[test]
    fn monitor_port_prefers_http() {
        let c = Camera { ports: vec![554, 37777], ..Default::default() };
        assert_eq!(c.monitor_port(), 554);
        let c = Camera { ports: vec![80, 554], ..Default::default() };
        assert_eq!(c.monitor_port(), 80);
    }

    #[test]
    fn tsv_is_ascii_and_tabbed() {
        let c = Camera {
            ip: "192.168.1.108".into(),
            vendor: "Dahua".into(),
            model: "ITC\t413 камер".into(),
            ports: vec![80, 37777],
            ..Default::default()
        };
        let line = to_tsv(&[c]);
        assert!(line.is_ascii());
        assert_eq!(line.trim_end().split('\t').count(), 6);
        assert!(line.starts_with("192.168.1.108\tDahua\tITC 413\t\t80\t80,37777"));
    }
}
