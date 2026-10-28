//! Modern Windows 11 style bottom tray flyout panel (Direct2D layered window).

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
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, Interface, PCWSTR, Result, w};
use windows_numerics::Vector2;

use snowdrift::card::Language;
use crate::prefs::Settings;
use snowdrift::simulation::Snowfall;

const CLASS: PCWSTR = w!("SnowdriftPanel");
const WIDTH: f32 = 280.0;
const HEIGHT: f32 = 440.0;
const MARGIN: f32 = 24.0;
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon-256.png");

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
    close_rect: D2D_RECT_F,
    rows: Vec<Row>,
    on_action: Rc<Box<dyn Fn(PanelAction)>>,
    d2d: ID2D1Factory,
    text: IDWriteFactory,
    imaging: IWICImagingFactory,
    icon: IWICBitmapSource,
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

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("Snowdrift Panel"),
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
                left: MARGIN + WIDTH - 26.0,
                top: MARGIN + 12.0,
                right: MARGIN + WIDTH - 8.0,
                bottom: MARGIN + 30.0,
            };

            let mut panel = PanelWindow {
                hwnd,
                size,
                scale,
                settings,
                login,
                hovered: None,
                close_rect,
                rows: Vec::new(),
                on_action: Rc::new(on_action),
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                text: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                imaging,
                icon: converter.cast()?,
            };
            panel.build_rows();
            Ok(panel)
        }
    }

    fn build_rows(&mut self) {
        let card_left = MARGIN;
        let card_top = MARGIN;
        let mut y = card_top + 48.0;
        let row_h = 32.0;

        self.rows.clear();
        let row_w = WIDTH - 16.0;
        let rx = card_left + 8.0;

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
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + row_h },
            });
            y += row_h + 2.0;
        }

        y += 8.0; // Separator space
        let kinds_group2 = [
            RowKind::BehindWindows,
            RowKind::ShowInScreenshots,
            RowKind::OpenAtLogin,
        ];
        for kind in kinds_group2 {
            self.rows.push(Row {
                kind,
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + row_h },
            });
            y += row_h + 2.0;
        }

        y += 8.0; // Separator space
        let kinds_group3 = [
            RowKind::Language,
            RowKind::CheckUpdates,
            RowKind::Quit,
        ];
        for kind in kinds_group3 {
            self.rows.push(Row {
                kind,
                rect: D2D_RECT_F { left: rx, top: y, right: rx + row_w, bottom: y + row_h },
            });
            y += row_h + 2.0;
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
            // Shadow
            if let Ok(shade) = brush([0.0, 0.0, 0.0], 0.04) {
                for step in 0..16 {
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

            // Glass background
            let shape = D2D1_ROUNDED_RECT { rect: card, radiusX: 14.0, radiusY: 14.0 };
            if let Ok(bg) = brush([0.11, 0.11, 0.13], 0.95) {
                target.FillRoundedRectangle(&shape, &bg);
            }
            if let Ok(border) = brush([1.0, 1.0, 1.0], 0.12) {
                target.DrawRoundedRectangle(&shape, &border, 1.0, None);
            }

            // Header Icon
            if let Ok(icon) = target.CreateBitmapFromWicBitmap(&self.icon, None) {
                let rect = D2D_RECT_F { left: card.left + 16.0, top: card.top + 13.0, right: card.left + 34.0, bottom: card.top + 31.0 };
                target.DrawBitmap(&icon, Some(&rect), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }

            // Header Title
            if let Ok(title_layout) = self.layout("Snowdrift", w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, 120.0) {
                if let Ok(white) = brush([1.0, 1.0, 1.0], 0.95) {
                    target.DrawTextLayout(Vector2 { X: card.left + 40.0, Y: card.top + 13.0 }, &title_layout, &white, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }

            // Version pill
            let ver = env!("CARGO_PKG_VERSION");
            let ver_txt = format!("v{ver}");
            let pill_rect = D2D_RECT_F { left: card.left + 104.0, top: card.top + 13.0, right: card.left + 148.0, bottom: card.top + 30.0 };
            if let Ok(pill_bg) = brush([1.0, 1.0, 1.0], 0.08) {
                target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: pill_rect, radiusX: 4.0, radiusY: 4.0 }, &pill_bg);
            }
            if let Ok(ver_layout) = self.layout(&ver_txt, w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 10.5, 44.0) {
                let _ = ver_layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                if let Ok(muted) = brush([0.7, 0.7, 0.75], 0.9) {
                    target.DrawTextLayout(Vector2 { X: pill_rect.left, Y: pill_rect.top + 1.0 }, &ver_layout, &muted, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }

            // Close button in header
            let cx = self.close_rect.left + (self.close_rect.right - self.close_rect.left) / 2.0;
            let cy = self.close_rect.top + (self.close_rect.bottom - self.close_rect.top) / 2.0;
            if let Ok(cross_b) = brush([0.7, 0.7, 0.7], 0.8) {
                let arm = 4.0;
                target.DrawLine(Vector2 { X: cx - arm, Y: cy - arm }, Vector2 { X: cx + arm, Y: cy + arm }, &cross_b, 1.3, None);
                target.DrawLine(Vector2 { X: cx - arm, Y: cy + arm }, Vector2 { X: cx + arm, Y: cy - arm }, &cross_b, 1.3, None);
            }

            // Header separator
            if let Ok(sep) = brush([1.0, 1.0, 1.0], 0.08) {
                target.DrawLine(Vector2 { X: card.left + 14.0, Y: card.top + 40.0 }, Vector2 { X: card.right - 14.0, Y: card.top + 40.0 }, &sep, 1.0, None);
            }

            // Draw each row
            for row in &self.rows {
                let is_hovered = self.hovered == Some(row.kind);
                if is_hovered {
                    if let Ok(hover_b) = brush([1.0, 1.0, 1.0], 0.06) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: row.rect, radiusX: 6.0, radiusY: 6.0 }, &hover_b);
                    }
                }

                let icon_x = row.rect.left + 10.0;
                let icon_y = row.rect.top + 8.0;
                self.draw_row_icon(target, row.kind, icon_x, icon_y);

                let text_x = row.rect.left + 36.0;
                let text_y = row.rect.top + 7.0;
                let (label, _sub) = self.row_labels(row.kind);
                if let Ok(lbl_layout) = self.layout(label, w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 12.0, 150.0) {
                    let col = if row.kind == RowKind::Quit { [1.0, 0.45, 0.45] } else { [0.93, 0.93, 0.95] };
                    if let Ok(lbl_b) = brush(col, 1.0) {
                        target.DrawTextLayout(Vector2 { X: text_x, Y: text_y }, &lbl_layout, &lbl_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                }

                // Control on right side
                self.draw_row_control(target, row.kind, row.rect);
            }

            // Group separators
            if let Ok(sep) = brush([1.0, 1.0, 1.0], 0.06) {
                let sep1_y = card.top + 48.0 + (34.0 * 5.0) + 3.0;
                target.DrawLine(Vector2 { X: card.left + 16.0, Y: sep1_y }, Vector2 { X: card.right - 16.0, Y: sep1_y }, &sep, 1.0, None);

                let sep2_y = sep1_y + 8.0 + (34.0 * 3.0) + 3.0;
                target.DrawLine(Vector2 { X: card.left + 16.0, Y: sep2_y }, Vector2 { X: card.right - 16.0, Y: sep2_y }, &sep, 1.0, None);
            }
        }
    }

    fn row_labels(&self, kind: RowKind) -> (&'static str, &'static str) {
        let is_ru = self.settings.language.is_ru();
        match kind {
            RowKind::Snowfall => if is_ru { ("Снегопад", "") } else { ("Snowfall", "") },
            RowKind::Accumulate => if is_ru { ("Сугробы", "") } else { ("Accumulate", "") },
            RowKind::Melt => if is_ru { ("Таяние снега", "") } else { ("Melt Snow", "") },
            RowKind::SnowOnWindows => if is_ru { ("Снег на окнах", "") } else { ("On Windows", "") },
            RowKind::ClearSnow => if is_ru { ("Очистить снег", "") } else { ("Clear Snow", "") },
            RowKind::BehindWindows => if is_ru { ("За окнами", "") } else { ("Behind Windows", "") },
            RowKind::ShowInScreenshots => if is_ru { ("На скриншотах", "") } else { ("In Screenshots", "") },
            RowKind::OpenAtLogin => if is_ru { ("Автозапуск", "") } else { ("Open at Login", "") },
            RowKind::Language => if is_ru { ("Язык", "") } else { ("Language", "") },
            RowKind::CheckUpdates => if is_ru { ("Обновления", "") } else { ("Updates", "") },
            RowKind::Quit => if is_ru { ("Выход", "") } else { ("Quit", "") },
        }
    }

    fn draw_row_icon(&self, target: &ID2D1RenderTarget, kind: RowKind, x: f32, y: f32) {
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let col = if kind == RowKind::Quit { [1.0, 0.45, 0.45] } else { [0.75, 0.80, 0.88] };
        let Ok(b) = brush(col, 0.95) else { return };

        unsafe {
            let cx = x + 7.0;
            let cy = y + 8.0;
            match kind {
                RowKind::Snowfall => {
                    // Small snowflake
                    for a in [0.0f32, 1.047, 2.094] {
                        let dx = a.cos() * 6.0;
                        let dy = a.sin() * 6.0;
                        target.DrawLine(Vector2 { X: cx - dx, Y: cy - dy }, Vector2 { X: cx + dx, Y: cy + dy }, &b, 1.2, None);
                    }
                }
                RowKind::Accumulate => {
                    // Drifts curves
                    target.DrawLine(Vector2 { X: cx - 6.0, Y: cy + 4.0 }, Vector2 { X: cx - 1.0, Y: cy - 2.0 }, &b, 1.4, None);
                    target.DrawLine(Vector2 { X: cx - 1.0, Y: cy - 2.0 }, Vector2 { X: cx + 2.0, Y: cy + 1.0 }, &b, 1.4, None);
                    target.DrawLine(Vector2 { X: cx + 2.0, Y: cy + 1.0 }, Vector2 { X: cx + 6.0, Y: cy - 3.0 }, &b, 1.4, None);
                }
                RowKind::Melt => {
                    // Teardrop
                    target.DrawLine(Vector2 { X: cx, Y: cy - 5.0 }, Vector2 { X: cx - 3.5, Y: cy + 2.0 }, &b, 1.3, None);
                    target.DrawLine(Vector2 { X: cx, Y: cy - 5.0 }, Vector2 { X: cx + 3.5, Y: cy + 2.0 }, &b, 1.3, None);
                    target.DrawLine(Vector2 { X: cx - 3.5, Y: cy + 2.0 }, Vector2 { X: cx + 3.5, Y: cy + 2.0 }, &b, 1.3, None);
                }
                RowKind::SnowOnWindows => {
                    // Window frame
                    let r = D2D_RECT_F { left: cx - 5.5, top: cy - 5.5, right: cx + 5.5, bottom: cy + 5.5 };
                    target.DrawRectangle(&r, &b, 1.2, None);
                    target.DrawLine(Vector2 { X: cx, Y: r.top }, Vector2 { X: cx, Y: r.bottom }, &b, 1.0, None);
                    target.DrawLine(Vector2 { X: r.left, Y: cy }, Vector2 { X: r.right, Y: cy }, &b, 1.0, None);
                }
                RowKind::ClearSnow => {
                    // Broom
                    target.DrawLine(Vector2 { X: cx + 4.0, Y: cy - 5.0 }, Vector2 { X: cx - 2.0, Y: cy + 2.0 }, &b, 1.5, None);
                    target.DrawLine(Vector2 { X: cx - 5.0, Y: cy + 4.0 }, Vector2 { X: cx, Y: cy + 4.0 }, &b, 1.8, None);
                }
                RowKind::BehindWindows => {
                    // Layers
                    target.DrawRectangle(&D2D_RECT_F { left: cx - 5.0, top: cy - 5.0, right: cx + 2.0, bottom: cy + 2.0 }, &b, 1.1, None);
                    target.DrawRectangle(&D2D_RECT_F { left: cx - 2.0, top: cy - 2.0, right: cx + 5.0, bottom: cy + 5.0 }, &b, 1.1, None);
                }
                RowKind::ShowInScreenshots => {
                    // Camera
                    let r = D2D_RECT_F { left: cx - 5.5, top: cy - 3.5, right: cx + 5.5, bottom: cy + 4.5 };
                    target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 1.5, radiusY: 1.5 }, &b, 1.1, None);
                    target.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy + 0.5 }, radiusX: 1.8, radiusY: 1.8 }, &b);
                }
                RowKind::OpenAtLogin => {
                    // Startup / Rocket
                    target.DrawLine(Vector2 { X: cx, Y: cy - 5.5 }, Vector2 { X: cx - 4.0, Y: cy + 2.5 }, &b, 1.3, None);
                    target.DrawLine(Vector2 { X: cx, Y: cy - 5.5 }, Vector2 { X: cx + 4.0, Y: cy + 2.5 }, &b, 1.3, None);
                    target.DrawLine(Vector2 { X: cx - 2.0, Y: cy + 4.5 }, Vector2 { X: cx + 2.0, Y: cy + 4.5 }, &b, 1.2, None);
                }
                RowKind::Language => {
                    // Globe
                    target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 5.5, radiusY: 5.5 }, &b, 1.1, None);
                    target.DrawLine(Vector2 { X: cx - 5.5, Y: cy }, Vector2 { X: cx + 5.5, Y: cy }, &b, 1.0, None);
                    target.DrawLine(Vector2 { X: cx, Y: cy - 5.5 }, Vector2 { X: cx, Y: cy + 5.5 }, &b, 1.0, None);
                }
                RowKind::CheckUpdates => {
                    // Circular sync
                    target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 5.0, radiusY: 5.0 }, &b, 1.2, None);
                }
                RowKind::Quit => {
                    // Power symbol
                    target.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy + 0.5 }, radiusX: 5.0, radiusY: 5.0 }, &b, 1.2, None);
                    target.DrawLine(Vector2 { X: cx, Y: cy - 4.5 }, Vector2 { X: cx, Y: cy + 0.5 }, &b, 1.4, None);
                }
            }
        }
    }

    fn draw_row_control(&self, target: &ID2D1RenderTarget, kind: RowKind, row_rect: D2D_RECT_F) {
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let right = row_rect.right - 10.0;
        let cy = row_rect.top + (row_rect.bottom - row_rect.top) / 2.0;

        unsafe {
            match kind {
                RowKind::Snowfall => {
                    let is_ru = self.settings.language.is_ru();
                    let title = if is_ru { self.settings.snowfall.title_ru() } else { self.settings.snowfall.title() };
                    let pill_w = 68.0;
                    let pill = D2D_RECT_F { left: right - pill_w, top: cy - 10.0, right, bottom: cy + 10.0 };
                    if let Ok(pill_b) = brush([1.0, 1.0, 1.0], 0.08) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: pill, radiusX: 4.0, radiusY: 4.0 }, &pill_b);
                    }
                    if let Ok(layout) = self.layout(title, w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 10.5, pill_w) {
                        let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        if let Ok(txt_b) = brush([0.85, 0.90, 0.95], 0.95) {
                            target.DrawTextLayout(Vector2 { X: pill.left, Y: pill.top + 1.0 }, &layout, &txt_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
                RowKind::Accumulate => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.settings.accumulate);
                }
                RowKind::Melt => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.settings.melt && self.settings.accumulate);
                }
                RowKind::SnowOnWindows => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.settings.snow_on_windows);
                }
                RowKind::ClearSnow => {
                    // Action chevron
                    if let Ok(ch_b) = brush([0.7, 0.7, 0.75], 0.8) {
                        target.DrawLine(Vector2 { X: right - 10.0, Y: cy - 4.0 }, Vector2 { X: right - 6.0, Y: cy }, &ch_b, 1.3, None);
                        target.DrawLine(Vector2 { X: right - 6.0, Y: cy }, Vector2 { X: right - 10.0, Y: cy + 4.0 }, &ch_b, 1.3, None);
                    }
                }
                RowKind::BehindWindows => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.settings.behind_windows);
                }
                RowKind::ShowInScreenshots => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.settings.show_in_screenshots);
                }
                RowKind::OpenAtLogin => {
                    self.draw_toggle_switch(target, right - 32.0, cy - 8.0, self.login);
                }
                RowKind::Language => {
                    let code = if self.settings.language.is_ru() { "RU" } else { "EN" };
                    let pill_w = 34.0;
                    let pill = D2D_RECT_F { left: right - pill_w, top: cy - 10.0, right, bottom: cy + 10.0 };
                    if let Ok(pill_b) = brush([1.0, 1.0, 1.0], 0.08) {
                        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: pill, radiusX: 4.0, radiusY: 4.0 }, &pill_b);
                    }
                    if let Ok(layout) = self.layout(code, w!("Segoe UI"), DWRITE_FONT_WEIGHT_BOLD, 10.5, pill_w) {
                        let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        if let Ok(txt_b) = brush([0.90, 0.90, 0.95], 0.95) {
                            target.DrawTextLayout(Vector2 { X: pill.left, Y: pill.top + 1.0 }, &layout, &txt_b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                    }
                }
                RowKind::CheckUpdates => {
                    if let Ok(ch_b) = brush([0.7, 0.7, 0.75], 0.8) {
                        target.DrawLine(Vector2 { X: right - 10.0, Y: cy - 4.0 }, Vector2 { X: right - 6.0, Y: cy }, &ch_b, 1.3, None);
                        target.DrawLine(Vector2 { X: right - 6.0, Y: cy }, Vector2 { X: right - 10.0, Y: cy + 4.0 }, &ch_b, 1.3, None);
                    }
                }
                RowKind::Quit => {}
            }
        }
    }

    fn draw_toggle_switch(&self, target: &ID2D1RenderTarget, x: f32, y: f32, on: bool) {
        let brush = |rgb: [f32; 3], alpha: f32| unsafe { target.CreateSolidColorBrush(&color(rgb, alpha), None) };
        let w = 32.0;
        let h = 16.0;
        let r = h / 2.0;
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        let shape = D2D1_ROUNDED_RECT { rect, radiusX: r, radiusY: r };

        unsafe {
            if on {
                if let Ok(on_bg) = brush([0.22, 0.60, 1.0], 0.95) {
                    target.FillRoundedRectangle(&shape, &on_bg);
                }
                let knob_x = x + w - r;
                let knob_y = y + r;
                if let Ok(white) = brush([1.0, 1.0, 1.0], 1.0) {
                    target.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: knob_x, Y: knob_y }, radiusX: r - 2.5, radiusY: r - 2.5 }, &white);
                }
            } else {
                if let Ok(off_bg) = brush([1.0, 1.0, 1.0], 0.08) {
                    target.FillRoundedRectangle(&shape, &off_bg);
                }
                if let Ok(border) = brush([1.0, 1.0, 1.0], 0.20) {
                    target.DrawRoundedRectangle(&shape, &border, 1.0, None);
                }
                let knob_x = x + r;
                let knob_y = y + r;
                if let Ok(muted_knob) = brush([0.7, 0.7, 0.75], 0.8) {
                    target.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: knob_x, Y: knob_y }, radiusX: r - 3.0, radiusY: r - 3.0 }, &muted_knob);
                }
            }
        }
    }

    fn layout(&self, text: &str, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32, width: f32) -> Result<IDWriteTextLayout> {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let format = self.text.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
            let layout = self.text.CreateTextLayout(&units, &format, width, 100.0)?;
            Ok(layout)
        }
    }

    fn dips(&self, point: POINT) -> (f32, f32) {
        (point.x as f32 / self.scale, point.y as f32 / self.scale)
    }

    fn handle_click(&mut self, point: (f32, f32)) {
        if contains(&self.close_rect, point) {
            close();
            return;
        }

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
        let panel_h = ((height + 2.0 * MARGIN) * scale).ceil() as i32;

        let x = (cursor.x - panel_w / 2).clamp(work.left + 8, work.right - panel_w - 8);
        let y = (work.bottom - panel_h - (8.0 * scale) as i32).max(work.top + 8);

        (scale, POINT { x, y })
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
