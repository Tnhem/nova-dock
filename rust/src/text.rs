//! Pixel-font text with per-character fallback; all runs share one baseline.

use crate::canvas::Canvas;
use ab_glyph::{point, Font, FontVec, GlyphId, PxScale, ScaleFont};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

/// name -> (file in fonts/, pixel size where it lands on the pixel grid, label).
pub const FONTS: &[(&str, &str, f32, &str)] = &[
    ("pixel-operator", "PixelOperator.ttf", 16.0, "Pixel Operator"),
    ("minecraft", "Monocraft.ttf", 9.0, "Minecraft (Monocraft)"),
    ("minecraft-large", "Monocraft.ttf", 18.0, "Minecraft large"),
    ("press-start", "PressStart2P-Regular.ttf", 8.0, "Press Start 2P (arcade)"),
    ("silkscreen", "Silkscreen-Regular.ttf", 8.0, "Silkscreen (tiny caps)"),
    ("unifont", "Unifont.otf", 16.0, "Unifont (also Chinese)"),
];

/// One font at one pixel size.
pub struct Face {
    font: FontVec,
    scale: PxScale,
    pub above: i32,
    pub below: i32,
}

struct GlyphBmp {
    bmp: Canvas,
    x: i32, // offset from pen position
    y: i32, // offset from baseline
    adv: f32,
}

impl Face {
    /// `size` is the em size in pixels, like PIL's `truetype(path, size)`.
    pub fn load(path: &Path, size: f32) -> Option<Face> {
        let data = std::fs::read(path).ok()?;
        let font = FontVec::try_from_vec_and_index(data, 0).ok()?;
        let upem = font.units_per_em().unwrap_or(1000.0);
        let scale = PxScale::from(size * font.height_unscaled() / upem);
        let mut f = Face { font, scale, above: 0, below: 0 };
        f.above = -f.extent("H\u{5468}\u{AC00}").0;
        f.below = f.extent("gjpqy").1.max(0);
        Some(f)
    }

    pub fn has(&self, ch: char) -> bool {
        self.font.glyph_id(ch) != GlyphId(0)
    }

    pub fn advance(&self, ch: char) -> f32 {
        self.font.as_scaled(self.scale).h_advance(self.font.glyph_id(ch))
    }

    /// (top, bottom) of the glyphs in s relative to the baseline, over the glyphs this font has.
    fn extent(&self, s: &str) -> (i32, i32) {
        let (mut top, mut bot) = (0, 0);
        for ch in s.chars().filter(|c| self.has(*c)) {
            let g = self.font.glyph_id(ch).with_scale_and_position(self.scale, point(0.0, 0.0));
            if let Some(og) = self.font.outline_glyph(g) {
                let b = og.px_bounds();
                top = top.min(b.min.y.round() as i32);
                bot = bot.max(b.max.y.round() as i32);
            }
        }
        (top, bot)
    }

    fn rasterize(&self, ch: char) -> GlyphBmp {
        let id = self.font.glyph_id(ch);
        let adv = self.font.as_scaled(self.scale).h_advance(id);
        let g = id.with_scale_and_position(self.scale, point(0.0, 0.0));
        match self.font.outline_glyph(g) {
            Some(og) => {
                let b = og.px_bounds();
                let mut bmp = Canvas::new(b.width().ceil() as i32, b.height().ceil() as i32);
                og.draw(|x, y, c| {
                    if c >= 0.5 {
                        bmp.on(x as i32, y as i32);
                    }
                });
                GlyphBmp { bmp, x: b.min.x.round() as i32, y: b.min.y.round() as i32, adv }
            }
            None => GlyphBmp { bmp: Canvas::new(0, 0), x: 0, y: 0, adv },
        }
    }
}

/// A fallback face, loaded from disk the first time a character needs it.
struct Slot {
    path: std::path::PathBuf,
    size: f32,
    face: std::cell::OnceCell<Option<Face>>,
}

impl Slot {
    fn get(&self) -> Option<&Face> {
        self.face.get_or_init(|| Face::load(&self.path, self.size)).as_ref()
    }
}

pub struct Text {
    primary: Option<Face>,
    fallback: Vec<Slot>,
    pub cap: i32,
    pub desc: i32,
    pick: RefCell<HashMap<char, usize>>, // 0 = primary, i = fallback[i - 1]
    glyphs: RefCell<HashMap<(usize, char), std::rc::Rc<GlyphBmp>>>,
}

