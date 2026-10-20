//! Reading release archives: zip (stored or deflated), tar and tar.gz; and a folder
//! ("dir": a 7z archive unpacked by 7-Zip, see `unpack_7z`). Members are
//! regular files and folders; links are listed (`link`) but never read, and whether a
//! path is safe to write (`safe_name`) is the installer's to check.
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub size: u64,
    pub dir: bool,
    pub link: bool,
    /// Zip only: where the member's local header starts, how it is stored, its CRC-32.
    at: u64,
    method: u16,
    crc: u32,
    packed: u64,
}

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.split('/').any(|part| part == "..")
}

/// Every member of the archive at `path`, of `kind` ("zip", "tar", "tar.gz").
pub fn members(path: &Path, kind: &str) -> io::Result<Vec<Member>> {
    match kind {
        "zip" => zip_members(&mut fs::File::open(path)?),
        "dir" => dir_members(path),
        "tar" | "tar.gz" => {
            let mut out = Vec::new();
            tar_walk(path, kind, |m, _| {
                out.push(m.clone());
                Ok(())
            })?;
            Ok(out)
        }
        _ => Err(bad(format!("cannot unpack a {kind} file"))),
    }
}

/// Hand every file member (not links), with a reader of its bytes, to `each`, in
/// archive order.
pub fn extract(
    path: &Path,
    kind: &str,
    mut each: impl FnMut(&Member, &mut dyn Read) -> io::Result<()>,
) -> io::Result<()> {
    match kind {
        "zip" => {
            let mut file = fs::File::open(path)?;
            let members = zip_members(&mut file)?;
            for m in members.iter().filter(|m| !m.dir && !m.link) {
                zip_read(&mut file, m, &mut each)?;
            }
            Ok(())
        }
        "tar" | "tar.gz" => tar_walk(path, kind, |m, r| match m.dir || m.link {
            true => Ok(()),
            false => each(m, r),
        }),
        "dir" => {
            for m in dir_members(path)?.iter().filter(|m| !m.dir && !m.link) {
                each(m, &mut fs::File::open(path.join(&m.name))?)?;
            }
            Ok(())
        }
        _ => Err(bad(format!("cannot unpack a {kind} file"))),
    }
}

