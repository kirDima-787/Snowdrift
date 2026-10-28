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

const CLASS: PCWSTR = w!("SnowdriftToast");
const WIDTH: f32 = 330.0;
const HEIGHT: f32 = 120.0;
const MARGIN: f32 = 24.0;
const TIMER_CLOSE: usize = 101;
const TIMER_PULSE: usize = 102;
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon-256.png");

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

struct ToastWindow {
    hwnd: HWND,
    size: (i32, i32),
    scale: f32,
    state: ToastState,
    is_ru: bool,
    hover_close: bool,
    hover_btn: bool,
    btn_rect: Option<D2D_RECT_F>,
    close_rect: D2D_RECT_F,
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
        t.paint_and_present();
    });
}

pub fn update_state(state: ToastState) {
    TOAST.with_borrow_mut(|toast| {
        let Some(t) = toast.as_mut() else { return };
        t.state = state;
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
        let (scale, origin) = placement(WIDTH, HEIGHT);
        let size = (((WIDTH + 2.0 * MARGIN) * scale).ceil() as i32, ((HEIGHT + 2.0 * MARGIN) * scale).ceil() as i32);

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

            let close_rect = D2D_RECT_F {
                left: MARGIN + WIDTH - 28.0,
                top: MARGIN + 10.0,
                right: MARGIN + WIDTH - 6.0,
                bottom: MARGIN + 32.0,
            };

            Ok(ToastWindow {
                hwnd,
                size,
                scale,
                state,
                is_ru,
                hover_close: false,
                hover_btn: false,
                btn_rect: None,
                close_rect,
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
        let (scale, origin) = placement(WIDTH, HEIGHT);
        self.scale = scale;
        self.size = (((WIDTH + 2.0 * MARGIN) * scale).ceil() as i32, ((HEIGHT + 2.0 * MARGIN) * scale).ceil() as i32);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                origin.x,
                origin.y,
                self.size.0,
                self.size.1,
                SWP_NOACTIVATE,
            );
        }
    }

    fn schedule_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_CLOSE);
            let _ = KillTimer(Some(self.hwnd), TIMER_PULSE);
            match &self.state {
                ToastState::UpToDate(_) | ToastState::Error(_) => {
                    // Auto-close after 4.5 seconds
                    let _ = SetTimer(Some(self.hwnd), TIMER_CLOSE, 4500, None);
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

    fn draw(&mut self, target: &ID2D1RenderTarget) {
        let card = D2D_RECT_F { left: MARGIN, top: MARGIN, right: MARGIN + WIDTH, bottom: MARGIN + HEIGHT };
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };

        unsafe {
            // Liquid glass shadow: soft translucent stacked shadows
            if let Ok(shade) = brush([0.0, 0.0, 0.0], 0.035) {
                for step in 0..14 {
                    let spread = step as f32 * 1.5;
                    let rect = D2D_RECT_F {
                        left: card.left - spread,
                        top: card.top - spread + 6.0,
                        right: card.right + spread,
                        bottom: card.bottom + spread + 6.0,
                    };
                    target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: 14.0 + spread, radiusY: 14.0 + spread }, &shade);
                }
            }

            // Liquid glass body: dark frosted acrylic fill
            let shape = D2D1_ROUNDED_RECT { rect: card, radiusX: 14.0, radiusY: 14.0 };
            if let Ok(bg) = brush([0.11, 0.11, 0.13], 0.94) {
                target.FillRoundedRectangle(&shape, &bg);
            }
            // Glass border
            if let Ok(border) = brush([1.0, 1.0, 1.0], 0.14) {
                target.DrawRoundedRectangle(&shape, &border, 1.0, None);
            }

            // Header: Snowflake icon
            let icon_rect = D2D_RECT_F { left: card.left + 16.0, top: card.top + 13.0, right: card.left + 34.0, bottom: card.top + 31.0 };
            let mut icon_drawn = false;
            if let Some(wic_icon) = &self.icon {
                if let Ok(icon) = target.CreateBitmapFromWicBitmap(wic_icon, None) {
                    target.DrawBitmap(&icon, Some(&icon_rect), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    icon_drawn = true;
                }
            }
            if !icon_drawn {
                if let Ok(b) = brush([0.65, 0.85, 1.0], 0.95) {
                    let cx = icon_rect.left + 9.0;
                    let cy = icon_rect.top + 9.0;
                    let r = 6.5;
                    for a in [0.0f32, 1.0472f32, 2.0944f32] {
                        let dx = a.sin() * r;
                        let dy = a.cos() * r;
                        target.DrawLine(Vector2 { X: cx - dx, Y: cy - dy }, Vector2 { X: cx + dx, Y: cy + dy }, &b, 1.3, None);
                    }
                }
            }

            // Header title: "Snowdrift"
            if let Ok(title_layout) = self.layout("Snowdrift", w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0, 150.0) {
                if let Ok(white) = brush([1.0, 1.0, 1.0], 0.95) {
                    target.DrawTextLayout(Vector2 { X: card.left + 40.0, Y: card.top + 13.0 }, &title_layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }

            // Close button "×" in top-right
            let cx = self.close_rect.left + (self.close_rect.right - self.close_rect.left) / 2.0;
            let cy = self.close_rect.top + (self.close_rect.bottom - self.close_rect.top) / 2.0;
            if self.hover_close {
                if let Ok(hover_bg) = brush([1.0, 1.0, 1.0], 0.12) {
                    target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT { rect: self.close_rect, radiusX: 6.0, radiusY: 6.0 },
                        &hover_bg,
                    );
                }
            }
            let cross_color = if self.hover_close { [1.0, 1.0, 1.0] } else { [0.75, 0.75, 0.75] };
            if let Ok(cross_b) = brush(cross_color, 0.9) {
                let arm = 4.0;
                target.DrawLine(Vector2 { X: cx - arm, Y: cy - arm }, Vector2 { X: cx + arm, Y: cy + arm }, &cross_b, 1.4, None);
                target.DrawLine(Vector2 { X: cx - arm, Y: cy + arm }, Vector2 { X: cx + arm, Y: cy - arm }, &cross_b, 1.4, None);
            }

            // Separator line
            if let Ok(sep) = brush([1.0, 1.0, 1.0], 0.07) {
                target.DrawLine(
                    Vector2 { X: card.left + 16.0, Y: card.top + 38.0 },
                    Vector2 { X: card.right - 16.0, Y: card.top + 38.0 },
                    &sep,
                    1.0,
                    None,
                );
            }

            // State specific contents
            self.btn_rect = None;
            let content_y = card.top + 46.0;

            match &self.state {
                ToastState::Checking => {
                    let title = if self.is_ru { "Проверка обновлений…" } else { "Checking for updates…" };
                    let sub = if self.is_ru { "Поиск новой версии на сервере…" } else { "Connecting to GitHub…" };
                    self.draw_text_pair(target, card.left + 18.0, content_y, title, sub);

                    // Pulse progress line
                    let bar_w = WIDTH - 36.0;
                    let track = D2D_RECT_F {
                        left: card.left + 18.0,
                        top: card.top + 98.0,
                        right: card.left + 18.0 + bar_w,
                        bottom: card.top + 101.0,
                    };
                    if let Ok(track_b) = brush([1.0, 1.0, 1.0], 0.08) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: 1.5, radiusY: 1.5 }, &track_b);
                    }
                    let seg_w = 60.0;
                    let offset = ((self.pulse_tick as f32 * 3.5) % (bar_w + seg_w)) - seg_w;
                    let fill_left = (card.left + 18.0 + offset).clamp(track.left, track.right);
                    let fill_right = (card.left + 18.0 + offset + seg_w).clamp(track.left, track.right);
                    if fill_right > fill_left {
                        let fill_rect = D2D_RECT_F { left: fill_left, top: track.top, right: fill_right, bottom: track.bottom };
                        if let Ok(pulse_b) = brush([0.35, 0.70, 1.0], 0.8) {
                            target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: fill_rect, radiusX: 1.5, radiusY: 1.5 }, &pulse_b);
                        }
                    }
                }
                ToastState::UpToDate(ver) => {
                    // Green checkmark badge
                    let badge_center = Vector2 { X: card.left + 28.0, Y: content_y + 16.0 };
                    if let Ok(green_b) = brush([0.18, 0.75, 0.40], 1.0) {
                        target.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: badge_center.X, Y: badge_center.Y }, radiusX: 10.0, radiusY: 10.0 }, &green_b);
                        if let Ok(check_b) = brush([1.0, 1.0, 1.0], 1.0) {
                            target.DrawLine(Vector2 { X: badge_center.X - 4.0, Y: badge_center.Y }, Vector2 { X: badge_center.X - 1.0, Y: badge_center.Y + 3.0 }, &check_b, 1.5, None);
                            target.DrawLine(Vector2 { X: badge_center.X - 1.0, Y: badge_center.Y + 3.0 }, Vector2 { X: badge_center.X + 4.5, Y: badge_center.Y - 3.5 }, &check_b, 1.5, None);
                        }
                    }
                    let title = if self.is_ru { "Обновление не требуется" } else { "Up to date" };
                    let sub = if self.is_ru {
                        format!("Установлена последняя версия ({ver})")
                    } else {
                        format!("You have the latest version ({ver})")
                    };
                    self.draw_text_pair(target, card.left + 46.0, content_y, title, &sub);
                }
                ToastState::Available { version, .. } => {
                    let title = if self.is_ru {
                        format!("Доступна версия {version}")
                    } else {
                        format!("Version {version} available")
                    };
                    let sub = if self.is_ru { "Нажмите, чтобы обновить сейчас" } else { "Click update to install now" };
                    self.draw_text_pair(target, card.left + 18.0, content_y, &title, sub);

                    // Update action button
                    let btn = D2D_RECT_F {
                        left: card.right - 108.0,
                        top: card.top + 72.0,
                        right: card.right - 18.0,
                        bottom: card.top + 102.0,
                    };
                    self.btn_rect = Some(btn);
                    let btn_fill = if self.hover_btn { [0.98, 0.96, 0.92] } else { [0.90, 0.88, 0.84] };
                    if let Ok(btn_b) = brush(btn_fill, 1.0) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: btn, radiusX: 5.0, radiusY: 5.0 }, &btn_b);
                    }
                    let label = if self.is_ru { "ОБНОВИТЬ" } else { "UPDATE" };
                    if let Ok(lbl_layout) = self.layout(label, w!("Segoe UI"), DWRITE_FONT_WEIGHT_BOLD, 10.5, 90.0) {
                        let _ = lbl_layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        let _ = lbl_layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        if let Ok(lbl_b) = brush([0.10, 0.08, 0.06], 1.0) {
                            target.DrawTextLayout(Vector2 { X: btn.left, Y: btn.top }, &lbl_layout, &lbl_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
                ToastState::Downloading { percent, downloaded, total } => {
                    let title = if self.is_ru {
                        format!("Скачивание обновления: {percent}%")
                    } else {
                        format!("Downloading update: {percent}%")
                    };
                    let dl_mb = *downloaded as f32 / (1024.0 * 1024.0);
                    let tot_mb = *total as f32 / (1024.0 * 1024.0);
                    let sub = if *total > 0 {
                        if self.is_ru {
                            format!("{dl_mb:.1} МБ из {tot_mb:.1} МБ")
                        } else {
                            format!("{dl_mb:.1} MB of {tot_mb:.1} MB")
                        }
                    } else {
                        if self.is_ru {
                            format!("{dl_mb:.1} МБ скачано")
                        } else {
                            format!("{dl_mb:.1} MB downloaded")
                        }
                    };
                    self.draw_text_pair(target, card.left + 18.0, content_y, &title, &sub);

                    // Download progress bar
                    let bar_w = WIDTH - 36.0;
                    let track = D2D_RECT_F {
                        left: card.left + 18.0,
                        top: card.top + 96.0,
                        right: card.left + 18.0 + bar_w,
                        bottom: card.top + 102.0,
                    };
                    if let Ok(track_b) = brush([1.0, 1.0, 1.0], 0.12) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: 3.0, radiusY: 3.0 }, &track_b);
                    }
                    let fill_w = (bar_w * (*percent as f32 / 100.0)).clamp(4.0, bar_w);
                    let fill_rect = D2D_RECT_F {
                        left: track.left,
                        top: track.top,
                        right: track.left + fill_w,
                        bottom: track.bottom,
                    };
                    if let Ok(fill_b) = brush([0.25, 0.65, 1.0], 0.95) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: fill_rect, radiusX: 3.0, radiusY: 3.0 }, &fill_b);
                    }
                }
                ToastState::Downloaded => {
                    let title = if self.is_ru { "Обновление готово!" } else { "Update ready!" };
                    let sub = if self.is_ru { "Запуск программы установки…" } else { "Launching installer…" };
                    self.draw_text_pair(target, card.left + 18.0, content_y, title, sub);
                }
                ToastState::Error(err) => {
                    let title = if self.is_ru { "Ошибка обновления" } else { "Update failed" };
                    self.draw_text_pair(target, card.left + 18.0, content_y, title, err);
                }
            }
        }
    }

    fn draw_text_pair(&self, target: &ID2D1RenderTarget, x: f32, y: f32, title: &str, sub: &str) {
        unsafe {
            if let Ok(t_layout) = self.layout(title, w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, WIDTH - 40.0) {
                if let Ok(t_brush) = target.CreateSolidColorBrush(&color([1.0, 1.0, 1.0], 0.95), None) {
                    target.DrawTextLayout(Vector2 { X: x, Y: y }, &t_layout, &t_brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }
            if let Ok(s_layout) = self.layout(sub, w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 11.5, WIDTH - 40.0) {
                if let Ok(s_brush) = target.CreateSolidColorBrush(&color([0.75, 0.75, 0.78], 0.90), None) {
                    target.DrawTextLayout(Vector2 { X: x, Y: y + 20.0 }, &s_layout, &s_brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }
        }
    }

    fn layout(&self, text: &str, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let format = self.text.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
            let layout = self.text.CreateTextLayout(&units, &format, width, 200.0)?;
            Ok(layout)
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
                let over_btn = t.btn_rect.map_or(false, |r| contains(&r, (x, y)));

                if over_close != t.hover_close || over_btn != t.hover_btn {
                    t.hover_close = over_close;
                    t.hover_btn = over_btn;
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
                if contains(&t.close_rect, (x, y)) {
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
        WM_DESTROY => {
            LRESULT(0)
        }
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

        (scale, POINT {
            x: work.right - toast_w - (16.0 * scale) as i32,
            y: work.bottom - toast_h - (12.0 * scale) as i32,
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
