//! Runs one admin-only command elevated (UAC) and waits for it to finish.
//!
//! The desktop app itself deliberately runs unelevated — a kiosk operator
//! opening a settings window should not be met with a UAC prompt, and the app
//! only needs Administrator rights for the four actions that actually change
//! the service. Each of those shells out to `zev-detector.exe <verb>` through
//! ShellExecuteEx's `runas` verb, so exactly one prompt appears, for exactly
//! the action the user clicked.

use std::path::Path;

/// Runs `exe arg` elevated, blocking until it exits, and returns its exit code.
#[cfg(windows)]
pub fn run_elevated(exe: &Path, arg: &str) -> anyhow::Result<u32> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    if !exe.exists() {
        anyhow::bail!("{} was not found next to the app.", exe.display());
    }

    let exe_w = wide(&exe.to_string_lossy());
    let verb_w = wide("runas");
    let args_w = wide(arg);
    // The elevated child writes to a console nobody is watching; the app
    // reports the outcome itself, so don't flash a black window on screen.
    let dir_w = exe.parent().map(|d| wide(&d.to_string_lossy()));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb_w.as_ptr();
    info.lpFile = exe_w.as_ptr();
    info.lpParameters = args_w.as_ptr();
    info.lpDirectory = dir_w.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
    info.nShow = SW_HIDE as i32;

    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 {
        anyhow::bail!("The Administrator prompt was declined or could not be shown.");
    }

    let mut code: u32 = 0;
    if info.hProcess != 0 {
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
    }
    Ok(code)
}

#[cfg(not(windows))]
pub fn run_elevated(_exe: &Path, _arg: &str) -> anyhow::Result<u32> {
    anyhow::bail!("Elevation is Windows-only")
}
