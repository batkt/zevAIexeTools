//! `zev-detector-ui.exe` — the desktop app.
//!
//! A thin Tauri shell over the same library the service uses. The window is
//! HTML/CSS, everything privileged happens here in Rust: reading and writing
//! config.toml, asking the SCM for the service state, and shelling out to
//! `zev-detector.exe` elevated for install / start / stop / uninstall.
//!
//! Nothing in this binary ever runs the detector loop — that is the service's
//! job. The one exception is `run_test`, which does a single collection pass
//! and uploads nothing, so a technician can see what a new machine produces
//! before turning the service on.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use zev_detector::config::{self, Config};
use zev_detector::{elevate, runner, sender, winsvc, SvcStatus};

/// Everything the window needs to describe this machine, gathered once at
/// load so the frontend does not have to make five separate calls.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentInfo {
    version: String,
    hostname: String,
    os: String,
    config_path: String,
    log_path: String,
    /// False on the very first launch — the frontend shows the setup wizard
    /// instead of the settings screen.
    config_exists: bool,
}

#[tauri::command]
fn agent_info() -> AgentInfo {
    AgentInfo {
        version: sender::AGENT_VERSION.to_string(),
        hostname: config::hostname(),
        os: sender::os_name(),
        config_path: Config::config_path().display().to_string(),
        log_path: runner::log_path().display().to_string(),
        config_exists: Config::config_path().exists(),
    }
}

#[tauri::command]
fn load_config() -> Config {
    let mut cfg = Config::load_or_default();
    if cfg.server.url.trim().is_empty() {
        cfg.server.url = config::d_server_url();
    }
    // A blank name means "use this machine's hostname" to the service; show
    // the resolved value rather than an empty box the user has to guess at.
    if cfg.agent.name.is_empty() {
        cfg.agent.name = config::hostname();
    }
    cfg
}

#[tauri::command]
fn save_config(mut cfg: Config) -> Result<(), String> {
    if cfg.server.url.trim().is_empty() {
        cfg.server.url = config::d_server_url();
    }
    cfg.server.url = cfg.server.url.trim().trim_end_matches('/').to_string();
    if cfg.agent.name.trim().is_empty() {
        cfg.agent.name = config::hostname();
    }
    for s in &mut cfg.services {
        if s.label.trim().is_empty() {
            s.label = s.name.clone();
        }
    }
    cfg.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn service_status() -> SvcStatus {
    winsvc::query()
}

/// Path to the agent binary that sits next to this app.
fn agent_exe() -> PathBuf {
    Config::exe_dir().join(if cfg!(windows) { "zev-detector.exe" } else { "zev-detector" })
}

#[tauri::command]
async fn service_action(action: String) -> Result<String, String> {
    let verb = match action.as_str() {
        "install" | "uninstall" | "start" | "stop" => action.clone(),
        other => return Err(format!("Unknown service action: {other}")),
    };

    // ShellExecuteEx blocks until the elevated child exits, so keep it off
    // the thread that is drawing the window.
    let result = tauri::async_runtime::spawn_blocking(move || {
        elevate::run_elevated(&agent_exe(), &verb).map(|code| (verb, code))
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Ok((verb, 0)) => Ok(match verb.as_str() {
            "install" => "Service installed.".into(),
            "uninstall" => "Service removed.".into(),
            "start" => "Service started.".into(),
            _ => "Service stopped.".into(),
        }),
        Ok((verb, code)) => Err(format!("`{verb}` failed (exit code {code}). See the log for details.")),
        Err(e) => Err(e.to_string()),
    }
}

/// One collection pass against the config currently on disk. Uploads nothing.
#[tauri::command]
async fn run_test() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let cfg = Config::load().map_err(|e| e.to_string())?;
        Ok(runner::collect_test_report(&cfg))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pick_file() -> Option<String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("Log files", &["log", "txt"])
            .add_filter("All files", &["*"])
            .pick_file()
            .map(|p| p.display().to_string())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
fn read_log(lines: usize) -> String {
    let path = runner::log_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            let start = all.len().saturating_sub(lines);
            all[start..].join("\n")
        }
        Err(_) => format!("No log yet at {}", path.display()),
    }
}

