//! `zev-detector parking-setup` — the installer's last step for the bundled
//! parking service (Dahua `dahua-service` or Hikvision `hikvision-service`).
//!
//!   zev-detector.exe parking-setup --kind dahua \
//!       --dir "C:\Program Files\Zev Detector\parking\dahua" --answers "%TEMP%\...\parking.txt"
//!
//! The answers file (written by the installer, deleted here as soon as it is
//! read because it holds the token and camera password) is `key=value` lines:
//!   token=…  barilgiinId=…  org_name=…  password=…  keep=1|0
//!   camera=192.168.1.108|entrance|dahua      (one line per camera)
//!
//! What it does, in order:
//!   1. If the service is already installed from ANOTHER folder (e.g. the old
//!      D:\zevtabs\dahuarust\…\target\release), stop and remove it — copying its
//!      config.toml over first when "keep existing settings" is ticked.
//!   2. Write config.toml (unless keeping an existing one). TOML is produced
//!      here, not in the installer, so it is valid and has no BOM (the
//!      Hikvision service does not strip one).
//!   3. Register the service (`<exe> install`, which also sets SCM restart
//!      actions) and start it.
//!   4. Add it to Zev Detector's own `[[services]]` so the detector watches and
//!      self-heals it.
//!
//! Photos are not affected by the move: both services store them under
//! D:\zevtabs\parking_images (or the ProgramData fallback), not next to the exe.

use crate::config::ServiceEntry;
use crate::services;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SERVER_URL: &str = "https://turees.zevtabs.mn/api/zogsoolSdkService";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Dahua,
    Hikvision,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().as_str() {
            "dahua" => Some(Kind::Dahua),
            "hikvision" | "hik" => Some(Kind::Hikvision),
            _ => None,
        }
    }
    pub fn service(self) -> &'static str {
        match self {
            Kind::Dahua => "DahuaParkingService",
            Kind::Hikvision => "zevHikvision",
        }
    }
    pub fn exe(self) -> &'static str {
        match self {
            Kind::Dahua => "dahua-service.exe",
            Kind::Hikvision => "hikvision-service.exe",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Dahua => "Dahua Parking (dahuarust)",
            Kind::Hikvision => "Hikvision Parking (hikRustExe)",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cam {
    pub ip: String,
    /// entrance | exit
    pub gate: String,
    /// dahua | hikvision | auto
    pub brand: String,
}

#[derive(Debug, Default, Clone)]
pub struct Answers {
    pub token: String,
    pub barilgiin_id: String,
    pub org_name: String,
    pub password: String,
    pub keep: bool,
    pub cameras: Vec<Cam>,
}

pub fn parse_answers(text: &str) -> Answers {
    let mut a = Answers::default();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "token" => a.token = v,
            "barilgiinId" => a.barilgiin_id = v,
            "org_name" => a.org_name = v,
            "password" => a.password = v,
            "keep" => a.keep = v == "1",
            "camera" => {
                let mut p = v.split('|').map(str::trim);
                let ip = p.next().unwrap_or("").to_string();
                if ip.parse::<std::net::Ipv4Addr>().is_err() {
                    continue;
                }
                let gate = match p.next().unwrap_or("") {
                    "exit" => "exit",
                    _ => "entrance",
                }
                .to_string();
                let brand = match p.next().unwrap_or("").to_ascii_lowercase().as_str() {
                    "dahua" => "dahua",
                    "hikvision" => "hikvision",
                    _ => "auto",
                }
                .to_string();
                a.cameras.push(Cam { ip, gate, brand });
            }
            _ => {}
        }
    }
    a
}

