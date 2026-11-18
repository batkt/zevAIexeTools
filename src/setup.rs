//! `zev-detector setup` — what the installer calls after copying files:
//! writes the PC name and the cameras the installer user picked into
//! config.toml, so nobody has to open the app to name the machine.
//!
//!   zev-detector.exe setup --name "Omnitriks" \
//!       --camera "192.168.1.108|80|Dahua|ITC413" --camera "192.168.1.109|554|Hikvision|"
//!
//! Cameras become `[[ports]]` entries with the camera as `host`, so the existing
//! port watcher reports a camera that drops off the network and the admin's
//! detector page shows it — no new monitoring code, no new heartbeat field.

use crate::config::{Config, PortEntry};

/// Label prefix that marks a port entry as installer-managed, so re-running
/// setup replaces the camera list instead of piling up stale cameras.
pub const CAMERA_LABEL_PREFIX: &str = "Камер:";

#[derive(Debug, Clone, PartialEq)]
pub struct CameraChoice {
    pub ip: String,
    pub port: u16,
    pub vendor: String,
    pub model: String,
}

/// Parses `ip|port|vendor|model` (everything after the IP is optional).
pub fn parse_camera(spec: &str) -> Result<CameraChoice, String> {
    let mut parts = spec.split('|').map(str::trim);
    let ip = parts.next().unwrap_or("");
    if ip.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(format!("Буруу IP хаяг: '{ip}'"));
    }
    let port = match parts.next() {
        Some(p) if !p.is_empty() => p.parse::<u16>().map_err(|_| format!("Буруу порт: '{p}'"))?,
        _ => 80,
    };
    Ok(CameraChoice {
        ip: ip.to_string(),
        port,
        vendor: parts.next().unwrap_or("").to_string(),
        model: parts.next().unwrap_or("").to_string(),
    })
}

fn camera_label(c: &CameraChoice) -> String {
    let what = [c.vendor.as_str(), c.model.as_str()]
        .iter()
        .filter(|s| !s.is_empty() && **s != "Unknown")
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if what.is_empty() {
        format!("{CAMERA_LABEL_PREFIX} {}", c.ip)
    } else {
        format!("{CAMERA_LABEL_PREFIX} {what} {}", c.ip)
    }
}

/// Applies name / cameras to `cfg`. `cameras: None` leaves the camera list alone.
pub fn apply(cfg: &mut Config, name: Option<&str>, cameras: Option<&[CameraChoice]>) {
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        cfg.agent.name = n.to_string();
    }
    if let Some(cams) = cameras {
        cfg.ports.retain(|p| !p.label.starts_with(CAMERA_LABEL_PREFIX));
        for c in cams {
            if cfg.ports.iter().any(|p| p.host == c.ip && p.port == c.port) {
                continue;
            }
            cfg.ports.push(PortEntry {
                port: c.port,
                label: camera_label(c),
                host: c.ip.clone(),
                must_listen: true,
            });
        }
    }
}

/// Loads config.toml strictly — a file that exists but does not parse is an
/// error, never silently replaced with defaults (that would wipe a site's
/// hand-tuned config during an upgrade).
pub fn load_strict() -> anyhow::Result<Config> {
    let path = Config::config_path();
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;
        toml::from_str::<Config>(&text)
            .map_err(|e| anyhow::anyhow!("{} is not valid TOML, not touching it: {e}", path.display()))?
    } else {
        Config::default_builtin()
    };
    cfg.apply_defaults();
    Ok(cfg)
}

/// CLI entry: `setup [--name N] [--camera SPEC]... [--clear-cameras]`.
pub fn run_cli(args: &[String]) -> Result<String, String> {
    let mut name: Option<String> = None;
    let mut cams: Vec<CameraChoice> = Vec::new();
    let mut touch_cams = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                name = args.get(i + 1).cloned();
                i += 1;
            }
            "--camera" => {
                let spec = args.get(i + 1).ok_or("--camera утга алга")?;
                cams.push(parse_camera(spec)?);
                touch_cams = true;
                i += 1;
            }
            "--clear-cameras" => touch_cams = true,
            other => return Err(format!("Үл мэдэгдэх параметр: {other}")),
        }
        i += 1;
    }

    let mut cfg = load_strict().map_err(|e| e.to_string())?;
    apply(&mut cfg, name.as_deref(), if touch_cams { Some(&cams) } else { None });
    cfg.save().map_err(|e| e.to_string())?;
    Ok(format!(
        "Хадгаллаа: {} — нэр '{}', {} камер",
        Config::config_path().display(),
        cfg.agent.name,
        cfg.ports.iter().filter(|p| p.label.starts_with(CAMERA_LABEL_PREFIX)).count()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        assert_eq!(
            parse_camera("192.168.1.108|37777|Dahua|ITC413").unwrap(),
            CameraChoice { ip: "192.168.1.108".into(), port: 37777, vendor: "Dahua".into(), model: "ITC413".into() }
        );
        assert_eq!(parse_camera("10.0.0.5").unwrap().port, 80);
        assert!(parse_camera("not-an-ip|80").is_err());
        assert!(parse_camera("10.0.0.5|99999").is_err());
    }

    #[test]
    fn replaces_only_installer_cameras() {
        let mut cfg = Config::default_builtin();
        cfg.ports = vec![PortEntry { port: 37777, label: "Dahua SDK".into(), host: "127.0.0.1".into(), must_listen: true }];
        let first = [parse_camera("192.168.1.108|80|Dahua|").unwrap()];
        apply(&mut cfg, Some(" Omnitriks "), Some(&first));
        assert_eq!(cfg.agent.name, "Omnitriks");
        assert_eq!(cfg.ports.len(), 2);
        assert_eq!(cfg.ports[1].label, "Камер: Dahua 192.168.1.108");

        let second = [parse_camera("192.168.1.109|554|Unknown|").unwrap()];
        apply(&mut cfg, None, Some(&second));
        assert_eq!(cfg.agent.name, "Omnitriks");
        assert_eq!(cfg.ports.len(), 2);
        assert_eq!(cfg.ports[0].label, "Dahua SDK");
        assert_eq!(cfg.ports[1].host, "192.168.1.109");
        assert_eq!(cfg.ports[1].label, "Камер: 192.168.1.109");

        apply(&mut cfg, None, None);
        assert_eq!(cfg.ports.len(), 2);
    }
}
