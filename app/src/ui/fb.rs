//! The Mini's framebuffer (fb0, 640x480, 32 bits): each frame is written to the page
//! not on show, turned half a turn (the panel is mounted upside down), then panned to.
use std::ffi::{c_ulong, c_void};
use std::os::fd::AsRawFd;

const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;
const FBIOPAN_DISPLAY: c_ulong = 0x4606;

/// fb_var_screeninfo, as 40 words.
type VarInfo = [u32; 40];
const XRES: usize = 0;
const YRES: usize = 1;
const YRES_VIRTUAL: usize = 3;
const YOFFSET: usize = 5;
const BITS: usize = 6;
const RED: usize = 8;
const GREEN: usize = 11;
const BLUE: usize = 14;
/// fb_var_screeninfo.transp: offset, length (no alpha channel when 0).
const TRANSP: usize = 17;
const TRANSP_LENGTH: usize = 18;

pub struct Fb {
    file: std::fs::File,
    var: VarInfo,
    map: *mut u8,
    map_len: usize,
    stride: usize,
    pages: u32,
    shown: u32,
    shifts: [u32; 3],
    /// The opaque value of the alpha channel where the layout has one.
    alpha: u32,
}

impl Fb {
    pub fn open() -> Result<Fb, String> {
        let fail = |what: &str| format!("/dev/fb0: {what}: {}", std::io::Error::last_os_error());
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .map_err(|e| format!("/dev/fb0: {e}"))?;
        let fd = file.as_raw_fd();
        let mut var: VarInfo = [0; 40];
        let mut fix = [0u8; 80];
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, var.as_mut_ptr()) < 0 {
                return Err(fail("FBIOGET_VSCREENINFO"));
            }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO as _, fix.as_mut_ptr()) < 0 {
                return Err(fail("FBIOGET_FSCREENINFO"));
            }
        }
        if var[BITS] != 32 || var[XRES] == 0 || var[YRES] == 0 {
            return Err(format!("/dev/fb0: {}x{} at {} bits, not 32", var[XRES], var[YRES], var[BITS]));
        }
        // fb_fix_screeninfo: id[16], smem_start (unsigned long), smem_len, type,
        // type_aux, visual, three u16 steps and padding, then line_length.
        let word = |at: usize| u32::from_ne_bytes([fix[at], fix[at + 1], fix[at + 2], fix[at + 3]]);
        let ul = std::mem::size_of::<c_ulong>();
        let smem_len = word(16 + ul) as usize;
        let line_length = word(16 + ul + 24) as usize;
        let pages = (var[YRES_VIRTUAL] / var[YRES]).clamp(1, 2);
        let stride = line_length.max(var[XRES] as usize * 4);
        let map_len = smem_len.max(stride * var[YRES] as usize * pages as usize);
        let map = unsafe {
            libc::mmap(std::ptr::null_mut(), map_len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
        };
        if map == libc::MAP_FAILED {
            return Err(fail("mmap"));
        }
        let shown = (var[YOFFSET] / var[YRES]).min(pages - 1);
        Ok(Fb {
            file,
            var,
            map: map as *mut u8,
            map_len,
            stride,
            pages,
            shown,
            shifts: [var[RED], var[GREEN], var[BLUE]],
            // The Mini's ARGB8888 takes 0xff at the top, as before (and when the
            // driver tells no alpha channel there); other layouts only where it is.
            alpha: match (var[TRANSP_LENGTH], [var[RED], var[GREEN], var[BLUE]]) {
                (0, [16, 8, 0]) => 0xff00_0000,
                (0, _) => 0,
                (n, _) => (((1u64 << n.min(8)) - 1) as u32) << var[TRANSP],
            },
        })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.var[XRES] as usize, self.var[YRES] as usize)
    }

    /// Show a frame of 0x00RRGGBB words, top row first, of the panel's size.
    pub fn show(&mut self, px: &[u32]) {
        let (w, h) = self.size();
        if px.len() < w * h {
            return;
        }
        let page = (self.shown + 1) % self.pages;
        let stride = self.stride / 4;
        let base = page as usize * self.stride * h;
        // The mapping is page aligned and the stride a whole number of pixels.
        let out = unsafe { std::slice::from_raw_parts_mut(self.map.add(base) as *mut u32, stride * h) };
        let [rs, gs, bs] = self.shifts;
        for y in 0..h {
            let row = &mut out[(h - 1 - y) * stride..(h - 1 - y) * stride + w];
            let src = &px[y * w..(y + 1) * w];
            let alpha = self.alpha;
            if (rs, gs, bs) == (16, 8, 0) {
                for (d, &s) in row.iter_mut().zip(src.iter().rev()) {
                    *d = alpha | s;
                }
            } else {
                for (d, &s) in row.iter_mut().zip(src.iter().rev()) {
                    *d = alpha | (s >> 16 & 0xff) << rs | (s >> 8 & 0xff) << gs | (s & 0xff) << bs;
                }
            }
        }
        self.var[YOFFSET] = page * h as u32;
        self.shown = page;
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), FBIOPAN_DISPLAY as _, self.var.as_mut_ptr());
        }
    }
}

impl Drop for Fb {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.map as *mut c_void, self.map_len);
        }
    }
}
