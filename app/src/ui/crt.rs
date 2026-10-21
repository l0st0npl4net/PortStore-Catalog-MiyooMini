//! A picture tube's light over the drawn frame: scan lines, the glow around what is
//! lit, a darker rim and rounded corners, the phosphor's afterglow when the picture
//! changes, and the tube's moments: switching on and off, losing and finding its sync,
//! a degauss. All of it in whole frames of 0x00RRGGBB, drawn only while something
//! moves.
use std::time::{Duration, Instant};

use super::theme::Effects;

/// A moment of the tube's, played over the frames that follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fx {
    /// A line of light opening into the picture, too bright at first.
    PowerOn,
    /// The picture folding into a line, the line into a dot.
    PowerOff,
    /// Bands of the picture torn sideways, the light flickering: another tab.
    Tear,
    /// The picture rolling down into place, its sync bar passing: another page.
    Roll,
    /// The picture wobbling as the tube is degaussed: another theme.
    Degauss,
    /// A short flicker: a window opening or closing, a filter.
    Blip,
    /// A small tear, now and then, while the screen is used.
    Glitch,
}

impl Fx {
    fn length(self) -> Duration {
        Duration::from_millis(match self {
            Fx::PowerOn => 900,
            Fx::PowerOff => 520,
            Fx::Tear => 240,
            Fx::Roll => 320,
            Fx::Degauss => 760,
            Fx::Blip => 120,
            Fx::Glitch => 130,
        })
    }
}

/// How long the screen keeps moving on its own (the band, the cursor) after a press.
const AWAKE: Duration = Duration::from_secs(40);
/// The phosphor's afterglow: what is left of a pixel's light a frame later, of 256.
const AFTERGLOW: u32 = 118;
/// The glow's strength, of 256.
const GLOW: u32 = 120;
/// How often the glow is made at most.
const GLOW_EVERY: Duration = Duration::from_millis(90);

/// What an output row shows.
struct Row {
    /// The row of the picture, or none (a line of light, or dark).
    src: Option<usize>,
    /// Torn sideways by so many pixels.
    dx: i32,
    /// Brightness, of 256.
    gain: u32,
    /// Washed towards white, of 255.
    white: u32,
    /// Without a picture row: the part lit as a line.
    line: (i32, i32),
}

pub struct Crt {
    w: usize,
    h: usize,
    pub effects: Effects,
    /// The colour of the tube's line of light.
    pub beam: u32,
    /// Scan lines, rim and corners together, and rim and corners alone (of 256).
    scan: Vec<u16>,
    rim: Vec<u16>,
    persist: Vec<u32>,
    settled: bool,
    small: Vec<[u16; 3]>,
    blur: Vec<[u16; 3]>,
    /// The glow at full width (a row per row of blocks), and at full size.
    wide: Vec<u32>,
    halo: Vec<u32>,
    /// When the glow was last made, and whether the picture has changed since.
    glow_at: Instant,
    glow_due: bool,
    lit: Vec<u32>,
    out: Vec<u32>,
    fx: Option<(Fx, Instant)>,
    seed: u32,
    awake_until: Option<Instant>,
    next_glitch: Option<Instant>,
    epoch: Instant,
}

impl Crt {
    pub fn new(w: usize, h: usize, effects: Effects) -> Crt {
        let (sw, sh) = (w / 4, h / 4);
        let mut crt = Crt {
            w,
            h,
            effects,
            beam: 0xffffff,
            scan: vec![256; w * h],
            rim: vec![256; w * h],
            persist: vec![0; w * h],
            settled: false,
            small: vec![[0; 3]; sw * sh],
            blur: vec![[0; 3]; sw * sh],
            wide: vec![0; sh * w],
            halo: vec![0; w * h],
            glow_at: Instant::now() - Duration::from_secs(1),
            glow_due: false,
            lit: vec![0; w * h],
            out: vec![0; w * h],
            fx: None,
            seed: 0x9e37_79b9,
            awake_until: None,
            next_glitch: None,
            epoch: Instant::now(),
        };
        crt.masks();
        crt
    }

