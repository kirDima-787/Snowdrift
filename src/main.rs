#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod light;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod prefs;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod screen;
#[cfg(windows)]
mod snow;
#[cfg(windows)]
mod tracker;
#[cfg(windows)]
mod welcome;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
#[cfg(windows)]
use windows::Win32::System::Threading::CreateMutexW;
#[cfg(windows)]
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
#[cfg(windows)]
use windows::core::w;

#[cfg(windows)]
fn main() {
    // What "Uninstall" in Settings runs; it removes everything, even with Snowdrift still running.
    if std::env::args().skip(1).any(|argument| argument == "--uninstall") {
        snowdrift::install::uninstall();
        return;
    }
    unsafe {
        let _instance = CreateMutexW(None, true, w!("Local\\Snowdrift"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    log::start();
    log!("Snowdrift {}", env!("CARGO_PKG_VERSION"));
    if let Err(error) = app::run() {
        log!("failed: {error}");
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}
