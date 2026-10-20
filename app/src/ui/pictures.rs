//! Ports' pictures (img/<id>-thumb-<hash>.png, img/<id>-cover-<hash>.png, made small
//! by index/build.py): from those shipped with Port Store, those downloaded before
//! (cache/img), or downloaded now, one at a time on a thread of their own.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::Store;

/// Pixels as 0xAARRGGBB words, top row first.
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

pub struct Pictures {
    home: PathBuf,
    loaded: HashMap<String, Option<Arc<Image>>>,
    asked: HashSet<String>,
    todo: Option<Sender<String>>,
    done: Receiver<String>,
}

impl Pictures {
    /// Pictures are downloaded from next to the catalog (`store.index_url`).
    pub fn new(store: Arc<Store>) -> Pictures {
        let (done_tx, done) = channel();
        let home = store.card.home.clone();
        let base = store.index_url.rsplit_once('/').map(|(b, _)| b.to_string());
        let todo = base.map(|base| {
            let (tx, rx) = channel::<String>();
            std::thread::spawn(move || {
                let cancel = AtomicBool::new(false);
                for name in rx {
                    let to = store.card.home.join("cache").join(&name);
                    if let Some(dir) = to.parent() {
                        let _ = fs::create_dir_all(dir);
                    }
                    let url = format!("{base}/{name}");
                    if let Err(e) = store.fetcher.get(&url, &to, &cancel, &mut |_| {}) {
                        eprintln!("portstore: picture {url}: {e}");
                        let _ = fs::remove_file(crate::fetch::part_of(&to));
                    }
                    if done_tx.send(name).is_err() {
                        return;
                    }
                }
            });
            tx
        });
        Pictures {
            home,
            loaded: HashMap::new(),
            asked: HashSet::new(),
            todo,
            done,
        }
    }

    /// The picture `name`, if it is at hand; asked for if not.
    pub fn get(&mut self, name: &str) -> Option<Arc<Image>> {
        if name.is_empty() || !safe(name) {
            return None;
        }
        if let Some(image) = self.loaded.get(name) {
            return image.clone();
        }
        for path in [self.home.join(name), self.home.join("cache").join(name)] {
            if path.exists() {
                let image = decode(&path).map(Arc::new);
                if image.is_none() {
                    eprintln!("portstore: picture {}: cannot read it", path.display());
                }
                self.loaded.insert(name.to_string(), image.clone());
                return image;
            }
        }
        if self.asked.insert(name.to_string()) {
            match &self.todo {
                Some(todo) => {
                    let _ = todo.send(name.to_string());
                }
                None => {
                    self.loaded.insert(name.to_string(), None);
                }
            }
        }
        None
    }

    /// Whether pictures came in since the last call (the screen is to be drawn again).
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok(name) = self.done.try_recv() {
            let path = self.home.join("cache").join(&name);
            let image = path.exists().then(|| decode(&path)).flatten().map(Arc::new);
            self.loaded.insert(name, image);
            any = true;
        }
        any
    }
}

/// Picture names come from the catalog: only img/<name>.png, nothing outside.
fn safe(name: &str) -> bool {
    name.strip_prefix("img/").is_some_and(|rest| {
        !rest.is_empty()
            && rest.ends_with(".png")
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && !rest.contains("..")
    })
}

pub fn decode(path: &Path) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(path).ok()?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = {
        let info = reader.info();
        (info.width as usize, info.height as usize)
    };
    // Pictures are small; anything larger is not one of the catalog's.
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buf).ok()?;
    let bytes = &buf[..frame.buffer_size()];
    let step = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return None,
    };
    let px = bytes
        .chunks_exact(step)
        .take(w * h)
        .map(|p| {
            let (r, g, b, a) = match step {
                4 => (p[0], p[1], p[2], p[3]),
                3 => (p[0], p[1], p[2], 255),
                2 => (p[0], p[0], p[0], p[1]),
                _ => (p[0], p[0], p[0], 255),
            };
            (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
        })
        .collect();
    Some(Image { w, h, px })
}
