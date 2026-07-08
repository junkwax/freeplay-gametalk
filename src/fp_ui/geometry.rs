//! Skewed-parallelogram fills via `SDL_RenderGeometry`.
//!
//! `sdl2` 0.35's safe `Canvas` API has no wrapper for `SDL_RenderGeometry`
//! (it postdates this crate version's bindings), but the function itself is
//! part of stable SDL2 (added 2.0.18; the bundled runtime here is 2.30) and
//! `sdl2::sys` — a straight re-export of `sdl2-sys`, already a transitive
//! dependency of the `sdl2` crate — exposes everything needed to call it by
//! hand: no new crate, no new native dependency. This is the mechanism the
//! handoff doc calls for: "Never use SDL_RenderFillRect for skewed elements
//! — use SDL_RenderGeometry with 2 triangles."
//!
//! Also used for the active menu row's horizontal fade, which needs a
//! per-vertex color gradient `SDL_RenderFillRect` can't do at all.

use super::layout::Scale;
use sdl2::pixels::Color;
use sdl2::render::Canvas;
use sdl2::sys;
use sdl2::video::Window;

/// Mirrors the stable C `SDL_Vertex` layout (position, color, tex_coord) —
/// hand-declared because this sdl2-sys version's pre-generated bindings
/// predate it.
#[repr(C)]
#[derive(Clone, Copy)]
struct SdlVertex {
    position: sys::SDL_FPoint,
    color: sys::SDL_Color,
    tex_coord: sys::SDL_FPoint,
}

extern "C" {
    fn SDL_RenderGeometry(
        renderer: *mut sys::SDL_Renderer,
        texture: *mut sys::SDL_Texture,
        vertices: *const SdlVertex,
        num_vertices: i32,
        indices: *const i32,
        num_indices: i32,
    ) -> i32;
}

fn vertex(x: f32, y: f32, color: Color) -> SdlVertex {
    SdlVertex {
        position: sys::SDL_FPoint { x, y },
        color: sys::SDL_Color {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        },
        tex_coord: sys::SDL_FPoint { x: 0.0, y: 0.0 },
    }
}

fn fill_triangles(canvas: &mut Canvas<Window>, verts: &[SdlVertex]) {
    unsafe {
        SDL_RenderGeometry(
            canvas.raw(),
            std::ptr::null_mut(),
            verts.as_ptr(),
            verts.len() as i32,
            std::ptr::null(),
            0,
        );
    }
}

/// Segments for circle geometry — enough that the polygon reads as smooth
/// at any size fp_ui actually draws circles (34px footer chips up to the
/// ~340px radar), rather than the pixelated rectangle-strip/line-segment
/// approximation this replaced.
const CIRCLE_SEGMENTS: usize = 64;

/// Fill a solid circle (logical center/radius) as a triangle fan.
pub fn fill_circle(canvas: &mut Canvas<Window>, scale: &Scale, cx: f32, cy: f32, r: f32, color: Color) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let center = scale.point(cx, cy);
    let center_v = vertex(center.0 as f32, center.1 as f32, color);
    let mut perim = Vec::with_capacity(CIRCLE_SEGMENTS + 1);
    for i in 0..=CIRCLE_SEGMENTS {
        let a = std::f32::consts::TAU * i as f32 / CIRCLE_SEGMENTS as f32;
        let p = scale.point(cx + a.cos() * r, cy + a.sin() * r);
        perim.push(vertex(p.0 as f32, p.1 as f32, color));
    }
    let mut verts = Vec::with_capacity(CIRCLE_SEGMENTS * 3);
    for i in 0..CIRCLE_SEGMENTS {
        verts.push(center_v);
        verts.push(perim[i]);
        verts.push(perim[i + 1]);
    }
    fill_triangles(canvas, &verts);
}

/// Stroke a circle outline (logical center/radius, logical border
/// `thickness`) as a ring of quads between an inner and outer radius.
pub fn stroke_circle(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    cx: f32,
    cy: f32,
    r: f32,
    thickness: f32,
    color: Color,
) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let r_out = r + thickness / 2.0;
    let r_in = (r - thickness / 2.0).max(0.0);
    let mut verts = Vec::with_capacity(CIRCLE_SEGMENTS * 6);
    for i in 0..CIRCLE_SEGMENTS {
        let a0 = std::f32::consts::TAU * i as f32 / CIRCLE_SEGMENTS as f32;
        let a1 = std::f32::consts::TAU * (i + 1) as f32 / CIRCLE_SEGMENTS as f32;
        let o0 = scale.point(cx + a0.cos() * r_out, cy + a0.sin() * r_out);
        let o1 = scale.point(cx + a1.cos() * r_out, cy + a1.sin() * r_out);
        let i0 = scale.point(cx + a0.cos() * r_in, cy + a0.sin() * r_in);
        let i1 = scale.point(cx + a1.cos() * r_in, cy + a1.sin() * r_in);
        let o0v = vertex(o0.0 as f32, o0.1 as f32, color);
        let o1v = vertex(o1.0 as f32, o1.1 as f32, color);
        let i0v = vertex(i0.0 as f32, i0.1 as f32, color);
        let i1v = vertex(i1.0 as f32, i1.1 as f32, color);
        verts.push(o0v);
        verts.push(o1v);
        verts.push(i1v);
        verts.push(o0v);
        verts.push(i1v);
        verts.push(i0v);
    }
    fill_triangles(canvas, &verts);
}

