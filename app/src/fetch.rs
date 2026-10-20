//! Downloads, with OnionOS's curl (OpenSSL, checked against Port Store's own cacert.pem).
//! file:// addresses are copied directly (tests, a catalog on the card).
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::Digest;

pub struct Fetcher {
    pub curl: PathBuf,
    pub cacert: Option<PathBuf>,
}

impl Fetcher {
    pub fn new(home: &Path) -> Fetcher {
        let onion = Path::new("/mnt/SDCARD/.tmp_update/bin/curl");
        let curl = match std::env::var_os("PORTSTORE_CURL") {
            Some(c) => PathBuf::from(c),
            None if onion.exists() => onion.to_path_buf(),
            None => PathBuf::from("curl"),
        };
        let cacert = Some(home.join("cacert.pem")).filter(|p| p.exists());
        Fetcher { curl, cacert }
    }

    /// Fetch `url` into `to`, telling `progress` the bytes so far. `cancel` stops it.
    pub fn get(
        &self,
        url: &str,
        to: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64),
    ) -> io::Result<()> {
        let part = part_of(to);
        if let Some(path) = url.strip_prefix("file://") {
            copy_with_progress(Path::new(path), &part, cancel, progress)?;
            return fs::rename(part, to);
        }
        let mut cmd = Command::new(&self.curl);
        cmd.args(["-fsSL", "--connect-timeout", "20", "--retry", "2", "-A", "PortStore"]);
        if let Some(ca) = &self.cacert {
            cmd.arg("--cacert").arg(ca);
        }
        if part.exists() {
            cmd.args(["-C", "-"]);
        }
        cmd.arg("-o").arg(&part).arg(url);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| io::Error::other(format!("{}: {e}", self.curl.display())))?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            progress(fs::metadata(&part).map(|m| m.len()).unwrap_or(0));
            std::thread::sleep(Duration::from_millis(150));
        };
        if !status.success() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            // No network at all (a Mini without Wi-Fi, Wi-Fi off): told apart, so the
            // file can be listed for downloading on a computer.
            let kind = match status.code() {
                Some(6) | Some(7) | Some(28) => io::ErrorKind::NotConnected,
                _ => io::ErrorKind::Other,
            };
            return Err(io::Error::new(kind, curl_error(status.code(), err.trim())));
        }
        progress(fs::metadata(&part).map(|m| m.len()).unwrap_or(0));
        fs::rename(part, to)
    }
}

pub fn part_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

fn curl_error(code: Option<i32>, text: &str) -> String {
    let what = match code {
        Some(6) => "no connection (cannot resolve the server): is Wi-Fi on?",
        Some(7) | Some(28) => "no connection to the server: is Wi-Fi on?",
        Some(22) => "the server refused the file (it may have been moved)",
        Some(35) | Some(60) | Some(77) => {
            "the secure connection failed: check the date and time (Clock app)"
        }
        Some(23) => "could not write the file: is the card full?",
        _ => "download failed",
    };
    match text.is_empty() {
        true => format!("{what} (curl {})", code.unwrap_or(-1)),
        false => format!("{what} (curl {}: {text})", code.unwrap_or(-1)),
    }
}

fn copy_with_progress(
    from: &Path,
    to: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> io::Result<()> {
    let mut src = fs::File::open(from)?;
    let mut dst = fs::File::create(to)?;
    let mut buf = vec![0; 1 << 16];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n])?;
        done += n as u64;
        progress(done);
    }
}

pub fn sha256_file(path: &Path, cancel: &AtomicBool) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
