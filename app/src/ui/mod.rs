//! Port Store's screens: the catalog in tabs, a port's page, jobs with their progress.
pub mod canvas;
pub mod fb;
pub mod input;

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
const FOOT: i32 = 444;
const ROW: i32 = 48;
const ROWS: usize = ((FOOT - TOP - 8) / ROW) as usize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    All,
    Installed,
    Updates,
    More,
}

const TABS: [(Tab, &str); 4] = [
    (Tab::All, "All"),
    (Tab::Installed, "Installed"),
    (Tab::Updates, "Updates"),
    (Tab::More, "More"),
];

#[derive(Clone, Debug)]
enum Action {
    Install(String),
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
    Needs(String, Vec<Have>),
    Done(Result<String, String>),
    Index(Result<Index, String>),
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
    /// The ROM being read for a check of needs.
    checking: Option<String>,
    free: Option<u64>,
    tab: Tab,
    pick: [usize; 4],
    top: [usize; 4],
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
        let mut app = App {
            store: Arc::new(store),
            index,
            states: HashMap::new(),
            needs: HashMap::new(),
            checking: None,
            free: None,
            tab: Tab::All,
            pick: [0; 4],
            top: [0; 4],
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
        self.dirty = true;
    }

    fn state(&self, id: &str) -> State {
        self.states.get(id).cloned().unwrap_or(State::Absent)
    }

