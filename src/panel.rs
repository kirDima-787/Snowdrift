//! Native Windows 11 style bottom tray context menu (Direct2D layered window).

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
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::prefs::Settings;
use snowdrift::card::Language;
use snowdrift::simulation::Snowfall;

const CLASS: PCWSTR = w!("SnowdriftPanel");
const WIDTH: f32 = 240.0;
const HEIGHT: f32 = 380.0;
const MARGIN: f32 = 16.0;
const ROW_H: f32 = 32.0;
const WM_MOUSELEAVE: u32 = 0x02A3;

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
    settings: Settings,
    login: bool,
    hovered: Option<RowKind>,
    rows: Vec<Row>,
    on_action: Rc<Box<dyn Fn(PanelAction)>>,
    d2d: ID2D1Factory,
    text: IDWriteFactory,
    imaging: IWICImagingFactory,
}

thread_local! {
    static PANEL: RefCell<Option<PanelWindow>> = const { RefCell::new(None) };
}

pub fn show(settings: Settings, login: bool, on_action: impl Fn(PanelAction) + 'static) {
    PANEL.with_borrow_mut(|panel| {
        if let Some(p) = panel.as_mut() {
            p.settings = settings;
            p.login = login;
            p.paint_and_present();
            unsafe {
                let _ = SetForegroundWindow(p.hwnd);
            }
            return;
        }
        if let Ok(mut new_panel) = PanelWindow::new(settings, login, Box::new(on_action)) {
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
            let _ = DestroyWindow(p.hwnd);
        }
    }
}

pub fn is_open() -> bool {
    PANEL.with_borrow(|p| p.is_some())
}

impl PanelWindow {
    fn new(settings: Settings, login: bool, on_action: Box<dyn Fn(PanelAction)>) -> Result<Self> {
        let _ = register_class();
        let (scale, origin) = placement(WIDTH, HEIGHT);
        let size = (((WIDTH + 2.0 * MARGIN) * scale).ceil() as i32, ((HEIGHT + 2.0 * MARGIN) * scale).ceil() as i32);

        unsafe {
            let imaging: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

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

            let mut panel = PanelWindow {
                hwnd,
                size,
                scale,
                settings,
                login,
                hovered: None,
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

    fn build_rows(&mut self) {
        let card_left = MARGIN;
        let card_top = MARGIN;
        let mut y = card_top + 6.0;

        self.rows.clear();
        let row_w = WIDTH - 8.0;
        let rx = card_left + 4.0;

        // Group 1: Snow simulation controls
        let kinds_group1 = [
            RowKind::Snowfall,
            RowKind::Accumulate,
            RowKind::Melt,
            RowKind::SnowOnWindows,
            RowKind::ClearSnow,
        ];
        for kind in kinds_group1 {
            self.rows.push(Row {
                kind,
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + ROW_H },
            });
            y += ROW_H;
        }

        y += 8.0; // Separator space
        // Group 2: Window / display settings
        let kinds_group2 = [
            RowKind::BehindWindows,
            RowKind::ShowInScreenshots,
            RowKind::OpenAtLogin,
        ];
        for kind in kinds_group2 {
            self.rows.push(Row {
                kind,
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + ROW_H },
            });
            y += ROW_H;
        }

        y += 8.0; // Separator space
        // Group 3: App lifecycle
        let kinds_group3 = [
            RowKind::Language,
            RowKind::CheckUpdates,
            RowKind::Quit,
        ];
        for kind in kinds_group3 {
            self.rows.push(Row {
                kind,
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + ROW_H },
            });
            y += ROW_H;
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

    fn draw(&self, target: &ID2D1RenderTarget) {
        let card = D2D_RECT_F { left: MARGIN, top: MARGIN, right: MARGIN + WIDTH, bottom: MARGIN + HEIGHT };
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };

        unsafe {
            // Windows 11 style smooth ambient drop shadow (14 passes with quadratic falloff)
            for step in 0..14 {
                let factor = 1.0 - (step as f32 / 14.0);
                let alpha = 0.032 * (factor * factor);
                let spread = (step as f32 + 1.0) * 1.15;
                let rect = D2D_RECT_F {
                    left: card.left - spread,
                    top: card.top - spread + 3.0,
                    right: card.right + spread,
                    bottom: card.bottom + spread + 3.0,
                };
                if let Ok(shade) = brush([0.0, 0.0, 0.0], alpha) {
                    target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: 8.0 + spread, radiusY: 8.0 + spread }, &shade);
                }
            }

            // Windows 11 dark acrylic background (#1f1f20 with 8px radius)
            let shape = D2D1_ROUNDED_RECT { rect: card, radiusX: 8.0, radiusY: 8.0 };
            if let Ok(bg) = brush([0.12, 0.12, 0.13], 0.96) {
                target.FillRoundedRectangle(&shape, &bg);
            }
            // Subtle 1px light border (matching Windows 11 context menu border)
            if let Ok(border) = brush([1.0, 1.0, 1.0], 0.09) {
                target.DrawRoundedRectangle(&shape, &border, 1.0, None);
            }

            // Draw items
            for row in &self.rows {
                let is_hovered = self.hovered == Some(row.kind);
                if is_hovered {
                    // Windows 11 rounded item hover highlight pill (inset by 4px, height 28px)
                    let item_rect = D2D_RECT_F {
                        left: row.rect.left + 4.0,
                        top: row.rect.top + 2.0,
                        right: row.rect.right - 4.0,
                        bottom: row.rect.bottom - 2.0,
                    };
                    if let Ok(hover_b) = brush([1.0, 1.0, 1.0], 0.08) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: item_rect, radiusX: 4.0, radiusY: 4.0 }, &hover_b);
                    }
                }

                // Left slot: checkmark for active toggle options, or action glyph
                self.draw_left_icon(target, row.kind, row.rect, is_hovered);

                // Item label: clean Windows 11 typography
                let text_x = row.rect.left + 36.0;
                let text_y = row.rect.top + 7.5;
                let label = self.row_label(row.kind);
                if let Ok(lbl_layout) = self.text_layout(&label, 12.5, WIDTH - 48.0) {
                    let col = if row.kind == RowKind::Quit && is_hovered {
                        [1.0, 0.45, 0.45]
                    } else if is_hovered {
                        [1.0, 1.0, 1.0]
                    } else {
                        [0.88, 0.88, 0.90]
                    };
                    if let Ok(lbl_b) = brush(col, 1.0) {
                        target.DrawTextLayout(Vector2 { X: text_x, Y: text_y }, &lbl_layout, &lbl_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                }
            }

            // Group separators: thin 1px subtle lines exactly like native Windows 11 menus
            if let Ok(sep) = brush([1.0, 1.0, 1.0], 0.08) {
                let sep1_y = card.top + 6.0 + (ROW_H * 5.0) + 4.0;
                target.DrawLine(Vector2 { X: card.left + 8.0, Y: sep1_y }, Vector2 { X: card.right - 8.0, Y: sep1_y }, &sep, 1.0, None);

                let sep2_y = sep1_y + 8.0 + (ROW_H * 3.0);
                target.DrawLine(Vector2 { X: card.left + 8.0, Y: sep2_y }, Vector2 { X: card.right - 8.0, Y: sep2_y }, &sep, 1.0, None);
            }
        }
    }

    fn draw_left_icon(&self, target: &ID2D1RenderTarget, kind: RowKind, row_rect: D2D_RECT_F, is_hovered: bool) {
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let icon_x = row_rect.left + 14.0;
        let icon_y = row_rect.top + 7.5;
        let cy = row_rect.top + (row_rect.bottom - row_rect.top) / 2.0;

        match kind {
            // Checkable toggle options: native Windows 11 checkmark ON THE LEFT when active
            RowKind::Accumulate => {
                if self.settings.accumulate {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            RowKind::Melt => {
                if self.settings.melt && self.settings.accumulate {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            RowKind::SnowOnWindows => {
                if self.settings.snow_on_windows {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            RowKind::BehindWindows => {
                if self.settings.behind_windows {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            RowKind::ShowInScreenshots => {
                if self.settings.show_in_screenshots {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            RowKind::OpenAtLogin => {
                if self.login {
                    self.draw_checkmark(target, icon_x, cy);
                }
            }
            // Action items: Segoe Fluent Icons on the left
            RowKind::Snowfall => {
                if let Ok(layout) = self.icon_layout("\u{E9CA}", 13.5) {
                    let col = if is_hovered { [1.0, 1.0, 1.0] } else { [0.85, 0.85, 0.88] };
                    if let Ok(b) = brush(col, 0.95) {
                        unsafe {
                            target.DrawTextLayout(Vector2 { X: icon_x, Y: icon_y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
            }
            RowKind::ClearSnow => {
                if let Ok(layout) = self.icon_layout("\u{E74D}", 13.5) {
                    let col = if is_hovered { [1.0, 1.0, 1.0] } else { [0.85, 0.85, 0.88] };
                    if let Ok(b) = brush(col, 0.95) {
                        unsafe {
                            target.DrawTextLayout(Vector2 { X: icon_x, Y: icon_y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
            }
            RowKind::Language => {
                if let Ok(layout) = self.icon_layout("\u{E774}", 13.5) {
                    let col = if is_hovered { [1.0, 1.0, 1.0] } else { [0.85, 0.85, 0.88] };
                    if let Ok(b) = brush(col, 0.95) {
                        unsafe {
                            target.DrawTextLayout(Vector2 { X: icon_x, Y: icon_y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
            }
            RowKind::CheckUpdates => {
                if let Ok(layout) = self.icon_layout("\u{E72C}", 13.5) {
                    let col = if is_hovered { [1.0, 1.0, 1.0] } else { [0.85, 0.85, 0.88] };
                    if let Ok(b) = brush(col, 0.95) {
                        unsafe {
                            target.DrawTextLayout(Vector2 { X: icon_x, Y: icon_y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
            }
            RowKind::Quit => {
                if let Ok(layout) = self.icon_layout("\u{E711}", 13.5) {
                    let col = if is_hovered { [1.0, 0.45, 0.45] } else { [0.85, 0.85, 0.88] };
                    if let Ok(b) = brush(col, 0.95) {
                        unsafe {
                            target.DrawTextLayout(Vector2 { X: icon_x, Y: icon_y }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
            }
        }
    }

    fn draw_checkmark(&self, target: &ID2D1RenderTarget, x: f32, cy: f32) {
        unsafe {
            if let Ok(brush) = target.CreateSolidColorBrush(&color([1.0, 1.0, 1.0], 0.95), None) {
                target.DrawLine(
                    Vector2 { X: x + 1.0, Y: cy + 0.5 },
                    Vector2 { X: x + 4.5, Y: cy + 4.0 },
                    &brush,
                    1.6,
                    None,
                );
                target.DrawLine(
                    Vector2 { X: x + 4.5, Y: cy + 4.0 },
                    Vector2 { X: x + 10.5, Y: cy - 3.5 },
                    &brush,
                    1.6,
                    None,
                );
            }
        }
    }

    fn row_label(&self, kind: RowKind) -> String {
        let is_ru = self.settings.language.is_ru();
        match kind {
            RowKind::Snowfall => {
                if is_ru {
                    let title = match self.settings.snowfall {
                        Snowfall::Off => "Выкл",
                        Snowfall::Light => "Мало",
                        Snowfall::Steady => "Обычный",
                        Snowfall::Heavy => "Сильный",
                        Snowfall::Variable => "Переменный",
                    };
                    format!("Снегопад: {title}")
                } else {
                    let title = match self.settings.snowfall {
                        Snowfall::Off => "Off",
                        Snowfall::Light => "Light",
                        Snowfall::Steady => "Steady",
                        Snowfall::Heavy => "Heavy",
                        Snowfall::Variable => "Variable",
                    };
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

    fn icon_layout(&self, glyph: &str, size: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = glyph.encode_utf16().collect();
        unsafe {
            let format = self.text.CreateTextFormat(
                w!("Segoe Fluent Icons"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            ).or_else(|_| {
                self.text.CreateTextFormat(
                    w!("Segoe MDL2 Assets"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
            })?;
            let layout = self.text.CreateTextLayout(&units, &format, size * 2.0, size * 2.0)?;
            Ok(layout)
        }
    }

    fn text_layout(&self, text: &str, size: f32, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let format = self.text.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("ru-ru"),
            ).or_else(|_| {
                self.text.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("ru-ru"),
                )
            })?;
            let layout = self.text.CreateTextLayout(&units, &format, width, 40.0)?;
            Ok(layout)
        }
    }

    fn dips(&self, point: POINT) -> (f32, f32) {
        (point.x as f32 / self.scale, point.y as f32 / self.scale)
    }

    fn handle_click(&mut self, point: (f32, f32)) {
        for row in &self.rows {
            if contains(&row.rect, point) {
                let action = match row.kind {
                    RowKind::Snowfall => {
                        let next = match self.settings.snowfall {
                            Snowfall::Off => Snowfall::Light,
                            Snowfall::Light => Snowfall::Steady,
                            Snowfall::Steady => Snowfall::Heavy,
                            Snowfall::Heavy => Snowfall::Variable,
                            Snowfall::Variable => Snowfall::Off,
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
                    RowKind::CheckUpdates => {
                        close();
                        PanelAction::CheckUpdates
                    }
                    RowKind::Quit => {
                        close();
                        PanelAction::Quit
                    }
                };
                self.paint_and_present();
                (self.on_action)(action);
                return;
            }
        }
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
            PANEL.with_borrow_mut(|panel| {
                let Some(p) = panel.as_mut() else { return };
                let at = p.dips(pt);
                p.handle_click(at);
            });
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
        let panel_w = ((width + 2.0 * MARGIN) * scale).ceil() as i32;

        // Position horizontally centered on cursor, clamped within work area with 8px margin
        let x = (cursor.x - panel_w / 2).clamp(work.left + 8, work.right - panel_w - 8);
        // Position snugly above taskbar: visible bottom of card is exactly 12px above taskbar top (work.bottom)
        let y = work.bottom - (12.0 * scale) as i32 - ((MARGIN + height) * scale) as i32;

        (scale, POINT { x, y: y.max(work.top + 8) })
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