/// The files and folders under `root`, by name from it, links told (not followed).
fn dir_members(root: &Path) -> io::Result<Vec<Member>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Member>) -> io::Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let meta = fs::symlink_metadata(&path)?;
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
            let dir = meta.is_dir();
            out.push(Member {
                name: if dir { format!("{rel}/") } else { rel },
                size: if meta.is_file() { meta.len() } else { 0 },
                dir,
                link: meta.file_type().is_symlink(),
                at: 0,
                method: 0,
                crc: 0,
                packed: 0,
            });
            if dir {
                walk(root, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

/// Unpack a 7z archive into `to` (made empty first) with 7-Zip: OnionOS's, or
/// PORTSTORE_7Z. Its members are then those of `to`, as kind "dir".
pub fn unpack_7z(archive: &Path, to: &Path) -> io::Result<()> {
    let onion = Path::new("/mnt/SDCARD/.tmp_update/bin/7z");
    let seven = match std::env::var_os("PORTSTORE_7Z") {
        Some(p) => std::path::PathBuf::from(p),
        None if onion.exists() => onion.to_path_buf(),
        None => std::path::PathBuf::from("7z"),
    };
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to)?;
    let mut out_dir = std::ffi::OsString::from("-o");
    out_dir.push(to);
    let done = std::process::Command::new(&seven)
        .arg("x")
        .arg("-y")
        .arg("-bd")
        .arg(out_dir)
        .arg(archive)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| bad(format!("{}: {e}", seven.display())))?;
    if !done.status.success() {
        let _ = fs::remove_dir_all(to);
        let err = String::from_utf8_lossy(&done.stderr);
        return Err(bad(format!("7-Zip could not unpack the file: {}", err.trim())));
    }
    Ok(())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn zip_members(file: &mut fs::File) -> io::Result<Vec<Member>> {
    let len = file.seek(SeekFrom::End(0))?;
    // The end record is the last 22 bytes, before a comment of up to 64 KiB.
    let tail_len = len.min(22 + 65_535);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(&tail, i) == 0x0605_4b50)
        .ok_or_else(|| bad("not a zip file"))?;
    let count = u16_at(&tail, end + 10) as usize;
    let dir_len = u32_at(&tail, end + 12) as u64;
    let dir_at = u32_at(&tail, end + 16) as u64;
    if count == 0xffff || dir_at == 0xffff_ffff {
        return Err(bad("zip64 archives are not read"));
    }
    if dir_at + dir_len > len {
        return Err(bad("zip directory out of the file"));
    }
    file.seek(SeekFrom::Start(dir_at))?;
    let mut dir = vec![0; dir_len as usize];
    file.read_exact(&mut dir)?;
    let mut out = Vec::with_capacity(count);
    let mut at = 0;
    for _ in 0..count {
        if at + 46 > dir.len() || u32_at(&dir, at) != 0x0201_4b50 {
            return Err(bad("broken zip directory"));
        }
        let made_by = u16_at(&dir, at + 4) >> 8;
        let flags = u16_at(&dir, at + 8);
        let method = u16_at(&dir, at + 10);
        let crc = u32_at(&dir, at + 16);
        let packed = u32_at(&dir, at + 20) as u64;
        let size = u32_at(&dir, at + 24) as u64;
        let name_len = u16_at(&dir, at + 28) as usize;
        let extra_len = u16_at(&dir, at + 30) as usize;
        let comment_len = u16_at(&dir, at + 32) as usize;
        let attrs = u32_at(&dir, at + 38);
        let local = u32_at(&dir, at + 42) as u64;
        let name_at = at + 46;
        if name_at + name_len > dir.len() {
            return Err(bad("broken zip directory"));
        }
        let mut name = String::from_utf8_lossy(&dir[name_at..name_at + name_len]).into_owned();
        // Zips made on Windows may separate folders with backslashes; unzip reads
        // them as slashes.
        if made_by == 0 {
            name = name.replace('\\', "/");
        }
        if flags & 1 != 0 {
            return Err(bad(format!("encrypted member: {name}")));
        }
        if size == 0xffff_ffff || packed == 0xffff_ffff || local == 0xffff_ffff {
            return Err(bad("zip64 archives are not read"));
        }
        // Unix-made archives keep the file type in the high half of the attributes.
        let link = made_by == 3 && (attrs >> 16) & 0o170000 == 0o120000;
        let dir_entry = name.ends_with('/');
        out.push(Member {
            name: name.trim_end_matches('/').to_string() + if dir_entry { "/" } else { "" },
            size,
            dir: dir_entry,
            link,
            at: local,
            method,
            crc,
            packed,
        });
        at = name_at + name_len + extra_len + comment_len;
    }
    Ok(out)
}

fn zip_read(
    file: &mut fs::File,
    m: &Member,
    each: &mut impl FnMut(&Member, &mut dyn Read) -> io::Result<()>,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(m.at))?;
    let mut head = [0u8; 30];
    file.read_exact(&mut head)?;
    if u32_at(&head, 0) != 0x0403_4b50 {
        return Err(bad(format!("broken zip member: {}", m.name)));
    }
    let skip = u16_at(&head, 26) as i64 + u16_at(&head, 28) as i64;
    file.seek(SeekFrom::Current(skip))?;
    let packed = (&mut *file).take(m.packed);
    let mut reader: Box<dyn Read> = match m.method {
        0 => Box::new(packed),
        8 => Box::new(flate2::read::DeflateDecoder::new(packed)),
        n => return Err(bad(format!("{}: zip method {n} is not read", m.name))),
    };
    let mut checked = Crc {
        inner: &mut reader,
        hasher: crc32fast::Hasher::new(),
        left: m.size,
    };
    each(m, &mut checked)?;
    // Whatever the caller left unread still has to match.
    io::copy(&mut checked, &mut io::sink())?;
    if checked.hasher.clone().finalize() != m.crc || checked.left != 0 {
        return Err(bad(format!("{}: damaged (CRC or size does not match)", m.name)));
    }
    Ok(())
}

