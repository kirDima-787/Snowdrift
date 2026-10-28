use std::cell::RefCell;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
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
use snowdrift::simulation::Snowfall;
use snowdrift::update::{self, Release};

const CLASS: PCWSTR = w!("SnowdriftController");
const WM_TRAY: u32 = WM_APP + 1;
const WM_UPDATE_CHECK: u32 = WM_APP + 2;
const WM_UPDATE_DOWNLOADED: u32 = WM_APP + 3;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

const ID_SNOWFALL: u32 = 100;
const ID_ACCUMULATE: u32 = 200;
const ID_MELT: u32 = 201;
const ID_SCREENSHOTS: u32 = 202;
const ID_LOGIN: u32 = 203;
const ID_WINDOWS: u32 = 204;
const ID_BEHIND: u32 = 205;
const ID_CLEAR: u32 = 207;
const ID_UPDATES: u32 = 208;
const ID_LANG_EN: u32 = 210;
const ID_LANG_RU: u32 = 211;
const ID_QUIT: u32 = 299;

const TIMER_WELCOME: usize = 1;
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
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
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
            if event == WM_CONTEXTMENU || event == NIN_SELECT || event == NIN_KEYSELECT {
                let x = (wparam.0 & 0xffff) as i16 as i32;
                let y = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                show_menu(window, x, y);
            }
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

