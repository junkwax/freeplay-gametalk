//! Font rendering: embedded 8x8 bitmap fallback + optional TTF loaded from
//! disk (`mk2.ttf`). The `Font` struct exposes a single API so the menu code
//! doesn't care which backend drew the glyphs.
//!
//! TTF paths cache each (text, color, scale) combination as a pre-rendered
//! `Texture`, so re-drawing the same menu every frame is effectively free.

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::ttf::Sdl2TtfContext;
use sdl2::video::{Window, WindowContext};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const GLYPH_W: u32 = 8;
pub const GLYPH_H: u32 = 8;
const FIRST_CHAR: u8 = 32;
const LAST_CHAR: u8 = 126;
const NUM_GLYPHS: usize = (LAST_CHAR - FIRST_CHAR + 1) as usize;

/// Classic 8x8 public-domain glyphs (ASCII 32..126). Bit 7 = leftmost pixel.
#[rustfmt::skip]
const GLYPHS: [[u8; 8]; NUM_GLYPHS] = [
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00], // ' '
    [0x18,0x18,0x18,0x18,0x00,0x00,0x18,0x00], [0x6C,0x6C,0x00,0x00,0x00,0x00,0x00,0x00],
    [0x6C,0xFE,0x6C,0x6C,0xFE,0x6C,0x00,0x00], [0x18,0x7C,0xC0,0x7C,0x06,0xFC,0x30,0x00],
    [0x00,0xC6,0xCC,0x18,0x30,0x66,0xC6,0x00], [0x38,0x6C,0x38,0x76,0xDC,0xCC,0x76,0x00],
    [0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00], [0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00],
    [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00], [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00],
    [0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00], [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30],
    [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00], [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00],
    [0x06,0x0C,0x18,0x30,0x60,0xC0,0x80,0x00], [0x7C,0xC6,0xCE,0xDE,0xF6,0xE6,0x7C,0x00],
    [0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00], [0x7C,0xC6,0x06,0x1C,0x30,0x66,0xFE,0x00],
    [0x7C,0xC6,0x06,0x3C,0x06,0xC6,0x7C,0x00], [0x1C,0x3C,0x6C,0xCC,0xFE,0x0C,0x1E,0x00],
    [0xFE,0xC0,0xFC,0x06,0x06,0xC6,0x7C,0x00], [0x38,0x60,0xC0,0xFC,0xC6,0xC6,0x7C,0x00],
    [0xFE,0xC6,0x0C,0x18,0x30,0x30,0x30,0x00], [0x7C,0xC6,0xC6,0x7C,0xC6,0xC6,0x7C,0x00],
    [0x7C,0xC6,0xC6,0x7E,0x06,0x0C,0x78,0x00], [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x00],
    [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x30], [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00],
    [0x00,0x00,0x7E,0x00,0x7E,0x00,0x00,0x00], [0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00],
    [0x7C,0xC6,0x0C,0x18,0x18,0x00,0x18,0x00], [0x7C,0xC6,0xDE,0xDE,0xDE,0xC0,0x78,0x00],
    [0x38,0x6C,0xC6,0xC6,0xFE,0xC6,0xC6,0x00], [0xFC,0x66,0x66,0x7C,0x66,0x66,0xFC,0x00],
    [0x3C,0x66,0xC0,0xC0,0xC0,0x66,0x3C,0x00], [0xF8,0x6C,0x66,0x66,0x66,0x6C,0xF8,0x00],
    [0xFE,0x62,0x68,0x78,0x68,0x62,0xFE,0x00], [0xFE,0x62,0x68,0x78,0x68,0x60,0xF0,0x00],
    [0x3C,0x66,0xC0,0xC0,0xCE,0x66,0x3A,0x00], [0xC6,0xC6,0xC6,0xFE,0xC6,0xC6,0xC6,0x00],
    [0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00], [0x1E,0x0C,0x0C,0x0C,0xCC,0xCC,0x78,0x00],
    [0xE6,0x66,0x6C,0x78,0x6C,0x66,0xE6,0x00], [0xF0,0x60,0x60,0x60,0x62,0x66,0xFE,0x00],
    [0xC6,0xEE,0xFE,0xFE,0xD6,0xC6,0xC6,0x00], [0xC6,0xE6,0xF6,0xDE,0xCE,0xC6,0xC6,0x00],
    [0x38,0x6C,0xC6,0xC6,0xC6,0x6C,0x38,0x00], [0xFC,0x66,0x66,0x7C,0x60,0x60,0xF0,0x00],
    [0x7C,0xC6,0xC6,0xC6,0xD6,0x7C,0x0E,0x00], [0xFC,0x66,0x66,0x7C,0x6C,0x66,0xE6,0x00],
    [0x7C,0xC6,0xE0,0x78,0x0E,0xC6,0x7C,0x00], [0x7E,0x7E,0x5A,0x18,0x18,0x18,0x3C,0x00],
    [0xC6,0xC6,0xC6,0xC6,0xC6,0xC6,0x7C,0x00], [0xC6,0xC6,0xC6,0xC6,0xC6,0x6C,0x38,0x00],
    [0xC6,0xC6,0xC6,0xD6,0xFE,0xEE,0xC6,0x00], [0xC6,0xC6,0x6C,0x38,0x6C,0xC6,0xC6,0x00],
    [0x66,0x66,0x66,0x3C,0x18,0x18,0x3C,0x00], [0xFE,0xC6,0x8C,0x18,0x32,0x66,0xFE,0x00],
    [0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00], [0xC0,0x60,0x30,0x18,0x0C,0x06,0x02,0x00],
    [0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00], [0x10,0x38,0x6C,0xC6,0x00,0x00,0x00,0x00],
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF], [0x30,0x18,0x0C,0x00,0x00,0x00,0x00,0x00],
    [0x00,0x00,0x78,0x0C,0x7C,0xCC,0x76,0x00], [0xE0,0x60,0x7C,0x66,0x66,0x66,0xDC,0x00],
    [0x00,0x00,0x7C,0xC6,0xC0,0xC6,0x7C,0x00], [0x1C,0x0C,0x7C,0xCC,0xCC,0xCC,0x76,0x00],
    [0x00,0x00,0x7C,0xC6,0xFE,0xC0,0x7C,0x00], [0x3C,0x66,0x60,0xF8,0x60,0x60,0xF0,0x00],
    [0x00,0x00,0x76,0xCC,0xCC,0x7C,0x0C,0xF8], [0xE0,0x60,0x6C,0x76,0x66,0x66,0xE6,0x00],
    [0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00], [0x06,0x00,0x06,0x06,0x06,0x66,0x66,0x3C],
    [0xE0,0x60,0x66,0x6C,0x78,0x6C,0xE6,0x00], [0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
    [0x00,0x00,0xEC,0xFE,0xD6,0xD6,0xD6,0x00], [0x00,0x00,0xDC,0x66,0x66,0x66,0x66,0x00],
    [0x00,0x00,0x7C,0xC6,0xC6,0xC6,0x7C,0x00], [0x00,0x00,0xDC,0x66,0x66,0x7C,0x60,0xF0],
    [0x00,0x00,0x76,0xCC,0xCC,0x7C,0x0C,0x1E], [0x00,0x00,0xDC,0x76,0x60,0x60,0xF0,0x00],
    [0x00,0x00,0x7E,0xC0,0x7C,0x06,0xFC,0x00], [0x30,0x30,0xFC,0x30,0x30,0x36,0x1C,0x00],
    [0x00,0x00,0xCC,0xCC,0xCC,0xCC,0x76,0x00], [0x00,0x00,0xC6,0xC6,0xC6,0x6C,0x38,0x00],
    [0x00,0x00,0xC6,0xD6,0xD6,0xFE,0x6C,0x00], [0x00,0x00,0xC6,0x6C,0x38,0x6C,0xC6,0x00],
    [0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0xF0], [0x00,0x00,0xFC,0x98,0x30,0x64,0xFC,0x00],
    [0x1C,0x30,0x30,0x60,0x30,0x30,0x1C,0x00], [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
    [0x70,0x18,0x18,0x0C,0x18,0x18,0x70,0x00], [0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00],
];

// --- Bitmap backend ---

struct BitmapFont<'a> {
    atlas: Texture<'a>,
}