/// TOML basic string with escapes — the token and org name are user input.
fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn render_config(kind: Kind, a: &Answers) -> String {
    let mut t = String::new();
    t.push_str("# Zev Detector суулгагчаас үүсгэсэн тохиргоо\n");
    match kind {
        Kind::Dahua => {
            t.push_str("# Машины дугаарын зургийг энэ компьютер дээр хадгалах хоног\n");
            t.push_str("image_retention_days = 30\n");
            if a.cameras.is_empty() {
                // Required key; must sit above the first [table] to stay top-level.
                t.push_str("cameras = []\n");
            }
            t.push_str("\n[server]\n");
            t.push_str(&format!("url          = {}\n", q(SERVER_URL)));
            t.push_str(&format!("token        = {}\n", q(&a.token)));
            t.push_str(&format!("barilgiinId  = {}\n", q(&a.barilgiin_id)));
            t.push_str("timeout_secs = 30\nretry_count  = 3\n\n");
            t.push_str("[sdk]\n");
            t.push_str("username                = \"admin\"\n");
            t.push_str("heartbeat_interval_secs = 30\nconnect_timeout_ms      = 5000\nmax_connect_retries     = 5\nport                    = 37777\n");
            t.push_str(&format!("org_name                = {}\n", q(&a.org_name)));
            t.push_str("company_name            = \"ParkEase\"\n");
            for c in &a.cameras {
                t.push_str("\n[[cameras]]\n");
                t.push_str(&format!("ip          = {}\n", q(&c.ip)));
                t.push_str(&format!("password    = {}\n", q(&a.password)));
                t.push_str("http_port   = 80\n");
                t.push_str(&format!("gate        = {}\n", q(&c.gate)));
                t.push_str("sambar_type = \"sambar\"\n");
                t.push_str(&format!("brand       = {}\n", q(&c.brand)));
            }
        }
        Kind::Hikvision => {
            t.push_str("api_port     = 5000\n");
            t.push_str(&format!("barilgiin_id = {}\n", q(&a.barilgiin_id)));
            t.push_str(&format!("org_name     = {}\n", q(&a.org_name)));
            t.push_str("company_name = \"ParkEase\"\n");
            t.push_str("# Машины зургийг энэ компьютер дээр хадгалах хоног\n");
            t.push_str("image_retention_days = 30\n\n");
            t.push_str("[server]\n");
            t.push_str(&format!("url          = {}\n", q(SERVER_URL)));
            t.push_str(&format!("token        = {}\n", q(&a.token)));
            t.push_str("timeout_secs = 30\n");
            if a.cameras.is_empty() {
                // `cameras` is a required key; an empty array keeps the file valid.
                t = t.replacen("api_port", "cameras      = []\napi_port", 1);
            }
            for c in &a.cameras {
                t.push_str("\n[[cameras]]\n");
                t.push_str(&format!("role     = {}\n", q(&c.gate)));
                t.push_str(&format!("ip       = {}\n", q(&c.ip)));
                t.push_str(&format!("password = {}\n", q(&a.password)));
            }
        }
    }
    t
}

