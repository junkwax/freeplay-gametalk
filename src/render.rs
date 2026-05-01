//! Frame rendering, core lifecycle, and probe-result formatting. Anything
//! that produces SDL canvas output or owns the libretro core's load
//! sequence lives here.
//!
//! `route_player` and `format_probe_result` aren't strictly "render" — they
//! produce input routing and human-readable diagnostic text — but they sit
//! adjacent to draw paths in the call graph, so they ride along here rather
//! than getting their own one-function modules.

use crate::font::Font;
use crate::input;
use crate::netplay;
use crate::retro::{
    self, FRAME_BUFFER, FRAME_HEIGHT, FRAME_PITCH, FRAME_WIDTH, PIXEL_FORMAT,
    RETRO_PIXEL_FORMAT_RGB565, RETRO_PIXEL_FORMAT_XRGB8888,
};
use crate::version;

use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;

/// Choose which set of input bindings to apply this frame. In netplay,
/// each peer ALWAYS controls the local handle (P1 if local_handle=0,
/// otherwise P2) regardless of which player the user originally bound.
/// In local play, honour the user's binding.
pub fn route_player(
    bound_player: input::Player,
    net: &Option<netplay::Session>,
    local_handle: usize,
) -> input::Player {
    if net.is_some() {
        if local_handle == 0 {
            input::Player::P1
        } else {
            input::Player::P2
        }
    } else {
        bound_player
    }
}

/// Format a network probe report as a vec of console-ready lines for the
/// Test Connection screen. Lines are layered L3 → L4 → L7 so the user can
/// see the failure boundary at a glance.
pub fn format_probe_result(
    peer: std::net::SocketAddr,
    self_rom_hash: u64,
    r: &netplay::ProbeReport,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("Target: {}   Duration: {} ms", peer, r.duration_ms));
    out.push("".into());
    out.push("L3 LOCAL STACK".into());
    if let Some(e) = &r.local_bind_error {
        out.push(format!("FAIL UDP bind failed: {}", e));
        out.push("   Another process holds the port, or the OS refused UDP.".into());
        out.push("   Close any other Freeplay / kill stray freeplay.exe processes.".into());
        return out;
    }
    out.push(format!("OK Bound ephemeral UDP port {}", r.local_port));
    out.push("".into());
    out.push("L3 ROUTE TO PEER".into());
    if let Some(e) = &r.send_error {
        out.push(format!("FAIL send_to() rejected: {}", e));
        out.push("   Routing table has no path to the target. Wrong subnet?".into());
        return out;
    }
    out.push(format!("OK Kernel accepted {} send_to() calls", r.sent));
    out.push("".into());
    out.push("L4 REACHABILITY (UDP round-trip)".into());
    if r.received == 0 {
        out.push(format!(
            "FAIL No replies in {} sends ({}% loss)",
            r.sent,
            r.loss_percent()
        ));
        out.push("   Walk the path outward from here:".into());
        out.push("    1. Host not actually on Host Match screen right now?".into());
        out.push("    2. Wrong IP / port typed in?".into());
        out.push(format!("       {}", peer));
        out.push("    3. Host-side Windows Firewall blocking inbound UDP".into());
        out.push("    4. Host-side router not forwarding UDP to their LAN IP".into());
        out.push("    5. ISP carrier-grade NAT on the host side".into());
        return out;
    }
    out.push(format!(
        "OK Received {}/{} replies ({}% loss)",
        r.received,
        r.sent,
        r.loss_percent()
    ));
    if let (Some(mn), Some(av), Some(mx)) = (r.rtt_min(), r.rtt_avg(), r.rtt_max()) {
        let jitter = mx - mn;
        out.push(format!(
            "OK RTT min/avg/max = {}/{}/{} ms  (jitter {} ms)",
            mn, av, mx, jitter
        ));
        if jitter > 50 {
            out.push("WARN Jitter above 50 ms — rollback netcode will feel rough.".into());
        }
    }
    out.push("".into());
    out.push("L4 NAT BEHAVIOUR".into());
    if let Some(obs) = r.observed_self {
        out.push(format!("OK Host saw us as {}", obs));
        if r.nat_rewrote_port {
            out.push(format!(
                "WARN Source port rewritten: bound {} -> host saw {}",
                r.local_port,
                obs.port()
            ));
            out.push("WARN Symmetric NAT — TURN relay fallback will be used.".into());
        } else {
            out.push("OK Port mapping stable — cone NAT or no NAT.".into());
        }
    } else {
        out.push("WARN Reply lacked host-identity fields (old Freeplay version).".into());
    }
    out.push("".into());
    out.push("L7 FREEPLAY BUILD COMPATIBILITY".into());
    match &r.host_version {
        Some(v) if v == version::VERSION => {
            out.push(format!("OK Host Freeplay version: {}  (matches ours)", v));
        }
        Some(v) => {
            out.push(format!(
                "FAIL Host Freeplay version: {}  (ours: {})",
                v,
                version::VERSION
            ));
        }
        None => {
            out.push("WARN Host didn't report a version (pre-v0.2 build).".into());
        }
    }
    if r.host_rom_hash == 0 {
        out.push("WARN Host's ROM hash unknown — can't verify ROM match.".into());
    } else if r.host_rom_hash == self_rom_hash {
        out.push(format!(
            "OK ROM hashes match on both sides (0x{:016x})",
            self_rom_hash
        ));
    } else {
        out.push(format!("FAIL ROM mismatch:  ours=0x{:016x}", self_rom_hash));
        out.push(format!("     host=0x{:016x}", r.host_rom_hash));
    }
    out
}

