//! Port Store's screens: the catalog in tabs, a port's page, jobs with their progress.
pub mod canvas;
pub mod fb;
pub mod input;
pub mod pictures;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::index::{Index, Port};
use crate::install::{free_bytes, Step};
use crate::needs::{self, Have, Roms};
use crate::{size, State, Store};
use canvas::Canvas;
use input::Key;
use pictures::Pictures;

/// The screen Port Store is laid out for at least: the Mini's. Larger ones (the
/// Mini v4 and Flip, 752x560) get the same layout, wider and with more rows.
pub const W: usize = 640;
pub const H: usize = 480;

const BG: u32 = 0x15171c;
const PANEL: u32 = 0x22262f;
const PICK: u32 = 0x2f3644;
const TEXT: u32 = 0xeceef2;
const DIM: u32 = 0x8d95a3;
const ACCENT: u32 = 0x4fa3ff;
const GOOD: u32 = 0x5cc98a;
const WARN: u32 = 0xf0a64a;
const BAD: u32 = 0xef6b6b;

const TOP: i32 = 56;
/// A port's row, and a group's heading above its ports.
const ROW: i32 = 54;
const HEADING: i32 = 30;
const END: i32 = 12;
/// How many ports Left / Right move by.
const PAGE: usize = 6;
const THUMB: i32 = 46;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Games,
    Apps,
    Installed,
    Updates,
    More,
}

const TABS: [(Tab, &str); 5] = [
    (Tab::Games, "Games"),
    (Tab::Apps, "Apps"),
    (Tab::Installed, "Installed"),
    (Tab::Updates, "Updates"),
    (Tab::More, "More"),
];

/// A row of the list: a group's heading (Miyoo64 (N64)), a port, an entry to install
/// by hand.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Heading(String),
    /// The end of a group, before ports of none.
    End,
    Port(usize),
    Manual(usize),
}

impl Item {
    fn pickable(&self) -> bool {
        matches!(self, Item::Port(_) | Item::Manual(_))
    }

    fn height(&self) -> i32 {
        match self {
            Item::Heading(_) => HEADING,
            Item::End => END,
            _ => ROW,
        }
    }
}

#[derive(Clone, Debug)]
enum Action {
    Install(String),
    /// Move the game files found in import/ for a port into place.
    Place(String),
    /// Several ports, one after another (marked ones, or every update).
    InstallMany(Vec<String>),
    Remove(String),
    Refresh,
}

#[derive(Clone, Debug)]
enum View {
    List,
    Details { id: String, scroll: usize },
    Manual { at: usize, scroll: usize },
    Confirm { text: String, yes: Action },
    Busy { title: String, step: Option<Step> },
    Message { title: String, text: String },
}

enum Msg {
    Step(Step),
    /// A ROM being read to check a port's needs.
    Checking(String),
    Needs(String, Vec<Have>, Vec<Option<needs::Placement>>),
    Done(Result<Finished, String>),
    Index(Result<Index, String>),
    /// The catalog downloaded on start, without a word on screen.
    Quiet(Result<Index, String>),
    /// The port a queue of installs is on: (place, of how many, name).
    Queue(usize, usize, String),
}

/// A job done.
enum Finished {
    Installed(String),
    /// Files moved from import/ for a port: how many.
    Placed(String, usize),
    /// A queue: those installed, those not (with why).
    Many(Vec<String>, Vec<(String, String)>),
    Removed,
}

#[derive(PartialEq, Eq)]
pub enum Flow {
    Stay,
    Exit,
}

pub struct App {
    store: Arc<Store>,
    index: Index,
    states: HashMap<String, State>,
    needs: HashMap<String, Vec<Have>>,
    /// Per port: game files found in import/ for its missing needs.
    imports: HashMap<String, Vec<Option<needs::Placement>>>,
    /// The ROM being read for a check of needs.
    checking: Option<String>,
    free: Option<u64>,
    tab: Tab,
    /// Games of one category only (Select goes through them).
    genre: Option<String>,
    /// Per tab: the row picked and the first row on show, by place in `items()`.
    pick: [usize; 5],
    top: [usize; 5],
    pictures: Pictures,
    /// Ports marked (X) for installing together (Start).
    marked: std::collections::BTreeSet<String>,
    /// Newest releases first (R2), instead of A to Z.
    newest: bool,
    /// The catalog could not be downloaded on start (no Wi-Fi).
    offline: bool,
    /// Port Store has updated itself: start the new one on leaving.
    pub restart: bool,
    view: View,
    /// Where to go back to from a message or a job.
    under: Vec<View>,
    jobs: (Sender<Msg>, Receiver<Msg>),
    busy: bool,
    cancel: Arc<AtomicBool>,
    pub dirty: bool,
}

impl App {
    pub fn new(store: Store) -> App {
        let index = store.load_index();
        store.repair(&index);
        let store = Arc::new(store);
        let mut app = App {
            pictures: Pictures::new(store.clone()),
            store,
            index,
            states: HashMap::new(),
            needs: HashMap::new(),
            imports: HashMap::new(),
            checking: None,
            free: None,
            tab: Tab::Games,
            genre: None,
            marked: Default::default(),
            newest: false,
            offline: false,
            restart: false,
            pick: [0; 5],
            top: [0; 5],
            view: View::List,
            under: Vec::new(),
            jobs: channel(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: true,
        };
        app.recount();
        app
    }

    fn recount(&mut self) {
        self.states = self
            .index
            .ports
            .iter()
            .map(|p| (p.id.clone(), self.store.state(p)))
            .collect();
        self.free = free_bytes(&self.store.card.root);
        let updates = self.states.values().filter(|s| matches!(s, State::Update(_))).count();
        self.store.note_updates(updates);
        self.dirty = true;
    }

    fn state(&self, id: &str) -> State {
        self.states.get(id).cloned().unwrap_or(State::Absent)
    }

    fn tab_at(&self) -> usize {
        TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0)
    }

