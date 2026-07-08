//! Logical-canvas -> window-space scaling. All fp_ui layout is authored in
//! 1920x1080 logical units; `Scale` converts a logical rect/size to actual
//! window pixels with one uniform factor plus letterbox offsets, so panels
//! and text scale together instead of text being drawn at a fixed size and
//! stretched (blurry) or panels drifting out of proportion with text.

use super::theme::{VH, VW};
use sdl2::rect::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub s: f32,
    pub off_x: f32,
    pub off_y: f32,
}

impl Scale {
    /// Uniform scale factor `s = min(win_w/1920, win_h/1080)` with the
    /// remaining space letterboxed (centered) on whichever axis doesn't
    /// exactly fill.
    pub fn compute(win_w: i32, win_h: i32) -> Scale {
        let win_w = win_w.max(1) as f32;
        let win_h = win_h.max(1) as f32;
        let s = (win_w / VW).min(win_h / VH);
        let off_x = (win_w - VW * s) * 0.5;
        let off_y = (win_h - VH * s) * 0.5;
        Scale { s, off_x, off_y }
    }

    /// Logical -> window-space rect. Rounds to the nearest pixel rather than
    /// truncating so panel edges don't accumulate a systematic sub-pixel bias
    /// at odd window sizes.
    #[allow(dead_code)]
    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let sx = (self.off_x + x * self.s).round() as i32;
        let sy = (self.off_y + y * self.s).round() as i32;
        let sw = (w * self.s).round().max(0.0) as u32;
        let sh = (h * self.s).round().max(0.0) as u32;
        Rect::new(sx, sy, sw, sh)
    }

    /// Logical point -> window-space point (for text origins, vertices, etc).
    pub fn point(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (self.off_x + x * self.s).round() as i32,
            (self.off_y + y * self.s).round() as i32,
        )
    }

    /// A logical length (e.g. a stroke width) -> window-space pixels.
    #[allow(dead_code)]
    pub fn len(&self, l: f32) -> f32 {
        l * self.s
    }

    /// A logical font size -> the concrete point size to rasterize at.
    /// Rounded (not truncated) and floored to 1pt so a tiny window never
    /// asks `SDL_ttf` for a 0pt font.
    pub fn font_px(&self, logical_px: f32) -> u16 {
        (logical_px * self.s).round().max(1.0) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_matches_exact_1920x1080() {
        let scale = Scale::compute(1920, 1080);
        assert!((scale.s - 1.0).abs() < f32::EPSILON);
        assert_eq!(scale.off_x, 0.0);
        assert_eq!(scale.off_y, 0.0);
    }

    #[test]
    fn compute_letterboxes_wider_window() {
        // 2560x1080 is wider than 16:9, so height is the binding dimension
        // and the extra width is split evenly as letterbox padding.
        let scale = Scale::compute(2560, 1080);
        assert!((scale.s - 1.0).abs() < f32::EPSILON);
        assert!(scale.off_x > 0.0);
        assert_eq!(scale.off_y, 0.0);
    }

    #[test]
    fn compute_letterboxes_taller_window() {
        let scale = Scale::compute(1920, 2160);
        assert!((scale.s - 1.0).abs() < f32::EPSILON);
        assert_eq!(scale.off_x, 0.0);
        assert!(scale.off_y > 0.0);
    }

    /// The scale-aware font cache re-rasterizes at a size derived from this
    /// function, so resizing the window must change the requested point
    /// size — never keep rendering at a stale logical size and stretch the
    /// texture. This is the "debug resize test" for the skeleton: proves the
    /// same logical size maps to a genuinely different pixel size, and thus
    /// a different font-cache key, at two window sizes.
    #[test]
    fn font_px_rescales_with_window_size() {
        let small = Scale::compute(960, 540); // half res -> s = 0.5
        let large = Scale::compute(1920, 1080); // native -> s = 1.0
        let logical_size = 52.0; // e.g. featured-fighter-name size
        assert_eq!(small.font_px(logical_size), 26);
        assert_eq!(large.font_px(logical_size), 52);
        assert_ne!(small.font_px(logical_size), large.font_px(logical_size));
    }

    #[test]
    fn font_px_never_rounds_to_zero() {
        let tiny = Scale::compute(10, 10);
        assert!(tiny.font_px(1.0) >= 1);
    }
}