fn show_menu(window: HWND, x: i32, y: i32) {
    let Some(settings) = APP.with_borrow(|app| app.as_ref().map(|app| app.settings)) else { return };
    let login = prefs::opens_at_login();
    let is_ru = settings.language.is_ru();
    let chosen = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let Ok(snowfall) = CreatePopupMenu() else { return };
        for (i, option) in Snowfall::ALL.iter().enumerate() {
            let title_str = if is_ru { option.title_ru() } else { option.title() };
            let title: Vec<u16> = title_str.encode_utf16().chain([0]).collect();
            let _ = AppendMenuW(snowfall, MF_STRING, (ID_SNOWFALL + i as u32) as usize, PCWSTR(title.as_ptr()));
        }
        let current = Snowfall::ALL.iter().position(|&s| s == settings.snowfall).unwrap_or(0) as u32;
        let _ = CheckMenuRadioItem(snowfall, ID_SNOWFALL, ID_SNOWFALL + 4, ID_SNOWFALL + current, MF_BYCOMMAND.0);
        let checked = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };

        let label_snowfall: Vec<u16> = (if is_ru { "Интенсивность" } else { "Snowfall" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_POPUP, snowfall.0 as usize, PCWSTR(label_snowfall.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let label_acc: Vec<u16> = (if is_ru { "Сугробы" } else { "Accumulate" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING | checked(settings.accumulate), ID_ACCUMULATE as usize, PCWSTR(label_acc.as_ptr()));

        let melt = MF_STRING | checked(settings.melt) | if settings.accumulate { MF_ENABLED } else { MF_GRAYED };
        let label_melt: Vec<u16> = (if is_ru { "Таяние снега" } else { "Melt Snow" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, melt, ID_MELT as usize, PCWSTR(label_melt.as_ptr()));

        let windows = MF_STRING | checked(settings.snow_on_windows) | if settings.behind_windows { MF_GRAYED } else { MF_ENABLED };
        let label_win: Vec<u16> = (if is_ru { "Снег на окнах" } else { "Snow on Windows" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, windows, ID_WINDOWS as usize, PCWSTR(label_win.as_ptr()));

        let label_clear: Vec<u16> = (if is_ru { "Очистить снег" } else { "Clear Snow" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING, ID_CLEAR as usize, PCWSTR(label_clear.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let label_behind: Vec<u16> = (if is_ru { "За окнами" } else { "Behind Windows" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING | checked(settings.behind_windows), ID_BEHIND as usize, PCWSTR(label_behind.as_ptr()));

        let label_shots: Vec<u16> = (if is_ru { "Видно на скриншотах" } else { "Show in Screenshots" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING | checked(settings.show_in_screenshots), ID_SCREENSHOTS as usize, PCWSTR(label_shots.as_ptr()));

        let label_login: Vec<u16> = (if is_ru { "Автозапуск при входе" } else { "Open at Login" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING | checked(login), ID_LOGIN as usize, PCWSTR(label_login.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let Ok(lang_menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(lang_menu, MF_STRING | checked(!is_ru), ID_LANG_EN as usize, w!("English"));
        let _ = AppendMenuW(lang_menu, MF_STRING | checked(is_ru), ID_LANG_RU as usize, w!("Русский"));
        let _ = CheckMenuRadioItem(lang_menu, ID_LANG_EN, ID_LANG_RU, if is_ru { ID_LANG_RU } else { ID_LANG_EN }, MF_BYCOMMAND.0);
        let label_lang: Vec<u16> = (if is_ru { "Язык" } else { "Language" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_POPUP, lang_menu.0 as usize, PCWSTR(label_lang.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let label_updates: Vec<u16> = (if is_ru { "Проверить обновления…" } else { "Check for Updates…" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING, ID_UPDATES as usize, PCWSTR(label_updates.as_ptr()));

        let label_quit: Vec<u16> = (if is_ru { "Выход" } else { "Quit" }).encode_utf16().chain([0]).collect();
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT as usize, PCWSTR(label_quit.as_ptr()));

        // A tray menu only closes on an outside click while its owner is in the foreground.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0, x, y, window, None).0 as u32;
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen
    };
    choose(window, chosen, login);
}

fn choose(window: HWND, chosen: u32, login: bool) {
    match chosen {
        0 => {}
        ID_QUIT => quit(window),
        ID_UPDATES => check_for_updates(window, true),
        ID_CLEAR => APP.with_borrow(|app| {
            if let Some(app) = app {
                send(app, Command::ClearSnow(!prefs::animations_enabled()));
            }
        }),
        ID_LOGIN => {
            let status = prefs::set_opens_at_login(!login);
            log!("open at login {}: {status:?}", !login);
        }
        ID_LANG_EN => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.language = snowdrift::card::Language::En;
            app.settings.save();
        }),
        ID_LANG_RU => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            app.settings.language = snowdrift::card::Language::Ru;
            app.settings.save();
        }),
        _ => APP.with_borrow_mut(|app| {
            let Some(app) = app else { return };
            let settings = &mut app.settings;
            let command = match chosen {
                ID_ACCUMULATE => {
                    settings.accumulate = !settings.accumulate;
                    Command::Accumulate(settings.accumulate)
                }
                ID_MELT => {
                    settings.melt = !settings.melt;
                    Command::Melt(settings.melt)
                }
                ID_WINDOWS => {
                    settings.snow_on_windows = !settings.snow_on_windows;
                    Command::SnowOnWindows(settings.snow_on_windows)
                }
                ID_BEHIND => {
                    settings.behind_windows = !settings.behind_windows;
                    Command::BehindWindows(settings.behind_windows)
                }
                ID_SCREENSHOTS => {
                    settings.show_in_screenshots = !settings.show_in_screenshots;
                    Command::ShowInScreenshots(settings.show_in_screenshots)
                }
                _ => {
                    let Some(&snowfall) = Snowfall::ALL.get(chosen.wrapping_sub(ID_SNOWFALL) as usize) else { return };
                    settings.snowfall = snowfall;
                    Command::Snowfall(snowfall)
                }
            };
            settings.save();
            send(app, command);
        }),
    }
}

fn check_for_updates(window: HWND, asked: bool) {
    let target = window.0 as isize;
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
            let text = if is_ru {
                format!(
                    "Доступна новая версия Snowdrift {} (у вас установлена {current}).\n\nЗагрузить и обновить сейчас?",
                    release.version
                )
            } else {
                format!(
                    "A new version of Snowdrift {} is available (you have {current}).\n\nDownload and update now?",
                    release.version
                )
            };
            if message(window, &text, MB_YESNO | MB_ICONINFORMATION) == IDYES {
                notify(window, "Snowdrift", if is_ru { "Загрузка обновления в фоне…" } else { "Downloading update in background…" });
                download_and_apply(window, release.url);
            }
        }
        Ok(None) if asked => {
            message(
                window,
                &if is_ru {
                    format!("У вас установлена последняя версия Snowdrift ({current}).")
                } else {
                    format!("You have the latest version of Snowdrift ({current}).")
                },
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Ok(None) => {}
        Err(error) => {
            log!("update check failed: {error}");
            if asked {
                message(
                    window,
                    &if is_ru {
                        format!("Не удалось проверить обновления.\n\n{error}")
                    } else {
                        format!("Failed to check for updates.\n\n{error}")
                    },
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }
}

fn download_and_apply(window: HWND, url: String) {
    let target = window.0 as isize;
    thread::spawn(move || {
        let res = Box::new(update::download_installer(&url));
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
    let is_ru = APP.with_borrow(|app| app.as_ref().map(|a| a.settings.language.is_ru())).unwrap_or(false);
    match result {
        Ok(installer) => {
            if let Err(error) = update::run_installer(&installer) {
                message(
                    window,
                    &if is_ru {
                        format!("Не удалось запустить установщик обновления:\n\n{error}")
                    } else {
                        format!("Failed to launch update installer:\n\n{error}")
                    },
                    MB_OK | MB_ICONERROR,
                );
            } else {
                quit(window);
            }
        }
        Err(error) => {
            message(
                window,
                &if is_ru {
                    format!("Не удалось загрузить обновление:\n\n{error}")
                } else {
                    format!("Failed to download update:\n\n{error}")
                },
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

fn message(window: HWND, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe { MessageBoxW(Some(window), PCWSTR(text.as_ptr()), w!("Snowdrift"), style | MB_SETFOREGROUND | MB_TOPMOST) }
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
