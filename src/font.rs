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
    fonts: HashMap<(FpFont, u16), sdl2::ttf::Font<'ttf, 'static>>,
    cache: HashMap<FpCacheKey, (Texture<'tc>, u32, u32)>,
    /// The `layout::Scale::s` factor as of the last `begin_frame` call. A
    /// screen typically requests a dozen-plus distinct pixel sizes (one per
    /// element), so eviction can't key on a single "keep this px" value —
    /// instead the whole cache is dropped in one shot whenever the window's
    /// scale factor itself changes.
    last_scale_bits: Option<u32>,
}

impl<'ttf, 'tc> FpFontCache<'ttf, 'tc> {
    pub fn new(tc: &'tc TextureCreator<WindowContext>, ctx: &'ttf Sdl2TtfContext) -> Self {
        Self {
            ctx,
            tc,
            fonts: HashMap::new(),
            cache: HashMap::new(),
            last_scale_bits: None,
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
            self.last_scale_bits = Some(bits);
        }
    }

    fn ensure_font(&mut self, font: FpFont, px: u16) -> Result<(), String> {
        if self.fonts.contains_key(&(font, px)) {
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
        let loaded = self.ctx.load_font(&path, px).map_err(|e| e.to_string())?;
        self.fonts.insert((font, px), loaded);
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
        if text.is_empty() {
            return Ok((0, 0));
        }
        self.ensure_font(font, px)?;
        let key = FpCacheKey {
            font,
            px,
            text: text.to_string(),
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        };
        if !self.cache.contains_key(&key) {
            let ttf_font = &self.fonts[&(font, px)];
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

    /// Measure `text` at pixel size `px` without drawing it.
    #[allow(dead_code)] // used starting with the Main Menu step (right-aligned labels)
    pub fn text_size(&mut self, font: FpFont, px: u16, text: &str) -> (u32, u32) {
        if text.is_empty() {
            return (0, 0);
        }
        if self.ensure_font(font, px).is_err() {
            return (0, 0);
        }
        self.fonts[&(font, px)].size_of(text).unwrap_or((0, 0))
    }

}