/// Lazy-load the FBNeo libretro core and open the audio queue. No-op if the
/// core is already loaded. The audio sample rate comes from the core's
/// reported timing (~48 kHz for MK2). Failed audio init is logged but
/// non-fatal — the game still runs silently.
pub fn ensure_core_loaded(
    core: &mut Option<retro::Core>,
    audio_queue: &mut Option<AudioQueue<i16>>,
    audio_subsystem: &sdl2::AudioSubsystem,
) -> Result<(), Box<dyn std::error::Error>> {
    if core.is_some() {
        return Ok(());
    }
    unsafe {
        let rom_path = crate::rom::find_rom_zip_string()
            .ok_or_else(|| "ROM zip not found next to the executable or in roms\\".to_string())?;
        let core_path = fbneo_core_path().ok_or_else(|| {
            "FBNeo core not found next to the executable or in cores\\".to_string()
        })?;
        let c = retro::load(&core_path, &rom_path)?;
        let rate = c.av_info.timing.sample_rate.round() as i32;
        let desired = AudioSpecDesired {
            freq: Some(if rate > 0 { rate } else { 48000 }),
            channels: Some(2),
            samples: Some(1024),
        };
        match audio_subsystem.open_queue::<i16, _>(None, &desired) {
            Ok(q) => {
                q.resume();
                *audio_queue = Some(q);
            }
            Err(e) => println!("Audio init failed: {e}"),
        }
        *core = Some(c);
    }
    Ok(())
}

fn fbneo_core_path() -> Option<String> {
    let name = platform_core_name();
    let mut candidates = vec![name.to_string(), format!("cores/{name}")];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
    {
        candidates.push(exe_dir.join(name).to_string_lossy().into_owned());
        candidates.push(
            exe_dir
                .join("cores")
                .join(name)
                .to_string_lossy()
                .into_owned(),
        );
    }
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

fn platform_core_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "fbneo_libretro.dll"
    }
    #[cfg(target_os = "linux")]
    {
        "fbneo_libretro.so"
    }
    #[cfg(target_os = "macos")]
    {
        "fbneo_libretro.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        "fbneo_libretro"
    }
}

