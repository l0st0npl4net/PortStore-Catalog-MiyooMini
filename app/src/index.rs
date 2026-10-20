//! The catalog: index.json, as index/build.py writes it.
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Index {
    pub format: u32,
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub ports: Vec<Port>,
    /// Catalog entries Port Store cannot install: a link only.
    #[serde(default)]
    pub manual: Vec<Manual>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Port {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub status: String,
    /// "free", or "owned": the game's own files are needed.
    #[serde(default)]
    pub assets: String,
    #[serde(default)]
    pub porter: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub url: String,
    /// Pictures, next to the catalog: small for the list, larger for the port's page.
    #[serde(default)]
    pub thumb: String,
    #[serde(default)]
    pub cover: String,
    pub version: String,
    #[serde(default)]
    pub date: String,
    pub file: File,
    /// Prefix taken off every member (a wrapper folder).
    #[serde(default)]
    pub strip: String,
    /// What of the archive goes onto the card, as path prefixes from the card's root.
    pub take: Vec<String>,
    /// A folder all of the archive goes into (an app zipped without its App/<Name>/).
    #[serde(default)]
    pub into: String,
    /// [from, to] prefixes: a member under `from` (past `strip`) goes under `to`, the
    /// first that matches; others are left out. For releases laid out otherwise.
    #[serde(default)]
    pub map: Vec<[String; 2]>,
    /// Card paths (prefixes) not installed even though `take` has them: an App's
    /// config.json for a game that belongs in Games > Ports only.
    #[serde(default)]
    pub skip: Vec<String>,
    /// A shortcut for Onion's Ports that Port Store makes, for a game released as an app.
    #[serde(default)]
    pub shortcut: Option<Shortcut>,
    /// The user's: never removed or overwritten (a prefix when it ends in '/').
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub needs: Vec<Need>,
    pub install: Install,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct File {
    pub url: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// "zip", "tar" or "tar.gz".
    pub kind: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Shortcut {
    /// The game's own launcher, from the card's root (App/Petals/launch.sh).
    pub launch: String,
    /// A picture of the port's, copied for the Ports list (App/Petals/icon.png).
    #[serde(default)]
    pub image: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Install {
    pub files: u64,
    pub bytes: u64,
    /// The folders the port owns (App/<Name>/, Roms/PORTS/Games/<Name>/).
    #[serde(default)]
    pub roots: Vec<String>,
    /// A file of the port's in each of them: all there, the port is on the card.
    #[serde(default)]
    pub marks: Vec<String>,
}

/// Something the user brings: game data or a ROM.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Need {
    pub what: String,
    #[serde(default)]
    pub hint: String,
    /// Paths that are there once it is in place.
    #[serde(default)]
    pub check: Vec<String>,
    /// A ROM, by the SHA-1 of its big-endian (.z64) bytes.
    #[serde(default)]
    pub sha1: String,
    /// Folders a ROM is looked for in.
    #[serde(default)]
    pub search: Vec<String>,
    /// An N64 ROM: .v64 and .n64 byte orders count as the same ROM.
    #[serde(default)]
    pub n64: bool,
    /// The title in the ROM's header: a ROM of the same game in another version is
    /// told apart from another game's.
    #[serde(default)]
    pub title: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Manual {
    pub name: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub assets: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub thumb: String,
    #[serde(default)]
    pub cover: String,
}

impl Index {
    pub fn parse(text: &str) -> Result<Index, String> {
        let index: Index = serde_json::from_str(text).map_err(|e| format!("index: {e}"))?;
        if index.format != FORMAT {
            return Err(format!(
                "index format {} (this Port Store reads {FORMAT}): update Port Store",
                index.format
            ));
        }
        Ok(index)
    }

    pub fn port(&self, id: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.id == id)
    }
}

impl Port {
    pub fn owned(&self) -> bool {
        self.assets == "owned"
    }

    /// Whether the port shows in Onion's Games > Ports (shortcuts of its own or made).
    pub fn in_ports(&self) -> bool {
        self.shortcut.is_some() || self.take.iter().any(|t| t.starts_with("Roms/PORTS"))
    }

    /// Apps (catalog category "app"), as against games.
    pub fn is_app(&self) -> bool {
        self.categories.iter().any(|c| c == "app")
    }

    /// Whether `path` (from the card's root) is the user's.
    pub fn kept(&self, path: &str) -> bool {
        self.keep.iter().any(|k| match k.ends_with('/') {
            true => path.starts_with(k.as_str()),
            false => path == k,
        })
    }

    pub fn text(&self) -> &str {
        if self.about.is_empty() {
            &self.notes
        } else {
            &self.about
        }
    }
}