    fn tab_at(&self) -> usize {
        TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0)
    }

    /// The ports in the tab on show, by their place in the catalog.
    fn rows(&self) -> Vec<usize> {
        let ports = self.index.ports.iter().enumerate();
        match self.tab {
            Tab::All => ports.map(|(i, _)| i).collect(),
            Tab::Installed => ports
                .filter(|(_, p)| matches!(self.state(&p.id), State::Installed(_) | State::Update(_)))
                .map(|(i, _)| i)
                .collect(),
            Tab::Updates => ports
                .filter(|(_, p)| matches!(self.state(&p.id), State::Update(_)))
                .map(|(i, _)| i)
                .collect(),
            Tab::More => (0..self.index.manual.len()).collect(),
        }
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
        let n = self.rows().len();
        let move_by = |pick: usize, by: i64| (pick as i64 + by).clamp(0, n.max(1) as i64 - 1) as usize;
        match key {
            Key::Up => self.pick[t] = if self.pick[t] == 0 { n.saturating_sub(1) } else { self.pick[t] - 1 },
            Key::Down => self.pick[t] = if self.pick[t] + 1 >= n { 0 } else { self.pick[t] + 1 },
            Key::Left => self.pick[t] = move_by(self.pick[t], -(ROWS as i64)),
            Key::Right => self.pick[t] = move_by(self.pick[t], ROWS as i64),
            Key::L1 => self.tab = TABS[(t + TABS.len() - 1) % TABS.len()].0,
            Key::R1 => self.tab = TABS[(t + 1) % TABS.len()].0,
            Key::Y => self.start(Action::Refresh),
            Key::A if n > 0 => {
                let row = self.rows()[self.pick[t].min(n - 1)];
                if self.tab == Tab::More {
                    self.view = View::Manual { at: row, scroll: 0 };
                } else {
                    let id = self.index.ports[row].id.clone();
                    self.open(id);
                }
            }
            Key::B | Key::Start => return Flow::Exit,
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
            Key::Down => self.view = View::Details { id, scroll: scroll + 1 },
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
            let _ = tx.send(Msg::Needs(id, have));
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
        };
        if matches!(self.view, View::Confirm { .. }) {
            self.view = self.under.pop().unwrap_or(View::List);
        }
        self.under.push(self.view.clone());
        self.view = View::Busy { title, step: None };
        let port = match &action {
            Action::Install(id) | Action::Remove(id) => self.port(id).cloned(),
            Action::Refresh => None,
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
                        .map(|_| port.id.clone())
                }
                Action::Remove(id) => store.remove(&id, port.as_ref()).map(|_| String::new()),
            };
            let _ = tx.send(Msg::Done(done.map_err(|e| e.to_string())));
        });
    }

    /// Take in what the jobs have sent.
    pub fn pump(&mut self) {
        while let Ok(msg) = self.jobs.1.try_recv() {
            self.dirty = true;
            match msg {
                Msg::Step(s) => {
                    if let View::Busy { step, .. } = &mut self.view {
                        *step = Some(s);
                    }
                }
                Msg::Checking(name) => self.checking = Some(name),
                Msg::Needs(id, have) => {
                    self.checking = None;
                    self.needs.insert(id, have);
                }
                Msg::Index(result) => {
                    self.busy = false;
                    self.back();
                    match result {
                        Ok(index) => {
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
                        Ok(id) if !id.is_empty() => self.installed(&id),
                        Ok(_) => {
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

    fn installed(&mut self, id: &str) {
        let Some(port) = self.port(id).cloned() else { return };
        self.needs.remove(id);
        let mut text = format!("{} {} is on the card.", port.name, port.version);
        let shortcuts = port.take.iter().any(|t| t.starts_with("Roms/PORTS"));
        let apps = port.install.roots.iter().any(|r| r.starts_with("App/"));
        if shortcuts && !self.store.has_ports_collection() {
            text += " To see it in Games, add Ports Collection in Package Manager (Emulators).";
        } else if shortcuts {
            text += " It is in Games → Ports once Port Store closes.";
        } else if apps {
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
        c.fill(0, 0, W as i32, TOP - 8, PANEL);
        c.text(20, 12, 24, TEXT, "Port Store");
        let mut x = W as i32 - 16;
        for (tab, name) in TABS.iter().rev() {
            let label = match tab {
                Tab::Updates => {
                    let n = self.states.values().filter(|s| matches!(s, State::Update(_))).count();
                    if n > 0 { format!("{name} {n}") } else { name.to_string() }
                }
                _ => name.to_string(),
            };
            let w = c.width(16, &label) + 20;
            x -= w;
            if *tab == self.tab {
                c.rounded(x, 11, w, 26, 13, ACCENT);
            }
            c.text(x + 10, 15, 16, if *tab == self.tab { 0x0d1117 } else { DIM }, &label);
            x -= 4;
        }

        let rows = self.rows();
        let t = self.tab_at();
        let pick = self.pick[t].min(rows.len().saturating_sub(1));
        self.pick[t] = pick;
        if pick < self.top[t] {
            self.top[t] = pick;
        } else if pick >= self.top[t] + ROWS {
            self.top[t] = pick + 1 - ROWS;
        }
        let top = self.top[t];
        if rows.is_empty() {
            let text = match self.tab {
                Tab::Installed => "Nothing installed with Port Store yet.",
                Tab::Updates => "Everything installed is up to date.",
                Tab::More => "Nothing more in the catalog.",
                Tab::All => "The catalog is empty: press Y to download it.",
            };
            let w = c.width(18, text);
            c.text((W as i32 - w) / 2, 220, 18, DIM, text);
        }
        for (slot, &row) in rows.iter().enumerate().skip(top).take(ROWS) {
            let y = TOP + (slot - top) as i32 * ROW;
            if slot == pick {
                c.rounded(10, y, W as i32 - 28, ROW - 4, 10, PICK);
            }
            if self.tab == Tab::More {
                let m = self.index.manual[row].clone();
                c.text_fit(24, y + 4, 19, TEXT, &m.name, 420);
                let sub = join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]);
                c.text_fit(24, y + 26, 13, DIM, &sub, 440);
                c.text_right(W as i32 - 34, y + 13, 14, DIM, "by hand");
                continue;
            }
            let port = self.index.ports[row].clone();
            let (label, color) = match self.state(&port.id) {
                State::Installed(_) => ("Installed".to_string(), GOOD),
                State::Update(_) => ("Update".to_string(), WARN),
                State::ByHand => ("On card".to_string(), DIM),
                State::Absent => (size(port.file.size), DIM),
            };
            let lw = c.width(15, &label);
            c.text_fit(24, y + 4, 19, TEXT, &port.name, W as i32 - 80 - lw);
            let sub = join(&[
                if port.group.is_empty() { port.categories.join(", ") } else { port.group.clone() },
                port.status.clone(),
                owned_word(&port.assets),
            ]);
            c.text_fit(24, y + 26, 13, DIM, &sub, W as i32 - 80 - lw);
            c.text_right(W as i32 - 34, y + 13, 15, color, &label);
        }
        if rows.len() > ROWS {
            let track = FOOT - TOP - 12;
            let h = (track * ROWS as i32 / rows.len() as i32).max(20);
            let y = TOP + (track - h) * top as i32 / (rows.len() - ROWS) as i32;
            c.rounded(W as i32 - 12, y, 4, h, 2, PICK);
        }
        let free = self.free.map(|f| format!("{} free", size(f))).unwrap_or_default();
        footer(
            c,
            &join(&[format!("Catalog {}", self.index_date()), free]),
            "A Open   Y Refresh   L1/R1 Tabs   B Exit",
        );
    }

    fn draw_details(&mut self, c: &mut Canvas, id: &str, scroll: usize) {
        let Some(port) = self.port(id).cloned() else { return };
        let state = self.state(id);
        c.fill(0, 0, W as i32, TOP - 8, PANEL);
        let (label, color) = match &state {
            State::Installed(v) => (format!("Installed {v}"), GOOD),
            State::Update(v) => (format!("{v} installed"), WARN),
            State::ByHand => ("On the card (by hand)".into(), DIM),
            State::Absent => (String::new(), DIM),
        };
        let lw = c.text_right(W as i32 - 20, 16, 15, color, &label);
        c.text_fit(20, 11, 24, TEXT, &port.name, W as i32 - 60 - lw);

        // The page as lines of (size, colour, text), scrolled as a whole.
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = W as i32 - 48;
        let para = |c: &mut Canvas, lines: &mut Vec<(u16, u32, String)>, px: u16, color: u32, text: &str| {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
        };
        let by = if port.porter.is_empty() { String::new() } else { format!("by {}", port.porter.join(", ")) };
        let meta = join(&[port.group.clone(), port.categories.join(", "), port.status.clone(), by]);
        para(c, &mut lines, 15, DIM, &meta);
        let ver = format!(
            "Version {}{} · {} download · {} on the card",
            port.version,
            if port.date.is_empty() { String::new() } else { format!(" ({})", port.date) },
            size(port.file.size),
            size(port.install.bytes)
        );
        para(c, &mut lines, 15, DIM, &ver);
        lines.push((8, DIM, String::new()));
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
                if !need.hint.is_empty() {
                    para(c, &mut lines, 14, DIM, &format!("   {}", need.hint));
                }
            }
            lines.push((8, DIM, String::new()));
        }
        if port.take.iter().any(|t| t.starts_with("Roms/PORTS")) && !self.store.has_ports_collection() {
            para(c, &mut lines, 15, WARN, "Ports Collection is not on the card: add it in Package Manager (Emulators) to start this port from Games.");
            lines.push((8, DIM, String::new()));
        }
        if !port.url.is_empty() {
            para(c, &mut lines, 14, ACCENT, port.url.trim_start_matches("https://"));
        }
        para(c, &mut lines, 13, DIM, &format!("File: {} (without Wi-Fi: put it in App/PortStore/downloads)", port.file.name));

        let mut y = TOP;
        let mut hidden = 0;
        for (px, color, text) in lines.iter().skip(scroll) {
            let h = *px as i32 + 7;
            if y + h > FOOT - 4 {
                hidden += 1;
                continue;
            }
            c.text(24, y, *px, *color, text);
            y += h;
        }
        if scroll > 0 || hidden > 0 {
            c.text_right(W as i32 - 16, FOOT - 22, 13, DIM, if hidden > 0 { "▼ more" } else { "▲" });
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
        keys.push("B Back");
        footer(c, "", &keys.join("   "));
    }

    fn draw_manual(&mut self, c: &mut Canvas, at: usize, scroll: usize) {
        let Some(m) = self.index.manual.get(at).cloned() else { return };
        c.fill(0, 0, W as i32, TOP - 8, PANEL);
        c.text_fit(20, 11, 24, TEXT, &m.name, W as i32 - 40);
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = W as i32 - 48;
        let meta = join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]);
        for (px, color, text) in [
            (15, DIM, meta.as_str()),
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
        for (px, color, text) in lines.iter().skip(scroll) {
            if y + *px as i32 + 7 > FOOT - 4 {
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
        Some(Step::Unpack { done, total }) => (
            format!("Unpacking {} of {}", size(*done), size(*total)),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Some(Step::Finish) => ("Putting the files in place…".into(), None),
    }
}

fn footer(c: &mut Canvas, left: &str, keys: &str) {
    c.fill(0, FOOT, W as i32, H as i32 - FOOT, PANEL);
    c.text(16, FOOT + 10, 14, DIM, left);
    c.text_right(W as i32 - 16, FOOT + 10, 14, TEXT, keys);
}

fn dialog(c: &mut Canvas, title: &str, text: &str, bar: Option<f32>, keys: &str) {
    let w = 520;
    let lines = c.wrap(17, text, w - 48);
    let h = 70 + lines.len() as i32 * 24 + if bar.is_some() { 30 } else { 0 } + if keys.is_empty() { 0 } else { 34 };
    let (x, y) = ((W as i32 - w) / 2, (H as i32 - h) / 2);
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
    if (w, h) != (W, H) {
        return Err(format!("the screen is {w}x{h}, Port Store draws 640x480"));
    }
    let mut input = input::Input::open();
    let mut app = App::new(store);
    let mut canvas = Canvas::new(W, H);
    loop {
        if app.dirty {
            app.draw(&mut canvas);
            fb.show(&canvas.px);
            app.dirty = false;
        }
        for key in input.poll(Duration::from_millis(100)) {
            if app.key(key) == Flow::Exit && !app.busy {
                return Ok(());
            }
        }
        app.pump();
    }
}

/// Screens as PNGs, for looking at them off the device: the list in each tab, the
/// page of each port given (or the first), a dialog.
pub fn shots(store: Store, dir: &std::path::Path, ids: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut app = App::new(store);
    let mut c = Canvas::new(W, H);
    let mut save = |app: &mut App, name: &str| {
        app.draw(&mut c);
        c.save_png(&dir.join(format!("{name}.png"))).map_err(|e| e.to_string())
    };
    for (tab, name) in TABS {
        app.tab = tab;
        save(&mut app, &format!("list-{}", name.to_lowercase()))?;
    }
    app.tab = Tab::All;
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
