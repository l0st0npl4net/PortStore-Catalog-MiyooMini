//! Port Store's screens, as a terminal on a picture tube: the catalog in tabs, a
//! port's page, windows for questions and jobs (with their event log), the filter and
//! the system's settings.
pub mod canvas;
pub mod crt;
pub mod fb;
pub mod input;
pub mod pictures;
pub mod theme;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::homebrew::{self, Homebrew};
use crate::index::{Index, Port};
use crate::install::{free_bytes, Step};
use crate::needs::{self, Have, Roms};
use crate::{size, State, Store};
use canvas::Canvas;
use crt::{Crt, Fx};
use input::Key;
use pictures::{Image, Pictures};
use theme::{Effects, Look, Palette, PictureMode};

/// The screen Port Store is laid out for at least: the Mini's. Larger ones (the
/// Mini v4 and Flip, 752x560) get the same layout, wider and with more rows.
pub const W: usize = 640;
pub const H: usize = 480;

/// The screen's edge, round everything.
const EDGE: i32 = 6;
/// The title bar, and the tabs (or a page's facts) under it.
const BAR: i32 = 28;
const TOP: i32 = EDGE + 2 * BAR + 6;
/// The bottom bar: the status line, or a page's buttons.
const FOOT: i32 = 30;
/// The column of keys beside the list.
const KEYS_W: i32 = 148;
/// A port's row, and a group's heading above its ports.
const ROW: i32 = 56;
const HEADING: i32 = 28;
const END: i32 = 12;
/// How many ports Left / Right move by.
const PAGE: usize = 6;
const THUMB: i32 = 46;

/// Text sizes: VT323 draws its own pixels at 20.
const SMALL: u16 = 20;
const BODY: u16 = 24;
const NAME: u16 = 30;
const BIG: u16 = 40;

/// The boot screen's length, in seconds.
const BOOT: f32 = 2.2;
/// A window opening: its frame (seconds), then its text typed (characters a second).
const OPEN: f32 = 0.14;
const TYPE: f32 = 900.0;
/// How long the cursor blinks after a press.
const AWAKE: Duration = Duration::from_secs(40);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Games,
    Apps,
    /// What ports run on (love-mini): installed with them, updated here.
    Deps,
    Homebrew,
    Installed,
    Updates,
    More,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::Games, "GAMES"),
    (Tab::Apps, "APPS"),
    (Tab::Homebrew, "HOMEBREW"),
    (Tab::Deps, "DEPENDENCIES"),
    (Tab::Installed, "INSTALLED"),
    (Tab::Updates, "UPDATES"),
    (Tab::More, "MORE"),
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
    /// A homebrew entry, by its place in the list.
    Brew(usize),
}

impl Item {
    fn pickable(&self) -> bool {
        matches!(self, Item::Port(_) | Item::Manual(_) | Item::Brew(_))
    }

    fn height(&self) -> i32 {
        match self {
            Item::Heading(_) => HEADING,
            Item::End => END,
            _ => ROW,
        }
    }
}

/// A row of the filter's window.
#[derive(Clone, Debug, PartialEq)]
enum Choice {
    All(usize),
    Category(String, usize),
    /// A line with a heading (the channels, the kinds of homebrew).
    Line(&'static str),
    Dev(usize),
}

impl Choice {
    fn pickable(&self) -> bool {
        !matches!(self, Choice::Line(_))
    }
}

/// The rows of the system's window.
const SYSTEM: [&str; 6] = ["THEME", "EFFECTS", "PICTURES", "DEV BUILDS", "PORT STORE IN GAMES", "ONIONOS PORTS"];

#[derive(Clone, Debug)]
enum Action {
    Install(String),
    /// Move the game files found in import/ for a port into place.
    Place(String),
    /// Several ports, one after another (marked ones, or every update).
    InstallMany(Vec<String>),
    Remove(String),
    Refresh,
    /// A homebrew entry, by slug.
    InstallBrew(String),
    RemoveBrew(String),
    RefreshBrew,
}

#[derive(Clone, Debug)]
enum View {
    List,
    Details { id: String, scroll: usize },
    Manual { at: usize, scroll: usize },
    Confirm { text: String, yes: Action },
    /// A job on screen: whether B can stop it (downloads and checks only).
    Busy { title: String, step: Option<Step>, stoppable: bool },
    Message { title: String, text: String },
    /// Select's window over the list: a category, the development builds.
    Filter { at: usize },
    /// L2's window: the theme, the tube's effects, pictures, development builds,
    /// Port Store in Games.
    System { at: usize },
    /// A homebrew entry's page.
    Brew { slug: String, scroll: usize },
    /// The ports new in the catalog since the last look.
    Arrivals { at: usize },
}

impl View {
    /// A window over a page (as against a page).
    fn over(&self) -> u8 {
        match self {
            View::List | View::Details { .. } | View::Manual { .. } => 0,
            View::Confirm { .. } => 1,
            View::Busy { .. } => 2,
            View::Message { .. } => 3,
            View::Filter { .. } => 4,
            View::System { .. } => 5,
            View::Arrivals { .. } => 6,
            View::Brew { .. } => 0,
        }
    }

    fn page(&self) -> u8 {
        match self {
            View::Details { .. } => 1,
            View::Manual { .. } => 2,
            View::Brew { .. } => 3,
            _ => 0,
        }
    }
}

enum Msg {
    Step(Step),
    /// A ROM being read to check a port's needs.
    Checking(String),
    Needs(String, u64, Vec<Have>, Vec<Option<needs::Placement>>),
    Done(Result<Finished, String>),
    Index(Result<Index, String>),
    /// The catalog downloaded on start, without a word on screen.
    Quiet(Result<Index, String>),
    /// The port a queue of installs is on: (place, of how many, name).
    Queue(usize, usize, String),
    /// The homebrew list downloaded (quietly when asked for by the tab).
    Homebrew(Result<Homebrew, String>, bool),
}

/// A job done.
enum Finished {
    Installed(String),
    /// Files moved from import/ for a port: how many.
    Placed(String, usize),
    /// A queue: those installed, those not (with why).
    Many(Vec<String>, Vec<(String, String)>),
    Removed,
    BrewInstalled(String),
    BrewRemoved,
}

#[derive(PartialEq, Eq)]
pub enum Flow {
    Stay,
    Exit,
}

/// What the screen shows, in short: a change of it is played on the tube.
#[derive(Clone, Debug, PartialEq)]
struct Sig {
    page: u8,
    over: u8,
    tab: Tab,
    filter: Option<String>,
    newest: bool,
    theme: String,
    dev: bool,
}

/// The tube's moment for a change of screen.
fn fx_between(a: &Sig, b: &Sig) -> Option<Fx> {
    if a.theme != b.theme {
        Some(Fx::Degauss)
    } else if a.page != b.page || a.tab != b.tab {
        // A port's page opens as another tab does: torn bands and flicker.
        Some(Fx::Tear)
    } else if a.over != b.over || a.filter != b.filter || a.newest != b.newest || a.dev != b.dev {
        Some(Fx::Blip)
    } else {
        None
    }
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
    /// Per tab: ports of one category only (Games and Apps; Select's window).
    filter: [Option<String>; TABS.len()],
    /// Per tab: the row picked and the first row on show, by place in `items()`.
    pick: [usize; TABS.len()],
    top: [usize; TABS.len()],
    /// Homebrew Hub's list, the state of each entry, and whether it is being
    /// downloaded (asked for once a session, when the tab is first shown).
    brews: Homebrew,
    brew_states: HashMap<String, State>,
    brews_loading: bool,
    brews_asked: bool,
    /// Ports new in the catalog, to announce; and the day each port was first seen.
    arrivals: Vec<String>,
    seen: HashMap<String, String>,
    /// Port Store's own entry in Games → MiyooPorts.
    in_games: bool,
    /// Games > Ports hidden, its ports in MiyooPorts' Ports folder.
    ports_hidden: bool,
    pictures: Pictures,
    /// Ports marked (X) for installing together (Start).
    marked: std::collections::BTreeSet<String>,
    /// Newest releases first (R2), instead of A to Z.
    newest: bool,
    /// The catalog could not be downloaded on start (no Wi-Fi).
    offline: bool,
    /// A catalog has been downloaded since Port Store started.
    online: bool,
    /// Port Store has updated itself: start the new one on leaving.
    pub restart: bool,
    /// A catalog downloaded while a job ran, taken in after it.
    held_index: Option<Index>,
    /// A newer catalog came: offer a newer Port Store once the list is on screen.
    offer_pending: bool,
    /// Per port, the number of its newest check of needs (older results are dropped).
    needs_gen: HashMap<String, u64>,
    /// How far each page can scroll, as last drawn.
    scroll_max: usize,
    view: View,
    /// Where to go back to from a message or a job.
    under: Vec<View>,
    jobs: (Sender<Msg>, Receiver<Msg>),
    busy: bool,
    cancel: Arc<AtomicBool>,
    pub dirty: bool,
    look: Look,
    /// The time the screen is drawn for, and since when it shows what it shows (for
    /// windows opening and text being typed).
    now: Instant,
    since: Instant,
    /// Everything as when done moving (screens taken off the device, tests).
    still: bool,
    /// When Port Store started: the boot screen, until it is done or a key is pressed.
    boot: Option<Instant>,
    last_key: Instant,
    drawn_at: Instant,
    /// A job's event log: (seconds into the job, kind of step, line).
    log: Vec<(u64, u32, String)>,
    log_at: Instant,
}

impl App {
    pub fn new(store: Store) -> App {
        let index = store.load_index();
        store.repair(&index);
        let look = Look::load(&store.card.home);
        let store = Arc::new(store);
        let now = Instant::now();
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
            filter: Default::default(),
            marked: Default::default(),
            newest: false,
            offline: false,
            online: false,
            restart: false,
            held_index: None,
            offer_pending: false,
            needs_gen: HashMap::new(),
            scroll_max: 0,
            pick: [0; TABS.len()],
            top: [0; TABS.len()],
            brews: Homebrew::default(),
            brew_states: HashMap::new(),
            brews_loading: false,
            brews_asked: false,
            arrivals: Vec::new(),
            seen: HashMap::new(),
            in_games: false,
            ports_hidden: false,
            view: View::List,
            under: Vec::new(),
            jobs: channel(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: true,
            look,
            now,
            since: now,
            still: false,
            boot: None,
            last_key: now,
            drawn_at: now,
            log: Vec::new(),
            log_at: now,
        };
        app.brews = app.store.load_homebrew();
        app.in_games = crate::miyooports::self_entry(&app.store.card.root);
        app.ports_hidden = crate::miyooports::ports_hidden(&app.store.card.root);
        app.look_for_arrivals();
        app.recount();
        app
    }

    /// Ports new in the catalog on show since the last look, added to those to
    /// announce.
    fn look_for_arrivals(&mut self) {
        let (new, seen) = self.store.arrivals(&self.index);
        self.seen = seen;
        for id in new {
            if !self.arrivals.contains(&id) {
                self.arrivals.push(id);
            }
        }
    }

    /// Whether a port came into the catalog in the last 14 days (on this card).
    fn is_new(&self, id: &str) -> bool {
        self.seen.get(id).is_some_and(|d| !d.is_empty() && *d >= crate::days_ago(14))
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
        let records = self.store.homebrew_records();
        self.brew_states = self
            .brews
            .entries
            .iter()
            .map(|e| (e.slug.clone(), homebrew::state_of(records.get(&e.slug), e)))
            .collect();
        self.dirty = true;
    }

    fn brew_state(&self, slug: &str) -> State {
        self.brew_states.get(slug).cloned().unwrap_or(State::Absent)
    }