fn wait_stopped(name: &str, timeout: Duration) {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        match services::query(name) {
            Ok(o) if o.state == "Running" || o.state.contains("Pending") => {}
            _ => return,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn run(exe: &Path, args: &[&str]) -> Result<String, String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().map_err(|e| format!("{}: {e}", exe.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    n(a) == n(b)
}

/// Returns a line per action taken, for the installer log / console.
pub fn setup(kind: Kind, dir: &Path, a: &Answers) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let exe = dir.join(kind.exe());
    if !exe.exists() {
        return Err(format!("{} олдсонгүй", exe.display()));
    }
    let cfg_path = dir.join("config.toml");
    let svc = kind.service();

    // 1. Service registered from another folder → move it here.
    let mut registered_here = false;
    if let Ok(old_exe) = services::exe_path(svc) {
        let old_path = PathBuf::from(old_exe.to_string_lossy().trim_matches('"').to_string());
        let old_dir = old_path.parent().map(Path::to_path_buf).unwrap_or_default();
        if same_dir(&old_dir, dir) {
            registered_here = true;
        } else {
            log.push(format!("Хуучин {svc} ({}) зогсоож шилжүүлж байна", old_dir.display()));
            let _ = services::stop(svc);
            wait_stopped(svc, Duration::from_secs(20));
            let old_cfg = old_dir.join("config.toml");
            if a.keep && !cfg_path.exists() && old_cfg.exists() {
                std::fs::copy(&old_cfg, &cfg_path).map_err(|e| format!("Хуучин config.toml хуулж чадсангүй: {e}"))?;
                log.push(format!("Хуучин тохиргоог хуулав: {}", old_cfg.display()));
            }
            let sc = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
                .join("System32")
                .join("sc.exe");
            run(&sc, &["delete", svc]).map_err(|e| format!("Хуучин үйлчилгээг устгаж чадсангүй: {e}"))?;
        }
    }

    // 2. config.toml
    if a.keep && cfg_path.exists() {
        log.push("Одоогийн config.toml-г хэвээр үлдээв".into());
    } else {
        if a.token.trim().is_empty() || a.barilgiin_id.trim().is_empty() {
            return Err("Token болон барилгын ID шаардлагатай".into());
        }
        if cfg_path.exists() {
            let bak = dir.join("config.toml.bak");
            let _ = std::fs::copy(&cfg_path, &bak);
            log.push(format!("Өмнөх тохиргоог {} болгож хадгалав", bak.display()));
        }
        std::fs::write(&cfg_path, render_config(kind, a)).map_err(|e| format!("config.toml бичиж чадсангүй: {e}"))?;
        log.push(format!("config.toml бичив ({} камер)", a.cameras.len()));
    }

    // 3. Register + start. A just-deleted service can stay "marked for
    //    deletion" for a moment, so creation is retried briefly.
    if !registered_here {
        let mut last = String::new();
        let mut ok = false;
        for _ in 0..10 {
            match run(&exe, &["install"]) {
                Ok(_) => {
                    ok = true;
                    break;
                }
                Err(e) => {
                    last = e;
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        if !ok {
            return Err(format!("{svc} бүртгэж чадсангүй: {last}"));
        }
        log.push(format!("{svc} үйлчилгээг бүртгэв"));
    }
    match services::start(svc) {
        Ok(_) => log.push(format!("{svc} эхэллээ")),
        Err(e) => {
            let running = services::query(svc).map(|o| o.state == "Running").unwrap_or(false);
            if !running {
                log.push(format!("{svc} эхлүүлж чадсангүй: {e} (config.toml-г шалгана уу)"));
            }
        }
    }

    // 4. Let Zev Detector watch / self-heal it.
    match crate::setup::load_strict() {
        Ok(mut cfg) => {
            if !cfg.services.iter().any(|s| s.name.eq_ignore_ascii_case(svc)) {
                cfg.services.push(ServiceEntry {
                    name: svc.to_string(),
                    label: kind.label().to_string(),
                    ..Default::default()
                });
                if let Err(e) = cfg.save() {
                    log.push(format!("Detector тохиргоо хадгалж чадсангүй: {e}"));
                } else {
                    log.push("Zev Detector энэ үйлчилгээг хянана".into());
                }
            }
        }
        Err(e) => log.push(format!("Detector тохиргоо уншиж чадсангүй: {e}")),
    }

    Ok(log)
}

/// CLI: `parking-setup --kind dahua|hikvision --dir DIR --answers FILE`
pub fn run_cli(args: &[String]) -> Result<String, String> {
    let get = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let kind = get("--kind").and_then(|k| Kind::parse(&k)).ok_or("--kind dahua|hikvision шаардлагатай")?;
    let dir = PathBuf::from(get("--dir").ok_or("--dir шаардлагатай")?);
    let answers_path = get("--answers").ok_or("--answers шаардлагатай")?;
    let text = std::fs::read_to_string(&answers_path).map_err(|e| format!("{answers_path}: {e}"))?;
    // Holds the token and camera password — never leave it in %TEMP%.
    let _ = std::fs::remove_file(&answers_path);
    let a = parse_answers(&text);
    let result = setup(kind, &dir, &a).map(|lines| lines.join("\n"));
    // The installer runs this hidden, so it reads this file to show the outcome.
    let report = match &result {
        Ok(t) => format!("OK\n{t}\n"),
        Err(e) => format!("ERROR\n{e}\n"),
    };
    let _ = std::fs::write(dir.join("parking-setup.log"), report);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Answers {
        parse_answers(
            "\u{feff}token=abc\"def\nbarilgiinId=698402e26bb3e94b5665bfe5\norg_name=Улаанбаатар буян\npassword=p@ss\\1\nkeep=0\n\
             camera=192.168.1.195|entrance|Dahua\ncamera=192.168.1.196|exit|Unknown\ncamera=bad|exit|\n",
        )
    }

    #[test]
    fn parses_answers() {
        let a = sample();
        assert_eq!(a.token, "abc\"def");
        assert!(!a.keep);
        assert_eq!(a.cameras.len(), 2);
        assert_eq!(a.cameras[0], Cam { ip: "192.168.1.195".into(), gate: "entrance".into(), brand: "dahua".into() });
        assert_eq!(a.cameras[1].gate, "exit");
        assert_eq!(a.cameras[1].brand, "auto");
    }

    #[test]
    fn dahua_config_is_valid_toml_with_required_keys() {
        let v: toml::Value = toml::from_str(&render_config(Kind::Dahua, &sample())).unwrap();
        assert_eq!(v["server"]["token"].as_str(), Some("abc\"def"));
        assert_eq!(v["server"]["barilgiinId"].as_str(), Some("698402e26bb3e94b5665bfe5"));
        assert_eq!(v["server"]["retry_count"].as_integer(), Some(3));
        assert_eq!(v["sdk"]["org_name"].as_str(), Some("Улаанбаатар буян"));
        assert_eq!(v["sdk"]["port"].as_integer(), Some(37777));
        let cams = v["cameras"].as_array().unwrap();
        assert_eq!(cams.len(), 2);
        assert_eq!(cams[1]["gate"].as_str(), Some("exit"));
        assert_eq!(cams[0]["password"].as_str(), Some("p@ss\\1"));
    }

    #[test]
    fn hik_config_is_valid_toml_with_required_keys() {
        let v: toml::Value = toml::from_str(&render_config(Kind::Hikvision, &sample())).unwrap();
        assert_eq!(v["barilgiin_id"].as_str(), Some("698402e26bb3e94b5665bfe5"));
        assert_eq!(v["server"]["url"].as_str(), Some(SERVER_URL));
        let cams = v["cameras"].as_array().unwrap();
        assert_eq!(cams[0]["role"].as_str(), Some("entrance"));
        assert_eq!(cams[1]["role"].as_str(), Some("exit"));
    }

    #[test]
    fn empty_camera_list_still_parses() {
        let mut a = sample();
        a.cameras.clear();
        for k in [Kind::Dahua, Kind::Hikvision] {
            let v: toml::Value = toml::from_str(&render_config(k, &a)).unwrap();
            assert_eq!(v["cameras"].as_array().map(|c| c.len()), Some(0), "{k:?}");
        }
    }

    #[test]
    fn same_dir_ignores_case_and_slash() {
        assert!(same_dir(Path::new("C:\\Program Files\\Zev Detector\\parking\\dahua\\"), Path::new("c:\\program files\\zev detector\\parking\\dahua")));
    }
}
