//! Ports' pictures (img/<id>-thumb-<hash>.png, img/<id>-cover-<hash>.png, made small
//! by index/build.py), and homebrew's screenshots (by their address on GitHub): from
//! those shipped with Port Store, those downloaded before (cache/img, cache/hb-img),
//! or downloaded now, one at a time on a thread of their own; made smaller to fit
//! where they are shown.
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
    /// Made smaller: (name, width, height).
    fitted: HashMap<(String, usize, usize), Arc<Image>>,
    /// The order pictures were loaded in: past KEEP, the oldest are let go (read from
    /// the card again when shown again), so browsing a long list never fills memory.
    order: std::collections::VecDeque<String>,
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
                    let to = local(&store.card.home, &name);
                    if let Some(dir) = to.parent() {
                        let _ = fs::create_dir_all(dir);
                    }
                    let url = if name.starts_with("https://") { name.clone() } else { format!("{base}/{name}") };
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
            fitted: HashMap::new(),
            order: Default::default(),
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
        for path in [self.home.join(name), local(&self.home, name)] {
            if path.exists() {
                let image = read(&path);
                if image.is_none() {
                    eprintln!("portstore: picture {}: cannot read it", path.display());
                }
                self.keep(name.to_string(), image.clone());
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

    /// The picture `name` made to fit in `w` x `h` (made smaller by the average of
    /// the pixels each covers; a smaller one stays as it is).
    pub fn get_fit(&mut self, name: &str, w: usize, h: usize) -> Option<Arc<Image>> {
        let image = self.get(name)?;
        if image.w <= w && image.h <= h {
            return Some(image);
        }
        let key = (name.to_string(), w, h);
        if let Some(fitted) = self.fitted.get(&key) {
            return Some(fitted.clone());
        }
        let fitted = Arc::new(shrink(&image, w, h));
        self.fitted.insert(key, fitted.clone());
        Some(fitted)
    }

    /// Whether pictures came in since the last call (the screen is to be drawn again).
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok(name) = self.done.try_recv() {
            let path = local(&self.home, &name);
            let image = path.exists().then(|| read(&path)).flatten();
            self.keep(name, image);
            any = true;
        }
        any
    }

    fn keep(&mut self, name: String, image: Option<Arc<Image>>) {
        const KEEP: usize = 120;
        self.loaded.insert(name.clone(), image);
        self.order.push_back(name);
        while self.order.len() > KEEP {
            if let Some(old) = self.order.pop_front() {
                self.loaded.remove(&old);
                self.fitted.retain(|(n, ..), _| *n != old);
            }
        }
    }
}

/// A picture as kept: no larger than a page shows it (a screenshot may be a
/// photograph of a few megapixels).
fn read(path: &std::path::Path) -> Option<Arc<Image>> {
    let image = decode(path)?;
    Some(Arc::new(match image.w > 224 || image.h > 168 {
        true => shrink(&image, 224, 168),
        false => image,
    }))
}

/// Where a picture is kept once downloaded.
fn local(home: &std::path::Path, name: &str) -> PathBuf {
    match name.starts_with("https://") {
        true => crate::homebrew::picture_path(home, name),
        false => home.join("cache").join(name),
    }
}

/// `image` made to fit `w` x `h`, keeping its shape: each pixel the average of the
/// ones it covers.
pub fn shrink(image: &Image, w: usize, h: usize) -> Image {
    let scale = (w as f32 / image.w as f32).min(h as f32 / image.h as f32).min(1.0);
    let (nw, nh) = (((image.w as f32 * scale).round() as usize).max(1), ((image.h as f32 * scale).round() as usize).max(1));
    let mut px = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let (y0, y1) = (y * image.h / nh, ((y + 1) * image.h / nh).max(y * image.h / nh + 1));
        for x in 0..nw {
            let (x0, x1) = (x * image.w / nw, ((x + 1) * image.w / nw).max(x * image.w / nw + 1));
            let mut sum = [0u32; 4];
            for sy in y0..y1.min(image.h) {
                for &p in &image.px[sy * image.w + x0..sy * image.w + x1.min(image.w)] {
                    for (i, s) in sum.iter_mut().enumerate() {
                        *s += (p >> (24 - 8 * i)) & 0xff;
                    }
                }
            }
            let n = ((y1.min(image.h) - y0) * (x1.min(image.w) - x0)).max(1) as u32;
            px.push(sum.iter().enumerate().fold(0, |acc, (i, s)| acc | (s / n) << (24 - 8 * i)));
        }
    }
    Image { w: nw, h: nh, px }
}

/// Picture names come from the catalogs: img/<name>.png next to the catalog, or a
/// homebrew screenshot on GitHub; nothing else.
fn safe(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("https://raw.githubusercontent.com/") {
        return rest.to_ascii_lowercase().ends_with(".png") && !rest.contains("..") && !rest.contains(['?', '#', '\\', ' ']);
    }
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
    // Pictures are small; anything larger is not one Port Store shows.
    if w == 0 || h == 0 || w > 2048 || h > 2048 {
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