    /// The Homebrew tab shown: its list downloaded once a session, unless the one at
    /// hand is today's.
    fn tab_shown(&mut self) {
        if self.tab != Tab::Homebrew || self.brews_asked || self.store.homebrew_url().is_none() {
            return;
        }
        self.brews_asked = true;
        if self.brews.generated.get(..10) == Some(crate::today().as_str()) {
            return;
        }
        self.brews_loading = true;
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Homebrew(store.refresh_homebrew(&AtomicBool::new(false)), true));
        });
    }

    fn state(&self, id: &str) -> State {
        self.states.get(id).cloned().unwrap_or(State::Absent)
    }

    fn tab_at(&self) -> usize {
        TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0)
    }

    /// Ports of the tab, before its filter: games or apps, development builds only
    /// when asked for.
    fn of_tab(&self, tab: Tab) -> impl Iterator<Item = (usize, &Port)> + '_ {
        let dev = self.look.dev;
        // what ports run on has a tab of its own, not among games or apps
        let deps = self.index.dependencies();
        self.index.ports.iter().enumerate().filter(move |(_, p)| {
            let dep = deps.contains(p.id.as_str());
            (dev || !p.dev)
                && match tab {
                    Tab::Games => !p.is_app() && !dep,
                    Tab::Apps => p.is_app() && !dep,
                    Tab::Deps => dep,
                    _ => false,
                }
        })
    }

    /// The rows of the tab on show. Games and apps are in alphabetical order, the
    /// ports of a group (a collection, one porter's games) together under its heading,
    /// in the place of the group's name.
    fn items(&self) -> Vec<Item> {
        let filter = &self.filter[self.tab_at()];
        let mut shown: Vec<(usize, &Port)> = match self.tab {
            Tab::Games | Tab::Apps => self
                .of_tab(self.tab)
                .filter(|(_, p)| filter.as_ref().is_none_or(|g| p.categories.contains(g)))
                .collect(),
            Tab::Deps => self.of_tab(Tab::Deps).collect(),
            Tab::Installed => self
                .index
                .ports
                .iter()
                .enumerate()
                .filter(|(_, p)| matches!(self.state(&p.id), State::Installed(_) | State::Update(_)))
                .collect(),
            Tab::Updates => self
                .index
                .ports
                .iter()
                .enumerate()
                .filter(|(_, p)| matches!(self.state(&p.id), State::Update(_)))
                .collect(),
            Tab::More => return (0..self.index.manual.len()).map(Item::Manual).collect(),
            Tab::Homebrew => return self.brew_items(),
        };
        let grouped = matches!(self.tab, Tab::Games | Tab::Apps) && !self.newest;
        if self.newest {
            shown.sort_by(|(_, a), (_, b)| b.date.cmp(&a.date).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            let mut out = shown.into_iter().map(|(i, _)| Item::Port(i)).collect();
            self.installed_brews(&mut out);
            return out;
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
        self.installed_brews(&mut out);
        out
    }

    /// In the Installed tab: homebrew installed with Port Store, after the ports,
    /// under a heading.
    fn installed_brews(&self, out: &mut Vec<Item>) {
        if self.tab == Tab::Installed {
            let mut brews: Vec<usize> = (0..self.brews.entries.len())
                .filter(|&i| matches!(self.brew_state(&self.brews.entries[i].slug), State::Installed(_) | State::Update(_)))
                .collect();
            brews.sort_by_key(|&i| self.brews.entries[i].title.to_lowercase());
            if !brews.is_empty() {
                out.push(Item::Heading("Homebrew".into()));
                out.extend(brews.into_iter().map(Item::Brew));
            }
        }
    }

    /// The Homebrew tab's rows: of the system or kind chosen, A to Z or newest in
    /// Homebrew Hub first.
    fn brew_items(&self) -> Vec<Item> {
        let filter = self.filter[self.tab_at()].as_deref();
        let mut shown: Vec<(usize, &homebrew::Entry)> = self
            .brews
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| filter.is_none_or(|f| e.system == f || e.kind == f))
            .collect();
        match self.newest {
            true => shown.sort_by_cached_key(|(_, e)| (std::cmp::Reverse(e.added.clone()), e.title.to_lowercase())),
            false => shown.sort_by_cached_key(|(_, e)| e.title.to_lowercase()),
        }
        shown.into_iter().map(|(i, _)| Item::Brew(i)).collect()
    }

    /// Select's window for the tab: all, each category with how many it has, and the
    /// development builds when the catalog has any for it.
    fn choices(&self) -> Vec<Choice> {
        if self.tab == Tab::Homebrew {
            let count = |f: &dyn Fn(&homebrew::Entry) -> bool| self.brews.entries.iter().filter(|e| f(e)).count();
            let mut out = vec![Choice::All(self.brews.entries.len())];
            for sys in ["GBA", "GBC", "GB", "NES"] {
                let n = count(&|e| e.system == sys);
                if n > 0 {
                    out.push(Choice::Category(sys.into(), n));
                }
            }
            out.push(Choice::Line("KIND"));
            for kind in ["game", "demo", "tool", "music"] {
                let n = count(&|e| e.kind == kind);
                if n > 0 {
                    out.push(Choice::Category(kind.into(), n));
                }
            }
            return out;
        }
        let dev_count = self.index.ports.iter().filter(|p| p.dev && p.is_app() == (self.tab == Tab::Apps)).count();
        let shown: Vec<&Port> = self.of_tab(self.tab).map(|(_, p)| p).collect();
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for p in &shown {
            for c in p.categories.iter().filter(|c| *c != "app") {
                *counts.entry(c.as_str()).or_default() += 1;
            }
        }
        let mut out = vec![Choice::All(shown.len())];
        out.extend(counts.into_iter().map(|(c, n)| Choice::Category(c.to_string(), n)));
        if dev_count > 0 {
            out.push(Choice::Line("CHANNEL"));
            out.push(Choice::Dev(dev_count));
        }
        out
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

    /// The look in force: settings.json's, the theme the palette.
    pub fn palette(&self) -> Palette {
        self.look.palette()
    }

    /// How much of a picture tube is played (none for the classic look).
    pub fn effects(&self) -> Effects {
        match self.palette().mono {
            true => self.look.effects,
            false => Effects::Off,
        }
    }

    fn sig(&self) -> Sig {
        let base = match self.view.over() {
            0 => &self.view,
            _ => self.under.iter().rev().find(|v| v.over() == 0).unwrap_or(&View::List),
        };
        Sig {
            page: base.page(),
            over: self.view.over(),
            tab: self.tab,
            filter: self.filter[self.tab_at()].clone(),
            newest: self.newest,
            theme: self.look.theme.clone(),
            dev: self.look.dev,
        }
    }

    /// Seconds the screen has shown what it shows.
    fn age(&self) -> f32 {
        match self.still {
            true => 1e6,
            false => self.now.saturating_duration_since(self.since).as_secs_f32(),
        }
    }

    fn booting(&self) -> bool {
        self.boot.is_some_and(|at| self.now.saturating_duration_since(at).as_secs_f32() < BOOT)
    }

    /// Whether the cursor blinks: the full effects, and a press not long ago.
    fn blinking(&self) -> bool {
        !self.still && self.effects() == Effects::Full && self.now.saturating_duration_since(self.last_key) < AWAKE
    }

    /// When the screen is to be drawn again without anything new: text being typed,
    /// a window opening, the spinner of a job, the cursor.
    fn next_draw(&self) -> Option<Duration> {
        if self.still {
            return None;
        }
        let over = self.view.over();
        // A window opening (a job's for its frame only, then its spinner).
        let opening = if over == 2 { OPEN * 2.0 } else { 1.2 };
        if self.booting() || (over != 0 && self.age() < opening) {
            Some(crt::FRAME)
        } else if over == 2 {
            Some(Duration::from_millis(125))
        } else if self.blinking() {
            Some(Duration::from_millis(530))
        } else {
            None
        }
    }

    // ---- input ----

    pub fn key(&mut self, key: Key) -> Flow {
        self.dirty = true;
        self.last_key = self.now;
        if self.booting() {
            self.boot = None;
            return Flow::Stay;
        }
        match self.view.clone() {
            View::List => return self.list_key(key),
            View::Details { id, scroll } => self.details_key(key, id, scroll),
            View::Manual { at, scroll } => match key {
                Key::B => self.view = View::List,
                Key::Up => self.view = View::Manual { at, scroll: scroll.saturating_sub(1) },
                Key::Down => self.view = View::Manual { at, scroll: (scroll + 1).min(self.scroll_max) },
                _ => {}
            },
            View::Confirm { yes, .. } => match key {
                Key::A => self.start(yes),
                Key::B => self.back(),
                _ => {}
            },
            View::Busy { step, stoppable, .. } => {
                let now = stoppable && matches!(step, None | Some(Step::Download { .. }) | Some(Step::Check));
                if key == Key::B && now {
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
            View::Filter { at } => self.filter_key(key, at),
            View::System { at } => self.system_key(key, at),
            View::Brew { slug, scroll } => self.brew_key(key, slug, scroll),
            View::Arrivals { at } => match key {
                Key::Up => self.view = View::Arrivals { at: at.saturating_sub(1) },
                Key::Down => self.view = View::Arrivals { at: (at + 1).min(self.arrivals.len().saturating_sub(1)) },
                Key::A => {
                    let id = self.arrivals.get(at).cloned();
                    self.arrivals.clear();
                    self.view = View::List;
                    if let Some(id) = id.filter(|id| self.port(id).is_some()) {
                        self.open(id);
                    }
                }
                Key::B => {
                    self.arrivals.clear();
                    self.view = View::List;
                }
                _ => {}
            },
        }
        Flow::Stay
    }

    fn brew_key(&mut self, key: Key, slug: String, scroll: usize) {
        let Some(e) = self.brews.entry(&slug).cloned() else {
            self.view = View::List;
            return;
        };
        let state = self.brew_state(&slug);
        match key {
            Key::B => self.view = View::List,
            Key::Up => self.view = View::Brew { slug, scroll: scroll.saturating_sub(1) },
            Key::Down => self.view = View::Brew { slug, scroll: (scroll + 1).min(self.scroll_max) },
            Key::A => {
                let verb = match state {
                    State::Installed(_) => "Reinstall",
                    State::Update(_) => "Update",
                    _ => "Install",
                };
                let text = format!(
                    "{verb} {} ({})? It downloads {} into Roms/{}.",
                    e.title,
                    e.system_name(),
                    size(e.file.size),
                    e.folder()
                );
                self.under.push(self.view.clone());
                self.view = View::Confirm { text, yes: Action::InstallBrew(slug) };
            }
            Key::X if matches!(state, State::Installed(_) | State::Update(_)) => {
                let text = format!("Remove {} from Roms/{}? Its saves stay.", e.title, e.folder());
                self.under.push(self.view.clone());
                self.view = View::Confirm { text, yes: Action::RemoveBrew(slug) };
            }
            _ => {}
        }
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
            Key::L1 => {
                self.tab = TABS[(t + TABS.len() - 1) % TABS.len()].0;
                self.tab_shown();
            }
            Key::R1 => {
                self.tab = TABS[(t + 1) % TABS.len()].0;
                self.tab_shown();
            }
            Key::Select if matches!(self.tab, Tab::Games | Tab::Apps | Tab::Homebrew) => {
                // The window opens on the filter in force.
                let choices = self.choices();
                let at = match &self.filter[t] {
                    Some(g) => choices.iter().position(|c| matches!(c, Choice::Category(x, _) if x == g)),
                    None => None,
                };
                self.view = View::Filter { at: at.unwrap_or(0) };
            }
            Key::L2 => self.view = View::System { at: 0 },
            Key::Y if self.tab == Tab::Homebrew => self.start(Action::RefreshBrew),
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
                Item::Brew(at) => self.view = View::Brew { slug: self.brews.entries[at].slug.clone(), scroll: 0 },
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

    fn filter_key(&mut self, key: Key, at: usize) {
        let choices = self.choices();
        let n = choices.len();
        let step = |by: usize| {
            let mut i = at;
            for _ in 0..n {
                i = (i + by) % n;
                if choices[i].pickable() {
                    break;
                }
            }
            i
        };
        let t = self.tab_at();
        match key {
            Key::Up => self.view = View::Filter { at: step(n - 1) },
            Key::Down => self.view = View::Filter { at: step(1) },
            Key::A => match choices.get(at) {
                Some(Choice::All(_)) => self.set_filter(t, None),
                Some(Choice::Category(c, _)) => self.set_filter(t, Some(c.clone())),
                Some(Choice::Dev(_)) => {
                    self.look.dev = !self.look.dev;
                    self.save_look();
                }
                _ => {}
            },
            Key::B | Key::Select => self.view = View::List,
            _ => {}
        }
    }

    fn set_filter(&mut self, t: usize, filter: Option<String>) {
        self.filter[t] = filter;
        self.pick[t] = 0;
        self.top[t] = 0;
        self.view = View::List;
    }

    fn system_key(&mut self, key: Key, at: usize) {
        let n = SYSTEM.len();
        match key {
            Key::Up => self.view = View::System { at: (at + n - 1) % n },
            Key::Down => self.view = View::System { at: (at + 1) % n },
            Key::Left => self.change(at, -1),
            Key::Right | Key::A => self.change(at, 1),
            Key::B | Key::L2 | Key::Select => self.view = View::List,
            _ => {}
        }
    }

    /// The system's setting `row` one step on (or back), kept in settings.json.
    fn change(&mut self, row: usize, by: i32) {
        let turn = |n: usize, at: usize| (at as i32 + by).rem_euclid(n as i32) as usize;
        match row {
            0 => {
                let themes = self.look.themes();
                let at = themes.iter().position(|t| *t == self.look.theme).unwrap_or(0);
                self.look.theme = themes[turn(themes.len(), at)].to_string();
            }
            1 => {
                let at = theme::EFFECTS.iter().position(|(e, ..)| *e == self.look.effects).unwrap_or(0);
                self.look.effects = theme::EFFECTS[turn(theme::EFFECTS.len(), at)].0;
            }
            2 => {
                let at = theme::PICTURES.iter().position(|(m, ..)| *m == self.look.pictures).unwrap_or(0);
                self.look.pictures = theme::PICTURES[turn(theme::PICTURES.len(), at)].0;
            }
            3 => self.look.dev = !self.look.dev,
            5 => {
                // Not a look either: Games > Ports hidden (its ports in MiyooPorts) or
                // shown, at once.
                let root = &self.store.card.root;
                let hidden = crate::miyooports::ports_hidden(root);
                let done = match hidden {
                    true => crate::miyooports::show_ports(root),
                    false => crate::miyooports::hide_ports(root),
                };
                match done {
                    Ok(_) => self.ports_hidden = !hidden,
                    Err(e) => {
                        self.view = View::List;
                        self.message("Not changed", &e.to_string());
                    }
                }
                return;
            }
            _ => {
                // Not a look: Port Store's entry in MiyooPorts is put on the card or
                // taken off it at once.
                match crate::miyooports::set_self_entry(&self.store.card.root, !self.in_games) {
                    Ok(()) => self.in_games = !self.in_games,
                    Err(e) => {
                        self.view = View::List;
                        self.message("Not changed", &e.to_string());
                    }
                }
                return;
            }
        }
        self.save_look();
    }

    fn save_look(&self) {
        if let Err(e) = self.look.save(&self.store.card.home) {
            eprintln!("portstore: settings.json: {e}");
        }
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
            Key::Down => self.view = View::Details { id, scroll: (scroll + 1).min(self.scroll_max) },
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
                let first = match self.store.requirements(&self.index, &port) {
                    Ok(first) => first,
                    Err(missing) => {
                        return self.message(
                            "Not in the catalog",
                            &format!("{} runs on {missing}, which the catalog does not have.", port.name),
                        );
                    }
                };
                self.under.push(self.view.clone());
                if first.is_empty() {
                    self.view = View::Confirm { text, yes: Action::Install(id) };
                } else {
                    let names: Vec<String> = first.iter().map(|p| format!("{} {}", p.name, p.version)).collect();
                    let bytes: u64 = first.iter().map(|p| p.file.size).sum();
                    text += &format!(" It runs on {}: installed first ({} more to download).", names.join(", "), size(bytes));
                    let mut ids: Vec<String> = first.into_iter().map(|p| p.id).collect();
                    ids.push(id);
                    self.view = View::Confirm { text, yes: Action::InstallMany(ids) };
                }
            }
            Key::X if matches!(state, State::Installed(_) | State::Update(_)) && id != crate::SELF_ID => {
                let users = self.store.required_by(&self.index, &id);
                if !users.is_empty() {
                    return self.message(
                        "Still needed",
                        &format!("{} runs {}. Remove them first.", port.name, users.join(", ")),
                    );
                }
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
        let gen = self.needs_gen.get(&id).map_or(1, |g| g + 1);
        self.needs_gen.insert(id.clone(), gen);
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        std::thread::spawn(move || {
            let mut roms = Roms::open(&store.card.home);
            let have = needs::check(&store.card.root, &port, &mut roms, &mut |path| {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let _ = tx.send(Msg::Checking(name.into_owned()));
            });
            let found = needs::importable(&store.card.root, &store.card.home, &port, &have, &mut roms);
            let _ = tx.send(Msg::Needs(id, gen, have, found));
        });
    }

    fn start(&mut self, action: Action) {
        if self.busy {
            return;
        }
        // The catalog may have changed under a dialog: a port gone from it is said so.
        if let Action::Install(id) = &action {
            if self.port(id).is_none() {
                if matches!(self.view, View::Confirm { .. }) {
                    self.view = self.under.pop().unwrap_or(View::List);
                }
                return self.message("Not in the catalog", "This port is no longer in the catalog.");
            }
        }
        if let Action::Remove(id) = &action {
            if id == crate::SELF_ID {
                return;
            }
        }
        self.busy = true;
        self.cancel.store(false, Ordering::Relaxed);
        let store = self.store.clone();
        let tx = self.jobs.0.clone();
        let cancel = self.cancel.clone();
        let name = |id: &str| self.port(id).map_or(id.to_string(), |p| p.name.clone());
        let brew_name = |slug: &str| self.brews.entry(slug).map_or(slug.to_string(), |e| e.title.clone());
        let (title, first) = match &action {
            Action::InstallBrew(slug) => (format!("Installing {}", brew_name(slug)), "LINK OPEN".to_string()),
            Action::RemoveBrew(slug) => (format!("Removing {}", brew_name(slug)), "REMOVE FILES".to_string()),
            Action::RefreshBrew => ("Downloading the homebrew list".to_string(), "FETCH HOMEBREW.JSON".to_string()),
            Action::Install(id) => (format!("Installing {}", name(id)), "LINK OPEN".to_string()),
            Action::Remove(id) => (format!("Removing {}", name(id)), "REMOVE FILES".to_string()),
            Action::Refresh => ("Downloading the catalog".to_string(), "FETCH INDEX.JSON".to_string()),
            Action::InstallMany(ids) => (format!("Installing {} ports", ids.len()), format!("QUEUE OF {}", ids.len())),
            Action::Place(_) => ("Putting your files in place".to_string(), "MOVE YOUR FILES".to_string()),
        };
        if matches!(self.view, View::Confirm { .. }) {
            self.view = self.under.pop().unwrap_or(View::List);
        }
        self.under.push(self.view.clone());
        let stoppable = matches!(
            action,
            Action::Install(_) | Action::InstallMany(_) | Action::Refresh | Action::InstallBrew(_) | Action::RefreshBrew
        );
        self.view = View::Busy { title, step: None, stoppable };
        self.log.clear();
        self.log_at = self.now;
        self.note(0, first);
        let port = match &action {
            Action::Install(id) | Action::Remove(id) => self.port(id).cloned(),
            _ => None,
        };
        let brew = match &action {
            Action::InstallBrew(slug) => self.brews.entry(slug).cloned(),
            _ => None,
        };
        let placements: Vec<needs::Placement> = match &action {
            Action::Place(id) => self.imports.get(id).into_iter().flatten().flatten().cloned().collect(),
            _ => Vec::new(),
        };
        let queue: Vec<Port> = match &action {
            // Port Store itself last: what is left of the queue still runs on the old one.
            // What a port runs on goes ahead of it (once).
            Action::InstallMany(ids) => {
                let mut q: Vec<Port> = Vec::new();
                for p in ids.iter().filter_map(|id| self.port(id)) {
                    for need in self.store.requirements(&self.index, p).unwrap_or_default() {
                        if !q.iter().any(|x| x.id == need.id) {
                            q.push(need);
                        }
                    }
                    if !q.iter().any(|x| x.id == p.id) {
                        q.push(p.clone());
                    }
                }
                q.sort_by_key(|p| p.id == crate::SELF_ID);
                q
            }
            _ => Vec::new(),
        };
        std::thread::spawn(move || {
            // A job that panics still ends: the screen is never left busy for good.
            let ended = tx.clone();
            let job = std::panic::AssertUnwindSafe(move || job(store, tx, cancel, action, port, brew, placements, queue));
            if std::panic::catch_unwind(job).is_err() {
                let _ = ended.send(Msg::Done(Err("Port Store ran into an error it did not expect: see cache/portstore.log".into())));
            }
        });
    }

    /// A line of the job's event log; a step of the same kind as the last line takes
    /// its place (a download's progress).
    fn note(&mut self, kind: u32, text: String) {
        let secs = self.now.saturating_duration_since(self.log_at).as_secs();
        match self.log.last_mut() {
            Some(last) if last.1 == kind => *last = (last.0, kind, text),
            _ => self.log.push((secs, kind, text)),
        }
    }
}

/// A job's work, on its own thread; its end is sent as Msg::Done (or Msg::Index).
#[allow(clippy::too_many_arguments)]
fn job(
    store: Arc<Store>,
    tx: Sender<Msg>,
    cancel: Arc<AtomicBool>,
    action: Action,
    port: Option<Port>,
    brew: Option<homebrew::Entry>,
    placements: Vec<needs::Placement>,
    queue: Vec<Port>,
) {
    let done = match action {
        Action::Refresh => {
            let _ = tx.send(Msg::Index(store.refresh(&cancel)));
            return;
        }
        Action::RefreshBrew => {
            let _ = tx.send(Msg::Homebrew(store.refresh_homebrew(&cancel), false));
            return;
        }
        Action::InstallBrew(slug) => match brew {
            Some(e) => store
                .install_homebrew(&e, &cancel, &mut |step| {
                    let _ = tx.send(Msg::Step(step));
                })
                .map(|_| Finished::BrewInstalled(slug)),
            None => Err(std::io::Error::other(format!("{slug} is not in the homebrew list"))),
        },
        Action::RemoveBrew(slug) => store.remove_homebrew(&slug).map(|_| Finished::BrewRemoved),
        Action::Install(id) => match port {
            Some(port) => store
                .install(&port, &cancel, &mut |step| {
                    let _ = tx.send(Msg::Step(step));
                })
                .map(|_| Finished::Installed(port.id.clone())),
            None => Err(std::io::Error::other(format!("{id} is not in the catalog"))),
        },
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
                // what it runs on did not install: neither does it
                if let Some(need) = port.requires.iter().find(|r| failed.iter().any(|(f, _): &(String, String)| f == *r)) {
                    failed.push((port.id.clone(), format!("needs {need}, which did not install")));
                    continue;
                }
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
}

impl App {
    /// Take in what the jobs have sent, and pictures downloaded.
    pub fn pump(&mut self) {
        if self.pictures.pump() {
            self.dirty = true;
        }
        if self.offer_pending && matches!(self.view, View::List) && !self.busy && !self.booting() {
            self.offer_self_update();
            self.dirty = true;
        }
        // New ports, once the list is on screen and nothing else is offered.
        let quiet = matches!(self.view, View::List) && !self.busy && !self.booting() && !self.offer_pending;
        if quiet && !self.arrivals.is_empty() && !self.still {
            self.arrivals.retain(|id| self.index.port(id).is_some());
            if !self.arrivals.is_empty() {
                self.view = View::Arrivals { at: 0 };
                self.dirty = true;
            }
        }
        while let Ok(msg) = self.jobs.1.try_recv() {
            self.dirty = true;
            match msg {
                Msg::Step(s) => {
                    let (kind, line, _) = step_line(Some(&s));
                    self.note(kind, line);
                    if let View::Busy { step, .. } = &mut self.view {
                        *step = Some(s);
                    }
                }
                Msg::Checking(name) => self.checking = Some(name),
                Msg::Queue(n, of, name) => {
                    self.note(100 + n as u32, format!("PORT {n}/{of}: {}", name.to_uppercase()));
                    if let View::Busy { title, step, .. } = &mut self.view {
                        *title = format!("Installing {n} of {of}: {name}");
                        *step = None;
                    }
                }
                Msg::Quiet(result) => match result {
                    // Not under a running job: it may be installing a port this
                    // catalog changes. Taken in once the job is done.
                    Ok(index) if self.busy => self.held_index = Some(index),
                    Ok(index) => self.take_index(index),
                    Err(_) => {
                        self.offline = true;
                        self.offer_pending = true;
                    }
                },
                Msg::Needs(id, gen, have, found) => {
                    // Only the newest check of a port counts.
                    if self.needs_gen.get(&id) == Some(&gen) {
                        self.checking = None;
                        self.imports.insert(id.clone(), found);
                        self.needs.insert(id, have);
                    }
                }
                Msg::Homebrew(result, quiet) => {
                    self.brews_loading = false;
                    if !quiet {
                        self.busy = false;
                        self.back();
                    }
                    match result {
                        Ok(list) => {
                            self.brews = list;
                            self.recount();
                            if !quiet {
                                let n = self.brews.entries.len();
                                self.message("Homebrew", &format!("{n} entries from Homebrew Hub."));
                            }
                        }
                        Err(e) if !quiet => self.message("Homebrew list not downloaded", &e),
                        Err(e) => eprintln!("portstore: homebrew list: {e}"),
                    }
                }
                Msg::Index(result) => {
                    self.busy = false;
                    self.back();
                    self.held_index = None;
                    match result {
                        Ok(index) => {
                            self.offline = false;
                            self.online = true;
                            self.store.repair(&index);
                            self.index = index;
                            self.look_for_arrivals();
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
                    if let Some(index) = self.held_index.take() {
                        self.take_index(index);
                    }
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
                        Ok(Finished::BrewInstalled(slug)) => {
                            let (title, system, folder) = self
                                .brews
                                .entry(&slug)
                                .map_or((slug.clone(), "", ""), |e| (e.title.clone(), e.system_name(), e.folder()));
                            self.message("Installed", &format!("{title} is in Games → {system} (Roms/{folder})."));
                        }
                        Ok(Finished::BrewRemoved) => {
                            self.view = View::List;
                            self.message("Removed", "The ROM and its box art are off the card; saves stay.");
                        }
                        Err(e) if e == "cancelled" => {}
                        Err(e) => self.message("Something went wrong", &e),
                    }
                }
            }
        }
    }

    /// A catalog downloaded quietly, taken in.
    fn take_index(&mut self, index: Index) {
        self.offline = false;
        self.online = true;
        self.store.repair(&index);
        self.index = index;
        self.look_for_arrivals();
        self.recount();
        self.offer_pending = true;
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

    /// A newer Port Store in the catalog: offered once, over the list, as soon as the
    /// list is on screen and nothing runs.
    fn offer_self_update(&mut self) {
        if !self.offer_pending || !matches!(self.view, View::List) || self.busy {
            return;
        }
        self.offer_pending = false;
        let Some(me) = self.port(crate::SELF_ID).cloned() else { return };
        let State::Update(have) = self.state(crate::SELF_ID) else { return };
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

    /// The catalog's link, as the screen says it.
    fn link(&self) -> &'static str {
        if self.store.index_url.is_empty() {
            "NONE"
        } else if self.online {
            "ONLINE"
        } else if self.offline {
            "OFFLINE"
        } else {
            "DIALING"
        }
    }

    fn updates(&self) -> usize {
        self.states.values().filter(|s| matches!(s, State::Update(_))).count()
    }

    // ---- drawing ----

    pub fn draw(&mut self, c: &mut Canvas) {
        let p = self.palette();
        c.mono = p.mono;
        c.clear(p.bg);
        if self.booting() && matches!(self.view, View::List) {
            return self.draw_boot(c, &p);
        }
        match self.view.clone() {
            View::List => self.draw_list(c, &p),
            View::Details { id, scroll } => self.draw_details(c, &p, &id, scroll),
            View::Manual { at, scroll } => self.draw_manual(c, &p, at, scroll),
            View::Confirm { text, .. } => {
                self.draw_under(c, &p);
                self.dialog(c, &p, "Confirm", &text, &[("A", "YES"), ("B", "NO")]);
            }
            View::Busy { title, step, stoppable } => {
                self.draw_under(c, &p);
                let stoppable = stoppable && matches!(step, None | Some(Step::Download { .. }) | Some(Step::Check));
                self.draw_busy(c, &p, &title, step.as_ref(), stoppable);
            }
            View::Message { title, text } => {
                self.draw_under(c, &p);
                self.dialog(c, &p, &title, &text, &[("A", "OK")]);
            }
            View::Filter { at } => {
                self.draw_list(c, &p);
                c.shade(0, 0, c.w as i32, c.h as i32, p.bg, 150);
                self.draw_filter(c, &p, at);
            }
            View::System { at } => {
                self.draw_list(c, &p);
                c.shade(0, 0, c.w as i32, c.h as i32, p.bg, 150);
                self.draw_system(c, &p, at);
            }
            View::Brew { slug, scroll } => self.draw_brew(c, &p, &slug, scroll),
            View::Arrivals { at } => {
                self.draw_list(c, &p);
                c.shade(0, 0, c.w as i32, c.h as i32, p.bg, 150);
                self.draw_arrivals(c, &p, at);
            }
        }
    }

    fn draw_under(&mut self, c: &mut Canvas, p: &Palette) {
        let saved = std::mem::replace(&mut self.view, View::List);
        if let Some(under) = self.under.last().cloned() {
            self.view = under;
            if self.view.over() == 0 {
                self.draw(c);
            }
        } else {
            self.draw_list(c, p);
        }
        self.view = saved;
        // Dim what is under the window.
        c.shade(0, 0, c.w as i32, c.h as i32, p.bg, 170);
    }

    /// The boot screen: Port Store's name, then what it finds, typed.
    fn draw_boot(&mut self, c: &mut Canvas, p: &Palette) {
        let age = self.boot.map_or(BOOT, |at| self.now.saturating_duration_since(at).as_secs_f32());
        let installed = self.states.values().filter(|s| matches!(s, State::Installed(_) | State::Update(_))).count();
        let lines = [
            format!("PORT STORE V{}", env!("CARGO_PKG_VERSION")),
            String::new(),
            format!("CARD ............ {}", self.free.map_or("?".into(), |f| format!("{} FREE", size(f)))),
            format!("CATALOG ......... {} / {} PORTS", self.index_date(), self.index.ports.len()),
            format!("INSTALLED ....... {installed} / {} UPDATES", self.updates()),
            format!("LINK ............ {}", self.link()),
            format!("DISPLAY ......... {}", self.look.theme_name()),
            String::new(),
            "READY.".to_string(),
        ];
        let mut left = ((age - 0.35).max(0.0) * 160.0) as usize;
        let (x, mut y) = (40, 60);
        let mut cursor = (x, y);
        for (n, line) in lines.iter().enumerate() {
            let px = if n == 0 { BIG } else { BODY };
            let shown: String = line.chars().take(left).collect();
            left = left.saturating_sub(line.chars().count() + 1);
            let w = c.text(x, y, px, if n == 0 { p.hi } else { p.fg }, &shown);
            if !shown.is_empty() || n == 0 {
                cursor = (x + w + 2, y);
            }
            if n == 0 {
                c.fill(x, y + px as i32 + 4, c.w as i32 - 2 * x, 2, p.dim);
            }
            y += px as i32 + if n == 0 { 18 } else { 6 };
            if left == 0 {
                break;
            }
        }
        if ((age * 2.0) as u32).is_multiple_of(2) {
            c.fill(cursor.0, cursor.1 + 2, 10, BODY as i32 - 4, p.fg);
        }
        c.text(x, c.h as i32 - 44, SMALL, p.dim, "ANY KEY TO SKIP");
    }

    /// The frame every page has: the screen's edge and the title bar. The space
    /// under it starts at EDGE + BAR.
    fn chrome(&mut self, c: &mut Canvas, p: &Palette, title: &str, right: &str) {
        let (w, h) = (c.w as i32, c.h as i32);
        c.frame(EDGE, EDGE, w - 2 * EDGE, h - 2 * EDGE, 2, p.dim);
        c.fill(EDGE, EDGE, w - 2 * EDGE, BAR, p.panel);
        c.fill(EDGE, EDGE + BAR - 2, w - 2 * EDGE, 2, p.dim);
        icon(c, p, EDGE + 7, EDGE + 7);
        let rw = c.text_right(w - EDGE - 10, EDGE + 4, SMALL, p.dim, right);
        let title = c.fit(SMALL, title, w - 2 * EDGE - 50 - rw);
        c.text(EDGE + 30, EDGE + 4, SMALL, p.fg, &title);
    }

    fn foot(c: &Canvas) -> i32 {
        c.h as i32 - EDGE - FOOT
    }

    /// The list window: its top, and the height its rows have.
    fn list_area(c: &Canvas) -> (i32, i32) {
        let top = TOP + BAR;
        (top, App::foot(c) - 8 - top)
    }

    fn draw_list(&mut self, c: &mut Canvas, p: &Palette) {
        let w = c.w as i32;
        let t = self.tab_at();
        let items = self.items();
        let filter = self.filter[t].clone();
        let what = match (self.tab, &filter) {
            (Tab::Games | Tab::Apps, Some(f)) => format!("{} : {}", TABS[t].1, f.to_uppercase()),
            (Tab::Games | Tab::Apps, None) => format!("{} : ALL", TABS[t].1),
            _ => TABS[t].1.to_string(),
        };
        let ports = items.iter().filter(|i| i.pickable()).count();
        self.chrome(c, p, &format!("PORT STORE \\\\ {what} \\\\ {ports} ENTRIES"), &clock());

        // The tabs, the one on show in reverse.
        let ty = EDGE + BAR + 2;
        c.text(EDGE + 10, ty + 2, SMALL, p.faint, "L1");
        c.text_right(w - EDGE - 10, ty + 2, SMALL, p.faint, "R1");
        let mut x = EDGE + 40;
        for (tab, name) in TABS {
            let label = match tab {
                Tab::Updates if self.updates() > 0 => format!("{name} {}", self.updates()),
                _ => name.to_string(),
            };
            let lw = c.width(SMALL, &label) + 16;
            if tab == self.tab {
                c.fill(x, ty, lw, BAR - 4, p.fg);
                c.text(x + 8, ty + 2, SMALL, p.bg, &label);
            } else {
                c.frame(x, ty, lw, BAR - 4, 1, p.faint);
                c.text(x + 8, ty + 2, SMALL, p.dim, &label);
            }
            x += lw + 6;
        }

        self.draw_keys(c, p, &items, filter.as_deref());

        // The list window.
        let wx = EDGE + KEYS_W + 8;
        let ww = w - wx - EDGE - 6;
        let (area_top, area) = App::list_area(c);
        window(c, p, wx, TOP, ww, App::foot(c) - 4 - TOP, TABS[t].1, &format!("{ports}"));
        let pick = App::pickable(&items, self.pick[t].min(items.len().saturating_sub(1)), 1);
        self.pick[t] = pick;
        // Scroll so the pick is in full view, its group's heading with it when it can.
        let mut top = self.top[t].min(pick);
        if pick == top && top > 0 && matches!(items[top - 1], Item::Heading(_)) {
            top -= 1;
        }
        while top < pick && items[top..=pick].iter().map(Item::height).sum::<i32>() > area {
            top += 1;
        }
        self.top[t] = top;
        if items.is_empty() {
            let text = match self.tab {
                Tab::Installed => "NOTHING INSTALLED WITH PORT STORE YET.",
                Tab::Deps => "NOTHING IN THE CATALOG RUNS ON ANOTHER PORT.",
                Tab::Updates => "EVERYTHING INSTALLED IS UP TO DATE.",
                Tab::More => "NOTHING MORE IN THE CATALOG.",
                Tab::Apps if filter.is_some() => "NO APPS OF THIS KIND.",
                Tab::Apps => "NO APPS IN THE CATALOG.",
                Tab::Games if filter.is_some() => "NO GAMES OF THIS KIND.",
                Tab::Games => "THE CATALOG IS EMPTY: PRESS Y TO DOWNLOAD IT.",
                Tab::Homebrew if self.brews_loading => "DOWNLOADING HOMEBREW HUB'S LIST...",
                Tab::Homebrew if filter.is_some() => "NOTHING OF THIS KIND.",
                Tab::Homebrew if self.store.homebrew_url().is_none() => "NO CATALOG ADDRESS IN SETTINGS.JSON.",
                Tab::Homebrew => "NO LIST YET: PRESS Y TO DOWNLOAD IT.",
            };
            let tw = c.width(SMALL, text);
            c.text(wx + (ww - tw) / 2, area_top + area / 2 - 10, SMALL, p.dim, text);
        }
        let (rx, rw) = (wx + 6, ww - 26);
        let mut y = area_top;
        let mut last = top;
        for (at, item) in items.iter().enumerate().skip(top) {
            if y + item.height() > area_top + area {
                break;
            }
            last = at;
            match item {
                Item::End => c.dashes(rx + 4, y + END / 2, rw - 8, 4, 4, p.faint),
                Item::Heading(name) => {
                    let tw = c.text(rx + 4, y + 4, SMALL, p.hi, &format!("> {}", name.to_uppercase()));
                    c.dashes(rx + 14 + tw, y + 15, rw - 18 - tw, 4, 4, p.faint);
                }
                Item::Manual(m) => {
                    let m = self.index.manual[*m].clone();
                    self.row(c, p, rx, y, rw, at == pick, &m.name, &m.thumb, false);
                    let sub = join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]).to_uppercase();
                    c.text_fit(rx + THUMB + 14, y + 32, SMALL, p.dim, &sub, rw - THUMB - 120);
                    c.text_right(rx + rw - 8, y + 32, SMALL, p.dim, "[BY HAND]");
                }
                Item::Port(i) => {
                    let port = self.index.ports[*i].clone();
                    let marked = self.marked.contains(&port.id);
                    self.row(c, p, rx, y, rw, at == pick, &port.name, &port.thumb, marked);
                    // Under a heading the group goes without saying.
                    let grouped = matches!(self.tab, Tab::Games | Tab::Apps) && !port.group.is_empty() && !self.newest;
                    // "app" goes without saying too: the tab says it.
                    let kinds: Vec<&str> = port.categories.iter().map(String::as_str).filter(|c| *c != "app").collect();
                    let sub = join(&[
                        if self.newest { port.date.clone() } else { String::new() },
                        if port.group.is_empty() || grouped { kinds.join(", ") } else { port.group.clone() },
                        port.status.clone(),
                        owned_word(&port.assets),
                    ])
                    .to_uppercase();
                    let tag_w = self.tag(c, p, rx + rw - 8, y + 32, &port);
                    let mut sx = rx + THUMB + 14;
                    if port.dev {
                        sx += inverse(c, p, sx, y + 32, "DEV", p.dim) + 8;
                    }
                    if port.auto {
                        sx += inverse(c, p, sx, y + 32, "AUTO", p.dim) + 8;
                    }
                    if self.is_new(&port.id) {
                        sx += inverse(c, p, sx, y + 32, "NEW", p.hi) + 8;
                    }
                    c.text_fit(sx, y + 32, SMALL, p.dim, &sub, rx + rw - 16 - tag_w - sx);
                }
                Item::Brew(i) => {
                    let e = self.brews.entries[*i].clone();
                    self.row(c, p, rx, y, rw, at == pick, &e.title, &e.shot, false);
                    let sub = join(&[
                        if self.newest { e.added.clone() } else { String::new() },
                        e.system.clone(),
                        e.kind.clone(),
                        e.devs.join(", "),
                    ])
                    .to_uppercase();
                    let tag_w = match self.brew_state(&e.slug) {
                        State::Installed(_) => c.text_right(rx + rw - 8, y + 32, SMALL, p.good, "[INSTALLED]"),
                        State::Update(_) => {
                            let tw = c.width(SMALL, "UPDATE") + 8;
                            inverse(c, p, rx + rw - 8 - tw, y + 32, "UPDATE", p.warn)
                        }
                        _ => c.text_right(rx + rw - 8, y + 32, SMALL, p.dim, &size(e.file.size)),
                    };
                    let sx = rx + THUMB + 14;
                    c.text_fit(sx, y + 32, SMALL, p.dim, &sub, rx + rw - 16 - tag_w - sx);
                }
            }
            y += item.height();
        }
        scrollbar(c, p, wx + ww - 18, area_top, area, top, last + 1 - top, items.len());

        // The status line.
        let fy = App::foot(c);
        c.fill(EDGE, fy, w - 2 * EDGE, 2, p.dim);
        let left = match self.marked.len() {
            0 if self.tab == Tab::Homebrew => format!(
                "HOMEBREW HUB \\\\ {} ENTRIES \\\\ LIST {}",
                self.brews.entries.len(),
                self.brews.generated.get(..10).unwrap_or("NONE YET")
            ),
            0 => format!(
                "CATALOG {} \\\\ {} \\\\ LINK {}",
                self.index_date(),
                self.free.map(|f| format!("{} FREE", size(f))).unwrap_or_default(),
                self.link()
            ),
            n => format!("{n} MARKED \\\\ START INSTALLS THEM"),
        };
        let right = format!("V{}", env!("CARGO_PKG_VERSION"));
        let rw = c.text_right(w - EDGE - 10, fy + 6, SMALL, p.faint, &right);
        let left = c.fit(SMALL, &left, w - 2 * EDGE - 40 - rw);
        c.text(EDGE + 10, fy + 6, SMALL, if self.marked.is_empty() { p.dim } else { p.hi }, &left);
    }

    /// The column of keys beside the list, and the list's mode under them.
    fn draw_keys(&mut self, c: &mut Canvas, p: &Palette, items: &[Item], filter: Option<&str>) {
        let mut keys: Vec<(&str, String)> = Vec::new();
        if !items.is_empty() {
            keys.push(("A", "OPEN".into()));
        }
        if !matches!(self.tab, Tab::More | Tab::Homebrew) && !items.is_empty() {
            keys.push(("X", "MARK".into()));
        }
        if !self.marked.is_empty() {
            keys.push(("ST", "INSTALL".into()));
        }
        match self.tab {
            Tab::Games | Tab::Apps | Tab::Homebrew => keys.push(("SEL", "FILTER".into())),
            Tab::Updates if !items.is_empty() => keys.push(("SEL", "UPD ALL".into())),
            _ => {}
        }
        if self.tab != Tab::More {
            keys.push(("R2", if self.newest { "A-Z" } else { "NEWEST" }.into()));
        }
        keys.push(("Y", "RELOAD".into()));
        keys.push(("L2", "SYSTEM".into()));
        keys.push(("B", "EXIT".into()));
        let x = EDGE + 10;
        let mut y = TOP + 4;
        for (key, label) in &keys {
            let kw = c.text(x, y, SMALL, p.hi, &format!("[{key}]"));
            c.text(x + kw.max(40) + 6, y, SMALL, p.fg, label);
            y += 24;
        }
        // The mode, boxed.
        y += 10;
        let mode = [
            format!("SORT: {}", if self.newest { "NEWEST" } else { "A-Z" }),
            format!("SHOW: {}", filter.map_or("ALL".to_string(), str::to_uppercase)),
            format!("DEV: {}", if self.look.dev { "SHOWN" } else { "HIDDEN" }),
        ];
        let bw = KEYS_W - 12;
        if y + 3 * 22 + 12 < App::foot(c) - 6 {
            c.frame(x - 4, y, bw, 3 * 22 + 10, 1, p.faint);
            for (n, line) in mode.iter().enumerate() {
                let line = c.fit(SMALL, line, bw - 12);
                c.text(x + 2, y + 5 + n as i32 * 22, SMALL, p.dim, &line);
            }
        }
    }

    /// A row: its picture (the first letter of the name without one), its name, and
    /// the pick's highlight and cursor.
    #[allow(clippy::too_many_arguments)]
    fn row(&mut self, c: &mut Canvas, p: &Palette, x: i32, y: i32, w: i32, picked: bool, name: &str, thumb: &str, marked: bool) {
        if picked {
            c.fill(x, y + 1, w, ROW - 4, mix(p.bg, p.fg, if p.mono { 34 } else { 255 }, p.faint));
            c.fill(x, y + 1, 3, ROW - 4, p.fg);
        }
        let (bx, by) = (x + 8, y + (ROW - 2 - THUMB) / 2);
        let shown = match self.look.pictures {
            PictureMode::Off => None,
            _ => self.pictures.get_fit(thumb, THUMB as usize, THUMB as usize),
        };
        match shown {
            Some(img) => {
                let (ix, iy) = (bx + (THUMB - img.w as i32) / 2, by + (THUMB - img.h as i32) / 2);
                self.picture(c, p, ix, iy, &img, 2);
            }
            None => {
                c.frame(bx, by, THUMB, THUMB, 1, p.faint);
                let letter: String = name.chars().take(1).collect::<String>().to_uppercase();
                if !letter.is_empty() {
                    let lw = c.width(NAME, &letter);
                    c.text(bx + (THUMB - lw) / 2, by + 8, NAME, p.dim, &letter);
                }
            }
        }
        if marked {
            c.frame(bx - 2, by - 2, THUMB + 4, THUMB + 4, 2, p.hi);
            c.fill(bx - 2, by - 2, 14, 14, p.hi);
            c.text(bx + 1, by - 5, SMALL, p.bg, "*");
        }
        let nx = x + THUMB + 14;
        let name_w = c.text_fit(nx, y + 2, NAME, if picked { p.hi } else { p.fg }, name, w - THUMB - 30);
        if picked && self.blinking() && self.age() % 1.06 < 0.53 {
            c.fill(nx + name_w + 4, y + 6, 10, 22, p.fg);
        }
    }

    /// A port's state at the right of its row, from `right`: its width.
    fn tag(&mut self, c: &mut Canvas, p: &Palette, right: i32, y: i32, port: &Port) -> i32 {
        match self.state(&port.id) {
            State::Installed(_) => c.text_right(right, y, SMALL, p.good, "[INSTALLED]"),
            State::Update(_) => {
                let tw = c.width(SMALL, "UPDATE") + 8;
                inverse(c, p, right - tw, y, "UPDATE", p.warn)
            }
            State::ByHand => c.text_right(right, y, SMALL, p.dim, "[ON CARD]"),
            State::Absent => c.text_right(right, y, SMALL, p.dim, &size(port.file.size)),
        }
    }

    /// A picture in the screen's light (dithered in its shades, in blocks of `cell`),
    /// or in its own colours when so chosen or in the classic look.
    fn picture(&self, c: &mut Canvas, p: &Palette, x: i32, y: i32, img: &Image, cell: usize) {
        if p.mono && self.look.pictures == PictureMode::Screen {
            c.image_dithered(x, y, img, cell, &p.ramp());
        } else {
            c.image(x, y, img);
        }
    }

    /// A page's picture: its window, with a ruler on two sides and cross hairs, as
    /// an instrument shows it; "no signal" without one.
    /// Its width and height: none when pictures are off.
    fn draw_cover(&mut self, c: &mut Canvas, p: &Palette, x: i32, y: i32, cover: &str, cell: usize) -> (i32, i32) {
        if self.look.pictures == PictureMode::Off {
            return (-12, 0);
        }
        let (pw, ph) = (224, 168);
        let ruler = 36;
        let (fx, fy) = (x + ruler, y);
        c.frame(fx - 2, fy - 2, pw + 4, ph + 4, 1, p.dim);
        match self.pictures.get_fit(cover, pw as usize, ph as usize) {
            Some(img) => {
                let (ix, iy) = (fx + (pw - img.w as i32) / 2, fy + (ph - img.h as i32) / 2);
                self.picture(c, p, ix, iy, &img, cell);
            }
            None => {
                // Static, as a screen without a signal.
                let mut seed = 0x2545_f491u32 ^ (self.age() * 12.0) as u32;
                for by in (0..ph).step_by(4) {
                    for bx in (0..pw).step_by(4) {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        if seed.is_multiple_of(5) {
                            c.fill(fx + bx, fy + by, 3, 3, p.faint);
                        }
                    }
                }
                let tw = c.width(BODY, "NO SIGNAL");
                c.fill(fx + (pw - tw) / 2 - 8, fy + ph / 2 - 16, tw + 16, 30, p.bg);
                c.text(fx + (pw - tw) / 2, fy + ph / 2 - 12, BODY, p.dim, "NO SIGNAL");
            }
        }
        if p.mono {
            // Cross hairs through the middle, and the rulers' ticks and numbers.
            c.dashes(fx, fy + ph / 2, pw, 2, 3, p.dim);
            c.dashes_down(fx + pw / 2, fy, ph, 2, 3, p.dim);
            for n in 0..=(ph / 14) {
                let ty = fy + ph - n * 14;
                c.fill(fx - 8, ty, if n % 2 == 0 { 6 } else { 3 }, 1, p.dim);
                if n % 2 == 0 && n > 0 {
                    c.text_right(fx - 10, ty - 8, 16, p.faint, &format!("{}", n * 10));
                }
            }
            for n in 0..=(pw / 16) {
                let tx = fx + n * 16;
                c.fill(tx, fy + ph + 3, 1, if n % 2 == 0 { 6 } else { 3 }, p.dim);
                if n % 2 == 0 && n > 0 && n < pw / 16 {
                    let label = format!("{}", n * 10);
                    let lw = c.width(16, &label);
                    c.text(tx - lw / 2, fy + ph + 9, 16, p.faint, &label);
                }
            }
        }
        (ruler + pw + 4, ph + 28)
    }

    /// The facts beside a page's picture: label, dotted leader, value.
    fn sheet(c: &mut Canvas, p: &Palette, x: i32, y: i32, w: i32, rows: &[(&str, String)]) -> i32 {
        let mut ty = y;
        for (label, value) in rows.iter().filter(|(_, v)| !v.is_empty()) {
            let lw = c.text(x, ty, SMALL, p.dim, label);
            let value = c.fit(SMALL, value, w - lw - 24);
            let vw = c.width(SMALL, &value);
            c.dashes(x + lw + 6, ty + 14, w - lw - vw - 14, 1, 3, p.faint);
            c.text(x + w - vw, ty, SMALL, p.fg, &value);
            ty += 23;
        }
        ty - y
    }

    /// The notes window of a page, its lines from `scroll` on: how many did not fit.
    fn notes(&mut self, c: &mut Canvas, p: &Palette, y: i32, title: &str, lines: &[(u16, u32, String)], scroll: usize) {
        let (w, x) = (c.w as i32 - 2 * EDGE - 12, EDGE + 6);
        let bottom = App::foot(c) - 4;
        window(c, p, x, y, w, bottom - y, title, "");
        let mut ty = y + BAR + 4;
        let mut hidden = 0;
        for (n, (px, color, text)) in lines.iter().enumerate().skip(scroll) {
            let lh = *px as i32 + 2;
            // In order: the first line that does not fit ends the page.
            if ty + lh > bottom - 4 {
                hidden = lines.len() - n;
                break;
            }
            c.text(x + 12, ty, *px, *color, text);
            ty += lh;
        }
        self.scroll_max = if hidden > 0 { scroll + 1 } else { scroll };
        if scroll > 0 || hidden > 0 {
            let shown = lines.len().saturating_sub(scroll + hidden);
            scrollbar(c, p, x + w - 18, y + BAR + 2, bottom - y - BAR - 6, scroll, shown, lines.len());
        }
    }

    fn draw_details(&mut self, c: &mut Canvas, p: &Palette, id: &str, scroll: usize) {
        let Some(port) = self.port(id).cloned() else { return };
        let state = self.state(id);
        let w = c.w as i32;
        let label = match &state {
            State::Installed(v) => format!("[INSTALLED {}]", v.to_uppercase()),
            State::Update(v) => format!("[{} INSTALLED]", v.to_uppercase()),
            State::ByHand => "[ON THE CARD BY HAND]".into(),
            State::Absent => "[NOT INSTALLED]".into(),
        };
        self.chrome(c, p, &format!("SUBJECT: {}", port.name.to_uppercase()), &label);
        let facts = join(&[port.group.clone(), port.categories.join(", "), port.status.clone()]).to_uppercase();
        let facts = c.fit(SMALL, &format!("GROUP: {}", facts.replace(" · ", " \\\\ ")), w - 2 * EDGE - 24);
        c.text(EDGE + 12, EDGE + BAR + 4, SMALL, p.dim, &facts);

        let (cw, ch) = self.draw_cover(c, p, EDGE + 6, TOP, &port.cover, 3);
        let sx = EDGE + 6 + cw + 18;
        let mut rows = vec![
            ("VERSION", port.version.clone()),
            ("RELEASED", port.date.clone()),
            ("DOWNLOAD", size(port.file.size)),
            ("ON CARD", size(port.install.bytes)),
            ("PORTER", port.porter.join(", ")),
            ("FILES", port.install.files.to_string()),
        ];
        if !port.source.is_empty() {
            rows.push(("SOURCE", port.source.clone()));
        }
        if port.dev {
            rows.push(("BUILD", "DEVELOPMENT".into()));
        }
        if port.owned() {
            rows.push(("NEEDS", "YOUR GAME FILES".into()));
        }
        if port.auto {
            rows.push(("RECIPE", "NONE: AS RELEASED".into()));
        }
        // a game on a runtime: whether that is on the card; a runtime: what runs on it
        for need in &port.requires {
            let name = self.port(need).map_or(need.clone(), |d| d.name.clone());
            let on = match self.state(need) {
                State::Installed(_) => "INSTALLED",
                State::Update(_) => "UPDATE",
                State::ByHand => "ON CARD",
                State::Absent => "COMES WITH IT",
            };
            rows.push(("RUNS ON", format!("{name} ({on})")));
        }
        let users = self.index.required_by(&port.id);
        if !users.is_empty() {
            let installed = users.iter().filter(|u| matches!(self.state(&u.id), State::Installed(_) | State::Update(_))).count();
            rows.push(("NEEDED BY", format!("{} IN CATALOG, {installed} INSTALLED", users.len())));
        }
        if self.is_new(&port.id) {
            rows.push(("IN CATALOG", format!("NEW, SINCE {}", self.seen.get(&port.id).cloned().unwrap_or_default())));
        }
        let sh = App::sheet(c, p, sx, TOP, w - sx - EDGE - 10, &rows);

        // The notes: what is said of it, what it needs, where it is from.
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = w - 2 * EDGE - 50;
        let para = |c: &mut Canvas, lines: &mut Vec<(u16, u32, String)>, px: u16, color: u32, text: &str| {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
        };
        // What you bring first: it is what the page is opened for, more often than not.
        if !port.needs.is_empty() {
            para(c, &mut lines, BODY, p.hi, "YOU BRING:");
            let have = self.needs.get(id);
            for (i, need) in port.needs.iter().enumerate() {
                let (mark, color, said) = match have.and_then(|h| h.get(i)) {
                    None => match &self.checking {
                        Some(name) => ("[..]", p.dim, format!("reading {name}")),
                        None => ("[..]", p.dim, "checking".to_string()),
                    },
                    Some(Have::Unknown) => ("[??]", p.dim, "see the port's page".to_string()),
                    Some(Have::Missing) => ("[--]", p.bad, "not found on the card".to_string()),
                    Some(Have::Found(f)) => ("[OK]", p.good, format!("found: {f}")),
                    Some(Have::WrongVersion(f)) => ("[!!]", p.warn, format!("{f} is another version of the game")),
                };
                para(c, &mut lines, BODY, color, &format!("{mark} {} — {said}", need.what));
                if let Some(Some(found)) = self.imports.get(id).and_then(|f| f.get(i)) {
                    para(c, &mut lines, SMALL, p.hi, &format!("     IN {}: START MOVES IT TO {}", found.shown, found.to));
                } else if !need.hint.is_empty() {
                    para(c, &mut lines, SMALL, p.dim, &format!("     {}", need.hint));
                }
            }
            lines.push((8, p.dim, String::new()));
        }
        if !port.text().is_empty() {
            para(c, &mut lines, BODY, p.fg, port.text());
            lines.push((8, p.dim, String::new()));
        }
        if !port.url.is_empty() {
            para(c, &mut lines, SMALL, p.hi, port.url.trim_start_matches("https://"));
        }
        para(c, &mut lines, SMALL, p.dim, &format!("FILE: {} (WITHOUT WI-FI: PUT IT IN APP/PORTSTORE/DOWNLOADS)", port.file.name));
        let ny = TOP + ch.max(sh) + 6;
        self.notes(c, p, ny, "NOTES", &lines, scroll);

        let verb = match state {
            State::Installed(_) => "REINSTALL",
            State::Update(_) => "UPDATE",
            _ => "INSTALL",
        };
        let mut keys = vec![("A", verb)];
        if matches!(state, State::Installed(_) | State::Update(_)) && id != crate::SELF_ID {
            keys.push(("X", "REMOVE"));
        }
        if !port.needs.is_empty() {
            keys.push(("Y", "CHECK AGAIN"));
        }
        if self.imports.get(id).is_some_and(|f| f.iter().any(|p| p.is_some())) {
            keys.push(("ST", "PLACE FILES"));
        }
        keys.push(("B", "BACK"));
        self.softkeys(c, p, &keys);
    }

    fn draw_manual(&mut self, c: &mut Canvas, p: &Palette, at: usize, scroll: usize) {
        let Some(m) = self.index.manual.get(at).cloned() else { return };
        let w = c.w as i32;
        self.chrome(c, p, &format!("SUBJECT: {}", m.name.to_uppercase()), "[BY HAND]");
        let facts = join(&[m.categories.join(", "), m.status.clone(), owned_word(&m.assets)]).to_uppercase();
        c.text_fit(EDGE + 12, EDGE + BAR + 4, SMALL, p.dim, &facts.replace(" · ", " \\\\ "), w - 2 * EDGE - 24);
        let (cw, ch) = self.draw_cover(c, p, EDGE + 6, TOP, &m.cover, 3);
        let sx = EDGE + 6 + cw + 18;
        let mut ty = TOP;
        for l in c.wrap(BODY, "Port Store has no recipe for this one yet: install it by hand from its page, on a computer.", w - sx - EDGE - 10) {
            c.text(sx, ty, BODY, p.fg, &l);
            ty += BODY as i32 + 2;
        }
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = w - 2 * EDGE - 50;
        for (px, color, text) in [(BODY, p.fg, m.notes.as_str()), (SMALL, p.hi, m.url.as_deref().unwrap_or("").trim_start_matches("https://"))] {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
            lines.push((8, p.dim, String::new()));
        }
        self.notes(c, p, TOP + ch.max(ty - TOP) + 6, "NOTES", &lines, scroll);
        self.softkeys(c, p, &[("B", "BACK")]);
    }

    /// A page's buttons in the bottom bar, from the right.
    fn softkeys(&mut self, c: &mut Canvas, p: &Palette, keys: &[(&str, &str)]) {
        let fy = App::foot(c);
        c.fill(EDGE, fy, c.w as i32 - 2 * EDGE, 2, p.dim);
        buttons(c, p, c.w as i32 - EDGE - 8, fy + 4, keys);
    }

    /// A window over the page: its frame opening, then its text typed.
    fn dialog(&mut self, c: &mut Canvas, p: &Palette, title: &str, text: &str, keys: &[(&str, &str)]) {
        let w = (c.w as i32 - 60).min(560);
        let mut lines = c.wrap(BODY, text, w - 40);
        // On the screen whole: what does not fit ends in "…".
        let room = ((c.h as i32 - 2 * EDGE - 40 - BAR - 60) / (BODY as i32 + 2)).max(1) as usize;
        if lines.len() > room {
            lines.truncate(room);
            if let Some(last) = lines.last_mut() {
                last.push_str(" …");
            }
        }
        let h = BAR + 14 + lines.len() as i32 * (BODY as i32 + 2) + 14 + if keys.is_empty() { 0 } else { 34 };
        let (x, y) = ((c.w as i32 - w) / 2, (c.h as i32 - h) / 2);
        if !self.opening(c, p, x, y, w, h) {
            return;
        }
        window(c, p, x, y, w, h, &title.to_uppercase(), "");
        let mut left = ((self.age() - OPEN) * TYPE).max(0.0) as usize;
        let mut ty = y + BAR + 12;
        for l in &lines {
            let shown: String = l.chars().take(left).collect();
            left = left.saturating_sub(l.chars().count());
            c.text(x + 20, ty, BODY, p.fg, &shown);
            ty += BODY as i32 + 2;
        }
        if !keys.is_empty() {
            buttons(c, p, x + w - 14, y + h - 36, keys);
        }
    }

    /// A window's frame opening from a line: whether it is open (its insides drawn).
    fn opening(&self, c: &mut Canvas, p: &Palette, x: i32, y: i32, w: i32, h: i32) -> bool {
        let reveal = (self.age() / OPEN).min(1.0);
        if reveal >= 1.0 {
            return true;
        }
        let hh = ((h as f32 * reveal) as i32).max(4);
        let top = y + (h - hh) / 2;
        c.fill(x, top, w, hh, p.panel);
        c.frame(x, top, w, hh, 2, p.hi);
        false
    }

    /// A job's window: its event log, the progress, the spinner.
    fn draw_busy(&mut self, c: &mut Canvas, p: &Palette, title: &str, step: Option<&Step>, stoppable: bool) {
        let w = (c.w as i32 - 60).min(560);
        let shown = 6;
        let h = BAR + 14 + shown as i32 * (SMALL as i32 + 3) + 40 + 36;
        let (x, y) = ((c.w as i32 - w) / 2, (c.h as i32 - h) / 2);
        if !self.opening(c, p, x, y, w, h) {
            return;
        }
        let spin = ["|", "/", "-", "\\"][(self.now.saturating_duration_since(self.log_at).as_millis() / 125 % 4) as usize];
        let title = format!("EVENT LOG \\\\ {}", title.to_uppercase());
        window(c, p, x, y, w, h, &title, spin);
        let mut ty = y + BAR + 10;
        let from = self.log.len().saturating_sub(shown);
        for (n, (secs, _, line)) in self.log.iter().enumerate().skip(from) {
            let last = n + 1 == self.log.len();
            let stamp = format!("T+{:02}:{:02}", secs / 60, secs % 60);
            let sw = c.text(x + 20, ty, SMALL, p.dim, &stamp);
            c.text(x + 26 + sw, ty, SMALL, p.faint, "|");
            let line = c.fit(SMALL, line, w - sw - 70);
            c.text(x + 40 + sw, ty, SMALL, if last { p.hi } else { p.fg }, &line);
            ty += SMALL as i32 + 3;
        }
        let (_, _, bar) = step_line(step);
        let by = y + BAR + 14 + shown as i32 * (SMALL as i32 + 3) + 6;
        segments(c, p, x + 20, by, w - 110, bar);
        let pct = bar.map_or("....".to_string(), |f| format!("{:3}%", (f.clamp(0.0, 1.0) * 100.0) as u32));
        c.text_right(x + w - 20, by + 2, SMALL, p.fg, &pct);
        if stoppable {
            buttons(c, p, x + w - 14, y + h - 36, &[("B", "ABORT")]);
        }
    }

    fn draw_filter(&mut self, c: &mut Canvas, p: &Palette, at: usize) {
        let choices = self.choices();
        let w = 300;
        let row = 26;
        let room = ((App::foot(c) - TOP - 20 - BAR - 44) / row).max(3) as usize;
        let h = BAR + 10 + choices.len().min(room) as i32 * row + 44;
        let (x, y) = (EDGE + KEYS_W + 20, TOP + 4);
        if !self.opening(c, p, x, y, w, h) {
            return;
        }
        let t = self.tab_at();
        window(c, p, x, y, w, h, &format!("FILTER \\\\ {}", TABS[t].1), "");
        let first = at.saturating_sub(room - 1);
        let mut ty = y + BAR + 6;
        for (n, choice) in choices.iter().enumerate().skip(first).take(room) {
            let picked = n == at;
            if picked {
                c.fill(x + 6, ty, w - 12, row - 2, p.fg);
            }
            let (ink, dim) = if picked { (p.bg, p.bg) } else { (p.fg, p.dim) };
            let current = match choice {
                Choice::All(_) => self.filter[t].is_none(),
                Choice::Category(name, _) => self.filter[t].as_deref() == Some(name),
                _ => false,
            };
            let (label, count) = match choice {
                Choice::All(n) => (format!("ALL {}", TABS[t].1), *n),
                Choice::Category(name, n) => (name.to_uppercase(), *n),
                Choice::Line(heading) => {
                    c.text(x + 14, ty + 1, SMALL, p.faint, &format!("-- {heading} --"));
                    ty += row;
                    continue;
                }
                Choice::Dev(n) => (format!("[{}] DEV BUILDS", if self.look.dev { "X" } else { " " }), *n),
            };
            c.text(x + 14, ty + 1, SMALL, ink, if current { ">" } else { " " });
            c.text_fit(x + 30, ty + 1, SMALL, ink, &label, w - 90);
            c.text_right(x + w - 16, ty + 1, SMALL, dim, &count.to_string());
            ty += row;
        }
        buttons(c, p, x + w - 12, y + h - 36, &[("A", "SET"), ("B", "CLOSE")]);
    }

    fn draw_system(&mut self, c: &mut Canvas, p: &Palette, at: usize) {
        let w = 380;
        let row = 30;
        let h = BAR + 12 + SYSTEM.len() as i32 * row + 50;
        let (x, y) = ((c.w as i32 - w) / 2, TOP + 20);
        if !self.opening(c, p, x, y, w, h) {
            return;
        }
        window(c, p, x, y, w, h, "SYSTEM", "");
        let values = [
            self.look.theme_name().to_string(),
            self.look.effects_name().to_string(),
            self.look.pictures_name().to_string(),
            if self.look.dev { "SHOWN" } else { "HIDDEN" }.to_string(),
            if self.in_games { "ON" } else { "OFF" }.to_string(),
            if self.ports_hidden { "IN MIYOOPORTS" } else { "SHOWN" }.to_string(),
        ];
        let mut ty = y + BAR + 8;
        for (n, (label, value)) in SYSTEM.iter().zip(values).enumerate() {
            let picked = n == at;
            if picked {
                c.fill(x + 6, ty, w - 12, row - 4, p.fg);
            }
            let ink = if picked { p.bg } else { p.fg };
            c.text(x + 16, ty + 2, SMALL, ink, label);
            c.text_right(x + w - 16, ty + 2, SMALL, ink, &format!("< {value} >"));
            ty += row;
        }
        buttons(c, p, x + w - 12, y + h - 36, &[("<>", "CHANGE"), ("B", "CLOSE")]);
    }

    /// A homebrew entry's page: its screenshot, its facts, what is said of it.
    fn draw_brew(&mut self, c: &mut Canvas, p: &Palette, slug: &str, scroll: usize) {
        let Some(e) = self.brews.entry(slug).cloned() else { return };
        let state = self.brew_state(slug);
        let w = c.w as i32;
        let label = match &state {
            State::Installed(_) => "[INSTALLED]".to_string(),
            State::Update(_) => "[NEWER ROM IN THE LIST]".to_string(),
            _ => "[NOT INSTALLED]".to_string(),
        };
        self.chrome(c, p, &format!("SUBJECT: {}", e.title.to_uppercase()), &label);
        let facts = format!("HOMEBREW \\\\ {} \\\\ {}", e.system_name().to_uppercase(), e.kind.to_uppercase());
        c.text_fit(EDGE + 12, EDGE + BAR + 4, SMALL, p.dim, &facts, w - 2 * EDGE - 24);
        let (cw, ch) = self.draw_cover(c, p, EDGE + 6, TOP, &e.shot, 1);
        let sx = EDGE + 6 + cw + 18;
        let rows = vec![
            ("DEVELOPER", e.devs.join(", ")),
            ("RELEASED", e.date.clone()),
            ("IN THE HUB", e.added.clone()),
            ("SIZE", size(e.file.size)),
            ("LICENSE", e.license.clone()),
            ("GOES TO", format!("Roms/{}", e.folder())),
        ];
        let sh = App::sheet(c, p, sx, TOP, w - sx - EDGE - 10, &rows);
        let mut lines: Vec<(u16, u32, String)> = Vec::new();
        let width = w - 2 * EDGE - 50;
        let para = |c: &mut Canvas, lines: &mut Vec<(u16, u32, String)>, px: u16, color: u32, text: &str| {
            for l in c.wrap(px, text, width) {
                lines.push((px, color, l));
            }
        };
        if !e.desc.is_empty() {
            para(c, &mut lines, BODY, p.fg, &e.desc);
            lines.push((8, p.dim, String::new()));
        }
        if !e.tags.is_empty() {
            para(c, &mut lines, SMALL, p.dim, &format!("TAGS: {}", e.tags.join(", ").to_uppercase()));
        }
        if !e.site.is_empty() {
            para(c, &mut lines, SMALL, p.hi, e.site.trim_start_matches("https://"));
        }
        para(c, &mut lines, SMALL, p.dim, &format!("FROM HOMEBREW HUB (HH.GBDEV.IO): hh.gbdev.io/game/{}", e.slug));
        para(c, &mut lines, SMALL, p.dim, &format!("FILE: {} (WITHOUT WI-FI: PUT IT IN APP/PORTSTORE/DOWNLOADS)", e.file.name));
        self.notes(c, p, TOP + ch.max(sh) + 6, "NOTES", &lines, scroll);
        let verb = match state {
            State::Installed(_) => "REINSTALL",
            State::Update(_) => "UPDATE",
            _ => "INSTALL",
        };
        let mut keys = vec![("A", verb)];
        if matches!(state, State::Installed(_) | State::Update(_)) {
            keys.push(("X", "REMOVE"));
        }
        keys.push(("B", "BACK"));
        self.softkeys(c, p, &keys);
    }

    /// The ports new in the catalog since the last look.
    fn draw_arrivals(&mut self, c: &mut Canvas, p: &Palette, at: usize) {
        let w = (c.w as i32 - 60).min(520);
        let row = 26;
        let room = ((App::foot(c) - TOP - 40 - BAR - 44) / row).max(3) as usize;
        let shown = self.arrivals.len().min(room);
        let h = BAR + 10 + shown as i32 * row + 44;
        let (x, y) = ((c.w as i32 - w) / 2, TOP + 10);
        if !self.opening(c, p, x, y, w, h) {
            return;
        }
        window(c, p, x, y, w, h, "NEW ARRIVALS", &self.arrivals.len().to_string());
        let first = at.saturating_sub(room - 1);
        let mut ty = y + BAR + 6;
        for (n, id) in self.arrivals.clone().iter().enumerate().skip(first).take(room) {
            let Some(port) = self.port(id).cloned() else { continue };
            let picked = n == at;
            if picked {
                c.fill(x + 6, ty, w - 12, row - 2, p.fg);
            }
            let (ink, dim) = if picked { (p.bg, p.bg) } else { (p.fg, p.dim) };
            let kind = if port.is_app() { "APP" } else { "GAME" };
            let kw = c.text_right(x + w - 16, ty + 1, SMALL, dim, kind);
            c.text_fit(x + 16, ty + 1, SMALL, ink, &port.name, w - kw - 44);
            ty += row;
        }
        buttons(c, p, x + w - 12, y + h - 36, &[("A", "OPEN"), ("B", "CLOSE")]);
    }
}

/// The small square at a title bar's left, as a window's.
fn icon(c: &mut Canvas, p: &Palette, x: i32, y: i32) {
    c.frame(x, y, 14, 14, 1, p.fg);
    c.fill(x + 4, y + 4, 6, 6, p.fg);
}

/// A window: its inside, frame and title bar (the square, the title, something at
/// the right).
#[allow(clippy::too_many_arguments)]
fn window(c: &mut Canvas, p: &Palette, x: i32, y: i32, w: i32, h: i32, title: &str, right: &str) {
    c.fill(x, y, w, h, p.panel);
    c.frame(x, y, w, h, 2, p.dim);
    c.fill(x, y + BAR - 4, w, 2, p.dim);
    icon(c, p, x + 6, y + 5);
    let rw = if right.is_empty() { 0 } else { c.text_right(x + w - 10, y + 2, SMALL, p.dim, right) };
    let title = c.fit(SMALL, title, w - 50 - rw);
    c.text(x + 28, y + 2, SMALL, p.fg, &title);
}

/// Text in reverse (a block of `color`, the text in the background's): its width.
fn inverse(c: &mut Canvas, p: &Palette, x: i32, y: i32, text: &str, color: u32) -> i32 {
    let w = c.width(SMALL, text) + 8;
    c.fill(x, y + 1, w, SMALL as i32 - 1, color);
    c.text(x + 4, y, SMALL, p.bg, text);
    w
}

/// Buttons in a row ending at `right`: the key in reverse, its word beside it.
fn buttons(c: &mut Canvas, p: &Palette, right: i32, y: i32, keys: &[(&str, &str)]) {
    let mut x = right;
    for (key, label) in keys.iter().rev() {
        let lw = c.width(SMALL, label);
        let kw = c.width(SMALL, key) + 8;
        let bw = kw + lw + 14;
        x -= bw;
        c.frame(x, y, bw, 24, 1, p.dim);
        c.fill(x + 2, y + 2, kw, 20, p.fg);
        c.text(x + 6, y + 1, SMALL, p.bg, key);
        c.text(x + kw + 8, y + 1, SMALL, p.fg, label);
        x -= 6;
    }
}

/// A progress bar of blocks; without a share known, a block running along.
fn segments(c: &mut Canvas, p: &Palette, x: i32, y: i32, w: i32, share: Option<f32>) {
    let n = (w / 12).max(1);
    let lit = share.map(|f| (f.clamp(0.0, 1.0) * n as f32).round() as i32);
    c.frame(x - 3, y - 3, n * 12 + 4, 22, 1, p.faint);
    for i in 0..n {
        let on = lit.is_some_and(|l| i < l);
        c.fill(x + i * 12, y, 10, 16, if on { p.fg } else { p.faint });
    }
}

/// A scroll bar: arrows in boxes, the track, and the part on show.
#[allow(clippy::too_many_arguments)]
fn scrollbar(c: &mut Canvas, p: &Palette, x: i32, y: i32, h: i32, top: usize, shown: usize, total: usize) {
    if total == 0 || (top == 0 && shown >= total) {
        return;
    }
    c.frame(x, y, 14, 14, 1, p.dim);
    c.arrow(x + 3, y + 4, 9, true, p.fg);
    c.frame(x, y + h - 14, 14, 14, 1, p.dim);
    c.arrow(x + 3, y + h - 9, 9, false, p.fg);
    let track = h - 32;
    c.dashes_down(x + 7, y + 16, track, 1, 2, p.faint);
    let th = (track * shown as i32 / total as i32).max(12);
    let ty = y + 16 + (track - th) * top as i32 / (total - shown.min(total)).max(1) as i32;
    c.fill(x + 3, ty, 9, th, p.fg);
}

/// `a` on `b` of 255, or `classic` for the classic look (no blend of its own).
fn mix(bg: u32, fg: u32, a: u32, classic: u32) -> u32 {
    if a >= 255 {
        classic
    } else {
        canvas::mix(bg, fg, a)
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

/// A job's step as a line of its event log: its kind (the same kind's next takes the
/// line's place), the line, and the share done when known.
fn step_line(step: Option<&Step>) -> (u32, String, Option<f32>) {
    match step {
        None => (0, "STARTING".into(), None),
        Some(Step::Download { done, total }) => (
            1,
            format!("DOWNLOAD {} / {}", size(*done), size(*total)),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Some(Step::Check) => (2, "VERIFY SHA-256".into(), None),
        Some(Step::Unpack { total: 0, .. }) => (3, "UNPACK".into(), None),
        Some(Step::Unpack { done, total }) => (
            3,
            format!("UNPACK {} / {}", size(*done), size(*total)),
            Some(*done as f32 / (*total).max(1) as f32),
        ),
        Some(Step::Finish) => (4, "PLACE FILES".into(), None),
    }
}

/// The time of day, as the title bar shows it.
fn clock() -> String {
    // SAFETY: localtime_r writes only the tm given.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
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
    let mut crt = Crt::new(w, h, app.effects());
    let start = Instant::now();
    app.now = start;
    app.boot = Some(start);
    crt.start(Fx::PowerOn, start);
    crt.wake(start);
    let mut sig = app.sig();
    // Frame times while something moves, said in the log now and then (to see how
    // near 60 a second the Mini comes).
    let mut stats = (0u32, Duration::ZERO, Duration::ZERO, Instant::now());
    'shown: loop {
        let now = Instant::now();
        app.now = now;
        crt.effects = app.effects();
        crt.beam = canvas::mix(app.palette().hi, 0xffffff, 140);
        let due = app.next_draw().is_some_and(|every| now.saturating_duration_since(app.drawn_at) >= every);
        let changed = app.dirty || due;
        if changed {
            app.draw(&mut canvas);
            app.dirty = false;
            app.drawn_at = now;
        }
        if changed || crt.next_frame(now).is_some() {
            fb.show(crt.frame(&canvas.px, changed, now));
            let took = now.elapsed();
            stats = (stats.0 + 1, stats.1 + took, stats.2.max(took), stats.3);
            if stats.3.elapsed() > Duration::from_secs(10) && stats.0 >= 30 {
                let (n, total, most, _) = stats;
                eprintln!("portstore: frames: {n}, {:?} each, {most:?} at most", total / n);
                stats = (0, Duration::ZERO, Duration::ZERO, Instant::now());
            }
        }
        let wait = [app.next_draw(), crt.next_frame(now), Some(Duration::from_millis(100))]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(Duration::from_millis(100));
        for key in input.poll(wait) {
            crt.wake(Instant::now());
            app.now = Instant::now();
            if app.key(key) == Flow::Exit && !app.busy {
                break 'shown;
            }
        }
        app.pump();
        let now_sig = app.sig();
        if now_sig != sig {
            if let Some(fx) = fx_between(&sig, &now_sig) {
                crt.start(fx, Instant::now());
            }
            app.since = Instant::now();
            sig = now_sig;
        }
    }
    // The tube switching off.
    let off = Instant::now();
    crt.start(Fx::PowerOff, off);
    while crt.playing(Instant::now()) {
        fb.show(crt.frame(&canvas.px, false, Instant::now()));
        std::thread::sleep(Duration::from_millis(30));
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

/// Screens as PNGs, for looking at them off the device, as the tube shows them: the
/// list in each tab, the page of each port given (or the first), the windows.
/// PORTSTORE_SHOT_THEME picks the theme (green by default).
pub fn shots(store: Store, dir: &std::path::Path, ids: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut app = App::new(store);
    app.still = true;
    // As once the catalog has come, without downloading it.
    app.online = !app.store.index_url.is_empty();
    if let Ok(theme) = std::env::var("PORTSTORE_SHOT_THEME") {
        app.look.theme = theme;
    }
    // PORTSTORE_SHOT_SIZE=752x560: the screens of the Mini v4 and Flip.
    let (w, h) = std::env::var("PORTSTORE_SHOT_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((W, H));
    let mut c = Canvas::new(w, h);
    let mut crt = Crt::new(w, h, app.effects());
    let mut save = |app: &mut App, name: &str| {
        app.draw(&mut c);
        // Pictures come in on their own thread: a little while for them.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(50));
            if app.pictures.pump() {
                app.draw(&mut c);
            }
        }
        crt.effects = app.effects();
        let px = match crt.effects {
            Effects::Off => c.px.clone(),
            _ => crt.lit(&c.px).to_vec(),
        };
        canvas::save_png(&dir.join(format!("{name}.png")), w, h, &px).map_err(|e| e.to_string())
    };
    // The boot screen, typed most of the way.
    app.still = false;
    app.now = Instant::now();
    app.boot = Some(app.now - Duration::from_millis(1800));
    save(&mut app, "boot")?;
    app.boot = None;
    app.still = true;
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
    app.key(Key::Select);
    save(&mut app, "filter")?;
    app.view = View::List;
    app.filter[0] = Some("racing".into());
    save(&mut app, "list-games-racing")?;
    app.filter[0] = None;
    app.key(Key::L2);
    save(&mut app, "system")?;
    app.view = View::List;
    // Homebrew: its list (from the copy at hand), filtered, and an entry's page.
    app.tab = Tab::Homebrew;
    app.filter[2] = Some("GBA".into());
    save(&mut app, "list-homebrew-gba")?;
    app.filter[2] = None;
    if let Some(e) = app.brews.entries.iter().find(|e| !e.shot.is_empty() && !e.desc.is_empty()).cloned() {
        app.view = View::Brew { slug: e.slug.clone(), scroll: 0 };
        save(&mut app, &format!("homebrew-{}", e.slug))?;
    }
    app.tab = Tab::Games;
    // New arrivals, as if the first three ports had just come.
    app.arrivals = app.index.ports.iter().filter(|p| p.id != crate::SELF_ID).take(3).map(|p| p.id.clone()).collect();
    app.view = View::Arrivals { at: 0 };
    save(&mut app, "arrivals")?;
    app.arrivals.clear();
    app.view = View::List;
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
        stoppable: true,
        title: "Installing Mario Kart 64".into(),
        step: Some(Step::Download { done: 600 << 10, total: 1381855 }),
    };
    app.log = vec![
        (0, 0, "LINK OPEN".into()),
        (0, 1, "DOWNLOAD 600.0 KB / 1.3 MB".into()),
    ];
    save(&mut app, "busy")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Port;

    fn app(ports: &[(&str, &str, &[&str])]) -> App {
        // A card of its own each: tests run side by side and keep settings.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("portstore-ui-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let mut app = App::new(Store::open(dir.clone(), dir.join("home")));
        app.look = Look::default();
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
                Item::Brew(b) => format!("hb {}", app.brews.entries[*b].title),
            })
            .collect()
    }

    #[test]
    fn what_ports_run_on_has_a_tab_of_its_own() {
        let mut app = app(&[("Game", "", &["action"]), ("Tool", "", &["app"]), ("love-mini", "", &["app"]), ("Rt", "", &["runtime"])]);
        app.index.ports[0].requires = vec!["love-mini".into()];
        app.tab = Tab::Games;
        assert_eq!(names(&app), ["Game"]);
        app.tab = Tab::Apps;
        assert_eq!(names(&app), ["Tool"], "a runtime is not an app");
        app.tab = Tab::Deps;
        assert_eq!(names(&app), ["love-mini", "Rt"], "required by a port, or a runtime by category");
    }

    #[test]
    fn a_game_on_a_runtime_installs_it_first() {
        let mut app = app(&[("Game", "", &["action"]), ("Runtime", "", &["app"])]);
        app.index.ports[0].requires = vec!["runtime".into()];
        app.details_key(Key::A, "game".into(), 0);
        match &app.view {
            View::Confirm { text, yes: Action::InstallMany(ids) } => {
                assert_eq!(ids, &["runtime", "game"]);
                assert!(text.contains("runs on Runtime"), "{text}");
            }
            other => panic!("{other:?}"),
        }
        // a runtime the catalog does not have: said so, nothing installed
        app.view = View::List;
        app.index.ports[0].requires = vec!["gone".into()];
        app.details_key(Key::A, "game".into(), 0);
        assert!(matches!(&app.view, View::Message { text, .. } if text.contains("gone")));
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
        app.filter[0] = Some("racing".into());
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
    fn select_opens_the_filter_and_a_pick_sets_it() {
        let mut app = app(&[("A", "", &["racing"]), ("B", "", &["arcade"]), ("R", "", &["racing"]), ("C", "", &["app"])]);
        app.key(Key::Select);
        assert_eq!(
            app.choices(),
            [Choice::All(3), Choice::Category("arcade".into(), 1), Choice::Category("racing".into(), 2)]
        );
        assert!(matches!(app.view, View::Filter { at: 0 }));
        app.key(Key::Down);
        app.key(Key::Down);
        app.key(Key::A);
        assert_eq!(app.filter[0].as_deref(), Some("racing"));
        assert!(matches!(app.view, View::List));
        assert_eq!(names(&app), ["A", "R"]);
        // It opens on the filter in force; B closes it as it was.
        app.key(Key::Select);
        assert!(matches!(app.view, View::Filter { at: 2 }));
        app.key(Key::Up);
        app.key(Key::B);
        assert_eq!(app.filter[0].as_deref(), Some("racing"));
        app.key(Key::Select);
        app.key(Key::Up);
        app.key(Key::Up);
        app.key(Key::A);
        assert_eq!(app.filter[0], None, "all again");
    }

    #[test]
    fn development_builds_show_only_when_asked_for() {
        let mut app = app(&[("A", "", &["action"]), ("A Dev", "", &["action"])]);
        app.index.ports[1].dev = true;
        assert_eq!(names(&app), ["A"]);
        app.key(Key::Select);
        let choices = app.choices();
        assert_eq!(choices.last(), Some(&Choice::Dev(1)));
        assert_eq!(choices[2], Choice::Line("CHANNEL"));
        // Down past the line, to the development builds.
        app.key(Key::Up);
        assert!(matches!(app.view, View::Filter { at: 3 }), "{:?}", app.view);
        app.key(Key::A);
        assert!(app.look.dev);
        assert!(matches!(app.view, View::Filter { .. }), "stays open");
        app.key(Key::B);
        assert_eq!(names(&app), ["A", "A Dev"]);
    }

    #[test]
    fn the_system_window_turns_the_theme_and_keeps_it() {
        let mut app = app(&[("A", "", &["x"])]);
        app.key(Key::L2);
        app.key(Key::Right);
        assert_eq!(app.look.theme, "amber");
        app.key(Key::Left);
        app.key(Key::Left);
        assert_eq!(app.look.theme, "classic");
        assert_eq!(app.effects(), Effects::Off, "the classic look plays no tube");
        app.key(Key::Down);
        app.key(Key::Right);
        assert_eq!(app.look.effects, Effects::Calm);
        assert_eq!(Look::load(&app.store.card.home).theme, "classic");
        app.key(Key::B);
        assert!(matches!(app.view, View::List));
    }

    fn brews(app: &mut App, list: &[(&str, &str, &str, &str)]) {
        app.brews.entries = list
            .iter()
            .map(|(slug, title, system, added)| homebrew::Entry {
                slug: slug.to_string(),
                title: title.to_string(),
                system: system.to_string(),
                kind: "game".into(),
                added: added.to_string(),
                ..Default::default()
            })
            .collect();
    }

    #[test]
    fn homebrew_lists_by_title_or_newest_and_filters_by_system() {
        let mut app = app(&[("A", "", &["x"])]);
        brews(&mut app, &[("z", "Zap", "GBA", "2026-01-01"), ("b", "Bloop", "GB", "2026-05-01"), ("c", "Cave", "GBA", "2025-01-01")]);
        app.key(Key::R1);
        app.key(Key::R1);
        assert_eq!(app.tab, Tab::Homebrew);
        assert_eq!(names(&app), ["hb Bloop", "hb Cave", "hb Zap"]);
        app.key(Key::R2);
        assert_eq!(names(&app), ["hb Bloop", "hb Zap", "hb Cave"]);
        app.key(Key::R2);
        app.key(Key::Select);
        assert_eq!(
            app.choices(),
            [
                Choice::All(3),
                Choice::Category("GBA".into(), 2),
                Choice::Category("GB".into(), 1),
                Choice::Line("KIND"),
                Choice::Category("game".into(), 3)
            ]
        );
        app.key(Key::Down);
        app.key(Key::A);
        assert_eq!(names(&app), ["hb Cave", "hb Zap"]);
        app.key(Key::A);
        assert!(matches!(&app.view, View::Brew { slug, .. } if slug == "c"));
        app.key(Key::X);
        assert!(matches!(app.view, View::Brew { .. }), "nothing to remove");
        app.key(Key::A);
        assert!(matches!(&app.view, View::Confirm { yes: Action::InstallBrew(s), .. } if s == "c"));
    }

    #[test]
    fn new_arrivals_open_on_the_list_and_lead_to_a_port() {
        let mut app = app(&[("A", "", &["x"]), ("B", "", &["x"])]);
        app.arrivals = vec!["b".into(), "gone".into()];
        app.pump();
        assert!(matches!(app.view, View::Arrivals { at: 0 }));
        assert_eq!(app.arrivals, ["b"], "only ports still in the catalog");
        app.key(Key::A);
        assert!(matches!(&app.view, View::Details { id, .. } if id == "b"));
        assert!(app.arrivals.is_empty(), "announced once");
    }

    #[test]
    fn pictures_can_be_turned_off() {
        let mut app = app(&[("A", "", &["x"])]);
        app.key(Key::L2);
        app.key(Key::Down);
        app.key(Key::Down);
        let mut seen = Vec::new();
        for _ in 0..3 {
            app.key(Key::Right);
            seen.push(app.look.pictures);
        }
        assert_eq!(seen, [PictureMode::Color, PictureMode::Off, PictureMode::Screen]);
        app.key(Key::Left);
        assert_eq!(Look::load(&app.store.card.home).pictures, PictureMode::Off);
        // A page without its picture still draws, its facts where the picture was.
        app.view = View::Details { id: "a".into(), scroll: 0 };
        app.draw(&mut Canvas::new(W, H));
    }

    #[test]
    fn a_change_of_screen_is_played_on_the_tube() {
        let mut app = app(&[("A", "", &["x"])]);
        let list = app.sig();
        app.key(Key::R1);
        assert_eq!(fx_between(&list, &app.sig()), Some(Fx::Tear));
        app.key(Key::L1);
        app.key(Key::A);
        assert_eq!(fx_between(&list, &app.sig()), Some(Fx::Tear), "a page opens as a tab does");
        let page = app.sig();
        app.key(Key::A);
        assert_eq!(fx_between(&page, &app.sig()), Some(Fx::Blip), "a window over the page");
        app.view = View::List;
        app.under.clear();
        let list = app.sig();
        app.key(Key::L2);
        app.key(Key::Right);
        assert_eq!(fx_between(&list, &app.sig()), Some(Fx::Degauss));
    }

    #[test]
    fn a_jobs_log_keeps_one_line_per_kind_of_step() {
        let mut app = app(&[("A", "", &["x"])]);
        app.note(0, "LINK OPEN".into());
        app.note(1, "DOWNLOAD 1 / 3".into());
        app.note(1, "DOWNLOAD 2 / 3".into());
        app.note(2, "VERIFY".into());
        let lines: Vec<&str> = app.log.iter().map(|l| l.2.as_str()).collect();
        assert_eq!(lines, ["LINK OPEN", "DOWNLOAD 2 / 3", "VERIFY"]);
    }

    #[test]
    fn a_key_during_the_boot_screen_only_skips_it() {
        let mut app = app(&[("A", "", &["x"]), ("B", "", &["x"])]);
        app.boot = Some(app.now);
        app.key(Key::Down);
        assert!(app.boot.is_none());
        assert_eq!(app.pick[0], 0);
    }

    #[test]
    fn port_stores_own_page_offers_no_remove() {
        let mut app = app(&[("Port Store", "", &["app"])]);
        app.index.ports[0].id = crate::SELF_ID.into();
        app.index.ports[0].version = format!("v{}", env!("CARGO_PKG_VERSION"));
        app.view = View::Details { id: crate::SELF_ID.into(), scroll: 0 };
        app.key(Key::X);
        assert!(matches!(app.view, View::Details { .. }), "no Confirm for removing itself");
    }

    #[test]
    fn a_port_gone_from_the_catalog_under_its_dialog_is_said_so() {
        let mut app = app(&[("A", "", &["x"])]);
        app.view = View::Confirm { text: String::new(), yes: Action::Install("gone".into()) };
        app.key(Key::A);
        assert!(!app.busy);
        assert!(matches!(&app.view, View::Message { title, .. } if title == "Not in the catalog"));
    }

    #[test]
    fn a_good_catalog_clears_offline_and_one_downloaded_during_a_job_waits_for_it() {
        let mut app = app(&[("A", "", &["x"])]);
        app.offline = true;
        app.busy = true;
        let mut newer = app.index.clone();
        newer.generated = "later".into();
        app.jobs.0.send(Msg::Quiet(Ok(newer))).unwrap();
        app.pump();
        assert!(app.held_index.is_some(), "held while the job runs");
        assert!(app.offline);
        app.jobs.0.send(Msg::Done(Ok(Finished::Removed))).unwrap();
        app.pump();
        assert!(app.held_index.is_none());
        assert_eq!(app.index.generated, "later");
        assert!(!app.offline);
    }

    #[test]
    fn only_the_newest_check_of_a_port_counts() {
        let mut app = app(&[("A", "", &["x"])]);
        app.needs_gen.insert("a".into(), 2);
        app.jobs.0.send(Msg::Needs("a".into(), 1, vec![Have::Missing], vec![None])).unwrap();
        app.pump();
        assert!(!app.needs.contains_key("a"), "an older check is dropped");
        app.jobs.0.send(Msg::Needs("a".into(), 2, vec![Have::Unknown], vec![None])).unwrap();
        app.pump();
        assert_eq!(app.needs["a"], [Have::Unknown]);
    }

    #[test]
    fn a_page_scrolls_no_further_than_its_end() {
        let mut app = app(&[("A", "", &["x"])]);
        app.view = View::Details { id: "a".into(), scroll: 0 };
        let mut c = Canvas::new(W, H);
        for _ in 0..20 {
            app.draw(&mut c);
            app.key(Key::Down);
        }
        assert!(matches!(app.view, View::Details { scroll, .. } if scroll <= 1), "{:?}", app.view);
    }

    #[test]
    fn every_screen_draws_in_every_theme_and_size() {
        let mut app = app(&[("A", "G", &["x"]), ("B", "", &["app"])]);
        app.still = true;
        for (w, h) in [(W, H), (752, 560)] {
            let mut c = Canvas::new(w, h);
            for (theme, _) in theme::THEMES {
                app.look.theme = theme.into();
                for view in [
                    View::List,
                    View::Details { id: "a".into(), scroll: 0 },
                    View::Filter { at: 0 },
                    View::System { at: 1 },
                    View::Message { title: "T".into(), text: "x ".repeat(400) },
                    View::Busy { title: "T".into(), step: Some(Step::Check), stoppable: true },
                ] {
                    app.view = view;
                    app.draw(&mut c);
                }
            }
        }
    }
}
