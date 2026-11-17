//! Kiosk & Browser Auto-Start module for Zev Detector.
//!
//! Enables unattended kiosk and parking display PCs to automatically:
//! 1. Open Microsoft Edge or Google Chrome on computer boot / user logon.
//! 2. Navigate directly to `https://turees.zevtabs.mn` (or any configured URL).
//! 3. Support Fullscreen Kiosk mode (`--kiosk`), App mode (`--app`), or Maximized.
//! 4. Auto-login or session preservation with credentials (`CAdmin1` / `15166`).
//! 5. Auto-heal: continuously ensure the browser remains open if an operator accidentally closes it.
//! 6. Bridge across Windows Session 0 (Service) into the active console user desktop.

use crate::config::KioskConfig;
use log::{error, info, warn};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
mod win_ffi {
    #[repr(C)]
    #[allow(non_snake_case)]
    pub struct STARTUPINFOW {
        pub cb: u32,
        pub lpReserved: *mut u16,
        pub lpDesktop: *mut u16,
        pub lpTitle: *mut u16,
        pub dwX: u32,
        pub dwY: u32,
        pub dwXSize: u32,
        pub dwYSize: u32,
        pub dwXCountChars: u32,
        pub dwYCountChars: u32,
        pub dwFillAttribute: u32,
        pub dwFlags: u32,
        pub wShowWindow: u16,
        pub cbReserved2: u16,
        pub lpReserved2: *mut u8,
        pub hStdInput: *mut std::ffi::c_void,
        pub hStdOutput: *mut std::ffi::c_void,
        pub hStdError: *mut std::ffi::c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    pub struct PROCESS_INFORMATION {
        pub hProcess: *mut std::ffi::c_void,
        pub hThread: *mut std::ffi::c_void,
        pub dwProcessId: u32,
        pub dwThreadId: u32,
    }

    #[link(name = "wtsapi32")]
    extern "system" {
        pub fn WTSGetActiveConsoleSessionId() -> u32;
        pub fn WTSQueryUserToken(SessionId: u32, phToken: *mut *mut std::ffi::c_void) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn ProcessIdToSessionId(dwProcessId: u32, pSessionId: *mut u32) -> i32;
        pub fn GetCurrentProcessId() -> u32;
        pub fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        pub fn CreateProcessAsUserW(
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
}

/// Returns the ProgramData directory used for kiosk scripts and logs.
pub fn kiosk_dir() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\Zev Detector")
}

/// Returns the path to the PowerShell kiosk launcher script.
pub fn launch_script_path() -> PathBuf {
    kiosk_dir().join("launch_kiosk.ps1")
}

/// Detects the executable path for Microsoft Edge or Google Chrome.
pub fn detect_browser(preference: &str) -> Option<PathBuf> {
    let edge_candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];

    let chrome_candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];

    match preference.to_lowercase().as_str() {
        "edge" => {
            for p in &edge_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
            // fallback to chrome if edge not found
            for p in &chrome_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
        }
        "chrome" => {
            for p in &chrome_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
            // fallback to edge if chrome not found
            for p in &edge_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
        }
        _ => {
            // "auto": prefer Edge on modern Windows, fallback to Chrome
            for p in &edge_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
            for p in &chrome_candidates {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    return Some(pb);
                }
            }
        }
    }

    // Try LocalAppData Chrome fallback
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = Path::new(&local).join(r"Google\Chrome\Application\chrome.exe");
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// Checks if any Edge or Chrome browser process running the Kiosk is currently active.
pub fn is_browser_running(preference: &str) -> bool {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let check_edge = preference == "auto" || preference == "edge";
    let check_chrome = preference == "auto" || preference == "chrome";

    sys.processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        let is_browser = (check_edge && name == "msedge.exe") || (check_chrome && name == "chrome.exe");
        if !is_browser {
            return false;
        }

        // Check if command line belongs to our dedicated kiosk profile or target
        let cmd = p.cmd();
        cmd.iter().any(|arg| {
            let s = arg.to_string_lossy();
            s.contains("ZevKioskProfile") || s.contains("turees.zevtabs.mn") || s.contains("--kiosk")
        })
    })
}

