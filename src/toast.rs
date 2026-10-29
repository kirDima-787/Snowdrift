//! Liquid glass update notification toast window (Direct2D layered window).

use std::cell::RefCell;
use std::mem::size_of;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetMonitorInfoW, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory, WICBitmapCacheOnLoad, WICBitmapDitherTypeNone,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, Interface, PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::panel::draw_glass_card;

const CLASS: PCWSTR = w!("SnowdriftToast");
const WIDTH: f32 = 364.0; // same width as a native Windows 11 toast
const MARGIN: f32 = 14.0; // transparent margin for the shadow
const RADIUS: f32 = 12.0;
const EDGE_GAP: f32 = 16.0; // gap between the card and the screen edge / taskbar
const TIMER_CLOSE: usize = 101;
const TIMER_PULSE: usize = 102;
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon-256.png");
const ACCENT: [f32; 3] = [0.38, 0.80, 1.0]; // Windows 11 dark-mode accent

#[derive(Clone, Debug, PartialEq)]
pub enum ToastState {
    Checking,
    UpToDate(String),
    Available { version: String, url: String },
    Downloading { percent: u32, downloaded: u32, total: u32 },
    Downloaded,
    Error(String),
}

pub struct ToastCallbacks {
    pub on_update: Box<dyn Fn(String)>,
    pub on_close: Box<dyn Fn()>,
}

/// Card height like a native toast: grows with its content (2-line text, progress bar, buttons)
fn card_height(state: &ToastState) -> f32 {
    match state {
        ToastState::Checking | ToastState::Downloading { .. } => 120.0,
        ToastState::Available { .. } => 148.0,
        ToastState::Error(_) => 124.0,
        ToastState::UpToDate(_) | ToastState::Downloaded => 106.0,
    }
}

fn window_size(state: &ToastState, scale: f32) -> (i32, i32) {
    (
        ((WIDTH + 2.0 * MARGIN) * scale).ceil() as i32,
        ((card_height(state) + 2.0 * MARGIN) * scale).ceil() as i32,
    )
}

struct ToastWindow {
    hwnd: HWND,
    size: (i32, i32),
    scale: f32,
    state: ToastState,
    is_ru: bool,
    hover_close: bool,
    hover_more: bool,
    hover_btn: bool,
    hover_dismiss: bool,
    btn_rect: Option<D2D_RECT_F>,
    dismiss_rect: Option<D2D_RECT_F>,
    close_rect: D2D_RECT_F,
    more_rect: D2D_RECT_F,
    callbacks: Option<Rc<ToastCallbacks>>,
    pulse_tick: u32,
    d2d: ID2D1Factory,
    text: IDWriteFactory,
    imaging: IWICImagingFactory,
    icon: Option<IWICBitmapSource>,
}

thread_local! {
    static TOAST: RefCell<Option<ToastWindow>> = const { RefCell::new(None) };
}