    /// Scan lines (every other row darker), a rim darker towards the corners, and the
    /// corners rounded as a tube's are.
    fn masks(&mut self) {
        let (w, h) = (self.w as f32, self.h as f32);
        let r = 22.0f32.min(w / 2.0).min(h / 2.0);
        for y in 0..self.h {
            for x in 0..self.w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let (nx, ny) = (fx / w * 2.0 - 1.0, fy / h * 2.0 - 1.0);
                let d = (nx * nx * 0.8 + ny * ny).min(2.0);
                let mut v = 1.0 - 0.30 * d * d.sqrt() / 1.6;
                // Inside a corner's circle: the distance to it, a pixel soft.
                let (cx, cy) = (fx.clamp(r, w - r), fy.clamp(r, h - r));
                let out = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() - r;
                v *= (0.5 - out).clamp(0.0, 1.0);
                let rim = (v.max(0.0) * 256.0) as u16;
                self.rim[y * self.w + x] = rim;
                self.scan[y * self.w + x] = if y % 2 == 1 { (rim as u32 * 196 / 256) as u16 } else { rim };
            }
        }
    }

    fn rand(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    /// -1.0 to 1.0.
    fn signed(&mut self) -> f32 {
        (self.rand() % 2001) as f32 / 1000.0 - 1.0
    }

    pub fn start(&mut self, fx: Fx, now: Instant) {
        if self.effects == Effects::Off {
            return;
        }
        // A tear does not cut the tube's switching short.
        if let Some((Fx::PowerOn | Fx::PowerOff, at)) = self.fx {
            if fx != Fx::PowerOff && now < at + Fx::PowerOn.length() {
                return;
            }
        }
        self.fx = Some((fx, now));
    }

    /// A press: the screen is in use, and moves on its own a while.
    pub fn wake(&mut self, now: Instant) {
        self.awake_until = Some(now + AWAKE);
        if self.next_glitch.is_none() {
            self.next_glitch = Some(now + Duration::from_secs(8));
        }
    }

    fn awake(&self, now: Instant) -> bool {
        self.effects == Effects::Full && self.awake_until.is_some_and(|t| now < t)
    }

    /// Whether frames are still to come without anything new drawn, and how soon.
    pub fn next_frame(&self, now: Instant) -> Option<Duration> {
        if self.effects == Effects::Off {
            return None;
        }
        if self.fx.is_some_and(|(fx, at)| now < at + fx.length()) || !self.settled {
            return Some(Duration::from_millis(33));
        }
        if self.glow_due {
            return Some((self.glow_at + GLOW_EVERY).saturating_duration_since(now));
        }
        if self.awake(now) {
            return Some(Duration::from_millis(50));
        }
        // One more after the band has gone, so it is not left on the screen.
        self.awake_until.map(|_| Duration::from_millis(50))
    }

    pub fn playing(&self, now: Instant) -> bool {
        self.fx.is_some_and(|(fx, at)| now < at + fx.length())
    }

    /// The frame to show for `content` (drawn anew when `changed`).
    pub fn frame<'a>(&'a mut self, content: &'a [u32], changed: bool, now: Instant) -> &'a [u32] {
        if self.effects == Effects::Off || content.len() != self.w * self.h {
            return content;
        }
        // The glow is made at most so often: text typed frame by frame does not
        // make it each frame; the last frame's is made when its time comes.
        let glow_now = (changed || self.glow_due) && now.saturating_duration_since(self.glow_at) >= GLOW_EVERY;
        if changed && !glow_now {
            self.glow_due = true;
        }
        if changed || !self.settled || glow_now {
            if changed || !self.settled {
                self.afterglow(content);
            }
            if glow_now {
                self.glow(content);
                self.glow_due = false;
                self.glow_at = now;
            }
            self.light();
        }
        if self.awake_until.is_some_and(|t| now >= t) && self.fx.is_none() {
            // The band goes: this frame is the still one.
            self.awake_until = None;
            return &self.lit;
        }
        if self.fx.is_some_and(|(fx, at)| now >= at + fx.length()) {
            self.fx = None;
        }
        if self.fx.is_none() && self.awake(now) && self.next_glitch.is_some_and(|t| now >= t) {
            self.fx = Some((Fx::Glitch, now));
            self.next_glitch = Some(now + Duration::from_millis(7000 + (self.rand() % 9000) as u64));
        }
        let rows = self.rows(now);
        match rows {
            None => &self.lit,
            Some(rows) => {
                self.compose(&rows);
                &self.out
            }
        }
    }

    /// The phosphor's afterglow: a pixel lit before fades out over a few frames.
    fn afterglow(&mut self, content: &[u32]) {
        let mut settled = true;
        for (p, &c) in self.persist.iter_mut().zip(content) {
            if *p == c {
                continue;
            }
            let ch = |s: u32| {
                let (old, new) = ((*p >> s) & 0xff, (c >> s) & 0xff);
                let faded = (old * AFTERGLOW) >> 8;
                // Close enough is the new light: the afterglow ends.
                if faded > new + 6 {
                    faded
                } else {
                    new
                }
            };
            let next = ch(16) << 16 | ch(8) << 8 | ch(0);
            if next != c {
                settled = false;
            }
            *p = next;
        }
        self.settled = settled;
    }

    /// The glow around what is lit: the picture at a quarter of its size, blurred,
    /// brought back to full size (and darker towards the rim). Made when the picture
    /// changes, not in the afterglow's frames.
    fn glow(&mut self, content: &[u32]) {
        let (w, h) = (self.w, self.h);
        let (sw, sh) = (w / 4, h / 4);
        if sw < 3 || sh < 3 {
            return;
        }
        // Each block's average.
        for by in 0..sh {
            for bx in 0..sw {
                let mut sum = [0u32; 3];
                for y in by * 4..by * 4 + 4 {
                    for &p in &content[y * w + bx * 4..y * w + bx * 4 + 4] {
                        sum[0] += (p >> 16) & 0xff;
                        sum[1] += (p >> 8) & 0xff;
                        sum[2] += p & 0xff;
                    }
                }
                self.small[by * sw + bx] = [(sum[0] >> 4) as u16, (sum[1] >> 4) as u16, (sum[2] >> 4) as u16];
            }
        }
        // Blurred twice, across and down, five blocks wide.
        for _ in 0..2 {
            for y in 0..sh {
                for x in 0..sw {
                    let mut s = [0u32; 3];
                    for k in x.saturating_sub(2)..(x + 3).min(sw) {
                        let p = self.small[y * sw + k];
                        s[0] += p[0] as u32;
                        s[1] += p[1] as u32;
                        s[2] += p[2] as u32;
                    }
                    self.blur[y * sw + x] = [(s[0] / 5) as u16, (s[1] / 5) as u16, (s[2] / 5) as u16];
                }
            }
            for y in 0..sh {
                for x in 0..sw {
                    let mut s = [0u32; 3];
                    for k in y.saturating_sub(2)..(y + 3).min(sh) {
                        let p = self.blur[k * sw + x];
                        s[0] += p[0] as u32;
                        s[1] += p[1] as u32;
                        s[2] += p[2] as u32;
                    }
                    self.small[y * sw + x] = [(s[0] / 5) as u16, (s[1] / 5) as u16, (s[2] / 5) as u16];
                }
            }
        }
        // Back to full width, a row for each row of blocks.
        for y in 0..sh {
            for x in 0..w {
                let pos = (x as i32 * 64 - 96).max(0);
                let (x0, f) = ((pos >> 8) as usize, (pos & 255) as u32);
                let (a, b) = (self.small[y * sw + x0.min(sw - 1)], self.small[y * sw + (x0 + 1).min(sw - 1)]);
                let lerp = |i: usize| (a[i] as u32 * (256 - f) + b[i] as u32 * f) >> 8;
                self.wide[y * w + x] = lerp(0) << 16 | lerp(1) << 8 | lerp(2);
            }
        }
        // And to full height, as strong as the glow is and the rim lets it be.
        for y in 0..h {
            let pos = (y as i32 * 64 - 96).max(0);
            let (y0, f) = ((pos >> 8) as usize, (pos & 255) as u32);
            let (ra, rb) = (y0.min(sh - 1) * w, (y0 + 1).min(sh - 1) * w);
            for x in 0..w {
                let v = blend(self.wide[ra + x], self.wide[rb + x], f);
                self.halo[y * w + x] = dim(v, (GLOW * self.rim[y * w + x] as u32) >> 8);
            }
        }
    }

    /// The picture's light: the afterglow under scan lines and rim, and the glow.
    fn light(&mut self) {
        for (((lit, &p), &scan), &halo) in self.lit.iter_mut().zip(&self.persist).zip(&self.scan).zip(&self.halo) {
            *lit = add(dim(p, scan as u32), halo);
        }
    }

    /// What each row shows now, or none when the lit picture is shown as it is.
    fn rows(&mut self, now: Instant) -> Option<Vec<Row>> {
        let (w, h) = (self.w as i32, self.h as i32);
        let plain = |y: usize| Row { src: Some(y), dx: 0, gain: 256, white: 0, line: (0, 0) };
        let mut rows: Vec<Row> = (0..self.h).map(plain).collect();
        let ease_out = |t: f32| 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
        let ease_in = |t: f32| t.clamp(0.0, 1.0).powi(2);
        let cy = h / 2;
        // The picture squeezed into the rows within `half` of the middle.
        let squeeze = |rows: &mut Vec<Row>, half: f32, gain: u32, white: u32| {
            for (y, row) in rows.iter_mut().enumerate() {
                let d = y as i32 - cy;
                if (d.abs() as f32) < half.max(1.0) {
                    let src = cy as f32 + d as f32 * (h as f32 / 2.0) / half.max(1.0);
                    *row = Row { src: Some((src as i32).clamp(0, h - 1) as usize), dx: 0, gain, white, line: (0, 0) };
                } else {
                    *row = Row { src: None, dx: 0, gain: 0, white: 0, line: (0, 0) };
                }
            }
        };
        let line = |rows: &mut Vec<Row>, half_w: f32, gain: u32| {
            for (y, row) in rows.iter_mut().enumerate() {
                let lit = (y as i32 - cy).abs() <= 1;
                let span = if lit { (w / 2 - half_w as i32, w / 2 + half_w as i32) } else { (0, 0) };
                *row = Row { src: None, dx: 0, gain, white: 255, line: span };
            }
        };
        match self.fx {
            Some((fx, at)) => {
                let u = (now.saturating_duration_since(at).as_secs_f32() / fx.length().as_secs_f32()).min(1.0);
                match fx {
                    Fx::PowerOn if u < 0.10 => line(&mut rows, ease_out(u / 0.10) * w as f32 / 2.0, 256),
                    Fx::PowerOn if u < 0.32 => {
                        let v = ease_out((u - 0.10) / 0.22);
                        squeeze(&mut rows, v * h as f32 / 2.0, 256 + (420.0 * (1.0 - v)) as u32, (190.0 * (1.0 - v)) as u32);
                    }
                    Fx::PowerOn => {
                        let s = ease_out((u - 0.32) / 0.68);
                        let flicker = self.signed() * 0.10 * (1.0 - s);
                        let gain = (256.0 * (1.0 + 0.35 * (1.0 - s) + flicker)) as u32;
                        for row in rows.iter_mut() {
                            row.gain = gain;
                        }
                        if s < 0.35 {
                            self.tear(&mut rows, (1.0 - s / 0.35) * 0.4);
                        }
                    }
                    Fx::PowerOff if u < 0.45 => {
                        let v = 1.0 - ease_in(u / 0.45);
                        squeeze(&mut rows, v * h as f32 / 2.0, 256 + (500.0 * (1.0 - v)) as u32, (230.0 * (1.0 - v)) as u32);
                    }
                    Fx::PowerOff if u < 0.80 => line(&mut rows, (1.0 - (u - 0.45) / 0.35) * w as f32 / 2.0 + 2.0, 256),
                    Fx::PowerOff => line(&mut rows, 2.0, (256.0 * (1.0 - (u - 0.80) / 0.20)) as u32),
                    Fx::Tear => self.tear(&mut rows, 1.0 - u),
                    Fx::Glitch => self.tear(&mut rows, (1.0 - u) * 0.35),
                    Fx::Roll => {
                        let off = (h as f32 * 0.35 * (1.0 - ease_out(u))) as i32;
                        let gain = (256.0 * (1.0 - 0.18 * (1.0 - u))) as u32;
                        for (y, row) in rows.iter_mut().enumerate() {
                            let src = (y as i32 + off).rem_euclid(h);
                            // The sync bar, where the picture's bottom meets its top.
                            let seam = (y as i32 - (h - off)).abs() < 7 && off > 0;
                            *row = Row { src: Some(src as usize), dx: 0, gain: if seam { 40 } else { gain }, white: 0, line: (0, 0) };
                        }
                    }
                    Fx::Degauss => {
                        let k = (1.0 - u).powi(2);
                        let pulse = 1.0 + 0.25 * k * (u * 30.0).sin();
                        for (y, row) in rows.iter_mut().enumerate() {
                            row.dx = (16.0 * k * (y as f32 * 0.045 + u * 55.0).sin()) as i32;
                            row.gain = (256.0 * pulse) as u32;
                        }
                    }
                    Fx::Blip => {
                        let gain = (256.0 * (1.0 + 0.4 * (1.0 - u) * if (u * 40.0).sin() > 0.0 { 1.0 } else { -0.6 })) as u32;
                        for row in rows.iter_mut() {
                            row.gain = gain;
                        }
                    }
                }
                Some(rows)
            }
            None if self.awake(now) => {
                // A band of brighter light rolling down, slowly, and the light's flicker.
                let period = 4.5f32;
                let t = (now - self.epoch).as_secs_f32() % period / period;
                let band = t * (h + 160) as f32 - 80.0;
                let flicker = 256 - self.rand() % 7;
                for (y, row) in rows.iter_mut().enumerate() {
                    let d = (y as f32 - band).abs();
                    let lift = if d < 60.0 { 0.07 * (1.0 + (std::f32::consts::PI * d / 60.0).cos()) / 2.0 } else { 0.0 };
                    row.gain = (flicker as f32 * (1.0 + lift)) as u32;
                }
                Some(rows)
            }
            None => None,
        }
    }

    /// Bands torn sideways and the light flickering, `k` strong (0 to 1).
    fn tear(&mut self, rows: &mut [Row], k: f32) {
        if k <= 0.0 {
            return;
        }
        let h = rows.len() as u32;
        let gain = (256.0 * (1.0 + self.signed() * 0.25 * k)) as u32;
        for row in rows.iter_mut() {
            row.gain = row.gain * gain / 256;
        }
        for _ in 0..3 {
            let y0 = (self.rand() % h) as usize;
            let len = 6 + (self.rand() % 50) as usize;
            let dx = (self.signed() * 42.0 * k) as i32;
            for row in rows.iter_mut().skip(y0).take(len) {
                row.dx += dx;
            }
        }
        // A row of the picture jumped to another place.
        let (a, b) = ((self.rand() % h) as usize, (self.rand() % h) as usize);
        if k > 0.5 {
            rows[a].src = rows[b].src;
            rows[a].white = 120;
        }
    }

    fn compose(&mut self, rows: &[Row]) {
        let w = self.w;
        for (y, row) in rows.iter().enumerate() {
            let out = &mut self.out[y * w..(y + 1) * w];
            let Some(src) = row.src else {
                let beam = scale(self.beam, row.gain, 0);
                for (x, p) in out.iter_mut().enumerate() {
                    let x = x as i32;
                    *p = if x >= row.line.0 && x < row.line.1 { beam } else { 0 };
                }
                continue;
            };
            let from = &self.lit[src * w..(src + 1) * w];
            if row.dx == 0 && row.gain == 256 && row.white == 0 {
                out.copy_from_slice(from);
                continue;
            }
            if row.dx == 0 && row.gain <= 256 && row.white == 0 {
                for (p, &f) in out.iter_mut().zip(from) {
                    *p = dim(f, row.gain);
                }
                continue;
            }
            for (x, p) in out.iter_mut().enumerate() {
                let sx = x as i32 - row.dx;
                *p = if sx < 0 || sx >= w as i32 { 0 } else { scale(from[sx as usize], row.gain, row.white) };
            }
        }
    }

    /// The frame with the tube's light, as it settles (for screens off the device).
    pub fn lit(&mut self, content: &[u32]) -> &[u32] {
        self.persist.copy_from_slice(content);
        self.settled = true;
        self.glow(content);
        self.light();
        &self.lit
    }
}

