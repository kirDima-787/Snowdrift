//! Tray context panel: liquid-glass Direct2D layered window.
//! Windows is the main target (real blurred backdrop + fade/slide animation);
//! macOS / Linux presets live in `Theme` so the look is ready when the window code gets ported.

use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::prefs::Settings;
use snowdrift::card::Language;
use snowdrift::simulation::Snowfall;

const CLASS: PCWSTR = w!("SnowdriftPanel");
const MARGIN: f32 = 14.0; // transparent margin around the card (shadow lives here)
const ROW_COUNT: f32 = 11.0;
const WM_MOUSELEAVE: u32 = 0x02A3;
const ANIM_TIMER: usize = 201;
const HOLD_TIMER: usize = 202;
const ANIM_MS: f32 = 180.0;
const ANIM_SLIDE: f32 = 12.0; // px the panel travels while appearing
const CURSOR_GAP: f32 = 10.0; // distance between the mouse and the panel

/// Per-platform look. Windows is the one that is actually used right now.
#[derive(Clone, Copy)]
struct Theme {
    width: f32,
    row_h: f32,
    pad: f32,
    sep: f32,
    radius: f32,
    hover_radius: f32,
    font: PCWSTR,
    font_size: f32,
    hover_fill: [f32; 3],
    hover_alpha: f32,
    blur_radius: f32,        // backdrop blur, px at 100% scale
    tint: (f32, f32),        // body tint alpha (top, bottom) when the backdrop is blurred
}

impl Theme {
    fn windows() -> Self {
        Theme {
            width: 248.0, row_h: 25.0, pad: 6.0, sep: 9.0, radius: 12.0, hover_radius: 6.0,
            font: w!("Segoe UI Variable Text"), font_size: 12.0,
            hover_fill: [1.0, 1.0, 1.0], hover_alpha: 0.09,
            blur_radius: 7.0, tint: (0.58, 0.70),
        }
    }
    fn mac() -> Self {
        // macOS menu: tighter rows, accent-colored selection, stronger blur
        Theme {
            width: 236.0, row_h: 22.0, pad: 5.0, sep: 11.0, radius: 10.0, hover_radius: 5.0,
            font: w!("SF Pro Text"), font_size: 12.5,
            hover_fill: [0.04, 0.52, 1.0], hover_alpha: 1.0,
            blur_radius: 9.0, tint: (0.50, 0.62),
        }
    }
    fn linux() -> Self {
        // GNOME-like: smaller radius, soft hover
        Theme {
            width: 240.0, row_h: 26.0, pad: 6.0, sep: 9.0, radius: 8.0, hover_radius: 6.0,
            font: w!("Cantarell"), font_size: 12.0,
            hover_fill: [1.0, 1.0, 1.0], hover_alpha: 0.11,
            blur_radius: 6.0, tint: (0.64, 0.76),
        }
    }
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::mac()
        } else if cfg!(target_os = "linux") {
            Self::linux()
        } else {
            Self::windows()
        }
    }
    fn height(&self) -> f32 {
        self.pad * 2.0 + self.row_h * ROW_COUNT + self.sep * 2.0
    }
}

pub enum PanelAction {
    Snowfall(Snowfall),
    Accumulate(bool),
    Melt(bool),
    SnowOnWindows(bool),
    ClearSnow,
    BehindWindows(bool),
    ShowInScreenshots(bool),
    OpenAtLogin(bool),
    Language(Language),
    CheckUpdates,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Snowfall,
    Accumulate,
    Melt,
    SnowOnWindows,
    ClearSnow,
    BehindWindows,
    ShowInScreenshots,
    OpenAtLogin,
    Language,
    CheckUpdates,
    Quit,
}

struct Row {
    kind: RowKind,
    rect: D2D_RECT_F,
}

struct PanelWindow {
    hwnd: HWND,
    size: (i32, i32),
    scale: f32,
    theme: Theme,
    origin: POINT,       // final window position
    slide_dir: i32,      // +1: slides up (panel is above the mouse), -1: slides down
    anim: Option<Instant>,
    pixels: Vec<u8>,     // last rendered frame, reused while animating
    backdrop: Option<Vec<u8>>, // blurred screenshot of what is behind the panel
    settings: Settings,
    login: bool,
    hovered: Option<RowKind>,
    holding_snowfall: Option<Instant>,
    holding_snowfall_triggered: bool,
    rows: Vec<Row>,
    on_action: Rc<Box<dyn Fn(PanelAction)>>,
    d2d: ID2D1Factory,
    text: IDWriteFactory,
    imaging: IWICImagingFactory,
}

thread_local! {
    static PANEL: RefCell<Option<PanelWindow>> = const { RefCell::new(None) };
    static BLIZZARD_UNLOCKED: Cell<bool> = const { Cell::new(false) };
}

pub fn unlock_blizzard() {
    BLIZZARD_UNLOCKED.set(true);
}