/// Generates the robust PowerShell launcher script for unattended browser kiosk operation.
pub fn generate_script_content(cfg: &KioskConfig) -> String {
    let url = if cfg.url.trim().is_empty() { "https://turees.zevtabs.mn" } else { cfg.url.trim() };
    let browser = if cfg.browser.trim().is_empty() { "auto" } else { cfg.browser.trim() };
    let mode = if cfg.mode.trim().is_empty() { "kiosk" } else { cfg.mode.trim() };
    let auto_login = if cfg.auto_login { "$true" } else { "$false" };
    let username = cfg.username.replace('\'', "''");
    let password = cfg.password.replace('\'', "''");

    format!(
        r#"# Zev Detector - Kiosk & Browser Auto-Launcher
param(
    [string]$Url = '{url}',
    [string]$Browser = '{browser}',
    [string]$Mode = '{mode}',
    [bool]$AutoLogin = {auto_login},
    [string]$Username = '{username}',
    [string]$Password = '{password}'
)

$logDir = "C:\ProgramData\Zev Detector"
if (-not (Test-Path $logDir)) {{ New-Item -ItemType Directory -Path $logDir -Force | Out-Null }}
$logFile = Join-Path $logDir "kiosk.log"

function Write-KioskLog($msg) {{
    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $line = "[$timestamp] $msg"
    Write-Output $line
    try {{ Add-Content -Path $logFile -Value $line -ErrorAction SilentlyContinue }} catch {{}}
}}

Write-KioskLog "=== Kiosk Launch Initiated: Url=$Url, Mode=$Mode, Browser=$Browser ==="

# 1. Resolve browser executable
$edgeCandidates = @(
    "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    "C:\Program Files\Microsoft\Edge\Application\msedge.exe"
)
$chromeCandidates = @(
    "C:\Program Files\Google\Chrome\Application\chrome.exe",
    "C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
)

$browserExe = $null
$isEdge = $false

if ($Browser -eq "edge") {{
    foreach ($p in $edgeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; $isEdge = $true; break }} }}
    if (-not $browserExe) {{
        foreach ($p in $chromeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; break }} }}
    }}
}} elseif ($Browser -eq "chrome") {{
    foreach ($p in $chromeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; break }} }}
    if (-not $browserExe) {{
        foreach ($p in $edgeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; $isEdge = $true; break }} }}
    }}
}} else {{
    # auto
    foreach ($p in $edgeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; $isEdge = $true; break }} }}
    if (-not $browserExe) {{
        foreach ($p in $chromeCandidates) {{ if (Test-Path $p) {{ $browserExe = $p; break }} }}
    }}
}}

if (-not $browserExe) {{
    Write-KioskLog "ERROR: No supported browser (Edge or Chrome) found on system."
    exit 1
}}

Write-KioskLog "Using browser: $browserExe"

# 2. Check if dedicated Kiosk instance is already running
$profileDir = "C:\ProgramData\ZevKioskProfile"
if (-not (Test-Path $profileDir)) {{ New-Item -ItemType Directory -Path $profileDir -Force | Out-Null }}
$port = 9222

$procName = [System.IO.Path]::GetFileNameWithoutExtension($browserExe)
$kioskRunning = $false

try {{
    $activeProcs = Get-CimInstance Win32_Process -Filter "Name = '$procName.exe'" -ErrorAction SilentlyContinue
    foreach ($p in $activeProcs) {{
        if ($p.CommandLine -match "ZevKioskProfile") {{
            $kioskRunning = $true
            break
        }}
    }}
}} catch {{}}

# 3. Assemble command-line arguments
$argsList = @(
    "--remote-debugging-port=$port",
    "--user-data-dir=$profileDir",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-session-crashed-bubble",
    "--disable-features=Translate"
)

if ($Mode -eq "kiosk") {{
    $argsList += "--kiosk"
    $argsList += $Url
}} elseif ($Mode -eq "app") {{
    $argsList += "--app=$Url"
}} elseif ($Mode -eq "maximized") {{
    $argsList += "--start-maximized"
    $argsList += $Url
}} else {{
    $argsList += $Url
}}

if (-not $kioskRunning) {{
    Write-KioskLog "Starting browser: $browserExe $($argsList -join ' ')"
    Start-Process -FilePath $browserExe -ArgumentList $argsList
}} else {{
    Write-KioskLog "Kiosk instance already active. Checking session..."
}}