impl<'a> BitmapFont<'a> {
    fn new(tc: &'a TextureCreator<WindowContext>) -> Result<Self, Box<dyn std::error::Error>> {
        let atlas_h = GLYPH_H * NUM_GLYPHS as u32;
        let mut atlas = tc.create_texture_streaming(PixelFormatEnum::ARGB8888, GLYPH_W, atlas_h)?;
        atlas.set_blend_mode(sdl2::render::BlendMode::Blend);
        atlas.with_lock(None, |buf: &mut [u8], pitch: usize| {
            for (gi, glyph) in GLYPHS.iter().enumerate() {
                for (row, &bits) in glyph.iter().enumerate() {
                    let y = gi * GLYPH_H as usize + row;
                    for col in 0..GLYPH_W as usize {
                        let on = (bits >> (7 - col)) & 1 == 1;
                        let off = y * pitch + col * 4;
                        if on {
                            buf[off] = 0xFF;
                            buf[off + 1] = 0xFF;
                            buf[off + 2] = 0xFF;
                            buf[off + 3] = 0xFF;
                        } else {
                            buf[off] = 0x00;
                            buf[off + 1] = 0x00;
                            buf[off + 2] = 0x00;
                            buf[off + 3] = 0x00;
                        }
                    }
                }
            }
        })?;
        Ok(Self { atlas })
    }

