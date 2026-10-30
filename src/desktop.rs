use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GW_CHILD, GW_HWNDNEXT, GetWindow, GetWindowThreadProcessId, IsWindowVisible, SMTO_NORMAL,
    SendMessageTimeoutW,
};
use windows::core::{BOOL, w};

use crate::tracker::{class_name, process_name};

/// Where an overlay sits behind windows: inside the shell's wallpaper layer, under the desktop icons.
#[derive(Clone, Copy, Debug)]
pub struct DesktopLayer {
    pub parent: HWND,
    /// The icons' window, which the overlay goes just behind when it shares a parent with it.
    pub icons: Option<HWND>,
}

/// Asks Explorer to split the wallpaper into a window of its own behind the icons, as wallpaper apps do,
/// and finds it. Undocumented, so it may find nothing.
pub fn find() -> Option<DesktopLayer> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok()?;
        // If icons and wallpaper already exist under Progman, return them immediately
        // without sending 0x052C, which prompts Explorer to tear down and rebuild the wallpaper,
        // causing desktop black flashes.
        if let Ok(icons) = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None)
            && FindWindowExW(Some(progman), None, w!("WorkerW"), None).is_ok()
        {
            return Some(DesktopLayer { parent: progman, icons: Some(icons) });
        }
        // Only if not found, send 0x052C to prompt Explorer to split them:
        for (wparam, lparam) in [(0xD, 1), (0, 0)] {
            let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(wparam), LPARAM(lparam), SMTO_NORMAL, 1000, None);
        }
        // Windows 11 24H2 and later keep the icons and the wallpaper as children of Progman.
        if let Ok(icons) = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None)
            && FindWindowExW(Some(progman), None, w!("WorkerW"), None).is_ok()
        {
            return Some(DesktopLayer { parent: progman, icons: Some(icons) });
        }
        // Earlier, the wallpaper is the top-level WorkerW right behind the one holding the icons.
        unsafe extern "system" fn behind_icons(window: HWND, found: LPARAM) -> BOOL {
            unsafe {
                if FindWindowExW(Some(window), None, w!("SHELLDLL_DefView"), None).is_err() {
                    return true.into();
                }
                *(found.0 as *mut Option<HWND>) = FindWindowExW(None, Some(window), w!("WorkerW"), None).ok();
            }
            false.into()
        }
        let mut found: Option<HWND> = None;
        let _ = EnumWindows(Some(behind_icons), LPARAM(&raw mut found as isize));
        found.map(|parent| DesktopLayer { parent, icons: None })
    }
}

/// The shell's desktop windows and what lives in them, front to back, for the log.
pub fn tree() -> String {
    unsafe {
        let mut parts = Vec::new();
        if let Ok(progman) = FindWindowW(w!("Progman"), None) {
            parts.push(format!("Progman [{}]", children(progman)));
        }
        let mut worker = FindWindowExW(None, None, w!("WorkerW"), None);
        while let Ok(window) = worker {
            if IsWindowVisible(window).as_bool() {
                parts.push(format!("WorkerW [{}]", children(window)));
            }
            worker = FindWindowExW(None, Some(window), w!("WorkerW"), None);
        }
        parts.join(", ")
    }
}

fn children(parent: HWND) -> String {
    let mut names = Vec::new();
    unsafe {
        let mut child = GetWindow(parent, GW_CHILD);
        while let Ok(window) = child {
            let mut process = 0;
            GetWindowThreadProcessId(window, Some(&mut process));
            let hidden = if IsWindowVisible(window).as_bool() { "" } else { " hidden" };
            names.push(format!("{} of {}{hidden}", class_name(window), process_name(process)));
            child = GetWindow(window, GW_HWNDNEXT);
        }
    }
    names.join(", ")
}