pub fn show(at: POINT, settings: Settings, login: bool, on_action: impl Fn(PanelAction) + 'static) {
    PANEL.with_borrow_mut(|panel| {
        if let Some(p) = panel.as_mut() {
            p.settings = settings;
            p.login = login;
            p.on_action = Rc::new(Box::new(on_action));
            p.holding_snowfall = None;
            p.holding_snowfall_triggered = false;
            p.reposition(at);
            p.start_anim();
            p.paint_and_present();
            unsafe {
                let _ = ShowWindow(p.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(p.hwnd);
            }
            return;
        }
        if let Ok(mut new_panel) = PanelWindow::new(at, settings, login, Box::new(on_action)) {
            new_panel.start_anim();
            new_panel.paint_and_present();
            unsafe {
                let _ = ShowWindow(new_panel.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(new_panel.hwnd);
            }
            *panel = Some(new_panel);
        }
    });
}

pub fn close() {
    if let Some(p) = PANEL.take() {
        unsafe {
            let _ = KillTimer(Some(p.hwnd), ANIM_TIMER);
            let _ = KillTimer(Some(p.hwnd), HOLD_TIMER);
            let _ = ReleaseCapture();
            let _ = DestroyWindow(p.hwnd);
        }
    }
}

pub fn is_open() -> bool {
    PANEL.with_borrow(|p| p.is_some())
}

impl PanelWindow {
    fn new(at: POINT, settings: Settings, login: bool, on_action: Box<dyn Fn(PanelAction)>) -> Result<Self> {
        let _ = register_class();
        let theme = Theme::current();
        let (scale, origin, above) = placement(at, theme.width, theme.height());
        let size = window_size(&theme, scale);

        unsafe {
            let imaging: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

            // Created hidden, so the screenshot below does not contain the panel itself
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("Snowdrift Menu"),
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

            let backdrop = capture_backdrop(origin, size).map(|mut px| {
                let r = ((theme.blur_radius * scale) as usize).max(2);
                blur(&mut px, size.0 as usize, size.1 as usize, r, 3);
                px
            });

            let mut panel = PanelWindow {
                hwnd,
                size,
                scale,
                theme,
                origin,
                slide_dir: if above { 1 } else { -1 },
                anim: None,
                pixels: Vec::new(),
                backdrop,
                settings,
                login,
                hovered: None,
                holding_snowfall: None,
                holding_snowfall_triggered: false,
                rows: Vec::new(),
                on_action: Rc::new(on_action),
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                text: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                imaging,
            };
            panel.build_rows();
            Ok(panel)
        }
    }

    fn reposition(&mut self, at: POINT) {
        let (scale, origin, above) = placement(at, self.theme.width, self.theme.height());
        self.scale = scale;
        self.origin = origin;
        self.slide_dir = if above { 1 } else { -1 };
        self.size = window_size(&self.theme, scale);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), origin.x, origin.y, self.size.0, self.size.1, SWP_NOACTIVATE);
        }
    }

    // ───────── animation: fade in + slide away from the mouse ─────────

    fn start_anim(&mut self) {
        self.anim = Some(Instant::now());
        unsafe {
            let _ = SetTimer(Some(self.hwnd), ANIM_TIMER, 8, None);
        }
    }

    /// (window alpha, vertical offset in px) for the current moment of the animation
    fn anim_state(&self) -> (u8, i32) {
        let Some(start) = self.anim else { return (255, 0) };
        let t = (start.elapsed().as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
        let e = 1.0 - (1.0 - t).powi(3); // ease-out cubic
        ((e * 255.0) as u8, ((1.0 - e) * ANIM_SLIDE * self.scale) as i32 * self.slide_dir)
    }

    fn tick_anim(&mut self) {
        let Some(start) = self.anim else { return };
        let done = start.elapsed().as_secs_f32() * 1000.0 >= ANIM_MS;
        if done {
            self.anim = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
            }
        }
        self.present_current();
    }

    /// Pushes the cached frame to the screen with the current animation alpha / offset (no re-render)
    fn present_current(&self) {
        if self.pixels.is_empty() {
            return;
        }
        let (alpha, dy) = self.anim_state();
        let position = POINT { x: self.origin.x, y: self.origin.y + dy };
        present(self.hwnd, self.size, &self.pixels, Some(position), alpha);
    }

    fn build_rows(&mut self) {
        let t = self.theme;
        let rx = MARGIN + 4.0;
        let row_w = t.width - 8.0;
        let mut y = MARGIN + t.pad;

        self.rows.clear();

        let groups: [&[RowKind]; 3] = [
            &[RowKind::Snowfall, RowKind::Accumulate, RowKind::Melt, RowKind::SnowOnWindows, RowKind::ClearSnow],
            &[RowKind::BehindWindows, RowKind::ShowInScreenshots, RowKind::OpenAtLogin],
            &[RowKind::Language, RowKind::CheckUpdates, RowKind::Quit],
        ];
        for (i, group) in groups.iter().enumerate() {
            if i > 0 {
                y += t.sep;
            }
            for &kind in group.iter() {
                self.rows.push(Row {
                    kind,
                    rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + t.row_h },
                });
                y += t.row_h;
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
                self.pixels = pixels;
                self.present_current();
            }
        }
    }

    fn draw(&self, target: &ID2D1RenderTarget) {
        let t = self.theme;
        let card = D2D_RECT_F { left: MARGIN, top: MARGIN, right: MARGIN + t.width, bottom: MARGIN + t.height() };
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };

        // Blurred screenshot of the desktop behind the panel (1 bitmap px == 1 window px)
        let backdrop = self.backdrop.as_ref().and_then(|px| unsafe {
            target
                .CreateBitmap(
                    D2D_SIZE_U { width: self.size.0 as u32, height: self.size.1 as u32 },
                    Some(px.as_ptr().cast()),
                    (self.size.0 * 4) as u32,
                    &D2D1_BITMAP_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                        dpiX: 96.0 * self.scale,
                        dpiY: 96.0 * self.scale,
                    },
                )
                .ok()
        });
        let tint = if backdrop.is_some() { t.tint } else { (0.80, 0.88) };
        draw_glass_card(target, card, t.radius, backdrop.as_ref(), tint);

        unsafe {
            let is_blizzard = self.settings.snowfall == Snowfall::Blizzard;
            for row in &self.rows {
                let is_hovered = self.hovered == Some(row.kind);
                let freeze_progress = if row.kind == RowKind::Snowfall {
                    if is_blizzard {
                        1.0
                    } else if let Some(start) = self.holding_snowfall {
                        let elapsed = start.elapsed().as_secs_f32();
                        // Freeze only begins when held past 350ms: prevents any flash on quick clicks
                        if elapsed < 0.35 {
                            0.0
                        } else {
                            ((elapsed - 0.35) / (3.0 - 0.35)).clamp(0.0, 1.0)
                        }
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                let item_rect = D2D_RECT_F { top: row.rect.top + 1.0, bottom: row.rect.bottom - 1.0, ..row.rect };
                let rounded = D2D1_ROUNDED_RECT { rect: item_rect, radiusX: t.hover_radius, radiusY: t.hover_radius };

                if freeze_progress > 0.0 {
                    let p = freeze_progress;
                    let cx = (item_rect.left + item_rect.right) / 2.0;

                    // 1. Soft ambient ice glow behind the button
                    if let Ok(glow_b) = brush([0.18, 0.60, 0.98], (if is_hovered { 0.35 } else { 0.22 }) * p) {
                        let glow_rect = D2D_RECT_F {
                            left: item_rect.left - 1.0,
                            top: item_rect.top - 0.5,
                            right: item_rect.right + 1.0,
                            bottom: item_rect.bottom + 1.5,
                        };
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: glow_rect, radiusX: t.hover_radius + 1.0, radiusY: t.hover_radius + 1.0 }, &glow_b);
                    }

                    // 2. Real 3D Ice Button Body (multi-stop glacial gradient)
                    let hover_add = if is_hovered { 0.15 } else { 0.0 };
                    if let Some(body) = vertical_gradient(
                        target, cx, item_rect.top, item_rect.bottom,
                        &[
                            (0.0, [0.25, 0.62, 0.94], (0.55 + hover_add) * p),
                            (0.48, [0.14, 0.44, 0.76], (0.70 + hover_add) * p),
                            (1.0, [0.08, 0.28, 0.54], (0.85 + hover_add) * p),
                        ],
                    ) {
                        target.FillRoundedRectangle(&rounded, &body);
                    }

                    // 3. Curved specular gloss highlight on upper 46% (glass refraction & tactile depth)
                    let gloss_rect = D2D_RECT_F {
                        left: item_rect.left + 1.0,
                        top: item_rect.top + 1.0,
                        right: item_rect.right - 1.0,
                        bottom: item_rect.top + (item_rect.bottom - item_rect.top) * 0.46,
                    };
                    if let Some(gloss) = vertical_gradient(
                        target, cx, gloss_rect.top, gloss_rect.bottom,
                        &[
                            (0.0, [0.88, 0.96, 1.0], 0.55 * p),
                            (0.80, [0.55, 0.82, 1.0], 0.15 * p),
                            (1.0, [0.40, 0.75, 1.0], 0.0),
                        ],
                    ) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: gloss_rect, radiusX: t.hover_radius - 1.0, radiusY: t.hover_radius - 1.0 }, &gloss);
                    }

                    // 4. Dimensional crystalline 3D bevel rim (top illuminated, bottom bevel)
                    if let Some(border) = vertical_gradient(
                        target, cx, item_rect.top, item_rect.bottom,
                        &[
                            (0.0, [0.88, 0.96, 1.0], 0.90 * p),
                            (0.45, [0.48, 0.80, 1.0], 0.55 * p),
                            (1.0, [0.18, 0.48, 0.80], 0.70 * p),
                        ],
                    ) {
                        target.DrawRoundedRectangle(&rounded, &border, 1.4, None);
                    }
                } else if is_hovered {
                    let (fill_col, fill_alpha) = if row.kind == RowKind::Quit {
                        ([1.0, 0.35, 0.35], 0.18)
                    } else {
                        (t.hover_fill, t.hover_alpha)
                    };
                    if let Ok(hover_b) = brush(fill_col, fill_alpha) {
                        target.FillRoundedRectangle(&rounded, &hover_b);
                    }
                }

                self.draw_left_icon(target, row.kind, row.rect, is_hovered, freeze_progress);

                let label = self.row_label(row.kind);
                if let Ok(lbl_layout) = self.text_layout(&label, t.font_size, t.width - 40.0) {
                    let col = if row.kind == RowKind::Quit && is_hovered {
                        [1.0, 0.55, 0.55]
                    } else if row.kind == RowKind::Snowfall && freeze_progress > 0.0 {
                        // Crisp ice-white text with high legibility
                        [
                            0.94 - 0.08 * freeze_progress,
                            0.97 + 0.03 * freeze_progress,
                            1.0,
                        ]
                    } else if is_hovered {
                        [1.0, 1.0, 1.0]
                    } else {
                        [0.93, 0.94, 0.97]
                    };
                    if let Ok(lbl_b) = brush(col, 1.0) {
                        target.DrawTextLayout(Vector2 { X: card.left + 32.0, Y: row.rect.top }, &lbl_layout, &lbl_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                }
            }

            // Hairline separators between groups
            if let Ok(sep) = brush([1.0, 1.0, 1.0], 0.10) {
                let sep1_y = card.top + t.pad + t.row_h * 5.0 + t.sep / 2.0;
                let sep2_y = sep1_y + t.sep + t.row_h * 3.0;
                for y in [sep1_y, sep2_y] {
                    target.DrawLine(Vector2 { X: card.left + 10.0, Y: y }, Vector2 { X: card.right - 10.0, Y: y }, &sep, 1.0, None);
                }
            }
        }
    }

    fn draw_left_icon(&self, target: &ID2D1RenderTarget, kind: RowKind, row_rect: D2D_RECT_F, is_hovered: bool, freeze_progress: f32) {
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let cx = MARGIN + 17.0;
        let cy = row_rect.top + self.theme.row_h / 2.0;

        let col = if kind == RowKind::Quit && is_hovered {
            [1.0, 0.45, 0.45]
        } else if kind == RowKind::Snowfall && freeze_progress > 0.0 {
            // Glowing ice-blue snowflake
            [
                0.85 - 0.45 * freeze_progress,
                0.88 + 0.04 * freeze_progress,
                0.92 + 0.08 * freeze_progress,
            ]
        } else if is_hovered {
            [1.0, 1.0, 1.0]
        } else {
            [0.85, 0.88, 0.92]
        };
        let alpha = if is_hovered { 1.0 } else { 0.90 };
        let Ok(b) = brush(col, alpha) else { return };

        // For checkmarks, bright WindBet cyan-white accent
        let check_col = [1.0, 1.0, 1.0];
        let Ok(check_b) = brush(check_col, 1.0) else { return };

        match kind {
            // Checkable toggle options: native checkmark ON THE LEFT when active
            RowKind::Accumulate => {
                if self.settings.accumulate {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            RowKind::Melt => {
                if self.settings.melt && self.settings.accumulate {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            RowKind::SnowOnWindows => {
                if self.settings.snow_on_windows {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            RowKind::BehindWindows => {
                if self.settings.behind_windows {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            RowKind::ShowInScreenshots => {
                if self.settings.show_in_screenshots {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            RowKind::OpenAtLogin => {
                if self.login {
                    self.draw_vector_checkmark(target, &check_b, cx, cy);
                }
            }
            // Action items: precision vector icon pack
            RowKind::Snowfall => {
                if freeze_progress > 0.2 {
                    if let Ok(glow_b) = brush([0.38, 0.76, 1.0], 0.35 * freeze_progress) {
                        self.draw_vector_snowflake(target, &glow_b, cx, cy);
                    }
                }
                self.draw_vector_snowflake(target, &b, cx, cy);
            }
            RowKind::ClearSnow => {
                self.draw_vector_broom(target, &b, cx, cy);
            }
            RowKind::Language => {
                self.draw_vector_globe(target, &b, cx, cy);
            }
            RowKind::CheckUpdates => {
                self.draw_vector_sync(target, &b, cx, cy);
            }
            RowKind::Quit => {
                self.draw_vector_exit(target, &b, cx, cy);
            }
        }
    }

    fn draw_vector_checkmark(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            target.DrawLine(
                Vector2 { X: cx - 4.5, Y: cy },
                Vector2 { X: cx - 1.2, Y: cy + 3.5 },
                brush,
                1.8,
                None,
            );
            target.DrawLine(
                Vector2 { X: cx - 1.2, Y: cy + 3.5 },
                Vector2 { X: cx + 4.5, Y: cy - 3.2 },
                brush,
                1.8,
                None,
            );
        }
    }

    fn draw_vector_snowflake(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            let r = 5.2f32;
            for a in [0.0f32, 1.0472f32, 2.0944f32] {
                let dx = a.sin() * r;
                let dy = a.cos() * r;
                target.DrawLine(Vector2 { X: cx - dx, Y: cy - dy }, Vector2 { X: cx + dx, Y: cy + dy }, brush, 1.3, None);
                let bdx = (a + 0.6).sin() * 1.8;
                let bdy = (a + 0.6).cos() * 1.8;
                target.DrawLine(Vector2 { X: cx + dx * 0.6, Y: cy + dy * 0.6 }, Vector2 { X: cx + dx * 0.6 + bdx, Y: cy + dy * 0.6 + bdy }, brush, 1.1, None);
                target.DrawLine(Vector2 { X: cx - dx * 0.6, Y: cy - dy * 0.6 }, Vector2 { X: cx - dx * 0.6 - bdx, Y: cy - dy * 0.6 - bdy }, brush, 1.1, None);
            }
        }
    }

    fn draw_vector_broom(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            target.DrawLine(Vector2 { X: cx + 4.0, Y: cy - 5.0 }, Vector2 { X: cx - 1.0, Y: cy + 0.5 }, brush, 1.6, None);
            target.DrawLine(Vector2 { X: cx - 1.0, Y: cy + 0.5 }, Vector2 { X: cx - 5.0, Y: cy + 3.5 }, brush, 1.4, None);
            target.DrawLine(Vector2 { X: cx - 1.0, Y: cy + 0.5 }, Vector2 { X: cx - 3.5, Y: cy + 5.0 }, brush, 1.4, None);
            target.DrawLine(Vector2 { X: cx - 1.0, Y: cy + 0.5 }, Vector2 { X: cx - 1.5, Y: cy + 5.5 }, brush, 1.4, None);
        }
    }

    fn draw_vector_globe(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            let r = 5.2f32;
            target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, brush, 1.3, None);
            target.DrawLine(Vector2 { X: cx - r, Y: cy }, Vector2 { X: cx + r, Y: cy }, brush, 1.1, None);
            target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r * 0.45, radiusY: r }, brush, 1.1, None);
        }
    }

    fn draw_vector_sync(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            let r = 4.8f32;
            target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, brush, 1.3, None);
            target.DrawLine(Vector2 { X: cx + r - 1.0, Y: cy - 2.0 }, Vector2 { X: cx + r + 2.0, Y: cy + 1.0 }, brush, 1.3, None);
            target.DrawLine(Vector2 { X: cx + r + 2.0, Y: cy + 1.0 }, Vector2 { X: cx + r - 2.5, Y: cy + 2.0 }, brush, 1.3, None);
        }
    }

    fn draw_vector_exit(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, cx: f32, cy: f32) {
        unsafe {
            let arm = 3.8f32;
            target.DrawLine(Vector2 { X: cx - arm, Y: cy - arm }, Vector2 { X: cx + arm, Y: cy + arm }, brush, 1.5, None);
            target.DrawLine(Vector2 { X: cx - arm, Y: cy + arm }, Vector2 { X: cx + arm, Y: cy - arm }, brush, 1.5, None);
        }
    }

    fn row_label(&self, kind: RowKind) -> String {
        let is_ru = self.settings.language.is_ru();
        match kind {
            RowKind::Snowfall => {
                let title = match self.settings.snowfall {
                    Snowfall::Off => if is_ru { "Выкл" } else { "Off" },
                    Snowfall::Light => if is_ru { "Мало" } else { "Light" },
                    Snowfall::Steady => if is_ru { "Обычный" } else { "Steady" },
                    Snowfall::Heavy => if is_ru { "Сильный" } else { "Heavy" },
                    Snowfall::Variable => if is_ru { "Переменный" } else { "Variable" },
                    Snowfall::Blizzard => if is_ru { "Метель" } else { "Blizzard" },
                };
                if is_ru {
                    format!("Снегопад: {title}")
                } else {
                    format!("Snowfall: {title}")
                }
            }
            RowKind::Accumulate => if is_ru { "Сугробы".into() } else { "Accumulate".into() },
            RowKind::Melt => if is_ru { "Таяние снега".into() } else { "Melt Snow".into() },
            RowKind::SnowOnWindows => if is_ru { "Снег на окнах".into() } else { "Snow on Windows".into() },
            RowKind::ClearSnow => if is_ru { "Очистить снег".into() } else { "Clear Snow".into() },
            RowKind::BehindWindows => if is_ru { "За окнами".into() } else { "Behind Windows".into() },
            RowKind::ShowInScreenshots => if is_ru { "На скриншотах".into() } else { "In Screenshots".into() },
            RowKind::OpenAtLogin => if is_ru { "Автозапуск".into() } else { "Open at Login".into() },
            RowKind::Language => if is_ru { "Язык: Русский".into() } else { "Language: English".into() },
            RowKind::CheckUpdates => if is_ru { "Проверить обновления…".into() } else { "Check for Updates…".into() },
            RowKind::Quit => if is_ru { "Выход".into() } else { "Quit".into() },
        }
    }

    fn text_layout(&self, text: &str, size: f32, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let make = |family: PCWSTR| {
                self.text.CreateTextFormat(
                    family,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("ru-ru"),
                )
            };
            // Theme font first, then safe Windows fallbacks
            let format = make(self.theme.font)
                .or_else(|_| make(w!("Segoe UI Variable Text")))
                .or_else(|_| make(w!("Segoe UI")))?;
            let layout = self.text.CreateTextLayout(&units, &format, width, self.theme.row_h)?;
            // Vertically centered in the row, never wraps
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Ok(layout)
        }
    }

    fn dips(&self, point: POINT) -> (f32, f32) {
        (point.x as f32 / self.scale, point.y as f32 / self.scale)
    }

    fn handle_click(&mut self, point: (f32, f32)) -> Option<PanelAction> {
        for row in &self.rows {
            if contains(&row.rect, point) {
                let action = match row.kind {
                    RowKind::Snowfall => {
                        let next = match self.settings.snowfall {
                            Snowfall::Off => Snowfall::Light,
                            Snowfall::Light => Snowfall::Steady,
                            Snowfall::Steady => Snowfall::Heavy,
                            Snowfall::Heavy => Snowfall::Variable,
                            Snowfall::Variable => {
                                if BLIZZARD_UNLOCKED.get() || self.settings.snowfall == Snowfall::Blizzard {
                                    Snowfall::Blizzard
                                } else {
                                    Snowfall::Off
                                }
                            }
                            Snowfall::Blizzard => Snowfall::Off,
                        };
                        self.settings.snowfall = next;
                        PanelAction::Snowfall(next)
                    }
                    RowKind::Accumulate => {
                        self.settings.accumulate = !self.settings.accumulate;
                        PanelAction::Accumulate(self.settings.accumulate)
                    }
                    RowKind::Melt => {
                        self.settings.melt = !self.settings.melt;
                        PanelAction::Melt(self.settings.melt)
                    }
                    RowKind::SnowOnWindows => {
                        self.settings.snow_on_windows = !self.settings.snow_on_windows;
                        PanelAction::SnowOnWindows(self.settings.snow_on_windows)
                    }
                    RowKind::ClearSnow => PanelAction::ClearSnow,
                    RowKind::BehindWindows => {
                        self.settings.behind_windows = !self.settings.behind_windows;
                        PanelAction::BehindWindows(self.settings.behind_windows)
                    }
                    RowKind::ShowInScreenshots => {
                        self.settings.show_in_screenshots = !self.settings.show_in_screenshots;
                        PanelAction::ShowInScreenshots(self.settings.show_in_screenshots)
                    }
                    RowKind::OpenAtLogin => {
                        self.login = !self.login;
                        PanelAction::OpenAtLogin(self.login)
                    }
                    RowKind::Language => {
                        self.settings.language = self.settings.language.toggle();
                        PanelAction::Language(self.settings.language)
                    }
                    RowKind::CheckUpdates => PanelAction::CheckUpdates,
                    RowKind::Quit => PanelAction::Quit,
                };
                return Some(action);
            }
        }
        None
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
        WM_ACTIVATE if (wparam.0 & 0xffff) == WA_INACTIVE as usize => {
            close();
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            close();
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            close();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == ANIM_TIMER => {
            PANEL.with_borrow_mut(|panel| {
                if let Some(p) = panel.as_mut() {
                    p.tick_anim();
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == HOLD_TIMER => {
            let triggered_action = PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return None };
                let Some(start) = p.holding_snowfall else {
                    unsafe { let _ = KillTimer(Some(hwnd), HOLD_TIMER); }
                    return None;
                };
                let elapsed = start.elapsed().as_secs_f32();
                let mut action = None;
                if elapsed >= 3.0 && !p.holding_snowfall_triggered {
                    p.holding_snowfall_triggered = true;
                    BLIZZARD_UNLOCKED.set(true);
                    p.settings.snowfall = Snowfall::Blizzard;
                    action = Some((PanelAction::Snowfall(Snowfall::Blizzard), p.on_action.clone()));
                }
                // Only repaint when freezing is actively visible (> 0.35s) or triggered
                if elapsed >= 0.35 || p.holding_snowfall_triggered {
                    p.paint_and_present();
                }
                action
            });
            if let Some((action, cb)) = triggered_action {
                cb(action);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let pt = point(lparam);
            PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return };
                let at = p.dips(pt);
                for row in &p.rows {
                    if contains(&row.rect, at) && row.kind == RowKind::Snowfall {
                        p.holding_snowfall = Some(Instant::now());
                        p.holding_snowfall_triggered = false;
                        unsafe {
                            let _ = SetCapture(hwnd);
                            let _ = SetTimer(Some(hwnd), HOLD_TIMER, 30, None);
                        }
                        // Do NOT call paint_and_present() here, so a fast click has zero freezing outline/flash
                        break;
                    }
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }

            let pt = point(lparam);
            PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return };
                let (x, y) = p.dips(pt);
                let mut hovered = None;
                for row in &p.rows {
                    if contains(&row.rect, (x, y)) {
                        hovered = Some(row.kind);
                        break;
                    }
                }
                if hovered != p.hovered {
                    p.hovered = hovered;
                    p.paint_and_present();
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return };
                if p.hovered.is_some() {
                    p.hovered = None;
                    p.paint_and_present();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let pt = point(lparam);
            let action_and_cb = PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return None };
                let was_triggered = p.holding_snowfall_triggered;
                let was_holding = p.holding_snowfall.take();
                p.holding_snowfall_triggered = false;
                unsafe {
                    let _ = KillTimer(Some(hwnd), HOLD_TIMER);
                    let _ = ReleaseCapture();
                }

                // If Blizzard was activated during this hold, or if the user held the button past the freeze threshold (>350ms),
                // do NOT cycle the row on release! The button remains frozen and Blizzard stays active.
                if was_triggered || was_holding.map(|s| s.elapsed().as_secs_f32() >= 0.35).unwrap_or(false) {
                    p.paint_and_present();
                    return None;
                }

                let at = p.dips(pt);
                let action = p.handle_click(at)?;
                let cb = p.on_action.clone();
                match action {
                    PanelAction::CheckUpdates | PanelAction::Quit => {}
                    _ => p.paint_and_present(),
                }
                Some((action, cb))
            });

            if let Some((action, cb)) = action_and_cb {
                if matches!(action, PanelAction::CheckUpdates | PanelAction::Quit) {
                    close();
                }
                cb(action);
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

fn window_size(theme: &Theme, scale: f32) -> (i32, i32) {
    (
        ((theme.width + 2.0 * MARGIN) * scale).ceil() as i32,
        ((theme.height() + 2.0 * MARGIN) * scale).ceil() as i32,
    )
}

/// Puts the card right above the mouse (centered on it). If there is no room above,
/// it opens below the mouse. Never overlaps the taskbar. Returns (dpi scale, window origin, opened_above).
fn placement(at: POINT, width: f32, height: f32) -> (f32, POINT, bool) {
    unsafe {
        // Real mouse position = where the user just clicked. The tray anchor is only used when the
        // menu was opened from the keyboard (mouse is far away from the tray icon).
        let mut cursor = POINT::default();
        let have_cursor = GetCursorPos(&mut cursor).is_ok();
        let anchor_set = at.x != 0 || at.y != 0;
        let pos = if !have_cursor {
            at
        } else if anchor_set && ((at.x - cursor.x).abs() > 300 || (at.y - cursor.y).abs() > 300) {
            at
        } else {
            cursor
        };

        let monitor = MonitorFromPoint(pos, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut _y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        let scale = dpi.max(96) as f32 / 96.0;
        let work = info.rcWork;
        let card_w = (width * scale).ceil() as i32;
        let card_h = (height * scale).ceil() as i32;
        let edge = (8.0 * scale) as i32;
        let gap = (CURSOR_GAP * scale) as i32;

        // Horizontally: centered on the mouse, kept inside the work area
        let min_x = work.left + edge;
        let max_x = (work.right - card_w - edge).max(min_x);
        let card_x = (pos.x - card_w / 2).clamp(min_x, max_x);

        // Vertically: above the mouse (but not lower than the taskbar edge)
        let bottom = (pos.y - gap).min(work.bottom - edge);
        let mut card_y = bottom - card_h;
        let mut above = true;
        if card_y < work.top + edge {
            // no room above -> open below the mouse
            above = false;
            let max_y = (work.bottom - card_h - edge).max(work.top + edge);
            card_y = (pos.y + gap).clamp(work.top + edge, max_y);
        }

        let m = (MARGIN * scale) as i32;
        (scale, POINT { x: card_x - m, y: card_y - m }, above)
    }
}

/// Screenshot of the screen area under the (still hidden) panel window, BGRA, alpha forced to 255.
fn capture_backdrop(origin: POINT, size: (i32, i32)) -> Option<Vec<u8>> {
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
        let mut out = None;
        if let Ok(bitmap) = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            if BitBlt(memory, 0, 0, size.0, size.1, Some(screen), origin.x, origin.y, SRCCOPY | CAPTUREBLT).is_ok() && !bits.is_null() {
                let len = (size.0 * size.1 * 4) as usize;
                let mut px = std::slice::from_raw_parts(bits.cast::<u8>() as *const u8, len).to_vec();
                for a in px.chunks_exact_mut(4) {
                    a[3] = 255;
                }
                out = Some(px);
            }
            SelectObject(memory, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        out
    }
}

/// Cheap gaussian-ish blur: `passes` x (horizontal + vertical) box blur on the BGR channels.
fn blur(pixels: &mut [u8], w: usize, h: usize, radius: usize, passes: usize) {
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return;
    }
    let mut tmp = vec![0u8; pixels.len()];
    for _ in 0..passes {
        blur_pass(pixels, &mut tmp, w, h, radius, true);
        blur_pass(&tmp, pixels, w, h, radius, false);
    }
}

fn blur_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines, step_i, step_l) = if horizontal { (w, h, 4, w * 4) } else { (h, w, w * 4, 4) };
    let div = (2 * r + 1) as u32;
    let last = len as isize - 1;
    let r = r as isize;
    for line in 0..lines {
        let base = line * step_l;
        for c in 0..3 {
            let at = |i: isize| base + (i.clamp(0, last) as usize) * step_i + c;
            let mut sum: u32 = 0;
            for i in -r..=r {
                sum += src[at(i)] as u32;
            }
            for i in 0..=last {
                dst[base + (i as usize) * step_i + c] = (sum / div) as u8;
                sum += src[at(i + r + 1)] as u32;
                sum -= src[at(i - r)] as u32;
            }
        }
        // alpha channel stays opaque
        for i in 0..len {
            dst[base + i * step_i + 3] = 255;
        }
    }
}