    fn draw(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        color: Color,
    ) -> Result<(), String> {
        self.atlas.set_color_mod(color.r, color.g, color.b);
        self.atlas.set_alpha_mod(color.a);
        let gw = (GLYPH_W * scale) as i32;
        for (i, ch) in text.bytes().enumerate() {
            let idx = if (FIRST_CHAR..=LAST_CHAR).contains(&ch) {
                (ch - FIRST_CHAR) as i32
            } else {
                0
            };
            let src = Rect::new(0, idx * GLYPH_H as i32, GLYPH_W, GLYPH_H);
            let dst = Rect::new(x + i as i32 * gw, y, GLYPH_W * scale, GLYPH_H * scale);
            canvas.copy(&self.atlas, src, dst)?;
        }
        Ok(())
    }
}

// --- TTF backend ---

/// Key into the TTF glyph/line cache.
#[derive(Hash, PartialEq, Eq)]
struct CacheKey {
    text: String,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
    scale: u32,
}

struct TtfBackend<'ttf, 'tc> {
    ctx: &'ttf Sdl2TtfContext,
    tc: &'tc TextureCreator<WindowContext>,
    fonts: HashMap<u32, sdl2::ttf::Font<'ttf, 'static>>,
    cache: HashMap<CacheKey, (Texture<'tc>, u32, u32)>,
    font_path: String,
    base_pt: u32,
}

impl<'ttf, 'tc> TtfBackend<'ttf, 'tc> {
    fn ensure_font(&mut self, scale: u32) -> Result<(), String> {
        if self.fonts.contains_key(&scale) {
            return Ok(());
        }
        let pt = (self.base_pt * scale) as u16;
        let font = self
            .ctx
            .load_font(&self.font_path, pt)
            .map_err(|e| e.to_string())?;
        self.fonts.insert(scale, font);
        Ok(())
    }

    fn render(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        color: Color,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        self.ensure_font(scale)?;
        let key = CacheKey {
            text: text.to_string(),
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
            scale,
        };
        if !self.cache.contains_key(&key) {
            let font = &self.fonts[&scale];
            let surf = font
                .render(text)
                .blended(color)
                .map_err(|e| e.to_string())?;
            let w = surf.width();
            let h = surf.height();
            let tex = self
                .tc
                .create_texture_from_surface(&surf)
                .map_err(|e| e.to_string())?;
            self.cache.insert(
                CacheKey {
                    text: text.to_string(),
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: color.a,
                    scale,
                },
                (tex, w, h),
            );
        }
        let (tex, w, h) = self.cache.get(&key).unwrap();
        canvas.copy(tex, None, Rect::new(x, y, *w, *h))?;
        Ok(())
    }

