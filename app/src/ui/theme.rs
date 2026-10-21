//! How Port Store looks: a monitor's phosphor (green, amber…) or the classic look,
//! how much of a picture tube it plays, and the choices kept in settings.json next to
//! the catalog's address.
use std::fs;
use std::path::{Path, PathBuf};

use super::canvas::mix;

/// The colours a screen is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: u32,
    /// A window's inside, a shade off the background.
    pub panel: u32,
    /// Grid lines, frames of what is not picked.
    pub faint: u32,
    /// Text said in passing: categories, hints.
    pub dim: u32,
    pub fg: u32,
    /// Titles, the pick.
    pub hi: u32,
    pub good: u32,
    pub warn: u32,
    pub bad: u32,
    /// The terminal's font and a picture tube's light; else the classic look.
    pub mono: bool,
}

impl Palette {
    /// A monitor of one phosphor: every shade between its glow and its dark.
    pub fn phosphor(fg: u32, bg: u32) -> Palette {
        let hi = mix(fg, 0xffffff, 120);
        Palette {
            bg,
            panel: mix(bg, fg, 14),
            faint: mix(bg, fg, 60),
            dim: mix(bg, fg, 150),
            fg,
            hi,
            good: fg,
            warn: hi,
            bad: hi,
            mono: true,
        }
    }

    /// Port Store's look before 1.1.
    pub fn classic() -> Palette {
        Palette {
            bg: 0x15171c,
            panel: 0x22262f,
            faint: 0x2f3644,
            dim: 0x8d95a3,
            fg: 0xeceef2,
            hi: 0x4fa3ff,
            good: 0x5cc98a,
            warn: 0xf0a64a,
            bad: 0xef6b6b,
            mono: false,
        }
    }

    /// Dark to light, for pictures shown in the screen's own light.
    pub fn ramp(&self) -> [u32; 5] {
        [self.bg, mix(self.bg, self.fg, 45), self.dim, self.fg, self.hi]
    }
}

/// The themes, in the order Left / Right go through them.
pub const THEMES: [(&str, &str); 6] = [
    ("green", "GREEN P1"),
    ("amber", "AMBER P3"),
    ("ice", "ICE"),
    ("white", "WHITE P4"),
    ("red", "RED"),
    ("classic", "CLASSIC"),
];

pub fn palette(theme: &str, custom: Option<(u32, u32)>) -> Palette {
    match theme {
        "amber" => Palette::phosphor(0xffb02e, 0x0c0700),
        "ice" => Palette::phosphor(0x7fdcff, 0x020a12),
        "white" => Palette::phosphor(0xdce6ee, 0x07090b),
        "red" => Palette::phosphor(0xff5a46, 0x0d0302),
        "classic" => Palette::classic(),
        "custom" => match custom {
            Some((fg, bg)) => Palette::phosphor(fg, bg),
            None => Palette::phosphor(0x3dff7a, 0x020b05),
        },
        _ => Palette::phosphor(0x3dff7a, 0x020b05),
    }
}

/// How much of a picture tube the screen plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effects {
    /// Scan lines and glow, flicker on changes, the tube switching on and off, and
    /// while the screen is used a slow rolling band and blinking cursor.
    Full,
    /// Scan lines, glow and the changes' flicker; nothing moves on its own.
    Calm,
    /// A plain screen: nothing played.
    Off,
}

pub const EFFECTS: [(Effects, &str, &str); 3] =
    [(Effects::Full, "full", "FULL"), (Effects::Calm, "calm", "CALM"), (Effects::Off, "off", "OFF")];

/// The choices of how it looks, as kept in settings.json.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub theme: String,
    /// Colours of the "custom" theme: (phosphor, background).
    pub custom: Option<(u32, u32)>,
    pub effects: Effects,
    /// Pictures in their own colours rather than in the screen's light.
    pub color_pictures: bool,
    /// Development builds (of catalogs that publish them) in the lists.
    pub dev: bool,
}

