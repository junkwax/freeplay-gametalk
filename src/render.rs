//! Frame rendering, core lifecycle, and probe-result formatting. Anything
//! that produces SDL canvas output or owns the libretro core's load
//! sequence lives here.
//!
//! `route_player` and `format_probe_result` aren't strictly "render" — they
//! produce input routing and human-readable diagnostic text — but they sit
//! adjacent to draw paths in the call graph, so they ride along here rather
//! than getting their own one-function modules.

use crate::config::RenderProfile;
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
use sdl2::render::Canvas;
use sdl2::video::Window;

const SDL_RENDERER_SOFTWARE_FLAG: u32 = 0x0000_0001;
const SDL_RENDERER_ACCELERATED_FLAG: u32 = 0x0000_0002;
const SDL_RENDERER_PRESENTVSYNC_FLAG: u32 = 0x0000_0004;
const SDL_RENDERER_TARGETTEXTURE_FLAG: u32 = 0x0000_0008;

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
            samples: Some(2048),
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

pub fn build_window_canvas(
    window: Window,
    profile: RenderProfile,
    shader_requested: bool,
) -> Result<Canvas<Window>, String> {
    let requested_driver = requested_render_driver(profile, shader_requested);
    if let Some(driver) = requested_driver.as_deref() {
        let _ = sdl2::hint::set("SDL_RENDER_DRIVER", driver);
    }
    let _ = sdl2::hint::set("SDL_RENDER_BATCHING", "1");

    let mut builder = window.into_canvas().target_texture();
    if profile.wants_acceleration() {
        builder = builder.accelerated();
    }
    if profile.wants_software() {
        builder = builder.software();
    }
    if profile.wants_vsync() {
        builder = builder.present_vsync();
    }

    let canvas = builder.build().map_err(|e| e.to_string())?;
    log_renderer_choice(&canvas, profile, requested_driver.as_deref());
    Ok(canvas)
}

fn requested_render_driver(profile: RenderProfile, shader_requested: bool) -> Option<String> {
    if shader_requested {
        if let Some(driver) = available_render_driver("opengl") {
            println!("[render] CRT SHADER requested; forcing SDL renderer: {driver}");
            return Some(driver);
        }
        println!("[render] CRT SHADER requested but SDL opengl renderer is not available");
    }

    if let Some(driver) = std::env::var("FREEPLAY_RENDER_DRIVER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return match available_render_driver(&driver) {
            Some(driver) => Some(driver),
            None => {
                println!(
                    "[render] requested SDL driver '{driver}' is not available; using SDL choice"
                );
                None
            }
        };
    }

    if profile.wants_software() {
        return Some("software".to_string());
    }

    if profile.wants_acceleration() {
        if let Some(driver) = preferred_hardware_render_driver() {
            println!("[render] preferred SDL hardware driver on this platform: {driver}");
        }
    }

    None
}

fn preferred_hardware_render_driver() -> Option<String> {
    #[cfg(target_os = "windows")]
    const PREFERRED: &[&str] = &["direct3d11", "direct3d", "opengl"];
    #[cfg(target_os = "macos")]
    const PREFERRED: &[&str] = &["metal", "opengl"];
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    const PREFERRED: &[&str] = &["opengl", "opengles2"];

    PREFERRED
        .iter()
        .find(|preferred| available_render_driver(preferred).is_some())
        .map(|driver| (*driver).to_string())
}

fn available_render_driver(name: &str) -> Option<String> {
    sdl2::render::drivers()
        .map(|driver| driver.name.to_string())
        .find(|driver| driver.eq_ignore_ascii_case(name))
}

fn log_renderer_choice(
    canvas: &Canvas<Window>,
    profile: RenderProfile,
    requested_driver: Option<&str>,
) {
    let available = sdl2::render::drivers()
        .map(|driver| driver.name.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    println!("[render] available SDL drivers: {available}");
    if let Some(driver) = requested_driver {
        println!("[render] requested SDL driver: {driver}");
    }
    let info = canvas.info();
    println!(
        "[render] profile={} selected={} flags=0x{:x} max_texture={}x{}",
        profile.label(),
        info.name,
        info.flags,
        info.max_texture_width,
        info.max_texture_height
    );
    log_renderer_capabilities(&info, profile);
}

fn log_renderer_capabilities(info: &sdl2::render::RendererInfo, profile: RenderProfile) {
    let software = info.flags & SDL_RENDERER_SOFTWARE_FLAG != 0;
    let accelerated = info.flags & SDL_RENDERER_ACCELERATED_FLAG != 0;
    let vsync = info.flags & SDL_RENDERER_PRESENTVSYNC_FLAG != 0;
    let target_texture = info.flags & SDL_RENDERER_TARGETTEXTURE_FLAG != 0;
    println!(
        "[render] capabilities: accelerated={accelerated} target_texture={target_texture} software={software} vsync={vsync}"
    );

    let recommendation = if accelerated && target_texture {
        "CRT SHADER is safe when the SDL opengl renderer is selected; CRT DELUXE and current filters are safe with cached hardware overlays"
    } else if accelerated {
        "sharp/smooth/scanlines are safe; heavy CRT overlays may cost CPU"
    } else {
        "sharp/smooth are safest; avoid heavy CRT filters during netplay"
    };
    let profile_recommendation = if accelerated && target_texture {
        RenderProfile::Hardware
    } else if software {
        RenderProfile::Software
    } else {
        RenderProfile::Auto
    };
    println!(
        "[render] profile recommendation: {}",
        profile_recommendation.label()
    );
    println!("[render] filter recommendation: {recommendation}");

    if profile.wants_vsync() {
        println!("[render] netplay note: VSYNC can add latency; prefer HARDWARE for online tests");
    }
}

pub fn recommended_profile(canvas: &Canvas<Window>) -> RenderProfile {
    let info = canvas.info();
    let accelerated = info.flags & SDL_RENDERER_ACCELERATED_FLAG != 0;
    let software = info.flags & SDL_RENDERER_SOFTWARE_FLAG != 0;
    let target_texture = info.flags & SDL_RENDERER_TARGETTEXTURE_FLAG != 0;

    if accelerated && target_texture {
        RenderProfile::Hardware
    } else if software {
        RenderProfile::Software
    } else {
        RenderProfile::Auto
    }
}

pub fn renderer_name(canvas: &Canvas<Window>) -> &'static str {
    canvas.info().name
}

pub fn netplay_safe_filter(
    canvas: &Canvas<Window>,
    filter: crate::config::VideoFilter,
    netplay_active: bool,
) -> crate::config::VideoFilter {
    if !netplay_active || !filter.needs_hardware_budget() || has_hardware_filter_budget(canvas) {
        return filter;
    }
    if filter.uses_opengl_shader() {
        crate::config::VideoFilter::CrtDeluxe
    } else {
        crate::config::VideoFilter::Scanlines
    }
}