fn present(hwnd: HWND, size: (i32, i32), pixels: &[u8], position: Option<POINT>, alpha: u8) {
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
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
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

/// Vertical gradient, top to bottom: stops = (position 0..1, rgb, alpha)
fn vertical_gradient(target: &ID2D1RenderTarget, x: f32, top: f32, bottom: f32, stops: &[(f32, [f32; 3], f32)]) -> Option<ID2D1LinearGradientBrush> {
    unsafe {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|&(position, rgb, alpha)| D2D1_GRADIENT_STOP { position, color: color(rgb, alpha) })
            .collect();
        let collection = target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: x, Y: top },
            endPoint: Vector2 { X: x, Y: bottom },
        };
        target.CreateLinearGradientBrush(&props, None, &collection).ok()
    }
}

/// Shared liquid-glass card (panel + toast): soft outer shadow, blurred backdrop (if given),
/// dark tint, top highlight and a thin light border. `tint` = body alpha (top, bottom).
pub(crate) fn draw_glass_card(target: &ID2D1RenderTarget, card: D2D_RECT_F, radius: f32, backdrop: Option<&ID2D1Bitmap>, tint: (f32, f32)) {
    unsafe {
        // Shadow is drawn as rings OUTSIDE the card so it never shows through the glass
        for step in 0..10 {
            let t = 1.0 - step as f32 / 10.0;
            let spread = 1.0 + step as f32;
            let rect = D2D_RECT_F {
                left: card.left - spread,
                top: card.top - spread,
                right: card.right + spread,
                bottom: card.bottom + spread + 2.0,
            };
            if let Ok(shade) = target.CreateSolidColorBrush(&color([0.0, 0.0, 0.0], 0.075 * t * t), None) {
                target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius + spread, radiusY: radius + spread }, &shade, 1.2, None);
            }
        }

        let cx = (card.left + card.right) / 2.0;
        let shape = D2D1_ROUNDED_RECT { rect: card, radiusX: radius, radiusY: radius };

        // Blurred desktop behind the glass
        if let Some(bitmap) = backdrop {
            if let Ok(bitmap_brush) = target.CreateBitmapBrush(bitmap, None, None) {
                target.FillRoundedRectangle(&shape, &bitmap_brush);
            }
        }

        // Dark tint, a bit lighter at the top
        if let Some(body) = vertical_gradient(
            target, cx, card.top, card.bottom,
            &[(0.0, [0.20, 0.20, 0.21], tint.0), (1.0, [0.10, 0.10, 0.11], tint.1)],
        ) {
            target.FillRoundedRectangle(&shape, &body);
        }

        // Soft highlight on the upper part
        let glow_rect = D2D_RECT_F {
            left: card.left + 1.0,
            top: card.top + 1.0,
            right: card.right - 1.0,
            bottom: card.top + (card.bottom - card.top) * 0.45,
        };
        if let Some(glow) = vertical_gradient(
            target, cx, glow_rect.top, glow_rect.bottom,
            &[(0.0, [1.0, 1.0, 1.0], 0.10), (1.0, [1.0, 1.0, 1.0], 0.0)],
        ) {
            target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: glow_rect, radiusX: radius - 1.0, radiusY: radius - 1.0 }, &glow);
        }

        // Border: bright on top, fading out at the bottom
        let border_rect = D2D_RECT_F { left: card.left + 0.5, top: card.top + 0.5, right: card.right - 0.5, bottom: card.bottom - 0.5 };
        if let Some(border) = vertical_gradient(
            target, cx, card.top, card.bottom,
            &[(0.0, [1.0, 1.0, 1.0], 0.30), (1.0, [1.0, 1.0, 1.0], 0.06)],
        ) {
            target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: border_rect, radiusX: radius - 0.5, radiusY: radius - 0.5 }, &border, 1.0, None);
        }
    }
}

fn color(rgb: [f32; 3], alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb[0], g: rgb[1], b: rgb[2], a: alpha }
}