    fn text_width(&mut self, text: &str, scale: u32) -> i32 {
        if text.is_empty() {
            return 0;
        }
        if self.ensure_font(scale).is_err() {
            return fallback_text_width(text, scale);
        }
        let font = &self.fonts[&scale];
        match font.size_of(text) {
            Ok((w, _h)) => w as i32,
            Err(_) => fallback_text_width(text, scale),
        }
    }
}

// --- Public Font wrapper ---

pub struct Font<'ttf, 'tc> {
    bitmap: BitmapFont<'tc>,
    ttf: Option<TtfBackend<'ttf, 'tc>>,
    #[allow(dead_code)]
    overlay_ttf: Option<TtfBackend<'ttf, 'tc>>,
}

impl<'ttf, 'tc> Font<'ttf, 'tc> {
    /// Construct a Font. Tries to load `mk2.ttf` via the given TTF context;
    /// if that fails for any reason (missing file, bad TTF), silently falls
    /// back to the embedded bitmap font.
    pub fn new(
        tc: &'tc TextureCreator<WindowContext>,
        ttf_ctx: Option<&'ttf Sdl2TtfContext>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bitmap = BitmapFont::new(tc)?;
        let ttf = load_ttf_backend(
            tc,
            ttf_ctx,
            &["media/mk2.ttf", "src/media/mk2.ttf", "mk2.ttf"],
            "mk2.ttf",
            15,
        );
        let overlay_candidates = overlay_font_candidates();
        let overlay_refs: Vec<&str> = overlay_candidates.iter().map(String::as_str).collect();
        let overlay_ttf = load_ttf_backend(tc, ttf_ctx, &overlay_refs, "overlay font", 24);
        Ok(Self {
            bitmap,
            ttf,
            overlay_ttf,
        })
    }

    pub fn draw(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        color: Color,
    ) -> Result<(), String> {
        if let Some(ttf) = self.ttf.as_mut() {
            ttf.render(canvas, text, x, y, scale, color)
        } else {
            self.bitmap.draw(canvas, text, x, y, scale, color)
        }
    }

    /// Draw using the embedded 8×8 bitmap font only, ignoring TTF.
    #[allow(dead_code)]
    pub fn draw_bitmap(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        color: Color,
    ) -> Result<(), String> {
        self.bitmap.draw(canvas, text, x, y, scale, color)
    }

    /// Fast monospace estimate (unused currently; reserved for future layout code).
    #[allow(dead_code)]
    pub fn text_width(text: &str, scale: u32) -> i32 {
        fallback_text_width(text, scale)
    }

    /// Exact width using the bitmap glyphs (8px per char · scale).
    #[allow(dead_code)]
    pub fn text_width_bitmap(&self, text: &str, scale: u32) -> i32 {
        fallback_text_width(text, scale)
    }

    /// Exact width in pixels. Uses TTF metrics when available, else bitmap math.
    pub fn text_width_exact(&mut self, text: &str, scale: u32) -> i32 {
        if let Some(ttf) = self.ttf.as_mut() {
            ttf.text_width(text, scale)
        } else {
            fallback_text_width(text, scale)
        }
    }

    /// Draw text using the mk2 scoreboard font. Falls back to bitmap.
    pub fn draw_overlay(
        &mut self,
        canvas: &mut Canvas<Window>,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        color: Color,
    ) -> Result<(), String> {
        if let Some(ttf) = self.overlay_ttf.as_mut() {
            ttf.render(canvas, text, x, y, scale, color)
        } else if let Some(ttf) = self.ttf.as_mut() {
            ttf.render(canvas, text, x, y, scale, color)
        } else {
            self.bitmap.draw(canvas, text, x, y, scale, color)
        }
    }