fn has_hardware_filter_budget(canvas: &Canvas<Window>) -> bool {
    let info = canvas.info();
    let accelerated = info.flags & SDL_RENDERER_ACCELERATED_FLAG != 0;
    let target_texture = info.flags & SDL_RENDERER_TARGETTEXTURE_FLAG != 0;
    accelerated && target_texture
}

/// Cache key for the pre-rendered filter overlay: the overlay's pixels only
/// depend on these, so it is rebuilt on filter switch or window resize and
/// costs a single texture copy per frame otherwise.
pub type OverlayKey = (crate::config::VideoFilter, u32, u32, bool);
pub type OverlayCache<'a> = Option<(OverlayKey, sdl2::render::Texture<'a>)>;

/// Blit the current emulator frame into the canvas. Does NOT call `present()` —
/// callers that want to overlay a HUD on top should draw, then present themselves.
#[allow(static_mut_refs)]
pub fn draw_emu_frame<'a>(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture: &mut sdl2::render::Texture<'a>,
    tc: &'a sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    overlay_cache: &mut OverlayCache<'a>,
    shader: Option<&mut crate::gl_crt::GlCrtRenderer>,
    filter: crate::config::VideoFilter,
    aspect: crate::config::AspectMode,
    crt_corner_bend: bool,
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
            let (out_w, out_h) = canvas.output_size()?;
            let dst = frame_destination(out_w, out_h, FRAME_WIDTH, FRAME_HEIGHT, aspect);
            if filter.uses_opengl_shader() {
                if let (Some(shader), Some(dst)) = (shader, dst) {
                    let shader_mode = filter.opengl_shader_mode().unwrap_or(0);
                    if shader
                        .draw(
                            canvas,
                            texture,
                            dst,
                            (FRAME_WIDTH, FRAME_HEIGHT),
                            (out_w, out_h),
                            shader_mode,
                        )
                        .is_ok()
                    {
                        return Ok(());
                    }
                }

                canvas.copy(texture, None, dst)?;
                draw_video_filter_overlay(
                    canvas,
                    tc,
                    overlay_cache,
                    crate::config::VideoFilter::CrtDeluxe,
                    dst,
                    crt_corner_bend,
                )?;
            } else {
                canvas.copy(texture, None, dst)?;
                draw_video_filter_overlay(canvas, tc, overlay_cache, filter, dst, crt_corner_bend)?;
            }
        }
    }
    Ok(())
}

fn frame_destination(
    out_w: u32,
    out_h: u32,
    frame_w: u32,
    frame_h: u32,
    aspect: crate::config::AspectMode,
) -> Option<Rect> {
    match aspect {
        crate::config::AspectMode::Stretch => Some(Rect::new(0, 0, out_w, out_h)),
        crate::config::AspectMode::Integer => {
            let scale = (out_w / frame_w).min(out_h / frame_h).max(1);
            let w = frame_w * scale;
            let h = frame_h * scale;
            let x = ((out_w - w) / 2) as i32;
            let y = ((out_h - h) / 2) as i32;
            Some(Rect::new(x, y, w, h))
        }
        crate::config::AspectMode::Fit => {
            let scale = (out_w as f32 / frame_w as f32).min(out_h as f32 / frame_h as f32);
            let w = (frame_w as f32 * scale).round().max(1.0) as u32;
            let h = (frame_h as f32 * scale).round().max(1.0) as u32;
            let x = ((out_w - w) / 2) as i32;
            let y = ((out_h - h) / 2) as i32;
            Some(Rect::new(x, y, w, h))
        }
    }
}

fn apply_scale_quality(filter: crate::config::VideoFilter) {
    let quality = match filter {
        crate::config::VideoFilter::Smooth
        | crate::config::VideoFilter::CrtDeluxe
        | crate::config::VideoFilter::CrtShader
        | crate::config::VideoFilter::CrtArcadeShader
        | crate::config::VideoFilter::CrtPvmShader
        | crate::config::VideoFilter::CrtCabinet => "linear",
        _ => "nearest",
    };
    let _ = sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", quality);
}

fn draw_video_filter_overlay<'a>(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    tc: &'a sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    cache: &mut OverlayCache<'a>,
    filter: crate::config::VideoFilter,
    dst: Option<Rect>,
    crt_corner_bend: bool,
) -> Result<(), String> {
    let Some(dst) = dst else {
        return Ok(());
    };
    if matches!(
        filter,
        crate::config::VideoFilter::Sharp | crate::config::VideoFilter::Smooth
    ) {
        return Ok(());
    }

    let key: OverlayKey = (filter, dst.width(), dst.height(), crt_corner_bend);
    let cached = matches!(cache, Some((k, _)) if *k == key);
    if !cached {
        *cache = build_overlay_texture(canvas, tc, key);
    }
    match cache {
        Some((_, tex)) => canvas.copy(tex, None, dst),
        // Render targets unsupported on this driver — draw procedurally.
        None => draw_overlay_layers(canvas, filter, dst, crt_corner_bend),
    }
}

/// Render the overlay once into a transparent target texture. Returns None
/// when the renderer can't do render targets; callers fall back to drawing
/// the layers directly every frame.
fn build_overlay_texture<'a>(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    tc: &'a sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    key: OverlayKey,
) -> OverlayCache<'a> {
    let (filter, w, h, bend) = key;
    let mut tex = tc
        .create_texture_target(PixelFormatEnum::ARGB8888, w, h)
        .ok()?;
    tex.set_blend_mode(sdl2::render::BlendMode::Blend);
    let mut draw_err: Option<String> = None;
    let target = canvas.with_texture_canvas(&mut tex, |c| {
        c.set_draw_color(Color::RGBA(0, 0, 0, 0));
        c.clear();
        if let Err(e) = draw_overlay_layers(c, filter, Rect::new(0, 0, w, h), bend) {
            draw_err = Some(e);
        }
    });
    if target.is_err() || draw_err.is_some() {
        return None;
    }
    Some((key, tex))
}