/// Blit the current emulator frame into the canvas. Does NOT call `present()` —
/// callers that want to overlay a HUD on top should draw, then present themselves.
#[allow(static_mut_refs)]
pub fn draw_emu_frame<'a>(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture: &mut sdl2::render::Texture<'a>,
    tc: &'a sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    filter: crate::config::VideoFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        if FRAME_WIDTH == 0 || FRAME_HEIGHT == 0 || FRAME_BUFFER.is_empty() {
            return Ok(());
        }
        let sdl_format = match PIXEL_FORMAT {
            RETRO_PIXEL_FORMAT_XRGB8888 => PixelFormatEnum::ARGB8888,
            RETRO_PIXEL_FORMAT_RGB565 => PixelFormatEnum::RGB565,
            _ => PixelFormatEnum::ARGB1555,
        };
        let q = texture.query();
        if q.width != FRAME_WIDTH || q.height != FRAME_HEIGHT || q.format != sdl_format {
            *texture = tc.create_texture_streaming(sdl_format, FRAME_WIDTH, FRAME_HEIGHT)?;
        }
        let size = (FRAME_HEIGHT as usize) * FRAME_PITCH;
        if FRAME_BUFFER.len() >= size {
            texture.update(None, &FRAME_BUFFER[..size], FRAME_PITCH)?;
            apply_scale_quality(filter);
            canvas.copy(texture, None, None)?;
            draw_video_filter_overlay(canvas, filter, FRAME_WIDTH, FRAME_HEIGHT)?;
        }
    }
    Ok(())
}

fn apply_scale_quality(filter: crate::config::VideoFilter) {
    let quality = match filter {
        crate::config::VideoFilter::Smooth => "linear",
        _ => "nearest",
    };
    let _ = sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", quality);
}

fn draw_video_filter_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    filter: crate::config::VideoFilter,
    frame_w: u32,
    frame_h: u32,
) -> Result<(), String> {
    match filter {
        crate::config::VideoFilter::Sharp | crate::config::VideoFilter::Smooth => {}
        crate::config::VideoFilter::Scanlines => {
            draw_scanlines(canvas, frame_w, frame_h, 58)?;
        }
        crate::config::VideoFilter::CrtLite => {
            canvas.set_draw_color(Color::RGBA(255, 220, 150, 12));
            canvas.fill_rect(Rect::new(0, 0, frame_w, frame_h))?;
            draw_scanlines(canvas, frame_w, frame_h, 46)?;
            draw_crt_vignette(canvas, frame_w, frame_h)?;
        }
    }
    Ok(())
}

fn draw_scanlines(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    frame_w: u32,
    frame_h: u32,
    alpha: u8,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(0, 0, 0, alpha));
    for y in (1..frame_h as i32).step_by(2) {
        canvas.fill_rect(Rect::new(0, y, frame_w, 1))?;
    }
    Ok(())
}

fn draw_crt_vignette(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    frame_w: u32,
    frame_h: u32,
) -> Result<(), String> {
    let fw = frame_w as i32;
    let fh = frame_h as i32;
    let edge = 20;
    canvas.set_draw_color(Color::RGBA(0, 0, 0, 34));
    canvas.fill_rect(Rect::new(0, 0, frame_w, 8))?;
    canvas.fill_rect(Rect::new(0, fh - 8, frame_w, 8))?;
    canvas.fill_rect(Rect::new(0, 0, 8, frame_h))?;
    canvas.fill_rect(Rect::new(fw - 8, 0, 8, frame_h))?;

    canvas.set_draw_color(Color::RGBA(0, 0, 0, 18));
    canvas.fill_rect(Rect::new(0, 0, frame_w, edge as u32))?;
    canvas.fill_rect(Rect::new(0, fh - edge, frame_w, edge as u32))?;
    canvas.fill_rect(Rect::new(0, 0, edge as u32, frame_h))?;
    canvas.fill_rect(Rect::new(fw - edge, 0, edge as u32, frame_h))?;
    Ok(())
}