struct Crc<'a> {
    inner: &'a mut dyn Read,
    hasher: crc32fast::Hasher,
    left: u64,
}

impl Read for Crc<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.left = self.left.checked_sub(n as u64).ok_or_else(|| bad("member longer than said"))?;
        Ok(n)
    }
}

fn octal(field: &[u8]) -> io::Result<u64> {
    // GNU tar writes sizes past 8 GiB in base 256, flagged by the top bit.
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return Ok(field[1..].iter().fold(0u64, |n, &b| n << 8 | b as u64));
    }
    let text: String = field
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| bad("broken tar header"))
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn tar_walk(
    path: &Path,
    kind: &str,
    mut each: impl FnMut(&Member, &mut dyn Read) -> io::Result<()>,
) -> io::Result<()> {
    let file = io::BufReader::new(fs::File::open(path)?);
    let mut r: Box<dyn Read> = match kind {
        "tar.gz" => Box::new(flate2::read::MultiGzDecoder::new(file)),
        _ => Box::new(file),
    };
    let mut long_name: Option<String> = None;
    let mut head = [0u8; 512];
    loop {
        if read_block(&mut r, &mut head)? == 0 || head.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let size = octal(&head[124..136])?;
        let kind = head[156];
        let padded = size.div_ceil(512) * 512;
        let mut body = (&mut r).take(size);
        match kind {
            // GNU long name, and pax headers (only "path" is taken from them).
            b'L' | b'x' => {
                let mut text = Vec::new();
                body.read_to_end(&mut text)?;
                if kind == b'L' {
                    long_name = Some(cstr(&text));
                } else {
                    long_name = pax_path(&text).or(long_name);
                }
            }
            b'g' => {
                io::copy(&mut body, &mut io::sink())?;
            }
            b'0' | 0 | b'7' | b'5' => {
                let mut name = long_name.take().unwrap_or_else(|| {
                    let prefix = cstr(&head[345..500]);
                    let name = cstr(&head[0..100]);
                    let ustar = &head[257..262] == b"ustar";
                    match ustar && !prefix.is_empty() {
                        true => format!("{prefix}/{name}"),
                        false => name,
                    }
                });
                while let Some(rest) = name.strip_prefix("./") {
                    name = rest.to_string();
                }
                let dir = kind == b'5' || name.ends_with('/');
                let name = name.trim_end_matches('/').to_string() + if dir { "/" } else { "" };
                if !(name.is_empty() || name == "/") {
                    let m = Member {
                        name,
                        size: if dir { 0 } else { size },
                        dir,
                        link: false,
                        at: 0,
                        method: 0,
                        crc: 0,
                        packed: size,
                    };
                    each(&m, &mut body)?;
                }
                io::copy(&mut body, &mut io::sink())?;
            }
            b'1' | b'2' => {
                let m = Member {
                    name: long_name.take().unwrap_or_else(|| cstr(&head[0..100])),
                    size: 0,
                    dir: false,
                    link: true,
                    at: 0,
                    method: 0,
                    crc: 0,
                    packed: 0,
                };
                each(&m, &mut io::empty())?;
                io::copy(&mut body, &mut io::sink())?;
            }
            // Devices, fifos and the like: not for an SD card.
            _ => {
                io::copy(&mut body, &mut io::sink())?;
            }
        }
        let mut pad = vec![0; (padded - size) as usize];
        r.read_exact(&mut pad)?;
    }
}

fn read_block(r: &mut dyn Read, buf: &mut [u8; 512]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(0),
            0 => return Err(bad("tar cut short")),
            n => got += n,
        }
    }
    Ok(got)
}

fn pax_path(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    text.lines().find_map(|line| {
        let (_, record) = line.split_once(' ')?;
        record.strip_prefix("path=").map(str::to_string)
    })
}