fn draw_overlay_layers(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    filter: crate::config::VideoFilter,
    dst: Rect,
    crt_corner_bend: bool,
) -> Result<(), String> {
    match filter {
        crate::config::VideoFilter::Sharp | crate::config::VideoFilter::Smooth => {}
        crate::config::VideoFilter::Scanlines => {
            draw_scanlines(canvas, dst, 58)?;
        }
        crate::config::VideoFilter::CrtLite => {
            canvas.set_draw_color(Color::RGBA(255, 220, 150, 12));
            canvas.fill_rect(dst)?;
            draw_scanlines(canvas, dst, 46)?;
            draw_crt_vignette(canvas, dst, 20, 34, 18)?;
        }
        crate::config::VideoFilter::CrtArcade => {
            canvas.set_draw_color(Color::RGBA(255, 225, 170, 10));
            canvas.fill_rect(dst)?;
            draw_scanlines(canvas, dst, 62)?;
            draw_shadow_mask(canvas, dst, 26)?;
            draw_center_bloom(canvas, dst, 16)?;
            draw_crt_vignette(canvas, dst, 24, 40, 22)?;
            if crt_corner_bend {
                draw_crt_corner_bend(canvas, dst, 26, 74, true)?;
            }
        }
        crate::config::VideoFilter::CrtDeluxe => {
            draw_crt_deluxe(canvas, dst, crt_corner_bend)?;
        }
        crate::config::VideoFilter::CrtShader => {
            draw_crt_deluxe(canvas, dst, crt_corner_bend)?;
        }
        crate::config::VideoFilter::CrtArcadeShader => {
            draw_crt_deluxe(canvas, dst, crt_corner_bend)?;
        }
        crate::config::VideoFilter::CrtPvmShader => {
            draw_crt_deluxe(canvas, dst, crt_corner_bend)?;
        }
        crate::config::VideoFilter::CrtCabinet => {
            canvas.set_draw_color(Color::RGBA(255, 205, 135, 24));
            canvas.fill_rect(dst)?;
            draw_scanlines(canvas, dst, 76)?;
            draw_shadow_mask(canvas, dst, 18)?;
            draw_crt_vignette(canvas, dst, 34, 56, 32)?;
            if crt_corner_bend {
                draw_crt_corner_bend(canvas, dst, 34, 94, true)?;
            }
        }
        crate::config::VideoFilter::PvmSharp => {
            canvas.set_draw_color(Color::RGBA(210, 235, 255, 8));
            canvas.fill_rect(dst)?;
            draw_scanlines(canvas, dst, 38)?;
            draw_shadow_mask(canvas, dst, 14)?;
            draw_crt_vignette(canvas, dst, 14, 18, 10)?;
            if crt_corner_bend {
                draw_crt_corner_bend(canvas, dst, 18, 44, false)?;
            }
        }
    }
    Ok(())
}

fn draw_crt_deluxe(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    crt_corner_bend: bool,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(255, 226, 184, 14));
    canvas.fill_rect(dst)?;
    draw_soft_center_glow(canvas, dst)?;
    draw_horizontal_beam_glow(canvas, dst)?;
    draw_deluxe_scanlines(canvas, dst, 58, 24)?;
    draw_deluxe_slot_mask(canvas, dst, 24)?;
    draw_convergence_fringe(canvas, dst, 12)?;
    draw_crt_vignette(canvas, dst, 30, 46, 24)?;
    if crt_corner_bend {
        draw_crt_corner_bend(canvas, dst, 30, 84, true)?;
        draw_glass_sheen(canvas, dst)?;
    }
    Ok(())
}

fn draw_scanlines(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    alpha: u8,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGBA(0, 0, 0, alpha));
    for y in ((dst.y() + 1)..(dst.y() + dst.height() as i32)).step_by(2) {
        canvas.fill_rect(Rect::new(dst.x(), y, dst.width(), 1))?;
    }
    Ok(())
}

fn draw_deluxe_scanlines(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    strong_alpha: u8,
    soft_alpha: u8,
) -> Result<(), String> {
    let y0 = dst.y();
    let y1 = dst.y() + dst.height() as i32;
    canvas.set_draw_color(Color::RGBA(0, 0, 0, strong_alpha));
    for y in ((y0 + 1)..y1).step_by(4) {
        canvas.fill_rect(Rect::new(dst.x(), y, dst.width(), 1))?;
    }
    canvas.set_draw_color(Color::RGBA(0, 0, 0, soft_alpha));
    for y in ((y0 + 3)..y1).step_by(4) {
        canvas.fill_rect(Rect::new(dst.x(), y, dst.width(), 1))?;
    }
    Ok(())
}

fn draw_deluxe_slot_mask(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    alpha: u8,
) -> Result<(), String> {
    let y = dst.y();
    let x0 = dst.x();
    let x1 = dst.x() + dst.width() as i32;
    let y1 = dst.y() + dst.height() as i32;
    let h = dst.height();
    let colors = [
        Color::RGBA(255, 55, 35, alpha),
        Color::RGBA(55, 255, 95, alpha),
        Color::RGBA(60, 120, 255, alpha),
    ];

    for (offset, color) in colors.into_iter().enumerate() {
        canvas.set_draw_color(color);
        for x in ((x0 + offset as i32)..x1).step_by(6) {
            canvas.fill_rect(Rect::new(x, y, 1, h))?;
        }
    }

    canvas.set_draw_color(Color::RGBA(0, 0, 0, alpha / 2));
    for x in ((x0 + 3)..x1).step_by(6) {
        canvas.fill_rect(Rect::new(x, y, 1, h))?;
    }
    for row in ((y + 3)..y1).step_by(6) {
        canvas.fill_rect(Rect::new(x0, row, dst.width(), 1))?;
    }
    Ok(())
}

fn draw_soft_center_glow(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
) -> Result<(), String> {
    let layers = [(18_i32, 12_u8), (12, 9), (7, 6)];
    for (div, alpha) in layers {
        let inset_x = (dst.width() as i32 / div).max(1);
        let inset_y = (dst.height() as i32 / div).max(1);
        let w = dst.width() as i32 - inset_x * 2;
        let h = dst.height() as i32 - inset_y * 2;
        if w > 0 && h > 0 {
            canvas.set_draw_color(Color::RGBA(255, 245, 218, alpha));
            canvas.fill_rect(Rect::new(
                dst.x() + inset_x,
                dst.y() + inset_y,
                w as u32,
                h as u32,
            ))?;
        }
    }
    Ok(())
}

fn draw_horizontal_beam_glow(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
) -> Result<(), String> {
    let cy = dst.y() + dst.height() as i32 / 2;
    let bands = [
        (dst.height() as i32 / 5, 6_u8),
        (dst.height() as i32 / 10, 8),
        (dst.height() as i32 / 18, 10),
    ];
    for (half_h, alpha) in bands {
        let half_h = half_h.max(1);
        let y = cy - half_h;
        let h = (half_h * 2).max(1) as u32;
        canvas.set_draw_color(Color::RGBA(255, 236, 210, alpha));
        canvas.fill_rect(Rect::new(dst.x(), y, dst.width(), h))?;
    }
    Ok(())
}