/// High-resolution match overlay drawn in window pixels, not emu logical pixels.
pub fn draw_fight_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    _window_h: i32,
    p1_name: &str,
    p2_name: &str,
    p1_wins: u32,
    p2_wins: u32,
    _mode_label: Option<&str>,
) -> Result<(), String> {
    let name_scale: u32 = 1;
    let score_scale: u32 = name_scale;
    let white = sdl2::pixels::Color::RGBA(248, 248, 250, 230);
    let accent = sdl2::pixels::Color::RGBA(45, 20, 55, 200);
    let fill = sdl2::pixels::Color::RGBA(20, 10, 30, 220);

    let center_x = window_w / 2;
    let gap = ((window_w as f32) * 0.18) as i32;
    let gap = gap.clamp(190, 280);
    let outer_pad = -22;
    let bar_y = -10;
    let bar_h = 36;
    let slant = 13;
    let left_x = outer_pad;
    let right_x = center_x + gap / 2;
    let half_w = (center_x - gap / 2 - outer_pad).max(150);
    let name_max_w = (half_w - 102).max(90);

    let left = draw_scoreplate(
        canvas, left_x, bar_y, half_w, bar_h, slant, false, fill, accent,
    )?;
    let right = draw_scoreplate(
        canvas, right_x, bar_y, half_w, bar_h, slant, true, fill, accent,
    )?;

    let p1 = fit_overlay_text(font, &p1_name.to_uppercase(), name_scale, name_max_w);
    let p2 = fit_overlay_text(font, &p2_name.to_uppercase(), name_scale, name_max_w);

    let p2_w = font.text_width_overlay(&p2, name_scale);
    let name_y = bar_y + 9;
    font.draw_overlay(canvas, &p1, left.name_x, name_y, name_scale, white)?;
    font.draw_overlay(canvas, &p2, right.name_x - p2_w, name_y, name_scale, white)?;

    let p1_score = p1_wins.to_string();
    let p2_score = p2_wins.to_string();
    let p1_score_w = font.text_width_overlay(&p1_score, score_scale);
    let p2_score_w = font.text_width_overlay(&p2_score, score_scale);
    font.draw_overlay(
        canvas,
        &p1_score,
        left.score_x - p1_score_w / 2,
        name_y,
        score_scale,
        white,
    )?;
    font.draw_overlay(
        canvas,
        &p2_score,
        right.score_x - p2_score_w / 2,
        name_y,
        score_scale,
        white,
    )?;

    Ok(())
}

struct ScoreplateLayout {
    name_x: i32,
    score_x: i32,
}

fn draw_scoreplate(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    slant: i32,
    mirror: bool,
    fill: sdl2::pixels::Color,
    accent: sdl2::pixels::Color,
) -> Result<ScoreplateLayout, String> {
    let tab_w = 70;
    let gap = 8;
    let main_x = if mirror { x } else { x + tab_w + gap };
    let main_w = w - tab_w - gap;
    let tab_x = if mirror { x + w - tab_w } else { x };

    draw_slanted_rect(canvas, main_x, y + 3, main_w, h - 3, slant, mirror, accent)?;
    draw_slanted_rect(
        canvas,
        main_x + if mirror { 2 } else { 4 },
        y + 7,
        main_w - 8,
        h - 11,
        slant,
        mirror,
        fill,
    )?;
    draw_slanted_rect(canvas, tab_x, y + 3, tab_w, h - 3, slant, mirror, accent)?;
    draw_slanted_rect(
        canvas,
        tab_x + if mirror { 4 } else { 2 },
        y + 7,
        tab_w - 8,
        h - 11,
        slant,
        mirror,
        fill,
    )?;

    Ok(ScoreplateLayout {
        name_x: if mirror {
            main_x + main_w - 30
        } else {
            main_x + 30
        },
        score_x: if mirror {
            tab_x + tab_w / 2 - 4
        } else {
            tab_x + tab_w / 2 + 4
        },
    })
}

fn draw_slanted_rect(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    slant: i32,
    mirror: bool,
    color: sdl2::pixels::Color,
) -> Result<(), String> {
    canvas.set_draw_color(color);
    for row in 0..h {
        let offset = slant * row / h;
        let (x1, x2) = if mirror {
            (x + offset, x + w + offset)
        } else {
            (x - offset, x + w - offset)
        };
        canvas.draw_line((x1, y + row), (x2, y + row))?;
    }
    Ok(())
}

fn fit_overlay_text(font: &mut Font, text: &str, scale: u32, max_w: i32) -> String {
    if font.text_width_overlay(text, scale) <= max_w {
        return text.to_string();
    }

    let mut out = String::new();
    for ch in text.chars() {
        let candidate = format!("{out}{ch}...");
        if font.text_width_overlay(&candidate, scale) > max_w {
            break;
        }
        out.push(ch);
    }

    if out.is_empty() {
        "...".to_string()
    } else {
        out.push_str("...");
        out
    }
}