# 4. Auto-login automation via Chrome DevTools Protocol (CDP)
if ($AutoLogin -and $Username -and $Password) {{
    Write-KioskLog "Performing automated login check for user: $Username"

    # Wait for CDP endpoint to respond (up to 15 seconds)
    $cdpConnected = $false
    $targets = $null
    for ($i = 0; $i -lt 15; $i++) {{
        Start-Sleep -Milliseconds 1000
        try {{
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$port/json" -TimeoutSec 2 -ErrorAction Stop
            if ($targets) {{
                $cdpConnected = $true
                break
            }}
        }} catch {{}}
    }}

    if (-not $cdpConnected) {{
        Write-KioskLog "WARNING: CDP port $port not responding. Continuing without DOM injection."
    }} else {{
        Start-Sleep -Seconds 1
        try {{
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$port/json" -TimeoutSec 2 -ErrorAction Stop
            $page = $targets | Where-Object {{ $_.type -eq "page" -and $_.url -match "turees\.zevtabs\.mn" }} | Select-Object -First 1
            if (-not $page) {{
                $page = $targets | Where-Object {{ $_.type -eq "page" }} | Select-Object -First 1
            }}

            if ($page -and $page.webSocketDebuggerUrl) {{
                Write-KioskLog "Connecting CDP WebSocket: $($page.webSocketDebuggerUrl)"
                $ws = New-Object System.Net.WebSockets.ClientWebSocket
                $cts = New-Object System.Threading.CancellationTokenSource(25000)
                $uri = New-Object System.Uri($page.webSocketDebuggerUrl)
                $ws.ConnectAsync($uri, $cts.Token).Wait()

                $loginJs = @"
(async function() {{
    for (let attempt = 0; attempt < 30; attempt++) {{
        if (!window.location.href.includes('turees.zevtabs.mn')) {{
            await new Promise(r => setTimeout(r, 500));
            continue;
        }}

        try {{
            if (window.location.pathname.startsWith('/khyanalt') || document.cookie.includes('tureestoken=')) {{
                return {{ success: true, message: 'Already authenticated', path: window.location.pathname }};
            }}
        }} catch(e) {{}}

        const userInp = document.querySelector('input[placeholder*="Нэвтрэх нэр"], input[name="nevtrekhNer"], input[type="text"]');
        const passInp = document.querySelector('input[placeholder*="••••"], input[name="nuutsUg"], input[type="password"]');
        const btn = Array.from(document.querySelectorAll('button')).find(b => 
            (b.textContent && b.textContent.includes('Нэвтрэх')) || b.type === 'submit'
        );

        if (userInp && passInp && btn) {{
            const setVal = (el, val) => {{
                const proto = Object.getPrototypeOf(el);
                const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set || Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
                if (setter) {{
                    setter.call(el, val);
                }} else {{
                    el.value = val;
                }}
                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true }}));
            }};

            setVal(userInp, '$Username');
            setVal(passInp, '$Password');

            await new Promise(r => setTimeout(r, 250));
            btn.click();
            return {{ success: true, message: 'Credentials entered and login clicked' }};
        }}

        await new Promise(r => setTimeout(r, 500));
    }}
    return {{ success: false, message: 'Login inputs not found within timeout' }};
}})()
"@

                $submitted = $false
                for ($attempt = 0; $attempt -lt 5; $attempt++) {{
                    try {{
                        $id = [System.Threading.Interlocked]::Increment([ref]100)
                        $req = @{{
                            id = $id
                            method = "Runtime.evaluate"
                            params = @{{
                                expression = $loginJs
                                awaitPromise = $true
                                returnByValue = $true
                            }}
                        }} | ConvertTo-Json -Depth 5

                        $bytes = [System.Text.Encoding]::UTF8.GetBytes($req)
                        $buf = [System.ArraySegment[byte]]::new($bytes)
                        $ws.SendAsync($buf, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $cts.Token).Wait()

                        $recvBuf = [System.ArraySegment[byte]]::new((New-Object byte[] 65536))
                        $res = $ws.ReceiveAsync($recvBuf, $cts.Token).Result
                        $resJson = [System.Text.Encoding]::UTF8.GetString($recvBuf.Array, 0, $res.Count) | ConvertFrom-Json

                        if ($resJson.error) {{
                            Start-Sleep -Milliseconds 800
                            continue
                        }}

                        $val = $resJson.result.result.value
                        if ($val) {{
                            Write-KioskLog "Auto-login result: $($val.message)"
                            $submitted = $true
                            break
                        }}
                    }} catch {{
                        Start-Sleep -Milliseconds 800
                    }}
                }}

                try {{ $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, "Done", $cts.Token).Wait() }} catch {{}}
            }}
        }} catch {{
            Write-KioskLog "Auto-login CDP execution warning: $_"
        }}
    }}
}}