fn draw_convergence_fringe(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    alpha: u8,
) -> Result<(), String> {
    if dst.width() < 12 || dst.height() < 12 {
        return Ok(());
    }
    let x0 = dst.x();
    let y0 = dst.y();
    let x1 = dst.x() + dst.width() as i32;
    let y1 = dst.y() + dst.height() as i32;
    canvas.set_draw_color(Color::RGBA(255, 45, 35, alpha));
    canvas.fill_rect(Rect::new(x0 + 1, y0, 1, dst.height()))?;
    canvas.fill_rect(Rect::new(x0, y0 + 1, dst.width(), 1))?;
    canvas.set_draw_color(Color::RGBA(40, 100, 255, alpha));
    canvas.fill_rect(Rect::new(x1 - 2, y0, 1, dst.height()))?;
    canvas.fill_rect(Rect::new(x0, y1 - 2, dst.width(), 1))?;
    Ok(())
}

fn draw_glass_sheen(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
) -> Result<(), String> {
    if dst.width() < 64 || dst.height() < 48 {
        return Ok(());
    }
    let x = dst.x() + (dst.width() as i32 / 8);
    let y = dst.y() + (dst.height() as i32 / 12);
    let w = (dst.width() as i32 - dst.width() as i32 / 4).max(1) as u32;
    for i in 0..5 {
        canvas.set_draw_color(Color::RGBA(255, 255, 255, 16_u8.saturating_sub(i * 3)));
        canvas.fill_rect(Rect::new(x + i as i32 * 4, y + i as i32 * 2, w / 2, 1))?;
    }
    Ok(())
}

fn draw_crt_vignette(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    edge: i32,
    hard_alpha: u8,
    soft_alpha: u8,
) -> Result<(), String> {
    let x = dst.x();
    let y = dst.y();
    let fw = dst.width() as i32;
    let fh = dst.height() as i32;
    canvas.set_draw_color(Color::RGBA(0, 0, 0, hard_alpha));
    canvas.fill_rect(Rect::new(x, y, dst.width(), 8))?;
    canvas.fill_rect(Rect::new(x, y + fh - 8, dst.width(), 8))?;
    canvas.fill_rect(Rect::new(x, y, 8, dst.height()))?;
    canvas.fill_rect(Rect::new(x + fw - 8, y, 8, dst.height()))?;

    canvas.set_draw_color(Color::RGBA(0, 0, 0, soft_alpha));
    canvas.fill_rect(Rect::new(x, y, dst.width(), edge as u32))?;
    canvas.fill_rect(Rect::new(x, y + fh - edge, dst.width(), edge as u32))?;
    canvas.fill_rect(Rect::new(x, y, edge as u32, dst.height()))?;
    canvas.fill_rect(Rect::new(x + fw - edge, y, edge as u32, dst.height()))?;
    Ok(())
}

fn draw_shadow_mask(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    alpha: u8,
) -> Result<(), String> {
    let y = dst.y();
    let h = dst.height();
    // One pass per color: SDL's renderer flushes its batch on every draw-color
    // change, so interleaving R/G/B columns costs ~3 draw calls per column.
    let colors = [
        Color::RGBA(255, 0, 0, alpha),
        Color::RGBA(0, 255, 0, alpha),
        Color::RGBA(0, 70, 255, alpha),
    ];
    for (offset, color) in colors.into_iter().enumerate() {
        canvas.set_draw_color(color);
        for x in (dst.x()..(dst.x() + dst.width() as i32)).step_by(3) {
            canvas.fill_rect(Rect::new(x + offset as i32, y, 1, h))?;
        }
    }
    Ok(())
}

fn draw_center_bloom(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    alpha: u8,
) -> Result<(), String> {
    let inset_x = (dst.width() as i32 / 10).max(1);
    let inset_y = (dst.height() as i32 / 8).max(1);
    let w = dst.width() as i32 - inset_x * 2;
    let h = dst.height() as i32 - inset_y * 2;
    if w > 0 && h > 0 {
        canvas.set_draw_color(Color::RGBA(255, 245, 220, alpha));
        canvas.fill_rect(Rect::new(
            dst.x() + inset_x,
            dst.y() + inset_y,
            w as u32,
            h as u32,
        ))?;
    }
    Ok(())
}

fn draw_crt_corner_bend(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    dst: Rect,
    radius: i32,
    alpha: u8,
    glass_highlight: bool,
) -> Result<(), String> {
    let x0 = dst.x();
    let y0 = dst.y();
    let x1 = dst.x() + dst.width() as i32;
    let y1 = dst.y() + dst.height() as i32;
    let r = radius
        .min(dst.width() as i32 / 5)
        .min(dst.height() as i32 / 5)
        .max(8);

    canvas.set_draw_color(Color::RGBA(0, 0, 0, alpha));
    for i in 0..r {
        let t = i as f32 / r as f32;
        let cut = ((1.0 - (1.0 - t).powf(2.0)) * r as f32) as i32;
        let inset = r - i;
        let shade_w = (inset + cut / 2).max(1) as u32;
        canvas.fill_rect(Rect::new(x0, y0 + i, shade_w, 1))?;
        canvas.fill_rect(Rect::new(x1 - shade_w as i32, y0 + i, shade_w, 1))?;
        canvas.fill_rect(Rect::new(x0, y1 - i - 1, shade_w, 1))?;
        canvas.fill_rect(Rect::new(x1 - shade_w as i32, y1 - i - 1, shade_w, 1))?;
    }

    canvas.set_draw_color(Color::RGBA(0, 0, 0, alpha / 2));
    for i in 0..(r / 2).max(1) {
        let inset = (r / 2 - i).max(1) as u32;
        canvas.fill_rect(Rect::new(x0 + i, y0, 1, inset))?;
        canvas.fill_rect(Rect::new(x1 - i - 1, y0, 1, inset))?;
        canvas.fill_rect(Rect::new(x0 + i, y1 - inset as i32, 1, inset))?;
        canvas.fill_rect(Rect::new(x1 - i - 1, y1 - inset as i32, 1, inset))?;
    }

    if glass_highlight {
        let highlight_y = y0 + (dst.height() as i32 / 18).max(5);
        let highlight_x = x0 + r;
        let highlight_w = dst.width().saturating_sub((r * 2) as u32);
        if highlight_w > 0 {
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 18));
            canvas.fill_rect(Rect::new(highlight_x, highlight_y, highlight_w, 1))?;
            canvas.set_draw_color(Color::RGBA(255, 255, 255, 8));
            canvas.fill_rect(Rect::new(
                highlight_x + r / 2,
                highlight_y + 2,
                highlight_w / 2,
                1,
            ))?;
        }
    }

    Ok(())
}

