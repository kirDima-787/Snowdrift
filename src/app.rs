use std::cell::RefCell;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::log;
use crate::prefs::{self, Settings};
use crate::snow::{self, Command, Links};
use crate::welcome::{self, Handlers};
use crate::panel::{self, PanelAction};
use crate::toast::{self, ToastCallbacks, ToastState};
use snowdrift::update::{self, Release};
use snowdrift::simulation::Snowfall;

const CLASS: PCWSTR = w!("SnowdriftController");
const WM_TRAY: u32 = WM_APP + 1;
const WM_UPDATE_CHECK: u32 = WM_APP + 2;
const WM_UPDATE_DOWNLOADED: u32 = WM_APP + 3;
const WM_UPDATE_PROGRESS: u32 = WM_APP + 4;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;



const TIMER_WELCOME: usize = 1;
const TIMER_TRAY_HOLD: usize = 1002;
const TIMER_UPDATE: usize = 2;
const TIMER_STARTUP: usize = 3;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct App {
    settings: Settings,
    commands: Sender<Command>,
    wake: HANDLE,
    snow: Option<JoinHandle<()>>,
    icon: HICON,
    locked: bool,
    display_off: bool,
    away: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// The tray icon and its menu, and the system events that pause the snow, on the main thread;
/// the snow itself runs on its own thread so menus and other modal loops never stall it.
pub fn run() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(w!("Snowdrift.App"));
        if let Ok(exe) = std::env::current_exe() {
            snowdrift::install::ensure_shortcuts(&exe);
            snowdrift::install::register_installed(&exe, env!("CARGO_PKG_VERSION"));
        }
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Snowdrift"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let settings = Settings::load();
        let (commands, receiver) = mpsc::channel();
        let wake = CreateEventW(None, false, false, None)?;
        let links = Links { wake: wake.0 as isize, controller: window.0 as isize };
        let snow = thread::Builder::new().name("snow".into()).spawn(move || snow::run(receiver, links, settings)).ok();
        let icon = tray_icon();
        add_tray_icon(window, icon);
        let _ = WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION);
        let _ = RegisterPowerSettingNotification(HANDLE(window.0), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE);
        if !settings.welcomed {
            SetTimer(Some(window), TIMER_WELCOME, 1200, None);
        }
        // Check for updates 5 seconds after startup in background
        SetTimer(Some(window), TIMER_UPDATE, 5000, None);

        if std::env::args().any(|arg| arg.eq_ignore_ascii_case("--startup")) {
            // Elegant startup notification when booting with Windows
            SetTimer(Some(window), TIMER_STARTUP, 1500, None);
        }

        APP.set(Some(App { settings, commands, wake, snow, icon, locked: false, display_off: false, away: false }));

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            let event = (lparam.0 & 0xffff) as u32;
            match event {
                WM_LBUTTONDOWN => {
                    unsafe {
                        let _ = SetTimer(Some(window), TIMER_TRAY_HOLD, 3000, None);
                    }
                }
                WM_LBUTTONUP => {
                    unsafe {
                        let _ = KillTimer(Some(window), TIMER_TRAY_HOLD);
                    }
                    let x = (wparam.0 & 0xffff) as i16 as i32;
                    let y = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                    show_tray_panel(window, x, y);
                }
                WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT => {
                    unsafe {
                        let _ = KillTimer(Some(window), TIMER_TRAY_HOLD);
                    }
                    let x = (wparam.0 & 0xffff) as i16 as i32;
                    let y = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                    show_tray_panel(window, x, y);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_TRAY_HOLD => {
            unsafe {
                let _ = KillTimer(Some(window), TIMER_TRAY_HOLD);
            }
            panel::unlock_blizzard();
            APP.with_borrow_mut(|app| {
                let Some(app) = app else { return };
                app.settings.snowfall = Snowfall::Blizzard;
                send(app, Command::Snowfall(Snowfall::Blizzard));
            });
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            update(|app| match wparam.0 as u32 {
                WTS_SESSION_LOCK => app.locked = true,
                WTS_SESSION_UNLOCK => app.locked = false,
                WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => app.away = true,
                WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => app.away = false,
                _ => {}
            });
            LRESULT(0)
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_POWERSETTINGCHANGE => {
            let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
            if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                let off = setting.Data[0] == 0;
                update(|app| app.display_off = off);
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            quit(window);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_WELCOME => {
            unsafe {
                let _ = KillTimer(Some(window), TIMER_WELCOME);
            }
            let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
            welcome::show(
                Handlers {
                    scatter: Box::new(|pieces| {
                        APP.with_borrow(|app| {
                            if let Some(app) = app {
                                send(app, Command::Scatter(pieces));
                            }
                        })
                    }),
                    finished: Box::new(|| {
                        APP.with_borrow_mut(|app| {
                            let Some(app) = app else { return };
                            app.settings.welcomed = true;
                            app.settings.save();
                        })
                    }),
                    skipped: Box::new(|| {
                        APP.with_borrow(|app| {
                            if let Some(app) = app {
                                send(app, Command::ReleaseSnowfall);
                            }
                        })
                    }),
                },
                is_ru,
            );
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_UPDATE => {
            unsafe {
                // Next check in 24 hours
                let _ = SetTimer(Some(window), TIMER_UPDATE, 24 * 60 * 60 * 1000, None);
            }
            check_for_updates(window, false);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_STARTUP => {
            unsafe {
                let _ = KillTimer(Some(window), TIMER_STARTUP);
            }
            let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
            if is_ru {
                notify(window, "Snowdrift", "Зимний снегопад запущен.");
            } else {
                notify(window, "Snowdrift", "Winter snowfall has started.");
            }
            LRESULT(0)
        }
        WM_UPDATE_CHECK => {
            let asked = wparam.0 != 0;
            let result = unsafe { *Box::from_raw(lparam.0 as *mut std::result::Result<Option<Release>, String>) };
            on_update_checked(window, result, asked);
            LRESULT(0)
        }
        WM_UPDATE_PROGRESS => {
            let percent = wparam.0 as u32;
            let read_kb = ((lparam.0 >> 16) & 0xffff) as u32;
            let total_kb = (lparam.0 & 0xffff) as u32;
            toast::update_progress(percent, read_kb * 1024, total_kb * 1024);
            LRESULT(0)
        }
        WM_UPDATE_DOWNLOADED => {
            let result = unsafe { *Box::from_raw(lparam.0 as *mut std::result::Result<PathBuf, String>) };
            on_update_downloaded(window, result);
            LRESULT(0)
        }
        _ if message != 0 && message == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            APP.with_borrow(|app| {
                if let Some(app) = app {
                    add_tray_icon(window, app.icon);
                    send(app, Command::ShellRestarted);
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Changes what pauses the snow and tells the snow thread whether it may run.
fn update(change: impl FnOnce(&mut App)) {
    APP.with_borrow_mut(|app| {
        let Some(app) = app else { return };
        change(app);
        let active = !app.locked && !app.display_off && !app.away;
        send(app, Command::Active(active));
    });
}

fn send(app: &App, command: Command) {
    let _ = app.commands.send(command);
    unsafe {
        let _ = SetEvent(app.wake);
    }
}

fn show_tray_panel(window: HWND, x: i32, y: i32) {
    if panel::is_open() {
        panel::close();
        return;
    }
    let Some(settings) = APP.with_borrow(|app| app.as_ref().map(|app| app.settings)) else { return };
    let login = prefs::opens_at_login();
    panel::show(POINT { x, y }, settings, login, move |action| {
        handle_panel_action(window, action);
    });
}

fn handle_panel_action(window: HWND, action: PanelAction) {
    match action {
        PanelAction::Quit => quit(window),
        PanelAction::CheckUpdates => check_for_updates(window, true),
        PanelAction::ClearSnow => APP.with_borrow(|app| {
            if let Some(app) = app {
                send(app, Command::ClearSnow(!prefs::animations_enabled()));
            }
        }),
        PanelAction::OpenAtLogin(on) => {
            let status = prefs::set_opens_at_login(on);
            log!("open at login {}: {status:?}", on);
        }
        PanelAction::Language(lang) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.language = lang;
            app.settings.save();
        }),
        PanelAction::Snowfall(snowfall) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.snowfall = snowfall;
            app.settings.save();
            send(app, Command::Snowfall(snowfall));
        }),
        PanelAction::Accumulate(accumulate) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.accumulate = accumulate;
            app.settings.save();
            send(app, Command::Accumulate(accumulate));
        }),
        PanelAction::Melt(melt) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.melt = melt;
            app.settings.save();
            send(app, Command::Melt(melt));
        }),
        PanelAction::SnowOnWindows(on_windows) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.snow_on_windows = on_windows;
            app.settings.save();
            send(app, Command::SnowOnWindows(on_windows));
        }),
        PanelAction::BehindWindows(behind) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.behind_windows = behind;
            app.settings.save();
            send(app, Command::BehindWindows(behind));
        }),
        PanelAction::ShowInScreenshots(shots) => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.show_in_screenshots = shots;
            app.settings.save();
            send(app, Command::ShowInScreenshots(shots));
        }),
    }
}