pub fn show(state: ToastState, is_ru: bool, callbacks: ToastCallbacks) {
    let cb = Rc::new(callbacks);
    TOAST.with_borrow_mut(|toast| {
        if let Some(t) = toast.as_mut() {
            t.state = state;
            t.is_ru = is_ru;
            t.callbacks = Some(cb);
            t.reposition();
            t.schedule_timer();
            t.paint_and_present();
            unsafe {
                let _ = ShowWindow(t.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(t.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            return;
        }
        if let Ok(mut new_toast) = ToastWindow::new(state, is_ru, cb) {
            new_toast.schedule_timer();
            new_toast.paint_and_present();
            unsafe {
                let _ = ShowWindow(new_toast.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(new_toast.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            *toast = Some(new_toast);
        }
    });
}

pub fn update_progress(percent: u32, downloaded: u32, total: u32) {
    TOAST.with_borrow_mut(|toast| {
        let Some(t) = toast.as_mut() else { return };
        t.state = ToastState::Downloading { percent, downloaded, total };
        t.sync_size();
        t.paint_and_present();
    });
}

pub fn update_state(state: ToastState) {
    TOAST.with_borrow_mut(|toast| {
        let Some(t) = toast.as_mut() else { return };
        t.state = state;
        t.sync_size();
        t.schedule_timer();
        t.paint_and_present();
    });
}

pub fn close() {
    if let Some(t) = TOAST.take() {
        unsafe {
            let _ = KillTimer(Some(t.hwnd), TIMER_CLOSE);
            let _ = KillTimer(Some(t.hwnd), TIMER_PULSE);
            let _ = DestroyWindow(t.hwnd);
        }
        if let Some(cb) = t.callbacks {
            (cb.on_close)();
        }
    }
}

impl ToastWindow {
    fn new(state: ToastState, is_ru: bool, callbacks: Rc<ToastCallbacks>) -> Result<Self> {
        let _ = register_class();
        let (scale, origin) = placement(WIDTH, card_height(&state));
        let size = window_size(&state, scale);

        unsafe {
            let imaging: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let icon: Option<IWICBitmapSource> = (|| -> Result<IWICBitmapSource> {
                let stream = SHCreateMemStream(Some(ICON_BYTES)).ok_or_else(Error::empty)?;
                let decoder = imaging.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
                let converter = imaging.CreateFormatConverter()?;
                converter.Initialize(
                    &decoder.GetFrame(0)?,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )?;
                converter.cast()
            })().ok();

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("Snowdrift Update"),
                WS_POPUP,
                origin.x,
                origin.y,
                size.0,
                size.1,
                None,
                None,
                None,
                None,
            )?;

            // Header buttons (28x28, centered 27px / 67px from the right edge, 23px from the top)
            let header_btn = |center_from_right: f32| D2D_RECT_F {
                left: MARGIN + WIDTH - center_from_right - 14.0,
                top: MARGIN + 9.0,
                right: MARGIN + WIDTH - center_from_right + 14.0,
                bottom: MARGIN + 37.0,
            };

            Ok(ToastWindow {
                hwnd,
                size,
                scale,
                state,
                is_ru,
                hover_close: false,
                hover_more: false,
                hover_btn: false,
                hover_dismiss: false,
                btn_rect: None,
                dismiss_rect: None,
                close_rect: header_btn(27.0),
                more_rect: header_btn(67.0),
                callbacks: Some(callbacks),
                pulse_tick: 0,
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                text: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                imaging,
                icon,
            })
        }
    }

    fn reposition(&mut self) {
        let (scale, origin) = placement(WIDTH, card_height(&self.state));
        self.scale = scale;
        self.size = window_size(&self.state, scale);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), origin.x, origin.y, self.size.0, self.size.1, SWP_NOACTIVATE);
        }
    }

    /// The card height depends on the state (progress bar / buttons) — resize only when it changes
    fn sync_size(&mut self) {
        if window_size(&self.state, self.scale) != self.size {
            self.reposition();
        }
    }

    fn schedule_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_CLOSE);
            let _ = KillTimer(Some(self.hwnd), TIMER_PULSE);
            match &self.state {
                ToastState::UpToDate(_) | ToastState::Error(_) => {
                    // Auto-close after 5 seconds, like a native toast
                    let _ = SetTimer(Some(self.hwnd), TIMER_CLOSE, 5000, None);
                }
                ToastState::Checking => {
                    let _ = SetTimer(Some(self.hwnd), TIMER_PULSE, 40, None);
                }
                _ => {}
            }
        }
    }

    fn paint_and_present(&mut self) {
        let (width, height) = (self.size.0, self.size.1);
        unsafe {
            let Ok(bitmap) = self.imaging.CreateBitmap(width as u32, height as u32, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad) else { return };
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * self.scale,
                dpiY: 96.0 * self.scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let Ok(target) = self.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties) else { return };
            target.BeginDraw();
            target.Clear(Some(&color([0.0; 3], 0.0)));
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            self.draw(&target);

            let _ = target.EndDraw(None, None);
            drop(target);

            let mut pixels = vec![0u8; (width * height * 4) as usize];
            if bitmap.CopyPixels(std::ptr::null(), (width * 4) as u32, &mut pixels).is_ok() {
                present(self.hwnd, self.size, &pixels, None);
            }
        }
    }

    /// (title, body) text for the current state
    fn texts(&self) -> (String, String) {
        let ru = self.is_ru;
        match &self.state {
            ToastState::Checking => (
                if ru { "Проверка обновлений" } else { "Checking for updates" }.into(),
                if ru { "Поиск новой версии…" } else { "Looking for a new version…" }.into(),
            ),
            ToastState::UpToDate(ver) => (
                if ru { "Обновление не требуется" } else { "You're up to date" }.into(),
                if ru { format!("Установлена последняя версия ({ver})") } else { format!("You have the latest version ({ver})") },
            ),
            ToastState::Available { version, .. } => (
                if ru { format!("Доступна версия {version}") } else { format!("Version {version} is available") },
                if ru { "Готово к установке".into() } else { "Ready to install".into() },
            ),
            ToastState::Downloading { percent, downloaded, total } => {
                let dl = *downloaded as f32 / (1024.0 * 1024.0);
                let tot = *total as f32 / (1024.0 * 1024.0);
                let body = match (*total > 0, ru) {
                    (true, true) => format!("{dl:.1} МБ из {tot:.1} МБ"),
                    (true, false) => format!("{dl:.1} MB of {tot:.1} MB"),
                    (false, true) => format!("{dl:.1} МБ скачано"),
                    (false, false) => format!("{dl:.1} MB downloaded"),
                };
                (if ru { format!("Скачивание обновления: {percent}%") } else { format!("Downloading update: {percent}%") }, body)
            }
            ToastState::Downloaded => (
                if ru { "Обновление готово" } else { "Update ready" }.into(),
                if ru { "Запуск программы установки…" } else { "Launching the installer…" }.into(),
            ),
            ToastState::Error(err) => {
                let short: String = if err.chars().count() > 100 { err.chars().take(99).collect::<String>() + "…" } else { err.clone() };
                (if ru { "Ошибка обновления" } else { "Update failed" }.into(), short)
            }
        }
    }

    fn draw(&mut self, target: &ID2D1RenderTarget) {
        let card = D2D_RECT_F { left: MARGIN, top: MARGIN, right: MARGIN + WIDTH, bottom: MARGIN + card_height(&self.state) };
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let (title, body) = self.texts();

        // Native toasts are almost opaque dark acrylic
        draw_glass_card(target, card, RADIUS, None, (0.90, 0.94));

        unsafe {
            // ── Header: app icon + app name, "···" and "×" on the right ──
            self.draw_app_icon(target, D2D_RECT_F { left: card.left + 14.0, top: card.top + 15.0, right: card.left + 30.0, bottom: card.top + 31.0 });
            self.draw_text(target, "Snowdrift", card.left + 40.0, card.top + 15.0, 200.0, 16.0, 12.0, DWRITE_FONT_WEIGHT_NORMAL, [0.90, 0.90, 0.92], 0.95, true);

            for (rect, hovered, is_close) in [(self.more_rect, self.hover_more, false), (self.close_rect, self.hover_close, true)] {
                if hovered {
                    if let Ok(h) = brush([1.0, 1.0, 1.0], 0.09) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: 5.0, radiusY: 5.0 }, &h);
                    }
                }
                let (cx, cy) = ((rect.left + rect.right) / 2.0, (rect.top + rect.bottom) / 2.0);
                if let Ok(b) = brush([1.0, 1.0, 1.0], if hovered { 1.0 } else { 0.85 }) {
                    if is_close {
                        let arm = 4.2;
                        target.DrawLine(Vector2 { X: cx - arm, Y: cy - arm }, Vector2 { X: cx + arm, Y: cy + arm }, &b, 1.1, None);
                        target.DrawLine(Vector2 { X: cx - arm, Y: cy + arm }, Vector2 { X: cx + arm, Y: cy - arm }, &b, 1.1, None);
                    } else {
                        for dx in [-5.0f32, 0.0, 5.0] {
                            target.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx + dx, Y: cy }, radiusX: 1.15, radiusY: 1.15 }, &b);
                        }
                    }
                }
            }

            // ── Title and body ──
            self.draw_text(target, &title, card.left + 16.0, card.top + 48.0, WIDTH - 32.0, 22.0, 14.0, DWRITE_FONT_WEIGHT_NORMAL, [1.0, 1.0, 1.0], 1.0, false);
            self.draw_text(target, &body, card.left + 16.0, card.top + 70.0, WIDTH - 32.0, 40.0, 13.0, DWRITE_FONT_WEIGHT_NORMAL, [0.80, 0.80, 0.83], 0.95, false);

            self.btn_rect = None;
            self.dismiss_rect = None;
            let bar_left = card.left + 16.0;
            let bar_w = WIDTH - 32.0;
            let bar_top = card.top + 98.0;

            match &self.state {
                ToastState::Checking => {
                    let track = D2D_RECT_F { left: bar_left, top: bar_top, right: bar_left + bar_w, bottom: bar_top + 4.0 };
                    if let Ok(t) = brush([1.0, 1.0, 1.0], 0.12) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: 2.0, radiusY: 2.0 }, &t);
                    }
                    let seg_w = 90.0;
                    let offset = ((self.pulse_tick as f32 * 4.0) % (bar_w + seg_w)) - seg_w;
                    let l = (bar_left + offset).clamp(track.left, track.right);
                    let r = (bar_left + offset + seg_w).clamp(track.left, track.right);
                    if r > l {
                        if let Ok(f) = brush(ACCENT, 1.0) {
                            target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: l, top: track.top, right: r, bottom: track.bottom }, radiusX: 2.0, radiusY: 2.0 }, &f);
                        }
                    }
                }
                ToastState::Downloading { percent, .. } => {
                    let track = D2D_RECT_F { left: bar_left, top: bar_top, right: bar_left + bar_w, bottom: bar_top + 4.0 };
                    if let Ok(t) = brush([1.0, 1.0, 1.0], 0.12) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: 2.0, radiusY: 2.0 }, &t);
                    }
                    let fill_w = (bar_w * (*percent as f32 / 100.0)).clamp(4.0, bar_w);
                    if let Ok(f) = brush(ACCENT, 1.0) {
                        target.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: track.left, top: track.top, right: track.left + fill_w, bottom: track.bottom }, radiusX: 2.0, radiusY: 2.0 },
                            &f,
                        );
                    }
                }
                ToastState::Available { .. } => {
                    // Two equal buttons across the bottom, like native toast actions
                    let top = card.top + 100.0;
                    let mid = (card.left + card.right) / 2.0;
                    let update = D2D_RECT_F { left: card.left + 16.0, top, right: mid - 4.0, bottom: top + 32.0 };
                    let dismiss = D2D_RECT_F { left: mid + 4.0, top, right: card.right - 16.0, bottom: top + 32.0 };
                    self.btn_rect = Some(update);
                    self.dismiss_rect = Some(dismiss);

                    let accent = if self.hover_btn { [0.50, 0.85, 1.0] } else { ACCENT };
                    if let Ok(b) = brush(accent, 1.0) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: update, radiusX: 5.0, radiusY: 5.0 }, &b);
                    }
                    if let Ok(b) = brush([1.0, 1.0, 1.0], if self.hover_dismiss { 0.14 } else { 0.09 }) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: dismiss, radiusX: 5.0, radiusY: 5.0 }, &b);
                    }
                    if let Ok(b) = brush([1.0, 1.0, 1.0], 0.07) {
                        target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: dismiss, radiusX: 5.0, radiusY: 5.0 }, &b, 1.0, None);
                    }
                    let (l1, l2) = if self.is_ru { ("Обновить", "Закрыть") } else { ("Update", "Dismiss") };
                    self.draw_text_centered(target, l1, update, 13.0, [0.0, 0.0, 0.0]);
                    self.draw_text_centered(target, l2, dismiss, 13.0, [1.0, 1.0, 1.0]);
                }
                _ => {}
            }
        }
    }

    fn draw_app_icon(&self, target: &ID2D1RenderTarget, rect: D2D_RECT_F) {
        unsafe {
            if let Some(wic_icon) = &self.icon {
                if let Ok(icon) = target.CreateBitmapFromWicBitmap(wic_icon, None) {
                    target.DrawBitmap(&icon, Some(&rect), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    return;
                }
            }
            // Fallback snowflake if the icon failed to load
            if let Ok(b) = target.CreateSolidColorBrush(&color([0.75, 0.88, 1.0], 0.95), None) {
                let cx = (rect.left + rect.right) / 2.0;
                let cy = (rect.top + rect.bottom) / 2.0;
                let r = 6.0f32;
                for a in [0.0f32, 1.0472f32, 2.0944f32] {
                    let (dx, dy) = (a.sin() * r, a.cos() * r);
                    target.DrawLine(Vector2 { X: cx - dx, Y: cy - dy }, Vector2 { X: cx + dx, Y: cy + dy }, &b, 1.2, None);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(&self, target: &ID2D1RenderTarget, text: &str, x: f32, y: f32, width: f32, height: f32, size: f32, weight: DWRITE_FONT_WEIGHT, rgb: [f32; 3], alpha: f32, vcenter: bool) {
        unsafe {
            let Ok(layout) = self.layout(text, weight, size, width) else { return };
            let _ = layout.SetMaxHeight(height);
            if vcenter {
                let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            }
            if let Ok(b) = target.CreateSolidColorBrush(&color(rgb, alpha), None) {
                target.DrawTextLayout(Vector2 { X: x, Y: y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
    }

    fn draw_text_centered(&self, target: &ID2D1RenderTarget, text: &str, rect: D2D_RECT_F, size: f32, rgb: [f32; 3]) {
        unsafe {
            let Ok(layout) = self.layout(text, DWRITE_FONT_WEIGHT_NORMAL, size, rect.right - rect.left) else { return };
            let _ = layout.SetMaxHeight(rect.bottom - rect.top);
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            if let Ok(b) = target.CreateSolidColorBrush(&color(rgb, 1.0), None) {
                target.DrawTextLayout(Vector2 { X: rect.left, Y: rect.top }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
    }

    fn layout(&self, text: &str, weight: DWRITE_FONT_WEIGHT, size: f32, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let make = |family: PCWSTR| {
                self.text.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("ru-ru"))
            };
            // Same font as the system toasts, with a fallback for older Windows
            let format = make(w!("Segoe UI Variable Text")).or_else(|_| make(w!("Segoe UI")))?;
            self.text.CreateTextLayout(&units, &format, width, 200.0)
        }
    }

    fn dips(&self, point: POINT) -> (f32, f32) {
        (point.x as f32 / self.scale, point.y as f32 / self.scale)
    }
}

fn register_class() -> Result<()> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
    });
    Ok(())
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == TIMER_CLOSE => {
            close();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_PULSE => {
            TOAST.with_borrow_mut(|toast| {
                if let Some(t) = toast.as_mut() {
                    t.pulse_tick = t.pulse_tick.wrapping_add(1);
                    t.paint_and_present();
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let pt = point(lparam);
            TOAST.with_borrow_mut(|toast| {
                let Some(t) = toast.as_mut() else { return };
                let (x, y) = t.dips(pt);
                let over_close = contains(&t.close_rect, (x, y));
                let over_more = contains(&t.more_rect, (x, y));
                let over_btn = t.btn_rect.map_or(false, |r| contains(&r, (x, y)));
                let over_dismiss = t.dismiss_rect.map_or(false, |r| contains(&r, (x, y)));

                if over_close != t.hover_close || over_more != t.hover_more || over_btn != t.hover_btn || over_dismiss != t.hover_dismiss {
                    t.hover_close = over_close;
                    t.hover_more = over_more;
                    t.hover_btn = over_btn;
                    t.hover_dismiss = over_dismiss;
                    t.paint_and_present();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let pt = point(lparam);
            let action = TOAST.with_borrow_mut(|toast| {
                let Some(t) = toast.as_mut() else { return None };
                let (x, y) = t.dips(pt);
                if contains(&t.close_rect, (x, y)) || t.dismiss_rect.map_or(false, |r| contains(&r, (x, y))) {
                    Some(0)
                } else if t.btn_rect.map_or(false, |r| contains(&r, (x, y))) {
                    Some(1)
                } else {
                    None
                }
            });
            match action {
                Some(0) => close(),
                Some(1) => {
                    let url_opt = TOAST.with_borrow(|toast| {
                        if let Some(t) = toast.as_ref() {
                            if let ToastState::Available { url, .. } = &t.state {
                                return Some((url.clone(), t.callbacks.clone()));
                            }
                        }
                        None
                    });
                    if let Some((url, Some(cb))) = url_opt {
                        (cb.on_update)(url);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn point(lparam: LPARAM) -> POINT {
    POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 }
}

fn contains(rect: &D2D_RECT_F, (x, y): (f32, f32)) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn placement(width: f32, height: f32) -> (f32, POINT) {
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut _y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        let scale = dpi.max(96) as f32 / 96.0;
        let work = info.rcWork;
        let toast_w = ((width + 2.0 * MARGIN) * scale).ceil() as i32;
        let toast_h = ((height + 2.0 * MARGIN) * scale).ceil() as i32;

        // Bottom-right corner above the taskbar; the card (not the shadow margin) sits EDGE_GAP from the edges
        (scale, POINT {
            x: work.right - toast_w + ((MARGIN - EDGE_GAP) * scale) as i32,
            y: work.bottom - toast_h + ((MARGIN - EDGE_GAP) * scale) as i32,
        })
    }
}

fn present(hwnd: HWND, size: (i32, i32), pixels: &[u8], position: Option<POINT>) {
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        if let Ok(bitmap) = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let size = SIZE { cx: size.0, cy: size.1 };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                position.as_ref().map(|p| p as *const POINT),
                Some(&size),
                Some(memory),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(memory, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
}

fn color(rgb: [f32; 3], alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb[0], g: rgb[1], b: rgb[2], a: alpha }
}