/// High-resolution match overlay drawn in window pixels, not emu logical pixels.
pub fn draw_fight_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    p1_name: &str,
    p2_name: &str,
    p1_wins: u32,
    p2_wins: u32,
    _mode_label: Option<&str>,
    style: crate::config::ScorebarStyle,
) -> Result<(), String> {
    match style {
        crate::config::ScorebarStyle::Plates => {
            draw_fight_overlay_plates(canvas, font, window_w, p1_name, p2_name, p1_wins, p2_wins)
        }
        crate::config::ScorebarStyle::Centered => {
            draw_fight_overlay_centered(canvas, font, window_w, window_h, p1_name, p2_name)
        }
    }
}

pub fn draw_chat_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    lines: &[String],
    draft: Option<&str>,
) -> Result<(), String> {
    let scale = 1;
    let pad = 10;
    let box_w = (window_w / 2).clamp(360, 620);
    let line_h = 20;
    let visible_lines = lines.len().min(4);
    let input_rows = if draft.is_some() { 1 } else { 0 };
    let box_h = pad * 2 + ((visible_lines + input_rows).max(1) as i32 * line_h);
    let x = 18;
    let y = window_h - box_h - 24;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 205));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 210));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;

    let mut row_y = y + pad;
    let first = lines.len().saturating_sub(visible_lines);
    for line in &lines[first..] {
        let clipped = fit_overlay_text(font, line, scale, box_w - pad * 2);
        font.draw_overlay(
            canvas,
            &clipped,
            x + pad,
            row_y,
            scale,
            Color::RGBA(230, 238, 255, 235),
        )?;
        row_y += line_h;
    }

    if let Some(draft) = draft {
        let prompt = format!("> {draft}_");
        let clipped = fit_overlay_text(font, &prompt, scale, box_w - pad * 2);
        font.draw_overlay(
            canvas,
            &clipped,
            x + pad,
            row_y,
            scale,
            Color::RGBA(255, 230, 130, 245),
        )?;
    } else if lines.is_empty() {
        font.draw_overlay(
            canvas,
            "T CHAT",
            x + pad,
            row_y,
            scale,
            Color::RGBA(150, 165, 195, 180),
        )?;
    }

    Ok(())
}

pub fn draw_net_stats_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    fps: Option<f32>,
    ping: Option<&str>,
    mode: &str,
    detail_rows: &[String],
) -> Result<(), String> {
    let scale = 1;
    let pad = 9;
    let line_h = 18;
    let fps_text = fps
        .map(|v| format!("{v:.1} FPS"))
        .unwrap_or_else(|| "-- FPS".to_string());
    let ping_text = format!("PING {}", ping.unwrap_or("--"));
    let mut rows = vec![mode.to_string(), fps_text, ping_text];
    rows.extend(detail_rows.iter().cloned());

    let mut content_w = font.text_width_overlay("NET STATS", scale);
    for row in &rows {
        content_w = content_w.max(font.text_width_exact(row, scale));
    }
    let box_w = (content_w + pad * 2).clamp(150, 280);
    let box_h = pad * 2 + 24 + line_h * rows.len() as i32;
    let x = window_w - box_w - 18;
    let y = window_h - box_h - 24;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 205));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 190));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    font.draw_overlay(
        canvas,
        "NET STATS",
        x + pad,
        y + pad,
        scale,
        Color::RGBA(255, 210, 90, 245),
    )?;

    let mut row_y = y + pad + 26;
    for row in rows {
        font.draw(
            canvas,
            &row,
            x + pad,
            row_y,
            scale,
            Color::RGBA(226, 234, 252, 230),
        )?;
        row_y += line_h;
    }

    Ok(())
}

pub fn draw_render_debug_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    fps: Option<f32>,
    renderer: &str,
    filter: crate::config::VideoFilter,
    netplay_active: bool,
) -> Result<(), String> {
    let scale = 1;
    let pad = 8;
    let line_h = 17;
    let fps_text = fps
        .map(|v| format!("{v:.1} FPS"))
        .unwrap_or_else(|| "-- FPS".to_string());
    let rows = [
        fps_text,
        format!("{} / {}", renderer.to_ascii_uppercase(), filter.label()),
        if netplay_active {
            "ONLINE SDL OVERLAYS".to_string()
        } else {
            "LOCAL".to_string()
        },
    ];

    let mut content_w = font.text_width_overlay("RENDER", scale);
    for row in &rows {
        content_w = content_w.max(font.text_width_exact(row, scale));
    }
    let box_w = (content_w + pad * 2).clamp(180, 390);
    let box_h = pad * 2 + 22 + line_h * rows.len() as i32;
    let x = 18;
    let y = 42;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 190));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 165));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, box_h as u32))?;
    font.draw_overlay(
        canvas,
        "RENDER",
        x + pad,
        y + pad,
        scale,
        Color::RGBA(255, 210, 90, 235),
    )?;

    let mut row_y = y + pad + 24;
    for row in rows {
        font.draw(
            canvas,
            &row,
            x + pad,
            row_y,
            scale,
            Color::RGBA(220, 232, 255, 225),
        )?;
        row_y += line_h;
    }
    Ok(())
}

