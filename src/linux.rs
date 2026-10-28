//! Linux backend for Snowdrift using pure Rust x11rb.
//! Creates a click-through transparent full-screen overlay on X11 / XWayland
//! and simulates falling snow across the screen and on top of open windows.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::shape;
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto::*;

use snowdrift::geometry::{Rect, V2};
use snowdrift::simulation::{SnowSimulation, Snowfall, Sprite, WindowSurface};

pub fn run() {
    println!("Snowdrift 1.0.0 (Linux)");
    println!("Connecting to X11 display...");

    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to X11 server: {e}");
            eprintln!("Make sure an X11 or XWayland display is running (DISPLAY environment variable).");
            return;
        }
    };

    let screen = &conn.setup().roots[screen_num];
    let width = screen.width_in_pixels;
    let height = screen.height_in_pixels;
    println!("Display size: {}x{}", width, height);

    // Look for a 32-bit ARGB visual for transparency
    let mut visual_id = screen.root_visual;
    let mut depth = 24u8;
    for d in &screen.allowed_depths {
        if d.depth == 32 {
            if let Some(vis) = d.visuals.first() {
                visual_id = vis.visual_id;
                depth = 32;
                break;
            }
        }
    }

    let colormap = conn.generate_id().unwrap();
    if depth == 32 {
        let _ = conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id);
    } else {
        println!("Note: 32-bit visual not found, falling back to 24-bit.");
    }

    let window = conn.generate_id().unwrap();
    let values = CreateWindowAux::new()
        .background_pixel(0)
        .border_pixel(0)
        .override_redirect(1) // Bypass window manager borders
        .colormap(if depth == 32 { colormap } else { 0 });

    if let Err(e) = conn.create_window(
        depth,
        window,
        screen.root,
        0,
        0,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &values,
    ) {
        eprintln!("Failed to create overlay window: {e}");
        return;
    }

    // Set click-through via XFixes input shape: clicks fall straight through to windows below
    if let Ok(region) = conn.generate_id() {
        let _ = xfixes::create_region(&conn, region, &[]);
        let _ = xfixes::set_window_shape_region(&conn, window, shape::SK::INPUT, 0, 0, region);
        let _ = xfixes::destroy_region(&conn, region);
    }

    // Map window
    let _ = conn.map_window(window);
    let _ = conn.flush();

    // Graphics context
    let gc = conn.generate_id().unwrap();
    let gc_aux = CreateGCAux::new().foreground(0xFF_FF_FF_FF);
    let _ = conn.create_gc(gc, window, &gc_aux);

    println!("Snowdrift is now running! Press Ctrl+C in terminal to stop.");

    let mut sim = SnowSimulation::new(width as f32, height as f32, Snowfall::Steady);
    let mut last_frame = Instant::now();
    let mut sprites = Vec::<Sprite>::new();
    let mut pixmap_buffer = vec![0u8; (width as usize) * (height as usize) * 4];

    let parked = HashSet::new();
    let elsewhere = HashSet::new();

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f32().min(0.1);
        last_frame = now;

        // Query cursor position
        let cursor = if let Ok(reply) = conn.query_pointer(screen.root) {
            if let Ok(ptr) = reply.reply() {
                V2::new(ptr.root_x as f32, ptr.root_y as f32)
            } else {
                V2::new(-1000.0, -1000.0)
            }
        } else {
            V2::new(-1000.0, -1000.0)
        };

        // Query top-level windows
        let mut surfaces = Vec::new();
        if let Ok(tree) = conn.query_tree(screen.root) {
            if let Ok(tree_reply) = tree.reply() {
                for &child in &tree_reply.children {
                    if child == window {
                        continue;
                    }
                    if let Ok(geom_cookie) = conn.get_geometry(child) {
                        if let Ok(geom) = geom_cookie.reply() {
                            if geom.width > 50 && geom.height > 50 {
                                surfaces.push(WindowSurface {
                                    id: child as isize,
                                    frame: Rect {
                                        x: geom.x as f32,
                                        y: geom.y as f32,
                                        width: geom.width as f32,
                                        height: geom.height as f32,
                                    },
                                    blockers: Vec::new(),
                                    corner_radius: 8.0,
                                    corner_exponent: 2.0,
                                });
                            }
                        }
                    }
                }
            }
        }

        sim.update_windows(&surfaces, dt, &parked, &elsewhere, false);
        sim.step(dt, cursor);

        let count = sim.sprite_count();
        if sprites.len() < count {
            sprites.resize(count, Sprite::default());
        }
        sim.write_sprites(&mut sprites[..count]);

        // Clear buffer
        pixmap_buffer.fill(0);

        // Draw sprites
        let w_usize = width as usize;
        let h_usize = height as usize;
        for s in &sprites[..count] {
            let px = s.position.x as isize;
            let py = s.position.y as isize;
            let radius = (s.size * 0.5).max(1.0) as isize;

            let min_x = (px - radius).max(0) as usize;
            let max_x = (px + radius).min(width as isize - 1) as usize;
            let min_y = (py - radius).max(0) as usize;
            let max_y = (py + radius).min(height as isize - 1) as usize;

            let alpha = (s.alpha * 255.0).clamp(0.0, 255.0) as u8;
            for y in min_y..=max_y {
                let row_offset = y * w_usize * 4;
                for x in min_x..=max_x {
                    let idx = row_offset + x * 4;
                    pixmap_buffer[idx] = 255;     // B
                    pixmap_buffer[idx + 1] = 255; // G
                    pixmap_buffer[idx + 2] = 255; // R
                    pixmap_buffer[idx + 3] = alpha; // A
                }
            }
        }

        // Blit to window
        let _ = conn.put_image(
            ImageFormat::Z_PIXMAP,
            window,
            gc,
            width,
            height,
            0,
            0,
            0,
            depth,
            &pixmap_buffer,
        );
        let _ = conn.flush();

        // Target ~60 FPS
        let elapsed = now.elapsed();
        if elapsed < Duration::from_millis(16) {
            std::thread::sleep(Duration::from_millis(16) - elapsed);
        }
    }
}