    /// Text width using the overlay font. Falls back.
    pub fn text_width_overlay(&mut self, text: &str, scale: u32) -> i32 {
        if let Some(ttf) = self.overlay_ttf.as_mut() {
            ttf.text_width(text, scale)
        } else if let Some(ttf) = self.ttf.as_mut() {
            ttf.text_width(text, scale)
        } else {
            fallback_text_width(text, scale)
        }
    }
}

fn load_ttf_backend<'ttf, 'tc>(
    tc: &'tc TextureCreator<WindowContext>,
    ttf_ctx: Option<&'ttf Sdl2TtfContext>,
    candidates: &[&str],
    name: &str,
    base_pt: u32,
) -> Option<TtfBackend<'ttf, 'tc>> {
    let ctx = ttf_ctx?;
    let path = resolve_font(candidates)?;
    match ctx.load_font(&path, base_pt as u16) {
        Ok(font) => {
            let mut fonts = HashMap::new();
            fonts.insert(1, font);
            println!("Loaded {name}: {path} ({}pt base)", base_pt);
            Some(TtfBackend {
                ctx,
                tc,
                fonts,
                cache: HashMap::new(),
                font_path: path.to_string(),
                base_pt,
            })
        }
        Err(e) => {
            println!("Failed to load {path}: {e} ({name} not used)");
            None
        }
    }
}

fn fallback_text_width(text: &str, scale: u32) -> i32 {
    text.len() as i32 * (GLYPH_W * scale) as i32
}

fn resolve_font(candidates: &[&str]) -> Option<String> {
    font_search_paths(candidates)
        .into_iter()
        .find(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
}

fn font_search_paths(candidates: &[&str]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = candidates.iter().map(PathBuf::from).collect();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        for candidate in candidates {
            paths.push(exe_dir.join(candidate));
        }
    }
    paths
}

fn overlay_font_candidates() -> Vec<String> {
    ["media/mk2.ttf", "src/media/mk2.ttf", "mk2.ttf"]
        .into_iter()
        .map(String::from)
        .collect()
}

// --- fp_ui scale-aware font cache ---
//
// Separate from `Font` above: the legacy menu draws at a handful of fixed
// integer `scale` multipliers of a base point size, which suits pixel-art
// bitmap glyphs. fp_ui instead computes a continuous point size every frame
// from `layout::Scale::font_px` (window_px = logical_px * s) and needs a
// cache keyed on that exact pixel size — re-rasterizing on resize rather
// than stretching a texture rendered at a stale size (blurry, and the
// classic tell that a "responsive" UI isn't actually scale-aware).

/// Bundled fp_ui font family/weight pairs (`assets/fonts/`), each shipped
/// under the SIL Open Font License (see the `OFL-*.txt` files alongside the
/// TTFs).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[allow(dead_code)] // most weights land with the screens that use them (steps 2-6)
pub enum FpFont {
    SairaCondensedMedium,
    SairaCondensedSemiBold,
    SairaCondensedBold,
    SairaCondensedExtraBold,
    SairaCondensedBlack,
    /// Plain (non-condensed) Saira — body text/hints/sub-labels, per the
    /// handoff doc's font table (`font-family:'Saira'`, distinct from the
    /// condensed family used for headings/menu labels).
    SairaRegular,
    SairaMedium,
    SairaSemiBold,
    SairaBold,
    ChakraPetchMedium,
    ChakraPetchSemiBold,
    ChakraPetchBold,
}

impl FpFont {
    fn filename(self) -> &'static str {
        match self {
            FpFont::SairaCondensedMedium => "SairaCondensed-Medium.ttf",
            FpFont::SairaCondensedSemiBold => "SairaCondensed-SemiBold.ttf",
            FpFont::SairaCondensedBold => "SairaCondensed-Bold.ttf",
            FpFont::SairaCondensedExtraBold => "SairaCondensed-ExtraBold.ttf",
            FpFont::SairaCondensedBlack => "SairaCondensed-Black.ttf",
            FpFont::SairaRegular => "Saira-Regular.ttf",
            FpFont::SairaMedium => "Saira-Medium.ttf",
            FpFont::SairaSemiBold => "Saira-SemiBold.ttf",
            FpFont::SairaBold => "Saira-Bold.ttf",
            FpFont::ChakraPetchMedium => "ChakraPetch-Medium.ttf",
            FpFont::ChakraPetchSemiBold => "ChakraPetch-SemiBold.ttf",
            FpFont::ChakraPetchBold => "ChakraPetch-Bold.ttf",
        }
    }
}