pub fn draw_lab_assist_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    history: &crate::input_history::InputHistory,
    hitboxes_on: bool,
    health_on: bool,
    timer_on: bool,
    dummy_status: &str,
    reset_slot_status: &str,
    punish_status: &str,
) -> Result<(), String> {
    let header_scale = 1;
    let body_scale = 1;
    let pad = 8;
    let line_h = 16;
    let header_gap = 30;
    let hotkeys = vec![
        format!("F2  BOXES {}", if hitboxes_on { "ON" } else { "OFF" }),
        format!("F3  HEALTH {}", if health_on { "ON" } else { "OFF" }),
        format!("F4  TIMER {}", if timer_on { "ON" } else { "OFF" }),
        format!("F5  DUMMY {dummy_status}"),
        format!("F6  LOAD {reset_slot_status}"),
        format!("F7  SAVE {reset_slot_status}"),
        "F8  LOAD GHOST".to_string(),
        "F9  SAVE GHOST".to_string(),
        format!("F10 PUNISH {punish_status}"),
        "F11 HIDE HELP".to_string(),
        "F12 VS GHOST".to_string(),
    ];
    let hotkey_h = pad * 2 + header_gap + line_h * hotkeys.len() as i32;
    let rows: Vec<(String, String)> = history
        .entries()
        .take(6)
        .map(|entry| {
            (
                crate::input_history::format_bits(entry.bits),
                format_input_frames(entry.frames),
            )
        })
        .collect();
    let input_rows = rows.len().max(1);
    let input_h = pad * 2 + header_gap + line_h * input_rows as i32;
    let mut content_w = font.text_width_overlay("LAB HOTKEYS", header_scale);
    content_w = content_w.max(font.text_width_overlay("P1 INPUTS", header_scale));
    for line in &hotkeys {
        content_w = content_w.max(font.text_width_exact(line, body_scale));
    }
    for (input, frames) in &rows {
        let row_w = font.text_width_exact(input, body_scale)
            + 48
            + font.text_width_exact(frames, body_scale);
        content_w = content_w.max(row_w);
    }
    let box_w = (content_w + pad * 2).clamp(196, 286);
    let x = window_w - box_w - 18;
    let gap = 8;
    let bottom_margin = 28;
    let min_y = 74;
    let stacked_h = hotkey_h + gap + input_h;
    let mut y = (window_h - stacked_h - bottom_margin).max(min_y);
    let input_y = y + hotkey_h + gap;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 190));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, hotkey_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 180));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, hotkey_h as u32))?;
    font.draw_overlay(
        canvas,
        "LAB HOTKEYS",
        x + pad,
        y + pad,
        header_scale,
        Color::RGBA(255, 210, 90, 240),
    )?;
    let mut row_y = y + pad + header_gap;
    for line in hotkeys {
        let clipped = fit_text_exact(font, &line, body_scale, box_w - pad * 2);
        font.draw(
            canvas,
            &clipped,
            x + pad,
            row_y,
            body_scale,
            Color::RGBA(210, 220, 245, 225),
        )?;
        row_y += line_h;
    }

    y = input_y;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 190));
    canvas.fill_rect(Rect::new(x, y, box_w as u32, input_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 180));
    canvas.draw_rect(Rect::new(x, y, box_w as u32, input_h as u32))?;
    font.draw_overlay(
        canvas,
        "P1 INPUTS",
        x + pad,
        y + pad,
        header_scale,
        Color::RGBA(255, 210, 90, 240),
    )?;
    let mut row_y = y + pad + header_gap;
    if rows.is_empty() {
        font.draw(
            canvas,
            "NO INPUT",
            x + pad,
            row_y,
            body_scale,
            Color::RGBA(150, 165, 195, 180),
        )?;
    } else {
        let frame_right = x + box_w - pad;
        let input_max_w = box_w - pad * 2 - 48;
        for (input, frames) in rows {
            let clipped = fit_text_exact(font, &input, body_scale, input_max_w);
            font.draw(
                canvas,
                &clipped,
                x + pad,
                row_y,
                body_scale,
                Color::RGBA(230, 238, 255, 230),
            )?;
            let frame_w = font.text_width_exact(&frames, body_scale);
            font.draw(
                canvas,
                &frames,
                frame_right - frame_w,
                row_y,
                body_scale,
                Color::RGBA(230, 238, 255, 230),
            )?;
            row_y += line_h;
        }
    }
    Ok(())
}

pub fn draw_replay_review_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    playback: &crate::match_replay::Playback,
    paused: bool,
    speed_label: &str,
    event_filter: crate::match_replay::ReplayEventFilter,
    clip_in: Option<usize>,
    clip_out: Option<usize>,
) -> Result<(), String> {
    let pad = 10;
    let panel_w = (window_w - 36).clamp(360, 760);
    let panel_h = 146;
    let x = (window_w - panel_w) / 2;
    let y = window_h - panel_h - 18;
    let scale = 1;
    let header_scale = 1;
    let frame = playback.current_frame();
    let total = playback.frame_count().max(1);
    let mins = frame / (55 * 60);
    let secs = (frame / 55) % 60;
    let state = if paused { "PAUSED" } else { "PLAYING" };
    let header = format!(
        "{state} {speed_label}  FILTER {}  FRAME {frame}/{total}  {mins:02}:{secs:02}",
        event_filter.label()
    );

    draw_replay_event_sidebar(canvas, font, window_w, window_h, playback, event_filter)?;

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 205));
    canvas.fill_rect(Rect::new(x, y, panel_w as u32, panel_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 190));
    canvas.draw_rect(Rect::new(x, y, panel_w as u32, panel_h as u32))?;

    font.draw_overlay(
        canvas,
        "REPLAY REVIEW",
        x + pad,
        y + pad,
        header_scale,
        Color::RGBA(255, 210, 90, 240),
    )?;
    let header_w = font.text_width_exact(&header, scale);
    font.draw(
        canvas,
        &header,
        x + panel_w - pad - header_w,
        y + pad + 2,
        scale,
        Color::RGBA(220, 230, 255, 230),
    )?;

    let timeline_x = x + pad;
    let timeline_y = y + 38;
    let timeline_w = panel_w - pad * 2;
    canvas.set_draw_color(Color::RGBA(56, 62, 84, 220));
    canvas.fill_rect(Rect::new(timeline_x, timeline_y, timeline_w as u32, 5))?;
    let progress_w = ((timeline_w as f32) * (frame as f32 / total as f32))
        .round()
        .clamp(0.0, timeline_w as f32) as u32;
    canvas.set_draw_color(Color::RGBA(120, 210, 255, 230));
    canvas.fill_rect(Rect::new(timeline_x, timeline_y, progress_w, 5))?;

    for marker in playback.markers() {
        if !event_filter.matches_marker(marker.kind) {
            continue;
        }
        let mx = timeline_x
            + ((timeline_w as f32) * (marker.frame as f32 / total as f32))
                .round()
                .clamp(0.0, timeline_w as f32) as i32;
        let color = replay_marker_color(marker.kind);
        canvas.set_draw_color(color);
        canvas.fill_rect(Rect::new(mx, timeline_y - 5, 2, 15))?;
    }
    if event_filter.matches_bookmarks() {
        for bookmark in playback.bookmarks() {
            let mx = timeline_x
                + ((timeline_w as f32) * (bookmark.frame as f32 / total as f32))
                    .round()
                    .clamp(0.0, timeline_w as f32) as i32;
            canvas.set_draw_color(replay_bookmark_color());
            canvas.fill_rect(Rect::new(mx - 2, timeline_y - 7, 5, 17))?;
        }
    }

    let inputs = playback.current_inputs().unwrap_or([0, 0]);
    let p1 = crate::input_history::format_bits(inputs[0]);
    let p2 = crate::input_history::format_bits(inputs[1]);
    let next_marker = next_replay_event_line(playback, frame, event_filter);
    let input_line = format!("P1 {p1}     P2 {p2}     {next_marker}");
    let clip_line = replay_clip_line(clip_in, clip_out);
    let controls_1 = "SPACE/START PAUSE   . / A STEP   F/GUIDE FILTER";
    let controls_2 = "UP/DOWN SPEED   M/RS BOOKMARK   DEL/LS REMOVE";
    let controls_3 = "LEFT/RIGHT +/-5S   PGUP/PGDN/LB/RB EVENT   I/O X/Y CLIP";
    let input_line = fit_text_exact(font, &input_line, scale, timeline_w);
    let clip_line = fit_text_exact(font, &clip_line, scale, timeline_w);
    let controls_1 = fit_text_exact(font, controls_1, scale, timeline_w);
    let controls_2 = fit_text_exact(font, controls_2, scale, timeline_w);
    let controls_3 = fit_text_exact(font, controls_3, scale, timeline_w);
    font.draw(
        canvas,
        &input_line,
        x + pad,
        y + 56,
        scale,
        Color::RGBA(230, 238, 255, 230),
    )?;
    font.draw(
        canvas,
        &clip_line,
        x + pad,
        y + 74,
        scale,
        Color::RGBA(220, 230, 255, 220),
    )?;
    font.draw(
        canvas,
        &controls_1,
        x + pad,
        y + 92,
        scale,
        Color::RGBA(150, 165, 195, 190),
    )?;
    font.draw(
        canvas,
        &controls_2,
        x + pad,
        y + 110,
        scale,
        Color::RGBA(150, 165, 195, 190),
    )?;
    font.draw(
        canvas,
        &controls_3,
        x + pad,
        y + 128,
        scale,
        Color::RGBA(150, 165, 195, 190),
    )?;

    Ok(())
}

