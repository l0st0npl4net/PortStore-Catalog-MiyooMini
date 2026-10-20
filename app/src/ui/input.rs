//! The Mini's buttons: keyboard keys on an evdev node (Onion's keymap_hw.h).
use std::io::Read;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    Menu,
}

pub fn key_of(code: u16) -> Option<Key> {
    Some(match code {
        103 => Key::Up,
        108 => Key::Down,
        105 => Key::Left,
        106 => Key::Right,
        57 => Key::A,
        29 => Key::B,
        42 => Key::X,
        56 => Key::Y,
        18 => Key::L1,
        20 => Key::R1,
        15 => Key::L2,
        14 => Key::R2,
        97 => Key::Select,
        28 => Key::Start,
        1 => Key::Menu,
        _ => return None,
    })
}

/// struct input_event: a timeval (two longs), then type, code and value.
const EVENT_BYTES: usize = 2 * std::mem::size_of::<std::ffi::c_long>() + 8;
const EV_KEY: u16 = 1;
const REPEAT_AFTER: Duration = Duration::from_millis(380);
const REPEAT_EVERY: Duration = Duration::from_millis(70);

pub struct Input {
    files: Vec<std::fs::File>,
    held: Option<(Key, Instant)>,
}

impl Input {
    pub fn open() -> Input {
        let mut files = Vec::new();
        for n in 0..4 {
            if let Ok(f) = std::fs::File::open(format!("/dev/input/event{n}")) {
                unsafe {
                    let fd = f.as_raw_fd();
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                }
                files.push(f);
            }
        }
        Input { files, held: None }
    }

    /// Presses within `wait` (and repeats of a held direction).
    pub fn poll(&mut self, wait: Duration) -> Vec<Key> {
        let mut fds: Vec<libc::pollfd> = self
            .files
            .iter()
            .map(|f| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        let wait = match self.held {
            Some((_, next)) => wait.min(next.saturating_duration_since(Instant::now())),
            None => wait,
        };
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as _, wait.as_millis() as i32);
        }
        let mut keys = Vec::new();
        let mut buf = [0u8; EVENT_BYTES * 16];
        for f in &mut self.files {
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for ev in buf[..n].chunks_exact(EVENT_BYTES) {
                    let at = EVENT_BYTES - 8;
                    let kind = u16::from_le_bytes([ev[at], ev[at + 1]]);
                    let code = u16::from_le_bytes([ev[at + 2], ev[at + 3]]);
                    let value = i32::from_le_bytes([ev[at + 4], ev[at + 5], ev[at + 6], ev[at + 7]]);
                    let Some(key) = (kind == EV_KEY).then(|| key_of(code)).flatten() else {
                        continue;
                    };
                    match value {
                        1 => {
                            keys.push(key);
                            let repeats = matches!(key, Key::Up | Key::Down | Key::Left | Key::Right);
                            self.held = repeats.then(|| (key, Instant::now() + REPEAT_AFTER));
                        }
                        0 if self.held.is_some_and(|(k, _)| k == key) => self.held = None,
                        _ => {}
                    }
                }
            }
        }
        if let Some((key, next)) = self.held {
            if Instant::now() >= next {
                keys.push(key);
                self.held = Some((key, Instant::now() + REPEAT_EVERY));
            }
        }
        keys
    }
}
