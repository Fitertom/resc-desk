//! Previews in «Проводник»: pictures and videos shown as themselves instead of a type icon.
//!
//! The desktop's shared thumbnail cache (`~/.cache/thumbnails/large`, the freedesktop spec:
//! a PNG named by the MD5 of the file's URI, stamped with its mtime) is read first, so what
//! Glint, GNOME's or KDE's file managers made is used, and what is made here serves them.
//! Missing ones are made on two worker threads: JPEG / PNG by our own decoders, a video's frame
//! by ffmpeg (when it is installed).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// The cache's "large" size: crisp at the biggest icon view.
const SIZE: usize = 256;
/// Previews kept in memory; the oldest asked-for go first.
const KEEP: usize = 600;

pub struct Thumb {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    pub video: bool,
}

enum State {
    Waiting,
    Ready(Arc<Thumb>),
    None,
}

struct Shared {
    queue: Mutex<VecDeque<(PathBuf, i64, bool)>>,
    cv: Condvar,
    done: Mutex<Vec<(PathBuf, Option<Thumb>)>>,
}

pub struct Thumbs {
    map: HashMap<PathBuf, (State, u64)>,
    tick: u64,
    shared: Option<Arc<Shared>>,
}

/// What kind of preview a file can have: picture, video, or none.
pub fn kind(ext: &str) -> Option<bool> {
    match ext {
        "jpg" | "jpeg" | "jpe" | "jfif" | "png" => Some(false),
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" | "ts" | "m2ts" | "3gp" | "ogv" => Some(true),
        _ => None,
    }
}

impl Thumbs {
    pub fn new() -> Thumbs {
        Thumbs { map: HashMap::new(), tick: 0, shared: None }
    }

    /// The preview of `path` if there is one yet; asks for it otherwise.
    pub fn get(&mut self, path: &Path, mtime: i64, video: bool) -> Option<Arc<Thumb>> {
        self.tick += 1;
        let tick = self.tick;
        if let Some((s, t)) = self.map.get_mut(path) {
            *t = tick;
            return match s {
                State::Ready(th) => Some(th.clone()),
                _ => None,
            };
        }
        self.map.insert(path.to_path_buf(), (State::Waiting, tick));
        let sh = self.shared.get_or_insert_with(|| {
            let s = Arc::new(Shared { queue: Mutex::new(VecDeque::new()), cv: Condvar::new(), done: Mutex::new(Vec::new()) });
            for _ in 0..2 {
                let s = s.clone();
                std::thread::spawn(move || worker(s));
            }
            s
        });
        // Newest first: what is on screen now beats what was scrolled past.
        sh.queue.lock().unwrap().push_front((path.to_path_buf(), mtime, video));
        sh.cv.notify_one();
        if self.map.len() > KEEP {
            let mut v: Vec<(PathBuf, u64)> = self.map.iter().map(|(p, (_, t))| (p.clone(), *t)).collect();
            v.sort_by_key(|x| x.1);
            for (p, _) in v.into_iter().take(self.map.len() - KEEP) {
                self.map.remove(&p);
            }
        }
        None
    }

    /// Previews being made.
    pub fn busy(&self) -> bool {
        self.map.values().any(|(s, _)| matches!(s, State::Waiting))
    }

    /// Take what the workers made: true when something new is to be drawn.
    pub fn poll(&mut self) -> bool {
        let Some(sh) = &self.shared else { return false };
        let done = std::mem::take(&mut *sh.done.lock().unwrap());
        let any = !done.is_empty();
        for (p, t) in done {
            if let Some(e) = self.map.get_mut(&p) {
                e.0 = match t {
                    Some(t) => State::Ready(Arc::new(t)),
                    None => State::None,
                };
            }
        }
        any
    }

    /// A folder left: what was asked for it and not started is not wanted any more.
    pub fn forget_queue(&mut self) {
        if let Some(sh) = &self.shared {
            let gone: Vec<_> = sh.queue.lock().unwrap().drain(..).collect();
            for (p, ..) in gone {
                self.map.remove(&p);
            }
        }
    }

    /// The file changed: make it again.
    pub fn invalidate(&mut self, path: &Path) {
        self.map.remove(path);
    }
}

fn worker(s: Arc<Shared>) {
    loop {
        let (path, mtime, video) = {
            let mut q = s.queue.lock().unwrap();
            loop {
                if let Some(j) = q.pop_front() {
                    break j;
                }
                q = s.cv.wait(q).unwrap();
            }
        };
        let t = make(&path, mtime, video);
        s.done.lock().unwrap().push((path, t));
    }
}

