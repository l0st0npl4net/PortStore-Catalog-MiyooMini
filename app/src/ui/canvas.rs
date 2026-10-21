//! Drawing in a frame of 0x00RRGGBB words: rectangles, lines, pictures and text (by
//! fontdue, glyphs kept once drawn). Text is VT323, a terminal's font, or DejaVu Sans
//! for the classic look; DejaVu also draws what VT323 has no glyph for.
use std::collections::HashMap;

static TERMINAL: &[u8] = include_bytes!("../../../assets/VT323-Regular.ttf");
static PLAIN: &[u8] = include_bytes!("../../../assets/DejaVuSans.ttf");

/// DejaVu is drawn this much smaller than VT323 at the same size: their capitals are
/// then of a height.
const PLAIN_SCALE: f32 = 0.8;

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    /// VT323 first (the terminal look), else DejaVu only.
    pub mono: bool,
    terminal: fontdue::Font,
    plain: fontdue::Font,
    glyphs: HashMap<(char, u16, bool), (fontdue::Metrics, Vec<u8>)>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        let font = |bytes| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).expect("a bundled font");
        Canvas {
            w,
            h,
            px: vec![0; w * h],
            mono: true,
            terminal: font(TERMINAL),
            plain: font(PLAIN),
            glyphs: HashMap::new(),
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.px.fill(color);
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
        let x1 = ((x + w).max(0) as usize).min(self.w);
        let y1 = ((y + h).max(0) as usize).min(self.h);
        for row in y0..y1 {
            if x0 < x1 {
                self.px[row * self.w + x0..row * self.w + x1].fill(color);
            }
        }
    }

    /// `color` over what is there, `a` of 255 of it.
    pub fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32, a: u32) {
        let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
        let x1 = ((x + w).max(0) as usize).min(self.w);
        let y1 = ((y + h).max(0) as usize).min(self.h);
        for row in y0..y1 {
            for p in &mut self.px[row * self.w + x0.min(x1)..row * self.w + x1] {
                *p = mix(*p, color, a);
            }
        }
    }

    /// A rectangle's outline, `t` pixels thick.
    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, color: u32) {
        self.fill(x, y, w, t, color);
        self.fill(x, y + h - t, w, t, color);
        self.fill(x, y, t, h, color);
        self.fill(x + w - t, y, t, h, color);
    }

    /// A line across of dashes `on` long, `off` apart.
    pub fn dashes(&mut self, x: i32, y: i32, w: i32, on: i32, off: i32, color: u32) {
        let mut at = 0;
        while at < w {
            self.fill(x + at, y, on.min(w - at), 1, color);
            at += on + off;
        }
    }

    /// A line down of dashes.
    pub fn dashes_down(&mut self, x: i32, y: i32, h: i32, on: i32, off: i32, color: u32) {
        let mut at = 0;
        while at < h {
            self.fill(x, y + at, 1, on.min(h - at), color);
            at += on + off;
        }
    }

    /// A triangle `s` wide, pointing up (or down), its top left at (x, y).
    pub fn arrow(&mut self, x: i32, y: i32, s: i32, up: bool, color: u32) {
        let rows = (s + 1) / 2;
        for r in 0..rows {
            let half = if up { r } else { rows - 1 - r };
            self.fill(x + s / 2 - half, y + r, 2 * half + 1, 1, color);
        }
    }

    /// A rectangle with its corners cut by `r` pixels' worth of a circle.
    pub fn rounded(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        let r = r.min(w / 2).min(h / 2);
        for dy in 0..h {
            let edge = if dy < r {
                r - dy
            } else if dy >= h - r {
                dy - (h - r) + 1
            } else {
                0
            };
            // The row's middle, from the corner circle's centre.
            let inset = match edge {
                0 => 0,
                e => {
                    let v = e as f32 - 0.5;
                    (r as f32 - ((r * r) as f32 - v * v).max(0.0).sqrt()).round() as i32
                }
            };
            self.fill(x + inset, y + dy, w - 2 * inset, 1, color);
        }
    }

    fn glyph(&mut self, c: char, px: u16) -> &(fontdue::Metrics, Vec<u8>) {
        let mono = self.mono && self.terminal.lookup_glyph_index(c) != 0;
        let (terminal, plain) = (&self.terminal, &self.plain);
        self.glyphs.entry((c, px, mono)).or_insert_with(|| match mono {
            true => terminal.rasterize(c, px as f32),
            false => plain.rasterize(c, px as f32 * PLAIN_SCALE),
        })
    }

    /// The baseline below the top of a line of text: the same for both fonts.
    pub fn ascent(&self, px: u16) -> i32 {
        (px as f32 * 0.8).round() as i32
    }

    pub fn width(&mut self, px: u16, text: &str) -> i32 {
        text.chars()
            .map(|c| self.glyph(c, px).0.advance_width)
            .sum::<f32>()
            .round() as i32
    }

    /// Text with its top at `y`; the width it took.
    pub fn text(&mut self, x: i32, y: i32, px: u16, color: u32, text: &str) -> i32 {
        let base = y + self.ascent(px);
        let mut pen = x as f32;
        for c in text.chars() {
            let (m, bitmap) = self.glyph(c, px).clone();
            let gx = pen.round() as i32 + m.xmin;
            let gy = base - m.height as i32 - m.ymin;
            self.blit(gx, gy, m.width, m.height, &bitmap, color);
            pen += m.advance_width;
        }
        (pen - x as f32).round() as i32
    }

    /// Text cut to `max` pixels wide, with an ellipsis when cut.
    pub fn text_fit(&mut self, x: i32, y: i32, px: u16, color: u32, text: &str, max: i32) -> i32 {
        let cut = self.fit(px, text, max);
        self.text(x, y, px, color, &cut)
    }

    /// `text`, cut to `max` pixels wide with an ellipsis when it is wider.
    pub fn fit(&mut self, px: u16, text: &str, max: i32) -> String {
        if self.width(px, text) <= max {
            return text.to_string();
        }
        let dots = self.width(px, "…");
        let mut cut = String::new();
        let mut w = 0;
        for c in text.chars() {
            let cw = self.width(px, &c.to_string());
            if w + cw + dots > max {
                break;
            }
            cut.push(c);
            w += cw;
        }
        cut = cut.trim_end().to_string();
        cut.push('…');
        cut
    }

    pub fn text_right(&mut self, right: i32, y: i32, px: u16, color: u32, text: &str) -> i32 {
        let w = self.width(px, text);
        self.text(right - w, y, px, color, text)
    }

    /// `text` broken into lines of at most `max` pixels, at spaces where it can be.
    pub fn wrap(&mut self, px: u16, text: &str, max: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let joined = format!("{line} {word}");
                if line.is_empty() {
                    line = word.to_string();
                } else if self.width(px, &joined) <= max {
                    line = joined;
                } else {
                    lines.push(std::mem::replace(&mut line, word.to_string()));
                }
                // A word too long for a line of its own is broken where it must be.
                while self.width(px, &line) > max {
                    let mut head = String::new();
                    for c in line.chars() {
                        head.push(c);
                        if head.chars().count() > 1 && self.width(px, &head) > max {
                            head.pop();
                            break;
                        }
                    }
                    line = line[head.len()..].to_string();
                    lines.push(head);
                }
            }
            lines.push(line);
        }
        lines
    }

    fn blit(&mut self, x: i32, y: i32, w: usize, h: usize, cover: &[u8], color: u32) {
        for gy in 0..h {
            let py = y + gy as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for gx in 0..w {
                let a = cover[gy * w + gx] as u32;
                let px = x + gx as i32;
                if a == 0 || px < 0 || px >= self.w as i32 {
                    continue;
                }
                let d = &mut self.px[py as usize * self.w + px as usize];
                *d = mix(*d, color, a);
            }
        }
    }

    /// A picture with its top left at (x, y), over what is there by its alpha.
    pub fn image(&mut self, x: i32, y: i32, image: &super::pictures::Image) {
        for iy in 0..image.h {
            let py = y + iy as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for ix in 0..image.w {
                let px = x + ix as i32;
                let p = image.px[iy * image.w + ix];
                let a = p >> 24;
                if a == 0 || px < 0 || px >= self.w as i32 {
                    continue;
                }
                let d = &mut self.px[py as usize * self.w + px as usize];
                *d = if a == 255 { p & 0xff_ffff } else { mix(*d, p, a) };
            }
        }
    }

    /// A picture as a terminal would show it: in blocks of `cell` pixels, each of the
    /// shades `ramp` (dark to light) its brightness comes to, dithered (Bayer 4x4).
    pub fn image_dithered(&mut self, x: i32, y: i32, image: &super::pictures::Image, cell: usize, ramp: &[u32]) {
        const BAYER: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
        let cell = cell.max(1);
        let steps = ramp.len().saturating_sub(1).max(1) as u32;
        for cy in (0..image.h).step_by(cell) {
            for cx in (0..image.w).step_by(cell) {
                // The block's middle stands for it.
                let (sx, sy) = ((cx + cell / 2).min(image.w - 1), (cy + cell / 2).min(image.h - 1));
                let p = image.px[sy * image.w + sx];
                let a = p >> 24;
                if a < 128 {
                    continue;
                }
                let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
                let lum = (r * 54 + g * 183 + b * 19) >> 8;
                // Brightness in sixteenths of a step, against the block's threshold.
                let at = lum * steps * 16 / 255;
                let threshold = BAYER[(cy / cell % 4) * 4 + cx / cell % 4];
                let level = ((at / 16) + u32::from(at % 16 > threshold)).min(steps) as usize;
                let w = cell.min(image.w - cx) as i32;
                let h = cell.min(image.h - cy) as i32;
                self.fill(x + cx as i32, y + cy as i32, w, h, ramp[level]);
            }
        }
    }

    /// The frame as a PNG (for looking at screens off the device).
    pub fn save_png(&self, path: &std::path::Path) -> std::io::Result<()> {
        save_png(path, self.w, self.h, &self.px)
    }
}

/// Pixels of 0x00RRGGBB as a PNG.
pub fn save_png(path: &std::path::Path, w: usize, h: usize, px: &[u32]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    let bytes: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    writer.write_image_data(&bytes).map_err(std::io::Error::other)
}

/// `over` on `under`, `a` of 255 of it.
pub fn mix(under: u32, over: u32, a: u32) -> u32 {
    let ch = |shift: u32| {
        let (u, o) = ((under >> shift) & 0xff, (over >> shift) & 0xff);
        ((o * a + u * (255 - a) + 127) / 255) << shift
    };
    ch(16) | ch(8) | ch(0)
}