#[tauri::command]
fn get_active_alerts() -> Vec<zev_detector::notify::ActiveAlert> {
    let state_file = Config::data_dir().join("active_alerts.json");
    if let Ok(text) = std::fs::read_to_string(&state_file) {
        if let Ok(snapshot) = serde_json::from_str::<zev_detector::notify::AlertSnapshot>(&text) {
            return snapshot.active_alerts;
        }
    }
    Vec::new()
}

#[tauri::command]
async fn dahua_status() -> zev_detector::dahua_monitor::DahuaStatusSnapshot {
    tauri::async_runtime::spawn_blocking(zev_detector::dahua_monitor::get_dahua_status)
        .await
        .unwrap_or_else(|_| zev_detector::dahua_monitor::DahuaStatusSnapshot::default())
}

#[tauri::command]
async fn dahua_action(action: String) -> Result<String, String> {
    let verb = match action.as_str() {
        "start" => "dahua-start",
        "stop" => "dahua-stop",
        "restart" => "dahua-restart",
        other => return Err(format!("Unknown action: {other}")),
    };

    let result = tauri::async_runtime::spawn_blocking(move || {
        elevate::run_elevated(&agent_exe(), verb).map(|code| (verb, code))
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Ok((verb, 0)) => Ok(format!("Action `{verb}` completed.")),
        Ok((verb, code)) => Err(format!("Action `{verb}` failed (exit code {code})")),
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledServiceInfo {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub is_parking_related: bool,
}

#[tauri::command]
async fn list_installed_services() -> Vec<InstalledServiceInfo> {
    tauri::async_runtime::spawn_blocking(|| {
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-Service | Select-Object -Property Name, DisplayName, Status | ConvertTo-Json -Compress",
            ])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                #[derive(serde::Deserialize)]
                struct RawSvc {
                    #[serde(rename = "Name")]
                    name: Option<String>,
                    #[serde(rename = "DisplayName")]
                    display_name: Option<String>,
                    #[serde(rename = "Status")]
                    status: Option<serde_json::Value>,
                }

                if let Ok(list) = serde_json::from_slice::<Vec<RawSvc>>(&out.stdout) {
                    let mut result: Vec<InstalledServiceInfo> = list
                        .into_iter()
                        .filter_map(|s| {
                            let name = s.name?;
                            let display_name = s.display_name.unwrap_or_else(|| name.clone());
                            let status = match s.status {
                                Some(serde_json::Value::Number(n)) => {
                                    if n.as_i64() == Some(4) {
                                        "Running".to_string()
                                    } else {
                                        "Stopped".to_string()
                                    }
                                }
                                Some(serde_json::Value::String(s)) => s,
                                _ => "Unknown".to_string(),
                            };
                            let lower_name = name.to_lowercase();
                            let lower_disp = display_name.to_lowercase();
                            let is_parking_related = lower_name.contains("dahua")
                                || lower_name.contains("parking")
                                || lower_name.contains("zev")
                                || lower_name.contains("camera")
                                || lower_disp.contains("dahua")
                                || lower_disp.contains("parking")
                                || lower_disp.contains("zev")
                                || lower_disp.contains("camera");

                            Some(InstalledServiceInfo {
                                name,
                                display_name,
                                status,
                                is_parking_related,
                            })
                        })
                        .collect();

                    // Sort: parking related first, then alphabetically by display_name
                    result.sort_by(|a, b| {
                        match (b.is_parking_related, a.is_parking_related) {
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            _ => a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()),
                        }
                    });

                    return result;
                }
            }
        }
        Vec::new()
    })
    .await
    .unwrap_or_default()
}

/// Brings the main window back from the tray.
fn show_main_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{TrayIconBuilder, TrayIconEvent};
    use tauri::WindowEvent;

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            agent_info,
            load_config,
            save_config,
            service_status,
            service_action,
            run_test,
            pick_file,
            read_log,
            get_active_alerts,
            dahua_status,
            dahua_action,
            list_installed_services,
        ])
        .setup(|app| {
            let open = MenuItem::with_id(app, "open", "Нээх", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Гарах", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Zev Detector")
                // Left click belongs to "open the window"; the menu is on right
                // click, which is what a Windows tray icon is expected to do.
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        show_main_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it instead of quitting. The detector
            // itself is the Windows service and is unaffected either way, but
            // people expect the tray icon to stay put — and reopening from the
            // tray is instant, where relaunching the exe is not.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Zev Detector");
}