impl Default for Look {
    fn default() -> Look {
        Look { theme: "green".into(), custom: None, effects: Effects::Full, color_pictures: false, dev: false }
    }
}

fn hex(v: Option<&serde_json::Value>) -> Option<u32> {
    let s = v?.as_str()?.trim().trim_start_matches('#');
    (s.len() == 6).then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

impl Look {
    pub fn palette(&self) -> Palette {
        palette(&self.theme, self.custom)
    }

    pub fn theme_name(&self) -> &'static str {
        THEMES.iter().find(|(id, _)| *id == self.theme).map_or("CUSTOM", |(_, name)| name)
    }

    pub fn effects_name(&self) -> &'static str {
        EFFECTS.iter().find(|(e, ..)| *e == self.effects).map_or("FULL", |(.., name)| name)
    }

    /// The themes Left / Right go through: the custom one too when settings.json
    /// gives its colours.
    pub fn themes(&self) -> Vec<&'static str> {
        let mut out: Vec<&str> = THEMES.iter().map(|(id, _)| *id).collect();
        if self.custom.is_some() {
            out.push("custom");
        }
        out
    }

    pub fn load(home: &Path) -> Look {
        let json: serde_json::Value = fs::read_to_string(home.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let text = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_lowercase);
        let colors = json.get("colors");
        let custom = hex(colors.and_then(|c| c.get("fg"))).zip(hex(colors.and_then(|c| c.get("bg"))));
        let mut look = Look::default();
        if let Some(t) = text("theme") {
            if THEMES.iter().any(|(id, _)| *id == t) || (t == "custom" && custom.is_some()) {
                look.theme = t;
            }
        }
        look.custom = custom;
        if let Some(e) = text("effects") {
            look.effects = EFFECTS.iter().find(|(_, id, _)| *id == e).map_or(look.effects, |(e, ..)| *e);
        }
        look.color_pictures = text("pictures").as_deref() == Some("color");
        look.dev = json.get("dev").and_then(|v| v.as_bool()).unwrap_or(false);
        look
    }

    /// Written into settings.json, keeping what else is there (the catalog's address,
    /// the custom colours), in one step: the file is never left half written.
    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let path = home.join("settings.json");
        let mut json: serde_json::Map<String, serde_json::Value> = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        json.insert("theme".into(), self.theme.clone().into());
        json.insert("effects".into(), EFFECTS.iter().find(|(e, ..)| *e == self.effects).map_or("full", |(_, id, _)| id).into());
        json.insert("pictures".into(), if self.color_pictures { "color" } else { "screen" }.into());
        json.insert("dev".into(), self.dev.into());
        let text = serde_json::to_string_pretty(&json).map_err(std::io::Error::other)? + "\n";
        let tmp: PathBuf = home.join("settings.json.new");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_keep_the_catalog_address_and_read_back() {
        let dir = std::env::temp_dir().join(format!("portstore-look-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), r##"{"index": "https://x/index.json", "colors": {"fg": "#ff00ff", "bg": "000010"}}"##).unwrap();
        let mut look = Look::load(&dir);
        assert_eq!(look, Look { custom: Some((0xff00ff, 0x000010)), ..Look::default() });
        look.theme = "custom".into();
        look.effects = Effects::Calm;
        look.color_pictures = true;
        look.dev = true;
        look.save(&dir).unwrap();
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(json["index"], "https://x/index.json");
        assert_eq!(Look::load(&dir), look);
        assert_eq!(look.palette().fg, 0xff00ff);
        assert!(look.themes().contains(&"custom"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_settings_fall_back() {
        let dir = std::env::temp_dir().join(format!("portstore-look2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), r#"{"theme": "custom", "effects": "loud", "dev": "yes"}"#).unwrap();
        assert_eq!(Look::load(&dir), Look::default(), "custom without colours, unknown effects");
        let _ = fs::remove_dir_all(&dir);
    }
}
