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

/// Fill a soft radial glow as a multi-stop `(0.0-1.0 fraction of (rx,ry),
/// color)` sequence (ascending, first stop at `0.0`) over an *ellipse*
/// (`rx`/`ry` independent, matching CSS `radial-gradient(Wpct Hpct at ...)`'s
/// two-value size against a non-square box — a single shared radius distorts
/// the true shape whenever the box isn't square, e.g. 1920x1080). The
/// innermost stop renders as a Gouraud-shaded triangle fan (like
/// `fill_circle`); each subsequent stop renders as an annulus ring of quads
/// between the previous and current stop's radius fraction, colored at each
/// edge so SDL interpolates the fade across the ring. Used to approximate
/// mockup CSS `radial-gradient(...)` background glows, which SDL2 has no
/// native primitive for.
pub fn fill_radial_ellipse_gradient(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    stops: &[(f32, Color)],
) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let point_at = |t: f32, a: f32| scale.point(cx + a.cos() * rx * t, cy + a.sin() * ry * t);
    for pair in stops.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t0 <= 0.0 {
            let center = scale.point(cx, cy);
            let center_v = vertex(center.0 as f32, center.1 as f32, c0);
            let mut perim = Vec::with_capacity(CIRCLE_SEGMENTS + 1);
            for i in 0..=CIRCLE_SEGMENTS {
                let a = std::f32::consts::TAU * i as f32 / CIRCLE_SEGMENTS as f32;
                let p = point_at(t1, a);
                perim.push(vertex(p.0 as f32, p.1 as f32, c1));
            }
            let mut verts = Vec::with_capacity(CIRCLE_SEGMENTS * 3);
            for i in 0..CIRCLE_SEGMENTS {
                verts.push(center_v);
                verts.push(perim[i]);
                verts.push(perim[i + 1]);
            }
            fill_triangles(canvas, &verts);
        } else {
            let mut verts = Vec::with_capacity(CIRCLE_SEGMENTS * 6);
            for i in 0..CIRCLE_SEGMENTS {
                let a0 = std::f32::consts::TAU * i as f32 / CIRCLE_SEGMENTS as f32;
                let a1 = std::f32::consts::TAU * (i + 1) as f32 / CIRCLE_SEGMENTS as f32;
                let o0 = point_at(t1, a0);
                let o1 = point_at(t1, a1);
                let i0 = point_at(t0, a0);
                let i1 = point_at(t0, a1);
                let o0v = vertex(o0.0 as f32, o0.1 as f32, c1);
                let o1v = vertex(o1.0 as f32, o1.1 as f32, c1);
                let i0v = vertex(i0.0 as f32, i0.1 as f32, c0);
                let i1v = vertex(i1.0 as f32, i1.1 as f32, c0);
                verts.push(o0v);
                verts.push(o1v);
                verts.push(i1v);
                verts.push(o0v);
                verts.push(i1v);
                verts.push(i0v);
            }
            fill_triangles(canvas, &verts);
        }
    }
}

/// Fill a skewed parallelogram (logical rect `(x, y, w, h)`, skewed by
/// `skew_deg` around its own local vertical center, same convention as
/// `fill_skewed_rect`) with a vertical multi-stop gradient — `stops` are
/// `(0.0-1.0 fraction of h, color)` pairs, ascending, first at `0.0` and
/// last at `1.0`. The shear offset is computed once against the *whole*
/// shape's height rather than per-sub-band, so adjacent bands share the
/// same edge position with no visible seam.
pub fn fill_skewed_rect_vgradient(
    canvas: &mut Canvas<Window>,
    scale: &Scale,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    skew_deg: f32,
    stops: &[(f32, Color)],
) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let skew = skew_deg.to_radians().tan();
    let shift_at = |local_y: f32| skew * (local_y - h / 2.0);
    for pair in stops.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        let ly0 = h * t0;
        let ly1 = h * t1;
        let s0 = shift_at(ly0);
        let s1 = shift_at(ly1);
        let tl = scale.point(x + s0, y + ly0);
        let tr = scale.point(x + w + s0, y + ly0);
        let bl = scale.point(x + s1, y + ly1);
        let br = scale.point(x + w + s1, y + ly1);
        let verts = [
            vertex(tl.0 as f32, tl.1 as f32, c0),
            vertex(tr.0 as f32, tr.1 as f32, c0),
            vertex(br.0 as f32, br.1 as f32, c1),
            vertex(tl.0 as f32, tl.1 as f32, c0),
            vertex(br.0 as f32, br.1 as f32, c1),
            vertex(bl.0 as f32, bl.1 as f32, c1),
        ];
        fill_triangles(canvas, &verts);
    }
}

/// Fill a full-box linear gradient band across the entire logical `VW x VH`
/// stage, CSS `linear-gradient(angle_deg, stops...)` convention (`0deg`
/// points "to top", increasing clockwise — `dx = sin(a)`, `dy = -cos(a)` is
/// the direction of increasing `%`). `stops` are `(0.0-1.0 fraction along
/// the gradient line, color)` pairs. The gradient-line span is derived from
/// the box's own corner projections (centered on the box), matching the CSS
/// spec's definition of the line length for an angled full-box gradient —
/// not just the box's width or height alone, which is wrong for diagonal
/// angles. Each stop pair renders as a wide quad perpendicular to the
/// gradient direction, long enough on the perpendicular axis to fully cover
/// the box regardless of rotation.
pub fn fill_linear_gradient_box(canvas: &mut Canvas<Window>, scale: &Scale, angle_deg: f32, stops: &[(f32, Color)]) {
    canvas.set_blend_mode(sdl2::render::BlendMode::Blend);
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let (px, py) = (-dy, dx);
    let w = super::theme::VW;
    let h = super::theme::VH;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let mut min_p = f32::MAX;
    let mut max_p = f32::MIN;
    for (cxp, cyp) in corners {
        let proj = (cxp - cx) * dx + (cyp - cy) * dy;
        min_p = min_p.min(proj);
        max_p = max_p.max(proj);
    }
    let span = max_p - min_p;
    let half_perp = (w * w + h * h).sqrt();
    for pair in stops.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        let p0 = min_p + span * t0;
        let p1 = min_p + span * t1;
        let (c0x, c0y) = (cx + dx * p0, cy + dy * p0);
        let (c1x, c1y) = (cx + dx * p1, cy + dy * p1);
        let tl = scale.point(c0x - px * half_perp, c0y - py * half_perp);
        let tr = scale.point(c0x + px * half_perp, c0y + py * half_perp);
        let bl = scale.point(c1x - px * half_perp, c1y - py * half_perp);
        let br = scale.point(c1x + px * half_perp, c1y + py * half_perp);
        let verts = [
            vertex(tl.0 as f32, tl.1 as f32, c0),
            vertex(tr.0 as f32, tr.1 as f32, c0),
            vertex(br.0 as f32, br.1 as f32, c1),
            vertex(tl.0 as f32, tl.1 as f32, c0),
            vertex(br.0 as f32, br.1 as f32, c1),
            vertex(bl.0 as f32, bl.1 as f32, c1),
        ];
        fill_triangles(canvas, &verts);
    }
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