fn draw_replay_event_sidebar(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    playback: &crate::match_replay::Playback,
    event_filter: crate::match_replay::ReplayEventFilter,
) -> Result<(), String> {
    if window_w < 760 {
        return Ok(());
    }

    enum SidebarRaw {
        Marker(crate::match_replay::ReplayMarkerKind),
        Bookmark(String),
    }

    struct SidebarEvent {
        frame: u32,
        label: String,
        color: Color,
    }

    let mut raw = Vec::new();
    for marker in playback.markers() {
        if event_filter.matches_marker(marker.kind) {
            raw.push((marker.frame, 0_u8, SidebarRaw::Marker(marker.kind)));
        }
    }
    if event_filter.matches_bookmarks() {
        for bookmark in playback.bookmarks() {
            raw.push((
                bookmark.frame,
                1_u8,
                SidebarRaw::Bookmark(bookmark.note.clone()),
            ));
        }
    }
    if raw.is_empty() {
        return Ok(());
    }
    raw.sort_by_key(|(frame, order, _)| (*frame, *order));

    let mut round_count = 0;
    let mut hit_count = 0;
    let mut bookmark_count = 0;
    let events: Vec<SidebarEvent> = raw
        .into_iter()
        .map(|(frame, _, kind)| match kind {
            SidebarRaw::Marker(kind) => {
                let label = match kind {
                    crate::match_replay::ReplayMarkerKind::RoundStart => {
                        round_count += 1;
                        format!("ROUND {round_count}")
                    }
                    crate::match_replay::ReplayMarkerKind::RoundWinP1 => "P1 ROUND".to_string(),
                    crate::match_replay::ReplayMarkerKind::RoundWinP2 => "P2 ROUND".to_string(),
                    crate::match_replay::ReplayMarkerKind::Hit => {
                        hit_count += 1;
                        format!("HIT {hit_count:02}")
                    }
                    crate::match_replay::ReplayMarkerKind::FirstHit => "FIRST HIT".to_string(),
                    crate::match_replay::ReplayMarkerKind::BigDamage => "BIG DAMAGE".to_string(),
                    crate::match_replay::ReplayMarkerKind::LowHealth => "LOW HEALTH".to_string(),
                    crate::match_replay::ReplayMarkerKind::MatchEnd => "MATCH END".to_string(),
                };
                SidebarEvent {
                    frame,
                    label,
                    color: replay_marker_color(kind),
                }
            }
            SidebarRaw::Bookmark(note) => {
                bookmark_count += 1;
                let label = if note.is_empty() {
                    format!("MARK {bookmark_count:02}")
                } else {
                    format!("MARK {bookmark_count:02} {note}")
                };
                SidebarEvent {
                    frame,
                    label,
                    color: replay_bookmark_color(),
                }
            }
        })
        .collect();

    let pad = 8;
    let panel_w = 226;
    let bottom_limit = window_h - 178;
    let panel_h = (bottom_limit - 88).clamp(132, 360);
    let x = window_w - panel_w - 18;
    let y = 88;
    let row_h = 18;
    let scale = 1;
    let frame = playback.current_frame();
    let active = events
        .iter()
        .rposition(|event| event.frame as usize <= frame)
        .unwrap_or(0);
    let max_rows = ((panel_h - 44) / row_h).max(3) as usize;
    let mut start = active.saturating_sub(max_rows / 2);
    if start + max_rows > events.len() {
        start = events.len().saturating_sub(max_rows);
    }
    let end = (start + max_rows).min(events.len());

    canvas.set_draw_color(Color::RGBA(8, 10, 18, 200));
    canvas.fill_rect(Rect::new(x, y, panel_w as u32, panel_h as u32))?;
    canvas.set_draw_color(Color::RGBA(95, 130, 210, 170));
    canvas.draw_rect(Rect::new(x, y, panel_w as u32, panel_h as u32))?;
    let sidebar_title = format!("{} EVENTS", event_filter.label());
    font.draw_overlay(
        canvas,
        &sidebar_title,
        x + pad,
        y + pad,
        scale,
        Color::RGBA(255, 210, 90, 240),
    )?;

    let mut row_y = y + 34;
    for (i, event) in events[start..end].iter().enumerate() {
        let absolute = start + i;
        let is_active = absolute == active;
        if is_active {
            canvas.set_draw_color(Color::RGBA(34, 42, 66, 230));
            canvas.fill_rect(Rect::new(
                x + 4,
                row_y - 3,
                (panel_w - 8) as u32,
                (row_h - 2) as u32,
            ))?;
        }
        canvas.set_draw_color(event.color);
        canvas.fill_rect(Rect::new(x + pad, row_y + 4, 4, 4))?;
        let stamp = format_review_clock(event.frame as usize);
        let stamp_w = font.text_width_exact(&stamp, scale);
        let label = fit_text_exact(font, &event.label, scale, panel_w - pad * 2 - stamp_w - 20);
        font.draw(
            canvas,
            &label,
            x + pad + 12,
            row_y,
            scale,
            if is_active {
                Color::RGBA(235, 242, 255, 245)
            } else {
                Color::RGBA(205, 214, 235, 220)
            },
        )?;
        font.draw(
            canvas,
            &stamp,
            x + panel_w - pad - stamp_w,
            row_y,
            scale,
            Color::RGBA(145, 156, 184, 210),
        )?;
        row_y += row_h;
    }

    Ok(())
}