Write-KioskLog "=== Kiosk Launch Completed Successfully ==="
"#
    )
}

/// Ensures the launcher script is written to disk in ProgramData.
pub fn ensure_launch_script(cfg: &KioskConfig) -> anyhow::Result<PathBuf> {
    let dir = kiosk_dir();
    std::fs::create_dir_all(&dir)?;
    let script = launch_script_path();
    let content = generate_script_content(cfg);
    std::fs::write(&script, content)?;
    Ok(script)
}

/// Configures Windows Startup so that the browser automatically opens when the computer boots/turns on.
pub fn setup_autostart(cfg: &KioskConfig) -> anyhow::Result<()> {
    let script_path = ensure_launch_script(cfg)?;

    if !cfg.enabled || !cfg.autostart {
        // Clean up shortcuts and registry keys if disabled
        let ps_clean = r#"
            $targets = @(
                [Environment]::GetFolderPath('Startup'),
                [Environment]::GetFolderPath('CommonStartup')
            )
            foreach ($dir in $targets) {
                if (-not [string]::IsNullOrWhiteSpace($dir)) {
                    $lnk = Join-Path $dir 'ZevtabsKiosk.lnk'
                    if (Test-Path $lnk) { Remove-Item $lnk -Force -ErrorAction SilentlyContinue }
                }
            }
            Remove-ItemProperty -Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ZevtabsKiosk' -ErrorAction SilentlyContinue
            Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ZevtabsKiosk' -ErrorAction SilentlyContinue
        "#;
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps_clean])
            .output();

        info!("Removed Kiosk autostart entries.");
        return Ok(());
    }

    info!("Setting up Kiosk autostart for {}", cfg.url);

    // 1. Create Startup Shortcuts via PowerShell COM in both User and Common Startup
    let script_path_str = script_path.to_string_lossy().replace('\'', "''");
    let ps_shortcut_script = format!(
        r#"$WshShell = New-Object -ComObject WScript.Shell
$targets = @(
    [Environment]::GetFolderPath('Startup'),
    [Environment]::GetFolderPath('CommonStartup')
)
foreach ($dir in $targets) {{
    if (-not [string]::IsNullOrWhiteSpace($dir)) {{
        try {{
            if (-not (Test-Path $dir)) {{ New-Item -ItemType Directory -Path $dir -Force | Out-Null }}
            $lnk = Join-Path $dir 'ZevtabsKiosk.lnk'
            $sc = $WshShell.CreateShortcut($lnk)
            $sc.TargetPath = 'powershell.exe'
            $sc.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File '{script_path_str}'"
            $sc.WorkingDirectory = 'C:\ProgramData\Zev Detector'
            $sc.Description = 'Zevtabs Kiosk Browser Auto-Launcher'
            $sc.Save()
            Write-Output "Created shortcut: $lnk"
        }} catch {{}}
    }}
}}
"#
    );

    let res = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_shortcut_script])
        .output();

    if let Ok(out) = res {
        if out.status.success() {
            info!("Created Startup shortcut(s) for Kiosk: {}", String::from_utf8_lossy(&out.stdout).trim());
        } else {
            warn!(
                "Warning while creating Startup shortcut: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    // 2. Also register under Run key for resilience
    let reg_cmd = format!(
        r#"Set-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ZevtabsKiosk' -Value "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \`"{script_path_str}\`"" -ErrorAction SilentlyContinue; Set-ItemProperty -Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ZevtabsKiosk' -Value "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \`"{script_path_str}\`"" -ErrorAction SilentlyContinue"#
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &reg_cmd])
        .output();

    Ok(())
}

/// Launches the browser kiosk immediately into the active user's desktop session.
pub fn launch(cfg: &KioskConfig) -> anyhow::Result<()> {
    let script_path = ensure_launch_script(cfg)?;
    let script_str = script_path.to_string_lossy();

    #[cfg(windows)]
    {
        use win_ffi::*;

        let session_id = unsafe { WTSGetActiveConsoleSessionId() };
        let mut current_sess: u32 = 0;
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut current_sess) };

        // If running as LocalSystem Session 0 Windows Service:
        if current_sess == 0 && session_id != 0xFFFF_FFFF {
            let mut user_token: *mut std::ffi::c_void = std::ptr::null_mut();
            if unsafe { WTSQueryUserToken(session_id, &mut user_token) } != 0 {
                let cmd = format!(
                    "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"",
                    script_str
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
                        CloseHandle(user_token);
                    }
                    info!("Successfully launched Kiosk process into active console session {session_id}");
                    return Ok(());
                } else {
                    unsafe { CloseHandle(user_token) };
                    warn!("CreateProcessAsUserW failed for session {session_id}, falling back to standard spawn");
                }
            }
        }
    }

    // Interactive session fallback
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-WindowStyle", "Hidden",
        "-File", &script_str,
    ]);

    #[cfg(windows)]
    cmd.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP

    let out = cmd.spawn()?;

    info!("Spawned interactive kiosk launcher PID {}", out.id());
    Ok(())
}