/// The 4 corners of a `skewX(deg)` parallelogram for a logical rect at
/// `(x, y, w, h)`, then converted to window space. `skew = tan(deg in
/// radians)`; each corner's x shifts by `skew * (local y - h/2)` — local to
/// the shape's own vertical center, matching CSS `transform: skewX()`'s
/// default transform-origin (the element's own center), NOT the canvas
/// origin. The handoff doc's own formula (`skew * that corner's absolute
/// y`) was tried first and produces a net sideways drift proportional to a
/// shape's absolute Y position — harmless near the top of the screen, badly
/// wrong for anything positioned further down (a menu row near the bottom
/// of a 1080-tall canvas would shift ~170px left of its intended x).
fn skewed_corners(scale: &Scale, x: f32, y: f32, w: f32, h: f32, skew_deg: f32) -> [(f32, f32); 4] {
    let skew = skew_deg.to_radians().tan();
    let top_shift = skew * (-h / 2.0);
    let bottom_shift = skew * (h / 2.0);
    let tl = scale.point(x + top_shift, y);
    let tr = scale.point(x + w + top_shift, y);
    let bl = scale.point(x + bottom_shift, y + h);
    let br = scale.point(x + w + bottom_shift, y + h);
    [
        (tl.0 as f32, tl.1 as f32),
        (tr.0 as f32, tr.1 as f32),
        (br.0 as f32, br.1 as f32),
        (bl.0 as f32, bl.1 as f32),
    ]
}

/// Fill a skewed parallelogram (logical rect `(x, y, w, h)`, skewed by
/// `skew_deg`) with a solid color. Draws 2 triangles: (tl, tr, br) and
/// (tl, br, bl).
pub fn fill_skewed_rect(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    skew_deg: f32,
    color: Color,
) {
    let [tl, tr, br, bl] = skewed_corners(scale, x, y, w, h, skew_deg);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let verts = [
        vertex(tl.0, tl.1, color),
        vertex(tr.0, tr.1, color),
        vertex(br.0, br.1, color),
        vertex(tl.0, tl.1, color),
        vertex(br.0, br.1, color),
        vertex(bl.0, bl.1, color),
    ];
    fill_triangles(canvas, &verts);
}

/// Fill an arbitrary solid-color triangle from 3 logical points — used for
/// the selected menu row's `&#9656;` chevron, which the mockup skews the
/// same as everything else in this design (`skewX(-9deg)`); simplest to
/// just hand the caller pre-skewed points rather than adding a whole
/// second skew parameter here.
pub fn fill_triangle(canvas: &mut Canvas<Window>, scale: &Scale, points: [(f32, f32); 3], color: Color) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let verts: Vec<SdlVertex> = points
        .iter()
        .map(|&(x, y)| {
            let p = scale.point(x, y);
            vertex(p.0 as f32, p.1 as f32, color)
        })
        .collect();
    fill_triangles(canvas, &verts);
}

/// Fill an axis-aligned logical rect `(x, y, w, h)` with a vertical color
/// gradient (`top` at y, `bottom` at y+h) — used for the Main Menu's top
/// background glow.
pub fn fill_vertical_gradient_rect(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    top: Color,
    bottom: Color,
) {
    let tl = scale.point(x, y);
    let tr = scale.point(x + w, y);
    let bl = scale.point(x, y + h);
    let br = scale.point(x + w, y + h);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let verts = [
        vertex(tl.0 as f32, tl.1 as f32, top),
        vertex(tr.0 as f32, tr.1 as f32, top),
        vertex(br.0 as f32, br.1 as f32, bottom),
        vertex(tl.0 as f32, tl.1 as f32, top),
        vertex(br.0 as f32, br.1 as f32, bottom),
        vertex(bl.0 as f32, bl.1 as f32, bottom),
    ];
    fill_triangles(canvas, &verts);
}

/// Fill an axis-aligned logical rect `(x, y, w, h)` with a horizontal color
/// gradient (`left` at x, `right` at x+w), interpolated per-pixel by
/// `SDL_RenderGeometry`'s vertex color blending — used for the active menu
/// row's `linear-gradient(90deg, accent-tint, transparent)` background,
/// which `SDL_RenderFillRect` has no equivalent for.
pub fn fill_horizontal_gradient_rect(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    left: Color,
    right: Color,
) {
    let tl = scale.point(x, y);
    let tr = scale.point(x + w, y);
    let bl = scale.point(x, y + h);
    let br = scale.point(x + w, y + h);
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let verts = [
        vertex(tl.0 as f32, tl.1 as f32, left),
        vertex(tr.0 as f32, tr.1 as f32, right),
        vertex(br.0 as f32, br.1 as f32, right),
        vertex(tl.0 as f32, tl.1 as f32, left),
        vertex(br.0 as f32, br.1 as f32, right),
        vertex(bl.0 as f32, bl.1 as f32, left),
    ];
    fill_triangles(canvas, &verts);
}