fn replay_marker_label(kind: crate::match_replay::ReplayMarkerKind) -> &'static str {
    match kind {
        crate::match_replay::ReplayMarkerKind::RoundStart => "ROUND",
        crate::match_replay::ReplayMarkerKind::RoundWinP1 => "P1 RD",
        crate::match_replay::ReplayMarkerKind::RoundWinP2 => "P2 RD",
        crate::match_replay::ReplayMarkerKind::Hit => "HIT",
        crate::match_replay::ReplayMarkerKind::FirstHit => "1ST",
        crate::match_replay::ReplayMarkerKind::BigDamage => "BIG",
        crate::match_replay::ReplayMarkerKind::LowHealth => "LOW",
        crate::match_replay::ReplayMarkerKind::MatchEnd => "END",
    }
}

fn replay_marker_color(kind: crate::match_replay::ReplayMarkerKind) -> Color {
    match kind {
        crate::match_replay::ReplayMarkerKind::RoundStart => Color::RGBA(255, 220, 120, 240),
        crate::match_replay::ReplayMarkerKind::RoundWinP1 => Color::RGBA(130, 235, 150, 245),
        crate::match_replay::ReplayMarkerKind::RoundWinP2 => Color::RGBA(150, 175, 255, 245),
        crate::match_replay::ReplayMarkerKind::Hit => Color::RGBA(235, 85, 85, 240),
        crate::match_replay::ReplayMarkerKind::FirstHit => Color::RGBA(255, 165, 90, 245),
        crate::match_replay::ReplayMarkerKind::BigDamage => Color::RGBA(255, 95, 210, 245),
        crate::match_replay::ReplayMarkerKind::LowHealth => Color::RGBA(120, 225, 255, 245),
        crate::match_replay::ReplayMarkerKind::MatchEnd => Color::RGBA(180, 235, 255, 245),
    }
}

fn replay_bookmark_color() -> Color {
    Color::RGBA(120, 245, 150, 245)
}

fn next_replay_event_line(
    playback: &crate::match_replay::Playback,
    frame: usize,
    filter: crate::match_replay::ReplayEventFilter,
) -> String {
    let marker = playback
        .markers()
        .iter()
        .find(|marker| marker.frame as usize > frame && filter.matches_marker(marker.kind))
        .map(|marker| (marker.frame, replay_marker_label(marker.kind).to_string()));
    let bookmark = playback
        .next_bookmark_after(frame)
        .filter(|_| filter.matches_bookmarks())
        .map(|bookmark| (bookmark.frame, "MARK".to_string()));
    marker
        .into_iter()
        .chain(bookmark)
        .min_by_key(|(event_frame, _)| *event_frame)
        .map(|(event_frame, label)| format!("NEXT {label} @{event_frame}"))
        .unwrap_or_else(|| {
            if filter == crate::match_replay::ReplayEventFilter::All {
                "NO NEXT EVENT".to_string()
            } else {
                format!("NO NEXT {} EVENT", filter.label())
            }
        })
}

fn replay_clip_line(clip_in: Option<usize>, clip_out: Option<usize>) -> String {
    match (clip_in, clip_out) {
        (Some(start), Some(end)) => {
            let from = start.min(end);
            let to = start.max(end);
            format!(
                "CLIP IN {}  OUT {}  LEN {}",
                format_review_clock(from),
                format_review_clock(to),
                format_review_duration(to.saturating_sub(from))
            )
        }
        (Some(start), None) => format!("CLIP IN {}  OUT --", format_review_clock(start)),
        (None, Some(end)) => format!("CLIP IN --  OUT {}", format_review_clock(end)),
        (None, None) => "CLIP IN --  OUT --".to_string(),
    }
}

fn format_review_clock(frame: usize) -> String {
    let mins = frame / (55 * 60);
    let secs = (frame / 55) % 60;
    format!("{mins:02}:{secs:02}")
}

fn format_review_duration(frames: usize) -> String {
    let tenths = frames.saturating_mul(10) / 55;
    format!("{}.{:01}s", tenths / 10, tenths % 10)
}

fn format_input_frames(frames: u32) -> String {
    if frames > 99 {
        "99+".into()
    } else {
        format!("{frames}f")
    }
}

fn draw_fight_overlay_plates(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    p1_name: &str,
    p2_name: &str,
    p1_wins: u32,
    p2_wins: u32,
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

/// Tag-only layout: just the gamertags, pulled toward the center with no
/// scoreplate art. Useful when streamers want a minimalist HUD or when the
/// player count overlap with the in-game health bars feels noisy.
fn draw_fight_overlay_centered(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    font: &mut Font,
    window_w: i32,
    window_h: i32,
    p1_name: &str,
    p2_name: &str,
) -> Result<(), String> {
    let name_scale: u32 = 1;
    let white = sdl2::pixels::Color::RGBA(248, 248, 250, 235);
    let shadow = sdl2::pixels::Color::RGBA(8, 6, 14, 200);

    let center_x = window_w / 2;
    // Tighter gap than before — gamertags should hug the MK2 timer rather than
    // float far apart.
    let inner_gap = ((window_w as f32) * 0.10).round() as i32;
    let inner_gap = inner_gap.clamp(90, 170);
    let name_max_w = ((center_x - inner_gap / 2) - 12).max(80);
    // Sit just under MK2's timer (top-center of emu frame). Anchor proportionally
    // to window height so it stays put across windowed/fullscreen sizes.
    let name_y = ((window_h as f32) * 0.065).round() as i32;
    let name_y = name_y.clamp(36, 84);

    let p1 = fit_overlay_text(font, &p1_name.to_uppercase(), name_scale, name_max_w);
    let p2 = fit_overlay_text(font, &p2_name.to_uppercase(), name_scale, name_max_w);

    let p1_w = font.text_width_overlay(&p1, name_scale);

    let p1_x = center_x - inner_gap / 2 - p1_w;
    let p2_x = center_x + inner_gap / 2;

    font.draw_overlay(canvas, &p1, p1_x + 1, name_y + 1, name_scale, shadow)?;
    font.draw_overlay(canvas, &p2, p2_x + 1, name_y + 1, name_scale, shadow)?;
    font.draw_overlay(canvas, &p1, p1_x, name_y, name_scale, white)?;
    font.draw_overlay(canvas, &p2, p2_x, name_y, name_scale, white)?;

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

fn fit_text_exact(font: &mut Font, text: &str, scale: u32, max_w: i32) -> String {
    if font.text_width_exact(text, scale) <= max_w {
        return text.to_string();
    }

    let mut out = String::new();
    for ch in text.chars() {
        let candidate = format!("{out}{ch}...");
        if font.text_width_exact(&candidate, scale) > max_w {
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