    /// The rows of the tab on show. Games and apps are in alphabetical order, the
    /// ports of a group (a collection, one porter's games) together under its heading,
    /// in the place of the group's name.
    fn items(&self) -> Vec<Item> {
        let ports = self.index.ports.iter().enumerate();
        let mut shown: Vec<(usize, &Port)> = match self.tab {
            Tab::Games => ports
                .filter(|(_, p)| !p.is_app())
                .filter(|(_, p)| self.genre.as_ref().is_none_or(|g| p.categories.contains(g)))
                .collect(),
            Tab::Apps => ports.filter(|(_, p)| p.is_app()).collect(),
            Tab::Installed => ports
                .filter(|(_, p)| matches!(self.state(&p.id), State::Installed(_) | State::Update(_)))
                .collect(),
            Tab::Updates => ports
                .filter(|(_, p)| matches!(self.state(&p.id), State::Update(_)))
                .collect(),
            Tab::More => return (0..self.index.manual.len()).map(Item::Manual).collect(),
        };
        let grouped = matches!(self.tab, Tab::Games | Tab::Apps) && !self.newest;
        if self.newest {
            shown.sort_by(|(_, a), (_, b)| b.date.cmp(&a.date).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            return shown.into_iter().map(|(i, _)| Item::Port(i)).collect();
        }
        let key = |p: &Port| match grouped && !p.group.is_empty() {
            true => (p.group.to_lowercase(), p.name.to_lowercase()),
            false => (p.name.to_lowercase(), String::new()),
        };
        shown.sort_by_key(|(_, p)| key(p));
        let mut out = Vec::new();
        let mut group = "";
        for (i, p) in shown {
            if grouped && p.group != group {
                // A group's end, before ports of no group, is a line: they are not its.
                if !group.is_empty() && p.group.is_empty() {
                    out.push(Item::End);
                }
                if !p.group.is_empty() {
                    out.push(Item::Heading(p.group.clone()));
                }
            }
            group = if grouped { p.group.as_str() } else { "" };
            out.push(Item::Port(i));
        }
        out
    }

    /// The categories of games, for Select to go through.
    fn genres(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .index
            .ports
            .iter()
            .filter(|p| !p.is_app())
            .flat_map(|p| p.categories.iter().cloned())
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// The pick on a port (not a heading), from `at` on in the direction `step`.
    fn pickable(items: &[Item], at: usize, step: i64) -> usize {
        let n = items.len() as i64;
        if n == 0 {
            return 0;
        }
        let mut i = at as i64;
        for _ in 0..n {
            let wrapped = i.rem_euclid(n) as usize;
            if items[wrapped].pickable() {
                return wrapped;
            }
            i += step;
        }
        0
    }

    fn port(&self, id: &str) -> Option<&Port> {
        self.index.port(id)
    }

    // ---- input ----

    pub fn key(&mut self, key: Key) -> Flow {
        self.dirty = true;
        match self.view.clone() {
            View::List => return self.list_key(key),
            View::Details { id, scroll } => self.details_key(key, id, scroll),
            View::Manual { at, scroll } => match key {
                Key::B => self.view = View::List,
                Key::Up => self.view = View::Manual { at, scroll: scroll.saturating_sub(1) },
                Key::Down => self.view = View::Manual { at, scroll: scroll + 1 },
                _ => {}
            },
            View::Confirm { yes, .. } => match key {
                Key::A => self.start(yes),
                Key::B => self.back(),
                _ => {}
            },
            View::Busy { step, .. } => {
                let stoppable = matches!(step, None | Some(Step::Download { .. }) | Some(Step::Check));
                if key == Key::B && stoppable {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            }
            View::Message { .. } => {
                if matches!(key, Key::A | Key::B) {
                    if self.restart {
                        return Flow::Exit;
                    }
                    self.back();
                }
            }
        }
        Flow::Stay
    }

    fn back(&mut self) {
        self.view = self.under.pop().unwrap_or(View::List);
    }

    fn list_key(&mut self, key: Key) -> Flow {
        let t = self.tab_at();
        let items = self.items();
        let n = items.len();
        let pick = self.pick[t].min(n.saturating_sub(1));
        // Pages: so many ports on, not wrapping round.
        let page = |by: i64| {
            let mut at = pick;
            for _ in 0..PAGE {
                let next = at as i64 + by.signum();
                if next < 0 || next >= n as i64 {
                    break;
                }
                at = next as usize;
                while !items[at].pickable() && at > 0 && at + 1 < n {
                    at = (at as i64 + by.signum()) as usize;
                }
            }
            App::pickable(&items, at, -by.signum())
        };
        match key {
            Key::Up if n > 0 => self.pick[t] = App::pickable(&items, (pick + n - 1) % n, -1),
            Key::Down if n > 0 => self.pick[t] = App::pickable(&items, (pick + 1) % n, 1),
            Key::Left if n > 0 => self.pick[t] = page(-1),
            Key::Right if n > 0 => self.pick[t] = page(1),
            Key::L1 => self.tab = TABS[(t + TABS.len() - 1) % TABS.len()].0,
            Key::R1 => self.tab = TABS[(t + 1) % TABS.len()].0,
            Key::Select if self.tab == Tab::Games => {
                let genres = self.genres();
                let at = self.genre.as_ref().and_then(|g| genres.iter().position(|x| x == g));
                self.genre = match at {
                    None => genres.first().cloned(),
                    Some(i) => genres.get(i + 1).cloned(),
                };
                self.pick[t] = 0;
                self.top[t] = 0;
            }
            Key::Y => self.start(Action::Refresh),
            Key::R2 if self.tab != Tab::More => {
                self.newest = !self.newest;
                self.pick[t] = 0;
                self.top[t] = 0;
            }
            Key::X if n > 0 => {
                if let Item::Port(at) = items[pick] {
                    let id = self.index.ports[at].id.clone();
                    if !self.marked.remove(&id) {
                        self.marked.insert(id);
                    }
                }
            }
            Key::Start if !self.marked.is_empty() => {
                let ids: Vec<String> = self.marked.iter().cloned().collect();
                let names: Vec<String> = ids.iter().filter_map(|id| self.port(id).map(|p| p.name.clone())).collect();
                self.under.push(self.view.clone());
                self.view = View::Confirm {
                    text: format!("Install {} ports: {}?", ids.len(), names.join(", ")),
                    yes: Action::InstallMany(ids),
                };
            }
            Key::Select if self.tab == Tab::Updates && n > 0 => {
                let ids: Vec<String> = items
                    .iter()
                    .filter_map(|i| match i {
                        Item::Port(p) => Some(self.index.ports[*p].id.clone()),
                        _ => None,
                    })
                    .collect();
                self.under.push(self.view.clone());
                self.view = View::Confirm {
                    text: format!("Update all {} ports?", ids.len()),
                    yes: Action::InstallMany(ids),
                };
            }
            Key::A if n > 0 => match items[pick].clone() {
                Item::Manual(at) => self.view = View::Manual { at, scroll: 0 },
                Item::Port(at) => {
                    let id = self.index.ports[at].id.clone();
                    self.open(id);
                }
                Item::Heading(_) | Item::End => {}
            },
            Key::B => return Flow::Exit,
            _ => {}
        }
        Flow::Stay
    }

    fn open(&mut self, id: String) {
        let checks = self.port(&id).is_some_and(|p| !p.needs.is_empty());
        if checks && !self.needs.contains_key(&id) {
            self.check_needs(id.clone());
        }
        self.view = View::Details { id, scroll: 0 };
    }

    fn details_key(&mut self, key: Key, id: String, scroll: usize) {
        let Some(port) = self.port(&id).cloned() else {
            self.view = View::List;
            return;
        };
        let state = self.state(&id);
        match key {
            Key::B => self.view = View::List,
            Key::Up => self.view = View::Details { id, scroll: scroll.saturating_sub(1) },
            Key::Down => self.view = View::Details { id, scroll: (scroll + 1).min(60) },
            Key::A => {
                let verb = match state {
                    State::Installed(_) => "Reinstall",
                    State::Update(_) => "Update",
                    _ => "Install",
                };
                let mut text = format!(
                    "{verb} {} {}? It downloads {} and takes {} on the card.",
                    port.name,
                    port.version,
                    size(port.file.size),
                    size(port.install.bytes)
                );
                if state == State::ByHand {
                    text += " Its folders are on the card already: files of the same name are replaced.";
                }
                self.under.push(self.view.clone());
                self.view = View::Confirm { text, yes: Action::Install(id) };
            }
            Key::X if matches!(state, State::Installed(_) | State::Update(_)) => {
                let mut text = format!("Remove {}?", port.name);
                if !port.keep.is_empty() {
                    text += " Your own files (game data, settings) stay.";
                }
                self.under.push(self.view.clone());
                self.view = View::Confirm { text, yes: Action::Remove(id) };
            }
            Key::Y if !port.needs.is_empty() => {
                self.needs.remove(&id);
                self.check_needs(id);
            }
            Key::Start if self.imports.get(&id).is_some_and(|f| f.iter().any(|p| p.is_some())) => {
                let moves: Vec<String> = self.imports[&id]
                    .iter()
                    .flatten()
                    .map(|p| format!("{} → {}", p.shown, p.to))
                    .collect();
                self.under.push(self.view.clone());
                self.view = View::Confirm {
                    text: format!("Move your files into place? {}", moves.join("; ")),
                    yes: Action::Place(id),
                };
            }

            _ => {}
        }
    }

    // ---- jobs ----

    fn check_needs(&mut self, id: String) {
        let Some(port) = self.port(&id).cloned() else { return };
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        std::thread::spawn(move || {
            let mut roms = Roms::open(&store.card.home);
            let have = needs::check(&store.card.root, &port, &mut roms, &mut |path| {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let _ = tx.send(Msg::Checking(name.into_owned()));
            });
            let found = needs::importable(&store.card.home, &port, &have, &mut roms);
            let _ = tx.send(Msg::Needs(id, have, found));
        });
    }

    fn start(&mut self, action: Action) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.cancel.store(false, Ordering::Relaxed);
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        let cancel = self.cancel.clone();
        let title = match &action {
            Action::Install(id) => format!("Installing {}", self.port(id).map_or(id.as_str(), |p| &p.name)),
            Action::Remove(id) => format!("Removing {}", self.port(id).map_or(id.as_str(), |p| &p.name)),
            Action::Refresh => "Downloading the catalog".to_string(),
            Action::InstallMany(ids) => format!("Installing {} ports", ids.len()),
            Action::Place(_) => "Putting your files in place".to_string(),
        };
        if matches!(self.view, View::Confirm { .. }) {
            self.view = self.under.pop().unwrap_or(View::List);
        }
        self.under.push(self.view.clone());
        self.view = View::Busy { title, step: None };
        let port = match &action {
            Action::Install(id) | Action::Remove(id) => self.port(id).cloned(),
            Action::Refresh | Action::InstallMany(_) | Action::Place(_) => None,
        };
        let placements: Vec<needs::Placement> = match &action {
            Action::Place(id) => self.imports.get(id).into_iter().flatten().flatten().cloned().collect(),
            _ => Vec::new(),
        };
        let queue: Vec<Port> = match &action {
            // Port Store itself last: what is left of the queue still runs on the old one.
            Action::InstallMany(ids) => {
                let mut q: Vec<Port> = ids.iter().filter_map(|id| self.port(id).cloned()).collect();
                q.sort_by_key(|p| p.id == crate::SELF_ID);
                q
            }
            _ => Vec::new(),
        };
        std::thread::spawn(move || {
            let done = match action {
                Action::Refresh => {
                    let _ = tx.send(Msg::Index(store.refresh(&cancel)));
                    return;
                }
                Action::Install(_) => {
                    let port = port.expect("a port of the catalog");
                    store
                        .install(&port, &cancel, &mut |step| {
                            let _ = tx.send(Msg::Step(step));
                        })
                        .map(|_| Finished::Installed(port.id.clone()))
                }
                Action::Remove(id) => store.remove(&id, port.as_ref()).map(|_| Finished::Removed),
                Action::Place(id) => placements
                    .iter()
                    .try_fold(0, |n, p| needs::place(&store.card.root, p).map(|m| n + m))
                    .map(|n| Finished::Placed(id, n)),
                Action::InstallMany(_) => {
                    let (mut done, mut failed) = (Vec::new(), Vec::new());
                    for (n, port) in queue.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = tx.send(Msg::Queue(n + 1, queue.len(), port.name.clone()));
                        match store.install(port, &cancel, &mut |step| {
                            let _ = tx.send(Msg::Step(step));
                        }) {
                            Ok(_) => done.push(port.id.clone()),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => break,
                            Err(e) => failed.push((port.id.clone(), e.to_string())),
                        }
                    }
                    Ok(Finished::Many(done, failed))
                }
            };
            let _ = tx.send(Msg::Done(done.map_err(|e| e.to_string())));
        });
    }

    /// Take in what the jobs have sent, and pictures downloaded.
    pub fn pump(&mut self) {
        if self.pictures.pump() {
            self.dirty = true;
        }
        while let Ok(msg) = self.jobs.1.try_recv() {
            self.dirty = true;
            match msg {
                Msg::Step(s) => {
                    if let View::Busy { step, .. } = &mut self.view {
                        *step = Some(s);
                    }
                }
                Msg::Checking(name) => self.checking = Some(name),
                Msg::Queue(n, of, name) => {
                    if let View::Busy { title, step } = &mut self.view {
                        *title = format!("Installing {n} of {of}: {name}");
                        *step = None;
                    }
                }
                Msg::Quiet(result) => match result {
                    Ok(index) => {
                        self.offline = false;
                        self.store.repair(&index);
                        self.index = index;
                        self.recount();
                        self.offer_self_update();
                    }
                    Err(_) => {
                        self.offline = true;
                        self.offer_self_update();
                    }
                },
                Msg::Needs(id, have, found) => {
                    self.checking = None;
                    self.imports.insert(id.clone(), found);
                    self.needs.insert(id, have);
                }
                Msg::Index(result) => {
                    self.busy = false;
                    self.back();
                    match result {
                        Ok(index) => {
                            self.store.repair(&index);
                            self.index = index;
                            self.recount();
                            let n = self.index.ports.len();
                            self.message("Catalog", &format!("{n} ports in the catalog of {}.", self.index_date()));
                        }
                        Err(e) => self.message("Catalog not downloaded", &e),
                    }
                }
                Msg::Done(result) => {
                    self.busy = false;
                    self.back();
                    self.recount();
                    match result {
                        Ok(Finished::Installed(id)) => self.installed(&id),
                        Ok(Finished::Placed(id, n)) => {
                            self.needs.remove(&id);
                            self.imports.remove(&id);
                            self.check_needs(id);
                            self.message("Done", &format!("{n} files moved into place."));
                        }
                        Ok(Finished::Many(done, failed)) => self.installed_many(done, failed),
                        Ok(Finished::Removed) => {
                            self.view = View::List;
                            self.message("Removed", "The port is off the card.");
                        }
                        Err(e) if e == "cancelled" => {}
                        Err(e) => self.message("Something went wrong", &e),
                    }
                }
            }
        }
    }

    /// Download the catalog on start, quietly: the Updates tab stays true, and a new
    /// Port Store is offered at once.
    pub fn refresh_quietly(&mut self) {
        if self.store.index_url.is_empty() {
            return;
        }
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Quiet(store.refresh(&AtomicBool::new(false))));
        });
    }

    /// A newer Port Store in the catalog: offered once, over the list.
    fn offer_self_update(&mut self) {
        let Some(me) = self.port(crate::SELF_ID).cloned() else { return };
        let State::Update(have) = self.state(crate::SELF_ID) else { return };
        if !matches!(self.view, View::List) || self.busy {
            return;
        }
        self.under.push(self.view.clone());
        self.view = View::Confirm {
            text: format!(
                "Port Store {} is out (this one is {have}). Update now? It starts again by itself.",
                me.version
            ),
            yes: Action::Install(me.id),
        };
    }

    fn installed_many(&mut self, done: Vec<String>, failed: Vec<(String, String)>) {
        self.marked.retain(|id| !done.contains(id));
        let name = |id: &str| self.port(id).map_or(id.to_string(), |p| p.name.clone());
        let mut text = format!("{} installed.", done.len());
        if !failed.is_empty() {
            let why: Vec<String> = failed.iter().map(|(id, e)| format!("{}: {e}", name(id))).collect();
            text += &format!(" Not installed: {}", why.join(" "));
        }
        let waiting: Vec<String> = done
            .iter()
            .filter_map(|id| self.port(id))
            .filter(|p| !p.needs.is_empty())
            .map(|p| p.name.clone())
            .collect();
        if !waiting.is_empty() {
            text += &format!(" Still needing your files: {}.", waiting.join(", "));
        }
        if done.iter().any(|id| id == crate::SELF_ID) {
            self.restart = true;
            text += " Port Store itself was updated: it starts again now.";
        }
        self.message("Done", &text);
    }

    fn installed(&mut self, id: &str) {
        let Some(port) = self.port(id).cloned() else { return };
        self.needs.remove(id);
        if id == crate::SELF_ID {
            self.restart = true;
            let text = format!("Port Store {} is on the card. It starts again now.", port.version);
            return self.message("Updated", &text);
        }
        let mut text = format!("{} {} is on the card.", port.name, port.version);
        let in_miyooports = self.store.card.installed(id).is_some_and(|r| r.shortcuts);
        if in_miyooports {
            text += " It is in Games → MiyooPorts.";
        } else if port.is_app() {
            text += " It is in Apps.";
        }
        if !port.needs.is_empty() {
            text += " It still needs your files: see its page.";
            self.check_needs(id.to_string());
        }
        self.message("Installed", &text);
    }

    fn message(&mut self, title: &str, text: &str) {
        self.under.push(self.view.clone());
        self.view = View::Message { title: title.into(), text: text.into() };
    }

    fn index_date(&self) -> String {
        self.index.generated.get(..10).unwrap_or("?").to_string()
    }

    // ---- drawing ----

    pub fn draw(&mut self, c: &mut Canvas) {
        c.clear(BG);
        match self.view.clone() {
            View::List => self.draw_list(c),
            View::Details { id, scroll } => self.draw_details(c, &id, scroll),
            View::Manual { at, scroll } => self.draw_manual(c, at, scroll),
            View::Confirm { text, .. } => {
                self.draw_under(c);
                dialog(c, "Are you sure?", &text, None, "A  Yes      B  No");
            }
            View::Busy { title, step } => {
                self.draw_under(c);
                let (text, bar) = step_text(step.as_ref());
                let stoppable = matches!(step, None | Some(Step::Download { .. }) | Some(Step::Check));
                dialog(c, &title, &text, bar, if stoppable { "B  Cancel" } else { "" });
            }
            View::Message { title, text } => {
                self.draw_under(c);
                dialog(c, &title, &text, None, "A  OK");
            }
        }
    }

    fn draw_under(&mut self, c: &mut Canvas) {
        let saved = std::mem::replace(&mut self.view, View::List);
        if let Some(under) = self.under.last().cloned() {
            self.view = under;
            if !matches!(self.view, View::Confirm { .. } | View::Busy { .. } | View::Message { .. }) {
                self.draw(c);
            }
        } else {
            self.draw_list(c);
        }
        self.view = saved;
        // Dim what is under the dialog.
        for p in c.px.iter_mut() {
            *p = canvas::mix(*p, 0x000000, 150);
        }
    }

    fn draw_list(&mut self, c: &mut Canvas) {
        c.fill(0, 0, c.w as i32, TOP - 8, PANEL);
        c.text(20, 12, 24, TEXT, "Port Store");
        let mut x = c.w as i32 - 12;
        for (tab, name) in TABS.iter().rev() {
            let label = match tab {
                Tab::Updates => {
                    let n = self.states.values().filter(|s| matches!(s, State::Update(_))).count();
                    if n > 0 { format!("{name} {n}") } else { name.to_string() }
                }
                Tab::Games => match &self.genre {
                    Some(g) => format!("{name}: {g}"),
                    None => name.to_string(),
                },
                _ => name.to_string(),
            };
            let w = c.width(15, &label) + 16;
            x -= w;
            if *tab == self.tab {
                c.rounded(x, 12, w, 25, 12, ACCENT);
            }
            c.text(x + 8, 16, 15, if *tab == self.tab { 0x0d1117 } else { DIM }, &label);
            x -= 2;
        }

        let items = self.items();
        let t = self.tab_at();
        let pick = App::pickable(&items, self.pick[t].min(items.len().saturating_sub(1)), 1);
        self.pick[t] = pick;
        // Scroll so the pick is in full view, its group's heading with it when it can.
        let mut top = self.top[t].min(pick);
        if pick == top && top > 0 && matches!(items[top - 1], Item::Heading(_)) {
            top -= 1;
        }
        while top < pick && items[top..=pick].iter().map(Item::height).sum::<i32>() > area(c) {
            top += 1;
        }
        self.top[t] = top;
        if items.is_empty() {
            let text = match self.tab {
                Tab::Installed => "Nothing installed with Port Store yet.",
                Tab::Updates => "Everything installed is up to date.",
                Tab::More => "Nothing more in the catalog.",
                Tab::Apps => "No apps in the catalog.",
                Tab::Games => "The catalog is empty: press Y to download it.",
            };
            let w = c.width(18, text);
            c.text((c.w as i32 - w) / 2, 220, 18, DIM, text);
        }
        let mut y = TOP;
        let mut last = top;
        for (at, item) in items.iter().enumerate().skip(top) {
            if y + item.height() > TOP + area(c) {
                break;
            }
            last = at;
            match item {
                Item::End => c.fill(20, y + END / 2, c.w as i32 - 48, 1, PICK),
                Item::Heading(name) => {
                    c.text(20, y + 8, 15, ACCENT, name);
                    let w = c.width(15, name);
                    c.fill(30 + w, y + 17, c.w as i32 - 58 - w, 1, PICK);
                }
                Item::Manual(m) => {
                    let m = self.index.manual[*m].clone();
                    self.row(c, y, at == pick, &m.name, &m.thumb, 420);
                    let sub = join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]);
                    c.text_fit(76, y + 29, 13, DIM, &sub, 440);
                    c.text_right(c.w as i32 - 34, y + 16, 14, DIM, "by hand");
                }
                Item::Port(p) => {
                    let port = self.index.ports[*p].clone();
                    let (label, color) = match self.state(&port.id) {
                        State::Installed(_) => ("Installed".to_string(), GOOD),
                        State::Update(_) => ("Update".to_string(), WARN),
                        State::ByHand => ("On card".to_string(), DIM),
                        State::Absent => (size(port.file.size), DIM),
                    };
                    let lw = c.width(15, &label);
                    self.row(c, y, at == pick, &port.name, &port.thumb, c.w as i32 - 130 - lw);
                    if self.marked.contains(&port.id) {
                        c.rounded(52, y + 30, 18, 18, 9, ACCENT);
                        c.text(55, y + 30, 14, 0x0d1117, "✓");
                    }
                    // Under a heading the group goes without saying.
                    let grouped = matches!(self.tab, Tab::Games | Tab::Apps) && !port.group.is_empty();
                    let sub = join(&[
                        if self.newest { port.date.clone() } else { String::new() },
                        if port.group.is_empty() || grouped { port.categories.join(", ") } else { port.group.clone() },
                        port.status.clone(),
                        owned_word(&port.assets),
                    ]);
                    c.text_fit(76, y + 29, 13, DIM, &sub, c.w as i32 - 130 - lw);
                    c.text_right(c.w as i32 - 34, y + 16, 15, color, &label);
                }
            }
            y += item.height();
        }
        if top > 0 || last + 1 < items.len() {
            let track = area(c) - 6;
            let h = (track * (last + 1 - top) as i32 / items.len() as i32).max(20);
            let y = TOP + (track - h) * top as i32 / (items.len() - (last + 1 - top)).max(1) as i32;
            c.rounded(c.w as i32 - 12, y, 4, h, 2, PICK);
        }
        let free = self.free.map(|f| format!("{} free", size(f))).unwrap_or_default();
        let sort = if self.newest { "R2 A-Z" } else { "R2 Newest" };
        let keys = match self.tab {
            Tab::Games => format!("A Open  X Mark  Select Genre  {sort}  B Exit"),
            Tab::Updates if !items.is_empty() => "A Open  Select Update all  L1/R1 Tabs  B Exit".to_string(),
            Tab::More => "A Open   L1/R1 Tabs   B Exit".to_string(),
            _ => format!("A Open  X Mark  {sort}  L1/R1 Tabs  B Exit"),
        };
        let left = match self.marked.len() {
            0 => join(&[self.index_date(), if self.offline { "offline".into() } else { free }]),
            n => format!("{n} marked: Start installs"),
        };
        footer(c, &left, &keys);
    }

    /// A row's background, picture (the first letter of the name without one) and
    /// name.
    fn row(&mut self, c: &mut Canvas, y: i32, picked: bool, name: &str, thumb: &str, name_width: i32) {
        if picked {
            c.rounded(10, y, c.w as i32 - 28, ROW - 4, 10, PICK);
        }
        let (bx, by) = (20, y + (ROW - 4 - THUMB) / 2);
        match self.pictures.get(thumb) {
            Some(img) => {
                let (ix, iy) = (bx + (THUMB - img.w as i32) / 2, by + (THUMB - img.h as i32) / 2);
                c.image(ix, iy, &img);
            }
            None => {
                c.rounded(bx, by, THUMB, THUMB, 8, if picked { BG } else { PANEL });
                let letter: String = name.chars().take(1).collect();
                if !letter.is_empty() {
                    let w = c.width(20, &letter);
                    c.text(bx + (THUMB - w) / 2, by + 11, 20, DIM, &letter);
                }
            }
        }
        c.text_fit(76, y + 6, 19, TEXT, name, name_width);
    }

    /// The top of a port's page: its picture, and beside it what is said of it in
    /// short. Its height.
    fn page_top(&mut self, c: &mut Canvas, y: i32, cover: &str, lines: &[(u16, u32, String)]) -> i32 {
        let img = self.pictures.get(cover);
        let (x, width) = match &img {
            Some(img) => (24 + img.w as i32 + 16, c.w as i32 - 48 - img.w as i32 - 16),
            None => (24, c.w as i32 - 48),
        };
        let mut ty = y;
        for (px, color, text) in lines {
            for l in c.wrap(*px, text, width) {
                c.text(x, ty, *px, *color, &l);
                ty += *px as i32 + 7;
            }
            ty += 4;
        }
        match img {
            Some(img) => {
                c.image(24, y, &img);
                (ty - y).max(img.h as i32) + 12
            }
            None => ty - y + 8,
        }
    }

    fn draw_details(&mut self, c: &mut Canvas, id: &str, scroll: usize) {
        let Some(port) = self.port(id).cloned() else { return };
        let state = self.state(id);
        c.fill(0, 0, c.w as i32, TOP - 8, PANEL);
        let (label, color) = match &state {
            State::Installed(v) => (format!("Installed {v}"), GOOD),
            State::Update(v) => (format!("{v} installed"), WARN),
            State::ByHand => ("On the card (by hand)".into(), DIM),
            State::Absent => (String::new(), DIM),
        };
        let lw = c.text_right(c.w as i32 - 20, 16, 15, color, &label);
        c.text_fit(20, 11, 24, TEXT, &port.name, c.w as i32 - 60 - lw);

        // The page: its top (picture and facts), then lines of (size, colour, text);
        // scrolling past the top is the first step down.
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = c.w as i32 - 48;
        let para = |c: &mut Canvas, lines: &mut Vec<(u16, u32, String)>, px: u16, color: u32, text: &str| {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
        };
        let by = if port.porter.is_empty() { String::new() } else { format!("by {}", port.porter.join(", ")) };
        let facts = vec![
            (15, DIM, join(&[port.group.clone(), port.categories.join(", "), port.status.clone()])),
            (15, DIM, by),
            (15, DIM, format!(
                "Version {}{}",
                port.version,
                if port.date.is_empty() { String::new() } else { format!(" ({})", port.date) }
            )),
            (15, DIM, format!("{} download · {} on the card", size(port.file.size), size(port.install.bytes))),
        ];
        let facts: Vec<_> = facts.into_iter().filter(|(_, _, t)| !t.is_empty()).collect();
        if !port.text().is_empty() {
            para(c, &mut lines, 17, TEXT, port.text());
            lines.push((8, DIM, String::new()));
        }
        if !port.needs.is_empty() {
            para(c, &mut lines, 17, TEXT, "You bring:");
            let have = self.needs.get(id);
            for (i, need) in port.needs.iter().enumerate() {
                let (mark, color, said) = match have.and_then(|h| h.get(i)) {
                    None => match &self.checking {
                        Some(name) => ("…", DIM, format!("reading {name}")),
                        None => ("…", DIM, "checking".to_string()),
                    },
                    Some(Have::Unknown) => ("•", DIM, "see the port's page".to_string()),
                    Some(Have::Missing) => ("✗", BAD, "not found on the card".to_string()),
                    Some(Have::Found(p)) => ("✓", GOOD, format!("found: {p}")),
                    Some(Have::WrongVersion(p)) => ("!", WARN, format!("{p} is another version of the game")),
                };
                para(c, &mut lines, 16, color, &format!("{mark} {} — {said}", need.what));
                if let Some(Some(found)) = self.imports.get(id).and_then(|f| f.get(i)) {
                    para(c, &mut lines, 14, ACCENT, &format!("   In {}: Start moves it to {}", found.shown, found.to));
                } else if !need.hint.is_empty() {
                    para(c, &mut lines, 14, DIM, &format!("   {}", need.hint));
                }
            }
            lines.push((8, DIM, String::new()));
        }

        if !port.url.is_empty() {
            para(c, &mut lines, 14, ACCENT, port.url.trim_start_matches("https://"));
        }
        para(c, &mut lines, 13, DIM, &format!("File: {} (without Wi-Fi: put it in App/PortStore/downloads)", port.file.name));

        let mut y = TOP;
        if scroll == 0 {
            y += self.page_top(c, TOP, &port.cover, &facts);
        }
        let mut hidden = 0;
        for (px, color, text) in lines.iter().skip(scroll.saturating_sub(1)) {
            let h = *px as i32 + 7;
            if y + h > foot(c) - 4 {
                hidden += 1;
                continue;
            }
            c.text(24, y, *px, *color, text);
            y += h;
        }
        if scroll > 0 || hidden > 0 {
            c.text_right(c.w as i32 - 16, foot(c) - 22, 13, DIM, if hidden > 0 { "▼ more" } else { "▲" });
        }
        let verb = match state {
            State::Installed(_) => "A Reinstall",
            State::Update(_) => "A Update",
            _ => "A Install",
        };
        let mut keys = vec![verb];
        if matches!(state, State::Installed(_) | State::Update(_)) {
            keys.push("X Remove");
        }
        if !port.needs.is_empty() {
            keys.push("Y Check again");
        }
        if self.imports.get(id).is_some_and(|f| f.iter().any(|p| p.is_some())) {
            keys.push("Start Place files");
        }

        keys.push("B Back");
        footer(c, "", &keys.join("   "));
    }

    fn draw_manual(&mut self, c: &mut Canvas, at: usize, scroll: usize) {
        let Some(m) = self.index.manual.get(at).cloned() else { return };
        c.fill(0, 0, c.w as i32, TOP - 8, PANEL);
        c.text_fit(20, 11, 24, TEXT, &m.name, c.w as i32 - 40);
        let facts = vec![(15, DIM, join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]))];
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = c.w as i32 - 48;
        for (px, color, text) in [
            (17, TEXT, m.notes.as_str()),
            (16, TEXT, "Port Store has no recipe for this one yet: install it by hand from its page, on a computer."),
            (14, ACCENT, m.url.as_deref().unwrap_or("").trim_start_matches("https://")),
        ] {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
            lines.push((8, DIM, String::new()));
        }
        let mut y = TOP;
        if scroll == 0 {
            y += self.page_top(c, TOP, &m.cover, &facts);
        }
        for (px, color, text) in lines.iter().skip(scroll.saturating_sub(1)) {
            if y + *px as i32 + 7 > foot(c) - 4 {
                break;
            }
            c.text(24, y, *px, *color, text);
            y += *px as i32 + 7;
        }
        footer(c, "", "B Back");
    }
}