impl Text {
    /// Primary pixel font plus fallbacks for characters it lacks (CJK, Hangul, symbols).
    pub fn new(fonts_dir: &Path, name: &str) -> Text {
        let (_, file, size, _) = FONTS.iter().find(|f| f.0 == name).copied().unwrap_or(FONTS[0]);
        let mut primary = Face::load(&fonts_dir.join(file), size);
        let win = Path::new("C:/Windows/Fonts");
        // Unifont first: ab_glyph only draws outlines, and SimSun's outlines break up at 12 px
        // (FreeType would use SimSun's embedded bitmaps). Unifont also covers Hangul.
        let fallback = [
            (fonts_dir.join("Unifont.otf"), 16.0),
            (win.join("malgun.ttf"), 12.0),
            (win.join("seguisym.ttf"), 12.0),
            (win.join("segoeui.ttf"), 12.0),
        ]
        .into_iter()
        .map(|(path, size)| Slot { path, size, face: std::cell::OnceCell::new() })
        .collect();
        let (cap, desc) = match primary.as_mut() {
            Some(f) => {
                f.above = -f.extent("H").0;
                (f.above, f.below)
            }
            None => (8, 2),
        };
        Text { primary, fallback, cap, desc, pick: RefCell::new(HashMap::new()), glyphs: RefCell::new(HashMap::new()) }
    }

    fn face(&self, i: usize) -> Option<&Face> {
        if i == 0 { self.primary.as_ref() } else { self.fallback[i - 1].get() }
    }

    fn face_for(&self, ch: char) -> usize {
        if let Some(i) = self.pick.borrow().get(&ch) {
            return *i;
        }
        let i = if ch.is_whitespace() || self.primary.as_ref().is_some_and(|f| f.has(ch)) {
            0
        } else {
            (0..self.fallback.len()).find(|i| self.fallback[*i].get().is_some_and(|f| f.has(ch))).map_or(0, |i| i + 1)
        };
        self.pick.borrow_mut().insert(ch, i);
        i
    }

    fn glyph(&self, face: usize, ch: char) -> Option<std::rc::Rc<GlyphBmp>> {
        if let Some(g) = self.glyphs.borrow().get(&(face, ch)) {
            return Some(g.clone());
        }
        let g = std::rc::Rc::new(self.face(face)?.rasterize(ch));
        self.glyphs.borrow_mut().insert((face, ch), g.clone());
        Some(g)
    }

    pub fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.face(self.face_for(c)).map_or(0.0, |f| f.advance(c))).sum()
    }

    pub fn above(&self, s: &str) -> i32 {
        s.chars().map(|c| self.face(self.face_for(c)).map_or(0, |f| f.above)).fold(self.cap, i32::max)
    }

    pub fn height(&self, s: &str) -> i32 {
        let below = s.chars().map(|c| self.face(self.face_for(c)).map_or(0, |f| f.below)).fold(self.desc, i32::max);
        self.above(s) + below
    }

    pub fn draw_base(&self, c: &mut Canvas, x: f32, baseline: i32, s: &str) -> f32 {
        let mut x = x;
        for ch in s.chars() {
            if let Some(g) = self.glyph(self.face_for(ch), ch) {
                c.overlay(&g.bmp, x.round() as i32 + g.x, baseline + g.y);
                x += g.adv;
            }
        }
        x
    }

    /// Draw s with the top of its tallest glyph at `top`.
    pub fn draw(&self, c: &mut Canvas, x: f32, top: i32, s: &str) -> f32 {
        self.draw_base(c, x, top + self.above(s), s)
    }

    /// s as a tightly cropped bitmap (for scaling up).
    pub fn render(&self, s: &str) -> Canvas {
        let mut c = Canvas::new(self.width(s) as i32 + 4, self.height(s) + 2);
        self.draw(&mut c, 0.0, 0, s);
        match c.bbox() {
            Some((x0, y0, x1, y1)) => c.crop(x0, y0, x1, y1),
            None => Canvas::new(1, 1),
        }
    }
}

/// A single big face (the classic clock), drawn with its glyph tops at `top`.
pub struct BigFont {
    face: Option<Face>,
}

impl BigFont {
    pub fn bahnschrift() -> BigFont {
        let face = (|| {
            let data = std::fs::read("C:/Windows/Fonts/bahnschrift.ttf").ok()?;
            let mut font = FontVec::try_from_vec_and_index(data, 0).ok()?;
            use ab_glyph::VariableFont;
            font.set_variation(b"wght", 700.0);
            let upem = font.units_per_em().unwrap_or(1000.0);
            let scale = PxScale::from(36.0 * font.height_unscaled() / upem);
            Some(Face { font, scale, above: 0, below: 0 })
        })();
        BigFont { face }
    }

    pub fn render(&self, s: &str) -> Canvas {
        let Some(f) = &self.face else { return Canvas::new(1, 1) };
        let width: f32 = s.chars().map(|c| f.advance(c)).sum();
        let mut c = Canvas::new(width as i32 + 4, 60);
        let mut x = 0.0;
        for ch in s.chars() {
            let g = f.rasterize(ch);
            c.overlay(&g.bmp, x as i32 + g.x, 45 + g.y);
            x += g.adv;
        }
        match c.bbox() {
            Some((x0, y0, x1, y1)) => c.crop(x0, y0, x1, y1),
            None => Canvas::new(1, 1),
        }
    }
}