fn check_for_updates(window: HWND, asked: bool) {
    let target = window.0 as isize;
    let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
    if asked {
        toast::show(
            ToastState::Checking,
            is_ru,
            ToastCallbacks {
                on_update: Box::new(move |url| {
                    download_and_apply(window, url);
                }),
                on_close: Box::new(|| {}),
            },
        );
    }
    thread::spawn(move || {
        let result = Box::new(update::check());
        unsafe {
            let _ = PostMessageW(
                Some(HWND(target as _)),
                WM_UPDATE_CHECK,
                WPARAM(asked as usize),
                LPARAM(Box::into_raw(result) as isize),
            );
        }
    });
}

fn on_update_checked(window: HWND, result: std::result::Result<Option<Release>, String>, asked: bool) {
    let current = env!("CARGO_PKG_VERSION");
    let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
    match result {
        Ok(Some(release)) => {
            toast::show(
                ToastState::Available { version: release.version.clone(), url: release.url.clone() },
                is_ru,
                ToastCallbacks {
                    on_update: Box::new(move |url| {
                        download_and_apply(window, url);
                    }),
                    on_close: Box::new(|| {}),
                },
            );
        }
        Ok(None) if asked => {
            toast::show(
                ToastState::UpToDate(current.to_string()),
                is_ru,
                ToastCallbacks {
                    on_update: Box::new(|_| {}),
                    on_close: Box::new(|| {}),
                },
            );
        }
        Ok(None) => {}
        Err(error) => {
            log!("update check failed: {error}");
            if asked {
                toast::show(
                    ToastState::Error(error),
                    is_ru,
                    ToastCallbacks {
                        on_update: Box::new(|_| {}),
                        on_close: Box::new(|| {}),
                    },
                );
            }
        }
    }
}