fn make(path: &Path, mtime: i64, video: bool) -> Option<Thumb> {
    let uri = file_uri(path);
    let cache = cache_dir().map(|d| d.join(format!("{}.png", hex(&md5(uri.as_bytes())))));
    if let Some(c) = &cache {
        if let Ok(data) = std::fs::read(c) {
            // Stale when the file changed after it was made.
            if png_text(&data, "Thumb::MTime").and_then(|m| m.parse::<i64>().ok()).is_none_or(|m| m == mtime) {
                if let Some(img) = crate::wallpaper::decode_png(&data) {
                    return Some(Thumb { w: img.w, h: img.h, px: img.px, video });
                }
            }
        }
    }
    let img = if video { video_frame(path)? } else { crate::wallpaper::decode_file(&path.to_string_lossy()).ok()? };
    let (w, h) = fit(img.w, img.h, SIZE);
    let px = if (w, h) == (img.w, img.h) { img.px } else { crate::wallpaper::thumb(&img, w, h) };
    if let Some(c) = cache {
        if let Some(d) = c.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let png = crate::png::encode_text(&px, w as i32, h as i32, w, &[("Thumb::URI", &uri), ("Thumb::MTime", &mtime.to_string()), ("Software", "resc-desk")]);
        // Written whole, then renamed: a reader never sees half a file.
        let tmp = c.with_extension(format!("png.{}", std::process::id()));
        if std::fs::write(&tmp, png).is_ok() {
            let _ = std::fs::rename(&tmp, &c);
        }
    }
    Some(Thumb { w, h, px, video })
}

/// A representative frame (ffmpeg's `thumbnail` filter: the most typical of the first ones,
/// not a black fade-in), turned as the phone meant it, at most SIZE wide or tall.
fn video_frame(path: &Path) -> Option<crate::wallpaper::Image> {
    let vf = format!("thumbnail=40,scale={SIZE}:{SIZE}:force_original_aspect_ratio=decrease");
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-threads", "2", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &vf, "-f", "image2pipe", "-vcodec", "png", "-"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    crate::wallpaper::decode_png(&out.stdout)
}

fn fit(w: usize, h: usize, max: usize) -> (usize, usize) {
    if w <= max && h <= max {
        return (w.max(1), h.max(1));
    }
    let k = max as f64 / w.max(h) as f64;
    (((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1))
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("thumbnails/large"))
}

/// `file://` URI, percent-encoded as the spec wants (the cache key depends on it).
pub fn file_uri(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~!$&'()*+,;=:@".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// A tEXt chunk's value.
fn png_text(data: &[u8], key: &str) -> Option<String> {
    let mut at = 8;
    while at + 8 <= data.len() {
        let len = u32::from_be_bytes(data[at..at + 4].try_into().ok()?) as usize;
        let kind = &data[at + 4..at + 8];
        let body = data.get(at + 8..at + 8 + len)?;
        if kind == b"tEXt" {
            if let Some(z) = body.iter().position(|&b| b == 0) {
                if &body[..z] == key.as_bytes() {
                    return Some(String::from_utf8_lossy(&body[z + 1..]).into_owned());
                }
            }
        }
        if kind == b"IDAT" {
            return None;
        }
        at += 12 + len;
    }
    None
}

/// MD5 (RFC 1321): the thumbnail cache names its files by it.
pub fn md5(msg: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11,
        16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut m = msg.to_vec();
    let bits = (msg.len() as u64).wrapping_mul(8);
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&bits.to_le_bytes());
    for chunk in m.chunks(64) {
        let w: Vec<u32> = chunk.chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(w[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn md5_and_uri() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"The quick brown fox jumps over the lazy dog")), "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(file_uri(Path::new("/home/a b/фото.jpg")), "file:///home/a%20b/%D1%84%D0%BE%D1%82%D0%BE.jpg");
    }
    #[test]
    fn png_text_roundtrip() {
        let png = crate::png::encode_text(&[0xff0000; 4], 2, 2, 2, &[("Thumb::MTime", "123")]);
        assert_eq!(png_text(&png, "Thumb::MTime").as_deref(), Some("123"));
        assert!(crate::wallpaper::decode_png(&png).is_some());
    }
}
