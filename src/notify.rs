//! Local on-screen and audio alerting module for Zev Detector.
//!
//! Provides multi-channel local alerting on the machine itself:
//! 1. `WTSSendMessageW` — native Win32 message box delivered from the Session 0
//!    service straight to the active console user's desktop with timeout.
//! 2. `MessageBeep` — audible hardware warning chime for kiosk / booth attendants.
//! 3. Windows Action Center toast notifications via background invocation.
//! 4. Shared `active_alerts.json` state file for the desktop UI to render live
//!    warning banners, diagnostic breakdowns, and modals.
//! 5. Automatic debouncing and resolution announcements when conditions recover.

use chrono::{DateTime, Utc};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    NetworkDown,
    NetworkDegraded,
    CameraPortClosed,
    DiskNearFull,
    CpuPegged,
    ServiceDown,
    HealLimitReached,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAlert {
    pub id: String,
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub advice: String,
    pub first_seen: String,
    pub last_notified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlertSnapshot {
    pub active_alerts: Vec<ActiveAlert>,
    pub updated_at: String,
}

pub struct LocalNotifier {
    enabled: bool,
    popup_enabled: bool,
    toast_enabled: bool,
    sound_enabled: bool,
    netqual_threshold: u8,
    disk_threshold: f32,
    cooldown: Duration,
    state_file: PathBuf,
    active: HashMap<AlertType, (ActiveAlert, Instant)>,
}

impl LocalNotifier {
    pub fn new(cfg: &crate::config::NotifyConfig, state_dir: &Path) -> Self {
        Self {
            enabled: cfg.enabled,
            popup_enabled: cfg.popup_enabled,
            toast_enabled: cfg.toast_enabled,
            sound_enabled: cfg.sound_enabled,
            netqual_threshold: cfg.netqual_threshold,
            disk_threshold: cfg.disk_threshold,
            cooldown: Duration::from_secs(cfg.cooldown_mins.max(1) * 60),
            state_file: state_dir.join("active_alerts.json"),
            active: HashMap::new(),
        }
    }

    /// Evaluates current metrics and triggers/updates/clears local alerts.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        net_score: Option<u8>,
        net_latency_ms: Option<u64>,
        net_loss_pct: Option<f32>,
        worst_disk_pct: f32,
        worst_disk_mount: &str,
        worst_disk_free_gb: f32,
        closed_ports: &[(u16, String)],
        healer_limit_hit: Option<&str>,
    ) {
        if !self.enabled {
            return;
        }

        let now_instant = Instant::now();
        let now_utc = Utc::now();

        // 1. Network checks
        if let Some(score) = net_score {
            let latency = net_latency_ms.unwrap_or(0);
            let loss = net_loss_pct.unwrap_or(0.0);

            if score == 0 || loss >= 99.0 {
                self.raise(
                    AlertType::NetworkDown,
                    AlertSeverity::Critical,
                    "🚨 Интернет тасарсан байна".to_string(),
                    format!("Интернэт бүрэн тасарсан байна."),
                    "Интернэт холболтыг шалгана уу.".to_string(),
                    now_instant,
                    now_utc,
                );
                self.clear(&AlertType::NetworkDegraded, now_utc);
            } else if score < self.netqual_threshold || loss > 0.0 {
                self.raise(
                    AlertType::NetworkDegraded,
                    AlertSeverity::Warning,
                    "💡 Дотоод сүлжээ тасарсан байна.".to_string(),
                    format!("Зогсоолын программ доголдож болзошгүй."),
                    "Интернэт холболты шалгана уу.".to_string(),
                    now_instant,
                    now_utc,
                );
                self.clear(&AlertType::NetworkDown, now_utc);
            } else {
                self.clear(&AlertType::NetworkDown, now_utc);
                self.clear(&AlertType::NetworkDegraded, now_utc);
            }
        }

        // 2. Closed Ports / Camera checks
        if let Some((port, label)) = closed_ports.first() {
            self.raise(
                AlertType::CameraPortClosed,
                AlertSeverity::Critical,
                "🚨 КАМЕР / ҮЙЛЧИЛГЭЭ ТАСРАВ".to_string(),
                format!("{label} (порт {port}) хаалттай эсвэл холболт тасарсан байна! Машин танихгүй байж болзошгүй."),
                "Камерын тэжээл болон сүлжээний холболтыг шалгана уу.".to_string(),
                now_instant,
                now_utc,
            );
        } else {
            self.clear(&AlertType::CameraPortClosed, now_utc);
        }

        // 3. Disk checks
        if worst_disk_pct >= self.disk_threshold {
            self.raise(
                AlertType::DiskNearFull,
                AlertSeverity::Critical,
                format!("⚠️ Диск {worst_disk_mount} дүүрч байна"),
                format!("{worst_disk_pct:.0}% дүүрсэн, {worst_disk_free_gb:.1} GB үлдсэн. Лог болон өгөгдөл бичигдэхгүй болох эрсдэлтэй."),
                "Хэрэгцээгүй файлуудыг устгаж диск чөлөөлнө үү.".to_string(),
                now_instant,
                now_utc,
            );
        } else {
            self.clear(&AlertType::DiskNearFull, now_utc);
        }

        // 4. Healer ceiling hit
        if let Some(svc_name) = healer_limit_hit {
            self.raise(
                AlertType::HealLimitReached,
                AlertSeverity::Critical,
                format!("🚨 {svc_name} үйлчилгээ гацав"),
                format!("{svc_name} 1 цагт хэт олон удаа унаж дахин ачааллаа. Үйлчилгээг гараар шалгах шаардлагатай."),
                "Task Manager эсвэл log файлыг шалгаж үйлчилгээг асаана уу.".to_string(),
                now_instant,
                now_utc,
            );
        } else {
            self.clear(&AlertType::HealLimitReached, now_utc);
        }

        self.persist_snapshot(now_utc);
    }

    #[allow(clippy::too_many_arguments)]
    fn raise(
        &mut self,
        alert_type: AlertType,
        severity: AlertSeverity,
        title: String,
        message: String,
        advice: String,
        now_instant: Instant,
        now_utc: DateTime<Utc>,
    ) {
        let should_notify = match self.active.get(&alert_type) {
            Some((_, last_notified_instant)) => {
                now_instant.duration_since(*last_notified_instant) >= self.cooldown
            }
            None => true,
        };

        let first_seen = self
            .active
            .get(&alert_type)
            .map(|(a, _)| a.first_seen.clone())
            .unwrap_or_else(|| now_utc.to_rfc3339());

        let alert = ActiveAlert {
            id: format!("{alert_type:?}"),
            alert_type: alert_type.clone(),
            severity: severity.clone(),
            title: title.clone(),
            message: message.clone(),
            advice: advice.clone(),
            first_seen,
            last_notified: now_utc.to_rfc3339(),
        };

        if should_notify {
            warn!("[LOCAL ALERT] {}: {} — {}", title, message, advice);
            self.dispatch_notifications(&severity, &title, &format!("{message}\n\nЗөвлөмж: {advice}"));
            self.active.insert(alert_type, (alert, now_instant));
        } else if let Some((existing, _)) = self.active.get_mut(&alert_type) {
            existing.message = message;
            existing.advice = advice;
        }
    }

    fn clear(&mut self, alert_type: &AlertType, _now_utc: DateTime<Utc>) {
        if let Some((cleared, _)) = self.active.remove(alert_type) {
            info!("[ALERT RESOLVED] {}: Асуудал шийдэгдэж хэвийн боллоо.", cleared.title);
            // Optionally notify recovery
            let ok_title = "✓ Асуудал шийдэгдлээ".to_string();
            let ok_msg = format!("{} — Хэвийн горимд шилжлээ.", cleared.title);
            self.dispatch_recovery(&ok_title, &ok_msg);
        }
    }

    fn dispatch_notifications(&self, severity: &AlertSeverity, title: &str, body: &str) {
        #[cfg(windows)]
        {
            if self.sound_enabled {
                platform::play_alert_sound(matches!(severity, AlertSeverity::Critical));
            }
            if self.popup_enabled {
                platform::show_session_popup(title, body, matches!(severity, AlertSeverity::Critical));
            }
            if self.toast_enabled {
                platform::show_windows_toast(title, body);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (severity, title, body);
        }
    }

    fn dispatch_recovery(&self, title: &str, body: &str) {
        #[cfg(windows)]
        {
            if self.toast_enabled {
                platform::show_windows_toast(title, body);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (title, body);
        }
    }

    fn persist_snapshot(&self, now_utc: DateTime<Utc>) {
        let snapshot = AlertSnapshot {
            active_alerts: self.active.values().map(|(a, _)| a.clone()).collect(),
            updated_at: now_utc.to_rfc3339(),
        };

        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            if let Some(parent) = self.state_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.state_file, json);
        }
    }

    pub fn snapshot(&self) -> AlertSnapshot {
        AlertSnapshot {
            active_alerts: self.active.values().map(|(a, _)| a.clone()).collect(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

// ─── Platform Win32 Notification Implementations ─────────────────────────────

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;
    use std::process::Command;
    use base64::Engine;

    const MB_ICONERROR: u32 = 0x00000010;
    const MB_ICONWARNING: u32 = 0x00000030;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct STARTUPINFOW {
        cb: u32,
        lpReserved: *mut u16,
        lpDesktop: *mut u16,
        lpTitle: *mut u16,
        dwX: u32,
        dwY: u32,
        dwXSize: u32,
        dwYSize: u32,
        dwXCountChars: u32,
        dwYCountChars: u32,
        dwFillAttribute: u32,
        dwFlags: u32,
        wShowWindow: u16,
        cbReserved2: u16,
        lpReserved2: *mut u8,
        hStdInput: *mut std::ffi::c_void,
        hStdOutput: *mut std::ffi::c_void,
        hStdError: *mut std::ffi::c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESS_INFORMATION {
        hProcess: *mut std::ffi::c_void,
        hThread: *mut std::ffi::c_void,
        dwProcessId: u32,
        dwThreadId: u32,
    }

    #[link(name = "wtsapi32")]
    extern "system" {
        fn WTSGetActiveConsoleSessionId() -> u32;
        fn WTSQueryUserToken(SessionId: u32, phToken: *mut *mut std::ffi::c_void) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn ProcessIdToSessionId(dwProcessId: u32, pSessionId: *mut u32) -> i32;
        fn GetCurrentProcessId() -> u32;
        fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CreateProcessAsUserW(
            hToken: *mut std::ffi::c_void,
            lpApplicationName: *const u16,
            lpCommandLine: *mut u16,
            lpProcessAttributes: *mut std::ffi::c_void,
            lpThreadAttributes: *mut std::ffi::c_void,
            bInheritHandles: i32,
            dwCreationFlags: u32,
            lpEnvironment: *mut std::ffi::c_void,
            lpCurrentDirectory: *const u16,
            lpStartupInfo: *mut STARTUPINFOW,
            lpProcessInformation: *mut PROCESS_INFORMATION,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn MessageBeep(uType: u32) -> i32;
    }

    pub fn play_alert_sound(is_critical: bool) {
        unsafe {
            let sound = if is_critical { MB_ICONERROR } else { MB_ICONWARNING };
            MessageBeep(sound);
        }
    }

    fn ensure_alert_script() -> PathBuf {
        let candidate = crate::config::Config::data_dir().join("show_alert.ps1");
        if candidate.exists() {
            return candidate;
        }
        let pdata = PathBuf::from(r"C:\ProgramData\Zev Detector");
        let _ = std::fs::create_dir_all(&pdata);
        let pdata_script = pdata.join("show_alert.ps1");
        if !pdata_script.exists() {
            const EMBEDDED_SCRIPT: &str = include_str!("../show_alert.ps1");
            let _ = std::fs::write(&pdata_script, EMBEDDED_SCRIPT);
        }
        pdata_script
    }

    /// Delivers an interactive modern ParkEase alert popup onto the
    /// active desktop using the new modern UI card design.
    pub fn show_session_popup(title: &str, message: &str, is_critical: bool) {
        let (desc, advice) = if let Some((m, a)) = message.split_once("\n\nЗөвлөмж: ") {
            (m, a)
        } else {
            (message, "")
        };
        let severity_str = if is_critical { "Critical" } else { "Warning" };

        let script = ensure_alert_script();
        let script_str = script.to_string_lossy();

        // Encode the PowerShell command into UTF-16LE Base64 so it is 100% immune to escaping issues
        let ps_code = format!(
            "& '{}' -Title '{}' -Message '{}' -Advice '{}' -Severity '{}' -TimeoutSecs 25",
            script_str.replace('\'', "''"),
            title.replace('\'', "''"),
            desc.replace('\'', "''"),
            advice.replace('\'', "''"),
            severity_str
        );
        let utf16_bytes: Vec<u8> = ps_code.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&utf16_bytes);

        let session_id = unsafe { WTSGetActiveConsoleSessionId() };
        let mut current_sess: u32 = 0;
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut current_sess) };

        let t = title.to_string();
        let d = desc.to_string();

        std::thread::spawn(move || {
            // Case A: Running as Session 0 Windows Service (LocalSystem)
            if current_sess == 0 && session_id != 0xFFFF_FFFF {
                let mut user_token: *mut std::ffi::c_void = std::ptr::null_mut();
                if unsafe { WTSQueryUserToken(session_id, &mut user_token) } != 0 {
                    let cmd = format!(
                        "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand {b64}"
                    );
                    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(std::iter::once(0)).collect();

                    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
                    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
                    si.lpDesktop = desktop.as_mut_ptr();

                    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

                    let ok = unsafe {
                        CreateProcessAsUserW(
                            user_token,
                            std::ptr::null(),
                            cmd_w.as_mut_ptr(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            0,
                            0x08000000, // CREATE_NO_WINDOW
                            std::ptr::null_mut(),
                            std::ptr::null(),
                            &mut si,
                            &mut pi,
                        )
                    };

                    if ok != 0 {
                        unsafe {
                            CloseHandle(pi.hProcess);
                            CloseHandle(pi.hThread);
                        }
                        unsafe { CloseHandle(user_token) };
                        return;
                    }
                    unsafe { CloseHandle(user_token) };
                }
            }

            // Case B: Running in user interactive session (or fallback)
            let res = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy", "Bypass",
                    "-WindowStyle", "Hidden",
                    "-EncodedCommand", &b64,
                ])
                .spawn();

            if res.is_err() {
                // Secondary fallback: clean tray balloon toast
                show_windows_toast(&t, &d);
            }
        });
    }

    /// Dispatches a non-intrusive Windows 10/11 Action Center notification
    pub fn show_windows_toast(title: &str, body: &str) {
        let title_esc = title.replace('\'', "''").replace('"', "`\"");
        let body_esc = body.replace('\'', "''").replace('"', "`\"");

        // Modern PowerShell Balloon / Toast script
        let ps_code = format!(
            "[void] [System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms'); \
             $n = New-Object System.Windows.Forms.NotifyIcon; \
             $n.Icon = [System.Drawing.SystemIcons]::Warning; \
             $n.Visible = $true; \
             $n.ShowBalloonTip(8000, '{title_esc}', '{body_esc}', [System.Windows.Forms.ToolTipIcon]::Warning); \
             Start-Sleep -Seconds 9; \
             $n.Dispose();"
        );

        let _ = Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_code])
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_score_evaluates_and_clears() {
        let temp_dir = std::env::temp_dir().join(format!("test_notify_{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let cfg = crate::config::NotifyConfig {
            enabled: true,
            popup_enabled: false,
            toast_enabled: false,
            sound_enabled: false,
            netqual_threshold: 70,
            disk_threshold: 90.0,
            cooldown_mins: 15,
        };

        let mut notifier = LocalNotifier::new(&cfg, &temp_dir);

        // Good network -> no alert
        notifier.evaluate(Some(95), Some(12), Some(0.0), 50.0, "C:\\", 100.0, &[], None);
        assert!(notifier.active.is_empty());

        // Degraded network -> raises NetworkDegraded alert
        notifier.evaluate(Some(45), Some(350), Some(5.0), 50.0, "C:\\", 100.0, &[], None);
        assert!(notifier.active.contains_key(&AlertType::NetworkDegraded));

        // Network drops completely -> raises NetworkDown and clears NetworkDegraded
        notifier.evaluate(Some(0), None, Some(100.0), 50.0, "C:\\", 100.0, &[], None);
        assert!(notifier.active.contains_key(&AlertType::NetworkDown));
        assert!(!notifier.active.contains_key(&AlertType::NetworkDegraded));

        // Network recovers to 100 -> clears all network alerts
        notifier.evaluate(Some(100), Some(8), Some(0.0), 50.0, "C:\\", 100.0, &[], None);
        assert!(notifier.active.is_empty());

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