fn download_and_apply(window: HWND, url: String) {
    let target = window.0 as isize;
    let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
    toast::show(
        ToastState::Downloading { percent: 0, downloaded: 0, total: 0 },
        is_ru,
        ToastCallbacks {
            on_update: Box::new(|_| {}),
            on_close: Box::new(|| {}),
        },
    );

    thread::spawn(move || {
        let progress = Box::new(move |read: u32, total: u32| {
            let percent = if total > 0 { ((read as u64 * 100) / total as u64) as u32 } else { 0 };
            let wp = percent as usize;
            let lp = ((read / 1024) as isize) << 16 | ((total / 1024) as isize & 0xffff);
            unsafe {
                let _ = PostMessageW(Some(HWND(target as _)), WM_UPDATE_PROGRESS, WPARAM(wp), LPARAM(lp));
            }
        });
        let res = Box::new(update::download_installer(&url, Some(progress)));
        unsafe {
            let _ = PostMessageW(
                Some(HWND(target as _)),
                WM_UPDATE_DOWNLOADED,
                WPARAM(0),
                LPARAM(Box::into_raw(res) as isize),
            );
        }
    });
}

fn on_update_downloaded(window: HWND, result: std::result::Result<PathBuf, String>) {
    match result {
        Ok(installer) => {
            toast::update_state(ToastState::Downloaded);
            if let Err(error) = update::run_installer(&installer) {
                toast::update_state(ToastState::Error(error));
            } else {
                quit(window);
            }
        }
        Err(error) => {
            toast::update_state(ToastState::Error(error));
        }
    }
}


fn notify(window: HWND, title: &str, text: &str) {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    for (slot, unit) in data.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
        *slot = unit;
    }
    for (slot, unit) in data.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
        *slot = unit;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn quit(window: HWND) {
    panel::close();
    toast::close();
    remove_tray_icon(window);
    if let Some(mut app) = APP.with_borrow_mut(Option::take) {
        send(&app, Command::Quit);
        if let Some(snow) = app.snow.take() {
            let _ = snow.join();
        }
        unsafe {
            let _ = DestroyIcon(app.icon);
        }
    }
    unsafe { PostQuitMessage(0) };
}

/// The app icon from the executable's resources, at the tray's size for the display's scale.
fn tray_icon() -> HICON {
    unsafe {
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem());
        let Ok(instance) = GetModuleHandleW(None) else { return HICON::default() };
        LoadImageW(Some(instance.into()), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .map_or(HICON::default(), |icon| HICON(icon.0))
    }
}

fn add_tray_icon(window: HWND, icon: HICON) {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, unit) in data.szTip.iter_mut().zip("Snowdrift".encode_utf16()) {
        *slot = unit;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
}

fn remove_tray_icon(window: HWND) {
    let data = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: 1, ..Default::default() };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}