#[derive(Hash, PartialEq, Eq)]
struct FpCacheKey {
    font: FpFont,
    px: u16,
    italic: bool,
    text: String,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

/// Scale-aware glyph/text cache for the fp_ui screen set. One `sdl2::ttf`
/// font per `(family, pixel size)` actually loaded, and one cached texture
/// per `(family, pixel size, text, color)` — both keyed on the *pixel* size
/// computed by `layout::Scale::font_px`, so a window resize naturally misses
/// the cache and re-rasterizes at the new size instead of stretching.
pub struct FpFontCache<'ttf, 'tc> {
    ctx: &'ttf Sdl2TtfContext,
    tc: &'tc TextureCreator<WindowContext>,
    fonts: HashMap<(FpFont, u16, bool), sdl2::ttf::Font<'ttf, 'static>>,
    cache: HashMap<FpCacheKey, (Texture<'tc>, u32, u32)>,
    /// The `layout::Scale::s` factor as of the last `begin_frame` call. A
    /// screen typically requests a dozen-plus distinct pixel sizes (one per
    /// element), so eviction can't key on a single "keep this px" value —
    /// instead the whole cache is dropped in one shot whenever the window's
    /// scale factor itself changes.
    last_scale_bits: Option<u32>,
    /// The header wordmark, lazily decoded once (not per-scale — it's a
    /// single high-res raster of the real logo, so `draw_logo` just scales
    /// the same texture rather than re-rasterizing like the TTF paths do).
    /// `None` after a failed load attempt so a missing asset doesn't retry
    /// a filesystem read every frame; `draw_logo` falls back to the caller
    /// drawing plain text in that case.
    logo: Option<(Texture<'tc>, u32, u32)>,
    logo_load_attempted: bool,
    /// `(font, px, text)` -> the actual opaque-pixel row span within the
    /// rendered surface, i.e. `(first_opaque_row, visible_height)` — not
    /// keyed on color since a glyph's shape doesn't depend on it. See
    /// `visible_span`'s doc comment for why this exists.
    span_cache: HashMap<(FpFont, u16, String), (u32, u32)>,
}

impl<'ttf, 'tc> FpFontCache<'ttf, 'tc> {
    pub fn new(tc: &'tc TextureCreator<WindowContext>, ctx: &'ttf Sdl2TtfContext) -> Self {
        Self {
            ctx,
            tc,
            fonts: HashMap::new(),
            cache: HashMap::new(),
            last_scale_bits: None,
            logo: None,
            logo_load_attempted: false,
            span_cache: HashMap::new(),
        }
    }

    /// Call once per draw with the current `layout::Scale::s`. Clears every
    /// cached font and texture the first time `s` differs from the previous
    /// call, so a resize re-rasterizes every size at the new scale instead
    /// of stretching textures rendered at a stale one.
    pub fn begin_frame(&mut self, s: f32) {
        let bits = s.to_bits();
        if self.last_scale_bits != Some(bits) {
            self.fonts.clear();
            self.cache.clear();
            self.span_cache.clear();
            self.last_scale_bits = Some(bits);
        }
    }

    fn ensure_font(&mut self, font: FpFont, px: u16, italic: bool) -> Result<(), String> {
        if self.fonts.contains_key(&(font, px, italic)) {
            return Ok(());
        }
        let candidates = [
            format!("assets/fonts/{}", font.filename()),
            format!("src/assets/fonts/{}", font.filename()),
            font.filename().to_string(),
        ];
        let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let path = resolve_font(&candidate_refs)
            .ok_or_else(|| format!("{} not found in assets/fonts/", font.filename()))?;
        let mut loaded = self.ctx.load_font(&path, px).map_err(|e| e.to_string())?;
        // Freetype's synthetic oblique — none of the bundled TTFs ship a real
        // italic weight, and adding one just for a handful of skewed mockup
        // labels (menu row labels, the "II"/"#1" ghost watermarks) isn't
        // worth a second font file per family. Close enough for those
        // decorative/large-scale uses; not used anywhere text legibility at
        // small sizes matters.
        if italic {
            loaded.set_style(sdl2::ttf::FontStyle::ITALIC);
        }
        self.fonts.insert((font, px, italic), loaded);
        Ok(())
    }

    /// Draw `text` at logical-derived pixel size `px`, top-left at `(x, y)`
    /// in window space. Returns the rendered texture's `(w, h)` in pixels so
    /// callers can lay out adjacent elements without a separate measure
    /// pass.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas<Window>,
        font: FpFont,
        px: u16,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    ) -> Result<(u32, u32), String> {
        self.draw_styled(canvas, font, px, text, x, y, color, false)
    }

    /// Same as `draw`, but with Freetype's synthetic oblique slant applied —
    /// matches the mockup's `transform: skewX(-9deg)` closely enough for the
    /// large decorative labels that use it (see `ensure_font`'s doc comment).
    pub fn draw_italic(
        &mut self,
        canvas: &mut Canvas<Window>,
        font: FpFont,
        px: u16,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    ) -> Result<(u32, u32), String> {
        self.draw_styled(canvas, font, px, text, x, y, color, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_styled(
        &mut self,
        canvas: &mut Canvas<Window>,
        font: FpFont,
        px: u16,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
        italic: bool,
    ) -> Result<(u32, u32), String> {
        if text.is_empty() {
            return Ok((0, 0));
        }
        self.ensure_font(font, px, italic)?;
        let key = FpCacheKey {
            font,
            px,
            italic,
            text: text.to_string(),
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        };
        if !self.cache.contains_key(&key) {
            let ttf_font = &self.fonts[&(font, px, italic)];
            let surf = ttf_font
                .render(text)
                .blended(color)
                .map_err(|e| e.to_string())?;
            let w = surf.width();
            let h = surf.height();
            let tex = self
                .tc
                .create_texture_from_surface(&surf)
                .map_err(|e| e.to_string())?;
            self.cache.insert(
                FpCacheKey {
                    font,
                    px,
                    italic,
                    text: text.to_string(),
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: color.a,
                },
                (tex, w, h),
            );
        }
        let (tex, w, h) = self.cache.get(&key).unwrap();
        canvas.copy(tex, None, Rect::new(x, y, *w, *h))?;
        Ok((*w, *h))
    }

    /// Per-character advance widths for `text`, measured as the *delta*
    /// between cumulative substring widths (`size_of(text[..=i])
    /// - size_of(text[..i])`) rather than each glyph's own isolated
    /// `size_of`. Isolated measurement double-counts each glyph's left/right
    /// side bearing, which for a proportional font produces visibly uneven
    /// gaps between specific letter pairs once fixed tracking is added on
    /// top — this is what a shaping engine does internally for correct
    /// advances, and what made `draw_tracked`'s spacing look uneven before.
    fn char_advances(&mut self, font: FpFont, px: u16, text: &str) -> Vec<(char, i32)> {
        let mut out = Vec::new();
        let mut prev_w = 0i32;
        let mut acc = String::new();
        for ch in text.chars() {
            acc.push(ch);
            let w = self.text_size(font, px, &acc).0 as i32;
            out.push((ch, w - prev_w));
            prev_w = w;
        }
        out
    }

    /// Draw `text` with extra fixed spacing (`tracking_px`, window-space)
    /// inserted after every character — SDL_ttf has no letter-spacing knob,
    /// so this renders one glyph at a time and advances by its correct
    /// in-context width (see `char_advances`) plus `tracking_px`, the same
    /// approach a browser uses internally for CSS `letter-spacing`. Returns
    /// the total drawn `(w, h)`. Spaces are measured but not rendered
    /// (`ttf_font.render(" ")` errors on some SDL_ttf builds since the glyph
    /// surface would be fully empty).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_tracked(
        &mut self,
        canvas: &mut Canvas<Window>,
        font: FpFont,
        px: u16,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
        tracking_px: i32,
    ) -> Result<(u32, u32), String> {
        let advances = self.char_advances(font, px, text);
        let mut cursor = x;
        let mut max_h = 0u32;
        let n = advances.len();
        for (i, (ch, advance)) in advances.into_iter().enumerate() {
            if ch != ' ' {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                let (_, h) = self.draw(canvas, font, px, s, cursor, y, color)?;
                max_h = max_h.max(h);
            }
            cursor += advance;
            if i + 1 < n {
                cursor += tracking_px;
            }
        }
        Ok(((cursor - x).max(0) as u32, max_h))
    }

    /// Measure `text` at pixel size `px` without drawing it.
    #[allow(dead_code)] // used starting with the Main Menu step (right-aligned labels)
    pub fn text_size(&mut self, font: FpFont, px: u16, text: &str) -> (u32, u32) {
        if text.is_empty() {
            return (0, 0);
        }
        if self.ensure_font(font, px, false).is_err() {
            return (0, 0);
        }
        self.fonts[&(font, px, false)].size_of(text).unwrap_or((0, 0))
    }

    /// Measure `text` as `draw_tracked` would render it, without drawing —
    /// so callers that right-align or center tracked text (e.g. the Main
    /// Menu's "MORTAL KOMBAT" caption) can compute the origin first. The
    /// natural (untracked) width already accounts for kerning across the
    /// whole string, so this just adds the extra tracking gaps on top
    /// rather than re-deriving per-character advances.
    pub fn text_size_tracked(&mut self, font: FpFont, px: u16, text: &str, tracking_px: i32) -> (u32, u32) {
        let (w, h) = self.text_size(font, px, text);
        let n = text.chars().count() as i32;
        let tracked_w = w as i32 + tracking_px * (n - 1).max(0);
        (tracked_w.max(0) as u32, h)
    }

    /// The *visually occupied* row span within `text`'s rendered surface:
    /// `(top_inset, visible_height)`, where `top_inset` is how many pixels
    /// of transparent padding sit above the first opaque pixel. `size_of`/
    /// `text_size`'s height is the font's full ascent+descent line height —
    /// for a short all-caps label that's noticeably taller than the actual
    /// glyphs (it reserves room for accents and descenders *other* glyphs in
    /// the font might need, even if this string doesn't use them). Stacking
    /// two lines by that measurement (as an earlier pass in `main_menu.rs`
    /// did) either overlaps them or leaves the pair looking bottom-heavy
    /// within its row, depending on which way the centering math leaned.
    /// This scans the real alpha channel so callers can align by the
    /// glyph's own visual box instead. Cached per `(font, px, text)` — not
    /// color, since a glyph's shape doesn't depend on it.
    pub fn visible_span(&mut self, font: FpFont, px: u16, text: &str) -> (u32, u32) {
        if text.is_empty() {
            return (0, 0);
        }
        let key = (font, px, text.to_string());
        if let Some(&v) = self.span_cache.get(&key) {
            return v;
        }
        if self.ensure_font(font, px, false).is_err() {
            return (0, 0);
        }
        let span = {
            let ttf_font = &self.fonts[&(font, px, false)];
            match ttf_font.render(text).blended(Color::RGB(255, 255, 255)) {
                Ok(surf) => opaque_row_span(&surf),
                Err(_) => (0, 0),
            }
        };
        self.span_cache.insert(key, span);
        span
    }

    fn ensure_logo(&mut self) {
        if self.logo.is_some() || self.logo_load_attempted {
            return;
        }
        self.logo_load_attempted = true;
        let candidates = ["assets/logo/wordmark.png", "src/assets/logo/wordmark.png"];
        let Some(path) = resolve_font(&candidates) else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Some((rgba, w, h)) = crate::png::decode_png(&bytes) else {
            return;
        };
        let Ok(mut tex) = self
            .tc
            .create_texture_static(PixelFormatEnum::RGBA32, w, h)
            .map_err(|e| e.to_string())
        else {
            return;
        };
        tex.set_blend_mode(sdl2::render::BlendMode::Blend);
        if tex.update(None, &rgba, w as usize * 4).is_ok() {
            self.logo = Some((tex, w, h));
        }
    }

    /// Draw the real FREEPLAY wordmark logo (rasterized from
    /// `freeplay-frontend/assets/freeplay-wordmark.svg`) at window-space
    /// `(x, y)`, scaled to `target_h` pixels tall preserving aspect ratio.
    /// Returns the drawn width in pixels on success so callers can lay out
    /// adjacent elements (e.g. the build tag) after it; `None` if the asset
    /// isn't available, so the caller can fall back to text.
    pub fn draw_logo(&mut self, canvas: &mut Canvas<Window>, x: i32, y: i32, target_h: u32) -> Option<u32> {
        self.ensure_logo();
        let (tex, w, h) = self.logo.as_ref()?;
        let draw_w = (*w as u64 * target_h as u64 / *h as u64) as u32;
        canvas.copy(tex, None, Rect::new(x, y, draw_w, target_h)).ok()?;
        Some(draw_w)
    }
}

/// Scans a rendered text surface's alpha channel for the first and last rows
/// containing any opaque pixel, returning `(first_opaque_row, visible_height)`
/// — `(0, surface_height)` if the surface is fully transparent (shouldn't
/// happen for non-empty text, but a safe fallback rather than a panic if a
/// font's glyph coverage is unexpectedly sparse). Reads raw pixel bytes as a
/// native-endian `u32` and decodes via `Color::from_u32`, which is correct
/// regardless of the surface's specific 32-bit pixel format (`.blended()`
/// always produces one) — this only runs on little-endian targets (Windows/
/// Linux/macOS on x86_64/ARM64, the only platforms this app ships for).
fn opaque_row_span(surf: &sdl2::surface::Surface) -> (u32, u32) {
    let w = surf.width() as usize;
    let h = surf.height();
    let pitch = surf.pitch() as usize;
    let pf = surf.pixel_format();
    let mut first = h;
    let mut last = 0u32;
    surf.with_lock(|bytes| {
        for y in 0..h {
            let row_off = y as usize * pitch;
            for x in 0..w {
                let off = row_off + x * 4;
                if off + 4 > bytes.len() {
                    break;
                }
                let px_val = u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
                if Color::from_u32(&pf, px_val).a != 0 {
                    first = first.min(y);
                    last = y + 1;
                    break;
                }
            }
        }
    });
    if first >= last {
        (0, h)
    } else {
        (first, last - first)
    }
}
