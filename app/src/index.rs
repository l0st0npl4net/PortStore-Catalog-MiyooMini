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
    /// A shortcut for OnionOS's Ports that Port Store makes, for a game released as an app.
    #[serde(default)]
    pub shortcut: Option<Shortcut>,
    /// The user's: never removed or overwritten (a prefix when it ends in '/').
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub needs: Vec<Need>,
    pub install: Install,
    /// A development build (the catalog's "dev" list): listed only when asked for.
    #[serde(default)]
    pub dev: bool,
    /// The catalog the port comes from when it is another's (slop.kitchen): any other
    /// version of it is an update, as such catalogs number their builds as they like.
    #[serde(default)]
    pub source: String,
    /// No recipe: its release goes onto the card as it is laid out, checked by the
    /// catalog's build like any other.
    #[serde(default)]
    pub auto: bool,
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
    /// What ~Import ports looks for before it shows the shortcut: a file `data` up to
    /// two folders below `dir`. By default the launcher itself.
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub data: String,
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

/// The catalog as read, before each entry is taken on its own.
#[derive(Deserialize)]
struct Raw {
    format: u32,
    #[serde(default)]
    generated: String,
    #[serde(default)]
    ports: Vec<serde_json::Value>,
    #[serde(default)]
    manual: Vec<serde_json::Value>,
    /// Development builds, apart: Port Store before 1.1 does not read them.
    #[serde(default)]
    dev: Vec<serde_json::Value>,
}

impl Index {
    /// The catalog from `text`. An entry Port Store cannot read, or one whose paths
    /// would lead off the card, is left out (and said in the log): one bad entry
    /// never costs the whole catalog.
    pub fn parse(text: &str) -> Result<Index, String> {
        let raw: Raw = serde_json::from_str(text).map_err(|e| format!("index: {e}"))?;
        if raw.format != FORMAT {
            return Err(format!(
                "index format {} (this Port Store reads {FORMAT}): update Port Store",
                raw.format
            ));
        }
        let dev = raw.dev.into_iter().map(|v| (v, true));
        let ports = raw
            .ports
            .into_iter()
            .map(|v| (v, false))
            .chain(dev)
            .filter_map(|(v, dev)| {
                let id = v.get("id").and_then(|i| i.as_str()).unwrap_or("?").to_string();
                match serde_json::from_value::<Port>(v) {
                    Ok(p) if p.sound() => Some(Port { dev: dev || p.dev, ..p }),
                    Ok(_) => {
                        eprintln!("portstore: catalog: {id}: a path off the card, left out");
                        None
                    }
                    Err(e) => {
                        eprintln!("portstore: catalog: {id}: {e}, left out");
                        None
                    }
                }
            })
            .collect();
        let manual = raw.manual.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
        Ok(Index { format: raw.format, generated: raw.generated, ports, manual })
    }

    pub fn port(&self, id: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.id == id)
    }
}

/// A path from the card's root (or a prefix of one), as the catalog gives it: relative,
/// never climbing out, no backslashes or NULs. Empty is no path at all.
pub fn card_path(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && !path.split('/').any(|part| part == ".." || part == ".")
}

/// Whether the keep rule `rule` covers `path`: a folder's when it ends in '/', a
/// pattern when it has '*' (any characters but '/') or '?' (one), else that file.
pub fn keeps(rule: &str, path: &str) -> bool {
    if rule.ends_with('/') {
        return path.starts_with(rule);
    }
    if !rule.contains(['*', '?']) {
        return path == rule;
    }
    fn matches(rule: &[char], path: &[char]) -> bool {
        match rule.split_first() {
            None => path.is_empty(),
            Some(('*', rest)) => (0..=path.len())
                .take_while(|&n| n == 0 || path[n - 1] != '/')
                .any(|n| matches(rest, &path[n..])),
            Some(('?', rest)) => path.first().is_some_and(|&c| c != '/') && matches(rest, &path[1..]),
            Some((c, rest)) => path.first() == Some(c) && matches(rest, &path[1..]),
        }
    }
    let (rule, path): (Vec<char>, Vec<char>) = (rule.chars().collect(), path.chars().collect());
    matches(&rule, &path)
}

impl Port {
    /// Whether everything the port would write or remove stays on the card: its id
    /// (a file and folder name) and every path its install rules and needs name.
    pub fn sound(&self) -> bool {
        let id = !self.id.is_empty()
            && self.id != "."
            && self.id != ".."
            && self.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        let name = !self.file.name.is_empty() && !self.file.name.contains('/') && card_path(&self.file.name);
        let mut paths: Vec<&str> = vec![self.into.as_str(), self.cover.as_str(), self.thumb.as_str()];
        paths.extend(self.take.iter().map(String::as_str));
        paths.extend(self.skip.iter().map(String::as_str));
        paths.extend(self.keep.iter().map(String::as_str));
        paths.extend(self.map.iter().map(|[_, to]| to.as_str()));
        for need in &self.needs {
            paths.extend(need.check.iter().map(String::as_str));
            paths.extend(need.search.iter().map(String::as_str));
        }
        if let Some(sc) = &self.shortcut {
            paths.extend([sc.launch.as_str(), sc.image.as_str(), sc.dir.as_str()]);
        }
        id && name && paths.into_iter().all(card_path)
    }

    pub fn owned(&self) -> bool {
        self.assets == "owned"
    }

    /// Apps (catalog category "app"), as against games.
    pub fn is_app(&self) -> bool {
        self.categories.iter().any(|c| c == "app")
    }

    /// Whether `path` (from the card's root) is the user's.
    pub fn kept(&self, path: &str) -> bool {
        self.keep.iter().any(|k| keeps(k, path))
    }

    pub fn text(&self) -> &str {
        if self.about.is_empty() {
            &self.notes
        } else {
            &self.about
        }
    }
}
