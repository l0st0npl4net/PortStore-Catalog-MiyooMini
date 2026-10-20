//! Drawing in a frame of 0x00RRGGBB words: rectangles and text (DejaVu Sans, by
//! fontdue, glyphs kept once drawn).
use std::collections::HashMap;

static FONT: &[u8] = include_bytes!("../../../assets/DejaVuSans.ttf");

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    font: fontdue::Font,
    glyphs: HashMap<(char, u16), (fontdue::Metrics, Vec<u8>)>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default())
            .expect("the bundled font");
        Canvas {
            w,
            h,
            px: vec![0; w * h],
            font,
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
        let font = &self.font;
        self.glyphs
            .entry((c, px))
            .or_insert_with(|| font.rasterize(c, px as f32))
    }

    pub fn ascent(&self, px: u16) -> i32 {
        self.font
            .horizontal_line_metrics(px as f32)
            .map(|m| m.ascent.round() as i32)
            .unwrap_or(px as i32)
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
        if self.width(px, text) <= max {
            return self.text(x, y, px, color, text);
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
        self.text(x, y, px, color, &cut)
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

    /// The frame as a PNG (for looking at screens off the device).
    pub fn save_png(&self, path: &std::path::Path) -> std::io::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(std::io::Error::other)?;
        let bytes: Vec<u8> = self
            .px
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect();
        writer.write_image_data(&bytes).map_err(std::io::Error::other)
    }
}

/// `over` on `under`, `a` of 255 of it.
pub fn mix(under: u32, over: u32, a: u32) -> u32 {
    let ch = |shift: u32| {
        let (u, o) = ((under >> shift) & 0xff, (over >> shift) & 0xff);
        ((o * a + u * (255 - a) + 127) / 255) << shift
    };
    ch(16) | ch(8) | ch(0)
}