fn owned_word(assets: &str) -> String {
    match assets {
        "owned" => "needs your game files".into(),
        _ => String::new(),
    }
}

fn join(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn step_text(step: Option<&Step>) -> (String, Option<f32>) {
    match step {
        None => ("Starting…".into(), None),
        Some(Step::Download { done, total }) => (
            format!("Downloading {} of {}", size(*done), size(*total)),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Some(Step::Check) => ("Checking the file…".into(), None),
        Some(Step::Unpack { total: 0, .. }) => ("Unpacking…".into(), None),
        Some(Step::Unpack { done, total }) => (
            format!("Unpacking {} of {}", size(*done), size(*total)),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Some(Step::Finish) => ("Putting the files in place…".into(), None),
    }
}

/// The footer's top, and the list's own space above it.
fn foot(c: &Canvas) -> i32 {
    c.h as i32 - 36
}

fn area(c: &Canvas) -> i32 {
    foot(c) - TOP - 6
}

fn footer(c: &mut Canvas, left: &str, keys: &str) {
    c.fill(0, foot(c), c.w as i32, c.h as i32 - foot(c), PANEL);
    c.text(16, foot(c) + 10, 14, DIM, left);
    c.text_right(c.w as i32 - 16, foot(c) + 10, 14, TEXT, keys);
}

fn dialog(c: &mut Canvas, title: &str, text: &str, bar: Option<f32>, keys: &str) {
    let w = 520;
    let lines = c.wrap(17, text, w - 48);
    let h = 70 + lines.len() as i32 * 24 + if bar.is_some() { 30 } else { 0 } + if keys.is_empty() { 0 } else { 34 };
    let (x, y) = ((c.w as i32 - w) / 2, (c.h as i32 - h) / 2);
    c.rounded(x, y, w, h, 14, PANEL);
    c.text_fit(x + 24, y + 18, 21, TEXT, title, w - 48);
    let mut ty = y + 56;
    for l in &lines {
        c.text(x + 24, ty, 17, TEXT, l);
        ty += 24;
    }
    if let Some(f) = bar {
        let bw = w - 48;
        c.rounded(x + 24, ty + 6, bw, 12, 6, PICK);
        let fw = ((bw as f32 * f.clamp(0.0, 1.0)) as i32).max(12);
        c.rounded(x + 24, ty + 6, fw, 12, 6, ACCENT);
        ty += 30;
    }
    if !keys.is_empty() {
        c.text_right(x + w - 24, ty + 8, 15, DIM, keys);
    }
}

/// Run on the device until the user leaves.
pub fn run(store: Store) -> Result<(), String> {
    let mut fb = fb::Fb::open()?;
    let (w, h) = fb.size();
    if w < W || h < H {
        return Err(format!("the screen is {w}x{h}, Port Store needs 640x480 at least"));
    }
    let mut input = input::Input::open();
    let mut app = App::new(store);
    app.refresh_quietly();
    let mut canvas = Canvas::new(w, h);
    'shown: loop {
        if app.dirty {
            app.draw(&mut canvas);
            fb.show(&canvas.px);
            app.dirty = false;
        }
        for key in input.poll(Duration::from_millis(100)) {
            if app.key(key) == Flow::Exit && !app.busy {
                break 'shown;
            }
        }
        app.pump();
    }
    if app.restart {
        // Port Store has updated itself: the new program, in the place of this one.
        use std::os::unix::process::CommandExt;
        drop(fb);
        // Not current_exe(): that is the replaced file, "portstore (deleted)".
        let exe = app.store.card.home.join("portstore");
        let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
        return Err(format!("starting the new Port Store: {err}"));
    }
    Ok(())
}

/// Screens as PNGs, for looking at them off the device: the list in each tab, the
/// page of each port given (or the first), a dialog.
pub fn shots(store: Store, dir: &std::path::Path, ids: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut app = App::new(store);
    // PORTSTORE_SHOT_SIZE=752x560: the screens of the Mini v4 and Flip.
    let (w, h) = std::env::var("PORTSTORE_SHOT_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((W, H));
    let mut c = Canvas::new(w, h);
    let mut save = |app: &mut App, name: &str| {
        app.draw(&mut c);
        // Pictures come in on their own thread: a little while for them.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(50));
            if app.pictures.pump() {
                app.draw(&mut c);
            }
        }
        c.save_png(&dir.join(format!("{name}.png"))).map_err(|e| e.to_string())
    };
    for (tab, name) in TABS {
        app.tab = tab;
        save(&mut app, &format!("list-{}", name.to_lowercase()))?;
    }
    app.tab = Tab::Games;
    let items = app.items();
    if let Some(at) = items.iter().rposition(|i| matches!(i, Item::Heading(_))) {
        app.pick[0] = at + 1;
        save(&mut app, "list-games-group")?;
    }
    app.pick[0] = 0;
    app.genre = Some("racing".into());
    save(&mut app, "list-games-racing")?;
    app.genre = None;
    let ids: Vec<String> = match ids.is_empty() {
        true => app.index.ports.first().map(|p| p.id.clone()).into_iter().collect(),
        false => ids.to_vec(),
    };
    for id in &ids {
        app.open(id.clone());
        // Wait for the check of the port's needs, as the screen would.
        for _ in 0..600 {
            app.pump();
            if app.needs.contains_key(id) || app.port(id).is_none_or(|p| p.needs.is_empty()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        save(&mut app, &format!("port-{id}"))?;
        app.key(Key::A);
        save(&mut app, &format!("confirm-{id}"))?;
        app.key(Key::B);
    }
    app.under.push(app.view.clone());
    app.view = View::Busy {
        title: "Installing Mario Kart 64".into(),
        step: Some(Step::Download { done: 600 << 10, total: 1381855 }),
    };
    save(&mut app, "busy")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Port;

    fn app(ports: &[(&str, &str, &[&str])]) -> App {
        let dir = std::env::temp_dir().join(format!("portstore-ui-{}", std::process::id()));
        let mut app = App::new(Store::open(dir.clone(), dir.join("home")));
        app.index.ports = ports
            .iter()
            .map(|(name, group, cats)| Port {
                id: name.to_lowercase(),
                name: name.to_string(),
                group: group.to_string(),
                categories: cats.iter().map(|c| c.to_string()).collect(),
                ..Port::default()
            })
            .collect();
        app
    }

    fn names(app: &App) -> Vec<String> {
        app.items()
            .iter()
            .map(|i| match i {
                Item::Heading(h) => format!("# {h}"),
                Item::End => "---".into(),
                Item::Port(p) => app.index.ports[*p].name.clone(),
                Item::Manual(m) => app.index.manual[*m].name.clone(),
            })
            .collect()
    }

    #[test]
    fn groups_sit_under_their_heading_in_the_place_of_their_name() {
        let mut app = app(&[
            ("Zork", "", &["rpg"]),
            ("Super Mario 64", "Miyoo64 (N64)", &["platform"]),
            ("Blips", "", &["puzzle"]),
            ("Mario Kart 64", "Miyoo64 (N64)", &["racing"]),
            ("Rewind", "", &["app"]),
        ]);
        assert_eq!(names(&app), ["Blips", "# Miyoo64 (N64)", "Mario Kart 64", "Super Mario 64", "---", "Zork"]);
        app.genre = Some("racing".into());
        assert_eq!(names(&app), ["# Miyoo64 (N64)", "Mario Kart 64"]);
        app.tab = Tab::Apps;
        assert_eq!(names(&app), ["Rewind"]);
    }

    #[test]
    fn headings_are_never_picked() {
        let mut app = app(&[("P", "G", &["x"]), ("Q", "G", &["x"]), ("Z", "", &["x"])]);
        // # G, P, Q, ---, Z
        assert_eq!(App::pickable(&app.items(), 0, 1), 1);
        app.pick[0] = 1;
        app.key(Key::Up);
        assert_eq!(app.pick[0], 4, "up from the first port wraps to the last");
        app.key(Key::Up);
        assert_eq!(app.pick[0], 2, "over the group's end");
        app.key(Key::Down);
        app.key(Key::Down);
        assert_eq!(app.pick[0], 1, "and down past the heading to the first");
    }

    #[test]
    fn newest_first_drops_the_groups() {
        let mut app = app(&[("A", "G", &["x"]), ("B", "", &["x"]), ("C", "G", &["x"])]);
        for (p, date) in app.index.ports.iter_mut().zip(["2026-01-01", "2026-03-01", "2026-02-01"]) {
            p.date = date.into();
        }
        app.key(Key::R2);
        assert_eq!(names(&app), ["B", "C", "A"]);
        app.key(Key::R2);
        assert_eq!(names(&app), ["B", "# G", "A", "C"]);
    }

    #[test]
    fn x_marks_ports_and_start_installs_them_together() {
        let mut app = app(&[("A", "", &["x"]), ("B", "", &["x"])]);
        app.key(Key::X);
        app.key(Key::Down);
        app.key(Key::X);
        app.key(Key::X);
        app.key(Key::Up);
        assert_eq!(app.marked.iter().cloned().collect::<Vec<_>>(), ["a"]);
        app.key(Key::Down);
        app.key(Key::X);
        app.key(Key::Start);
        match &app.view {
            View::Confirm { yes: Action::InstallMany(ids), .. } => assert_eq!(ids, &["a", "b"]),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn select_goes_through_the_genres_of_games() {
        let mut app = app(&[("A", "", &["racing"]), ("B", "", &["arcade"]), ("C", "", &["app"])]);
        let mut seen = Vec::new();
        for _ in 0..3 {
            app.key(Key::Select);
            seen.push(app.genre.clone());
        }
        assert_eq!(seen, [Some("arcade".into()), Some("racing".into()), None]);
    }
}