/// Each channel `s` of 256 (at most 256).
fn dim(p: u32, s: u32) -> u32 {
    (((p & 0xff00ff) * s) >> 8) & 0xff00ff | (((p & 0xff00) * s) >> 8) & 0xff00
}

/// `a` to `b`, `f` of 256 of the way.
fn blend(a: u32, b: u32, f: u32) -> u32 {
    let rb = ((a & 0xff00ff) * (256 - f) + (b & 0xff00ff) * f) >> 8;
    let g = ((a & 0xff00) * (256 - f) + (b & 0xff00) * f) >> 8;
    rb & 0xff00ff | g & 0xff00
}

/// Two lights added, each channel at most full.
fn add(a: u32, b: u32) -> u32 {
    let rb = (a & 0xff00ff) + (b & 0xff00ff);
    let g = (a & 0xff00) + (b & 0xff00);
    let (rb_over, g_over) = (rb & 0x0100_0100, g & 0x1_0000);
    (rb | (rb_over - (rb_over >> 8))) & 0xff00ff | (g | (g_over - (g_over >> 8))) & 0xff00
}

/// A pixel `gain` bright (of 256) and washed `white` (of 255) towards white.
fn scale(p: u32, gain: u32, white: u32) -> u32 {
    let ch = |s: u32| {
        let v = ((((p >> s) & 0xff) * gain) >> 8).min(255);
        (v + (255 - v) * white / 255).min(255) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_afterglow_fades_then_settles() {
        let mut crt = Crt::new(8, 8, Effects::Calm);
        let now = Instant::now();
        crt.frame(&[0xffffff; 64], true, now);
        let dark = [0u32; 64];
        assert_ne!(crt.frame(&dark, true, now)[27], 0, "the first frame after still glows");
        let mut frames = 0;
        let mut at = now;
        while crt.next_frame(at).is_some() && frames < 50 {
            at += Duration::from_millis(33);
            crt.frame(&dark, false, at);
            frames += 1;
        }
        assert!(frames < 20, "settled in {frames} frames");
        assert_eq!(crt.frame(&dark, false, at)[27], 0);
    }

    #[test]
    fn off_shows_the_frame_as_drawn() {
        let mut crt = Crt::new(4, 4, Effects::Off);
        let frame = [0x123456; 16];
        crt.start(Fx::PowerOn, Instant::now());
        assert_eq!(crt.frame(&frame, true, Instant::now()), &frame);
        assert!(crt.next_frame(Instant::now()).is_none());
    }

    #[test]
    fn switching_on_starts_dark_and_ends_on_the_picture() {
        let mut crt = Crt::new(64, 48, Effects::Calm);
        let at = Instant::now();
        let frame = vec![0x40c060; 64 * 48];
        crt.start(Fx::PowerOn, at);
        let first = crt.frame(&frame, true, at).to_vec();
        assert_eq!(first[0], 0, "dark above the line");
        let end = at + Duration::from_secs(2);
        let shown = crt.frame(&frame, false, end).to_vec();
        assert!(crt.next_frame(end).is_none());
        assert_eq!(shown[24 * 64 + 32], crt.lit(&frame)[24 * 64 + 32]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// cargo test --release bench_frames -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_frames() {
        let (w, h) = (640, 480);
        let mut crt = Crt::new(w, h, Effects::Full);
        let frame: Vec<u32> = (0..w * h).map(|i| if i % 7 == 0 { 0x40ff70 } else { 0x020b05 }).collect();
        let other: Vec<u32> = (0..w * h).map(|i| if i % 5 == 0 { 0x40ff70 } else { 0x020b05 }).collect();
        let t = Instant::now();
        for n in 0..60 {
            crt.frame(if n % 2 == 0 { &frame } else { &other }, true, Instant::now());
        }
        eprintln!("light, changed every frame: {:?}/frame", t.elapsed() / 60);
        let t = Instant::now();
        let mut n = 0;
        crt.frame(&frame, true, Instant::now());
        while !crt.settled {
            crt.frame(&frame, false, Instant::now());
            n += 1;
        }
        eprintln!("afterglow: {n} frames, {:?}/frame", t.elapsed() / (n + 1));
        crt.wake(Instant::now());
        let t = Instant::now();
        for _ in 0..60 {
            crt.frame(&frame, false, Instant::now());
        }
        eprintln!("rolling band: {:?}/frame", t.elapsed() / 60);
        crt.start(Fx::Tear, Instant::now());
        let t = Instant::now();
        for _ in 0..60 {
            crt.frame(&frame, false, Instant::now());
        }
        eprintln!("tear over a still picture: {:?}/frame", t.elapsed() / 60);
    }
}

#[cfg(test)]
mod sheet {
    use super::*;

    /// Each moment of the tube at a few points, half size, in one picture:
    /// PORTSTORE_FX_SRC=screen.png PORTSTORE_FX_OUT=dir cargo test fx_sheet -- --ignored
    #[test]
    #[ignore]
    fn fx_sheet() {
        let (Ok(src), Ok(out)) = (std::env::var("PORTSTORE_FX_SRC"), std::env::var("PORTSTORE_FX_OUT")) else { return };
        let img = crate::ui::pictures::decode(std::path::Path::new(&src)).expect("a PNG");
        let (w, h) = (img.w, img.h);
        let frame: Vec<u32> = img.px.iter().map(|p| p & 0xff_ffff).collect();
        let points = [0.04, 0.15, 0.25, 0.4, 0.6, 0.9];
        let fxs = [Fx::PowerOn, Fx::PowerOff, Fx::Tear, Fx::Roll, Fx::Degauss, Fx::Blip];
        let (cw, ch) = (w / 2, h / 2);
        let mut sheet = vec![0x202020u32; cw * points.len() * ch * fxs.len()];
        let sw = cw * points.len();
        for (row, fx) in fxs.iter().enumerate() {
            for (col, u) in points.iter().enumerate() {
                let mut crt = Crt::new(w, h, Effects::Calm);
                let at = Instant::now();
                crt.frame(&frame, true, at - Duration::from_secs(1));
                crt.start(*fx, at);
                let t = at + fx.length().mul_f32(*u);
                let shown = crt.frame(&frame, false, t).to_vec();
                for y in 0..ch {
                    for x in 0..cw {
                        sheet[(row * ch + y) * sw + col * cw + x] = shown[y * 2 * w + x * 2];
                    }
                }
            }
        }
        let path = std::path::Path::new(&out).join("fx-sheet.png");
        crate::ui::canvas::save_png(&path, sw, ch * fxs.len(), &sheet).unwrap();
    }
}