/// Periodic health check: if kiosk is enabled and auto-heal is on,
/// checks if the browser is running. If closed or crashed, relaunches it!
pub fn check_and_heal(cfg: &KioskConfig) {
    if !cfg.enabled || !cfg.auto_heal {
        return;
    }

    #[cfg(windows)]
    {
        use win_ffi::WTSGetActiveConsoleSessionId;
        let session_id = unsafe { WTSGetActiveConsoleSessionId() };
        if session_id == 0xFFFF_FFFF {
            // No console user logged in currently; nothing to display to
            return;
        }
    }

    if !is_browser_running(&cfg.browser) {
        info!("Kiosk browser is not active; auto-healing and reopening {}...", cfg.url);
        if let Err(e) = launch(cfg) {
            error!("Kiosk auto-heal failed to launch: {e}");
        }
    }
}

/// Tests the login credentials against the Zevtabs API endpoint.
pub async fn test_credentials(url: &str, username: &str, password: &str) -> Result<String, String> {
    if username.trim().is_empty() || password.trim().is_empty() {
        return Err("Нэвтрэх нэр эсвэл нууц үг хоосон байна".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let api_url = format!("{}/api/ajiltanNevtrey", url.trim_end_matches('/'));
    let payload = serde_json::json!({
        "nevtrekhNer": username.trim(),
        "nuutsUg": password.trim()
    });

    let resp = client.post(&api_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Холболтын алдаа: {e}"))?;

    let body: serde_json::Value = resp.json()
        .await
        .map_err(|e| format!("Хариу задлахад алдаа: {e}"))?;

    if body.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let user_ner = body.get("result")
            .and_then(|r| r.get("ner"))
            .and_then(|n| n.as_str())
            .unwrap_or("Ажиллагч");
        let org_ner = body.get("result")
            .and_then(|r| r.get("baiguullagiinNer"))
            .and_then(|n| n.as_str())
            .unwrap_or("Байгууллага");
        Ok(format!("Амжилттай: {user_ner} ({org_ner})"))
    } else {
        let err_msg = body.get("aldaa")
            .and_then(|a| a.as_str())
            .unwrap_or("Нэвтрэлт амжилтгүй боллоо");
        Err(err_msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_generation() {
        let cfg = KioskConfig {
            enabled: true,
            url: "https://turees.zevtabs.mn".to_string(),
            browser: "edge".to_string(),
            mode: "kiosk".to_string(),
            autostart: true,
            auto_heal: true,
            username: "CAdmin1".to_string(),
            password: "15166".to_string(),
            auto_login: true,
        };

        let script = generate_script_content(&cfg);
        assert!(script.contains("https://turees.zevtabs.mn"));
        assert!(script.contains("CAdmin1"));
        assert!(script.contains("15166"));
        assert!(script.contains("--kiosk"));
        assert!(script.contains("--remote-debugging-port="));
    }

    #[test]
    fn test_detect_browser() {
        let b = detect_browser("auto");
        assert!(b.is_some());
    }
}
