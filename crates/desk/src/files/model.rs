//! What the file manager shows: a folder's entries (std::fs, no libraries), their types
//! (by extension, names in Russian like Windows' «Тип» column), Windows-style natural
//! order, the places of the navigation pane (home folders from user-dirs.dirs, disks from
//! /proc/self/mountinfo) and Windows' ways of writing sizes and dates.
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    /// A folder (or a link to one).
    pub dir: bool,
    pub link: bool,
    /// A link whose target is gone.
    pub broken: bool,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: i64,
    pub ctime: i64,
    pub mode: u32,
    pub uid: u32,
    pub hidden: bool,
    /// Lower-case extension ("tar.gz" for the compound ones).
    pub ext: String,
}

impl Entry {
    pub fn exec(&self) -> bool {
        !self.dir && self.mode & 0o111 != 0
    }

    pub fn kind(&self) -> Kind {
        kind_of(self)
    }

    /// Readable by us (for folders: can be listed).
    pub fn readable(&self) -> bool {
        let me = sys::uid();
        if me == 0 {
            return true;
        }
        let bits = if self.uid == me { self.mode >> 6 } else { self.mode };
        bits & 0o4 != 0 && (!self.dir || bits & 0o1 != 0)
    }
}

pub fn ext_of(name: &str) -> String {
    let l = name.to_lowercase();
    for c in ["pkg.tar.zst", "tar.gz", "tar.xz", "tar.zst", "tar.bz2"] {
        if l.len() > c.len() + 1 && l.ends_with(&format!(".{c}")) {
            return c.to_string();
        }
    }
    match l.rfind('.') {
        Some(i) if i > 0 && i + 1 < l.len() => l[i + 1..].to_string(),
        _ => String::new(),
    }
}

/// The name without its extension (all of it for folders).
pub fn stem_len(name: &str, dir: bool) -> usize {
    if dir {
        return name.chars().count();
    }
    let e = ext_of(name);
    let n = name.chars().count();
    if e.is_empty() { n } else { n - e.chars().count() - 1 }
}

fn entry(path: PathBuf, name: String, hidden_list: &[String]) -> Option<Entry> {
    let lm = std::fs::symlink_metadata(&path).ok()?;
    let link = lm.file_type().is_symlink();
    let (m, broken) = if link {
        match std::fs::metadata(&path) {
            Ok(m) => (m, false),
            Err(_) => (lm.clone(), true),
        }
    } else {
        (lm.clone(), false)
    };
    let dir = m.is_dir();
    let hidden = name.starts_with('.') || hidden_list.iter().any(|h| *h == name);
    let ext = if dir { String::new() } else { ext_of(&name) };
    Some(Entry {
        size: if dir { 0 } else { m.len() },
        mtime: m.mtime(),
        ctime: m.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(m.ctime(), |d| d.as_secs() as i64),
        mode: m.mode(),
        uid: m.uid(),
        name,
        path,
        dir,
        link,
        broken,
        hidden,
        ext,
    })
}

/// One entry for `path` (for the address bar and the places).
pub fn stat(path: &Path) -> Option<Entry> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    entry(path.to_path_buf(), name, &[])
}

/// A folder's entries (hidden ones included; the view filters them).
pub fn list(dir: &Path) -> std::io::Result<Vec<Entry>> {
    // `.hidden`: names hidden the way Linux file managers do it (Windows' "Hidden" attribute).
    let hidden: Vec<String> = std::fs::read_to_string(dir.join(".hidden")).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(en) = entry(e.path(), name, &hidden) {
            v.push(en);
        }
    }
    Ok(v)
}

// ------------------------------------------------------------------ order

/// Windows' StrCmpLogicalW: digit runs compare as numbers, letters without case; «ё» next to «е».
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let key = |c: char| -> u32 {
        let l = c.to_lowercase().next().unwrap_or(c);
        match l {
            'ё' => 'е' as u32 * 2 + 1,
            _ => l as u32 * 2,
        }
    };
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(c) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(c);
                    y.next();
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let o = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2)).then_with(|| n2.len().cmp(&n1.len()));
                if o != Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = key(c).cmp(&key(d));
                if o != Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

// ------------------------------------------------------------------ types

pub struct Kind {
    /// «Тип» column, like Windows («Документ PDF», «Файл "XYZ"»).
    pub name: String,
    /// Icon theme names, `;`-separated fallbacks.
    pub icon: String,
    pub mime: &'static str,
    pub image: bool,
}

/// ext → (mime, Russian name, icon names).
const TYPES: &[(&str, &str, &str, &str)] = &[
    ("pdf", "application/pdf", "Документ PDF", "application-pdf"),
    ("png", "image/png", "Изображение PNG", "image-png;image-x-generic"),
    ("jpg", "image/jpeg", "Изображение JPEG", "image-jpeg;image-x-generic"),
    ("jpeg", "image/jpeg", "Изображение JPEG", "image-jpeg;image-x-generic"),
    ("webp", "image/webp", "Изображение WebP", "image-webp;image-x-generic"),
    ("gif", "image/gif", "Изображение GIF", "image-gif;image-x-generic"),
    ("bmp", "image/bmp", "Изображение BMP", "image-bmp;image-x-generic"),
    ("svg", "image/svg+xml", "Изображение SVG", "image-svg+xml;image-x-generic"),
    ("ico", "image/vnd.microsoft.icon", "Значок", "image-x-ico;image-x-generic"),
    ("txt", "text/plain", "Текстовый документ", "text-plain;text-x-generic"),
    ("md", "text/markdown", "Документ Markdown", "text-markdown;text-x-markdown;text-x-generic"),
    ("csv", "text/csv", "Таблица CSV", "text-csv;x-office-spreadsheet"),
    ("log", "text/x-log", "Журнал", "text-x-log;text-x-generic"),
    ("conf", "text/plain", "Файл настроек", "text-x-generic"),
    ("ini", "text/plain", "Файл настроек", "text-x-generic"),
    ("toml", "application/toml", "Файл настроек TOML", "text-x-generic"),
    ("json", "application/json", "Файл JSON", "application-json;text-x-generic"),
    ("xml", "application/xml", "Документ XML", "text-xml;text-x-generic"),
    ("html", "text/html", "Документ HTML", "text-html"),
    ("htm", "text/html", "Документ HTML", "text-html"),
    ("css", "text/css", "Таблица стилей CSS", "text-css;text-x-generic"),
    ("js", "text/javascript", "Сценарий JavaScript", "application-javascript;text-javascript;text-x-script"),
    ("py", "text/x-python", "Сценарий Python", "text-x-python;text-x-script"),
    ("rs", "text/rust", "Исходный код Rust", "text-rust;text-x-rust;text-x-script"),
    ("c", "text/x-csrc", "Исходный код C", "text-x-csrc;text-x-script"),
    ("h", "text/x-chdr", "Заголовок C", "text-x-chdr;text-x-script"),
    ("cpp", "text/x-c++src", "Исходный код C++", "text-x-c++src;text-x-script"),
    ("sh", "application/x-shellscript", "Сценарий оболочки", "application-x-shellscript;text-x-script"),
    ("mp3", "audio/mpeg", "Аудио MP3", "audio-mpeg;audio-x-generic"),
    ("flac", "audio/flac", "Аудио FLAC", "audio-flac;audio-x-flac;audio-x-generic"),
    ("ogg", "audio/ogg", "Аудио Ogg", "audio-ogg;audio-x-generic"),
    ("opus", "audio/ogg", "Аудио Opus", "audio-x-generic"),
    ("wav", "audio/x-wav", "Аудио WAV", "audio-x-wav;audio-x-generic"),
    ("m4a", "audio/mp4", "Аудио M4A", "audio-mp4;audio-x-generic"),
    ("mp4", "video/mp4", "Видео MP4", "video-mp4;video-x-generic"),
    ("mkv", "video/x-matroska", "Видео Matroska", "video-x-matroska;video-x-generic"),
    ("webm", "video/webm", "Видео WebM", "video-webm;video-x-generic"),
    ("avi", "video/x-msvideo", "Видео AVI", "video-x-msvideo;video-x-generic"),
    ("mov", "video/quicktime", "Видео QuickTime", "video-quicktime;video-x-generic"),
    ("zip", "application/zip", "Архив ZIP", "application-zip;application-x-zip;package-x-generic"),
    ("7z", "application/x-7z-compressed", "Архив 7-Zip", "application-x-7z-compressed;package-x-generic"),
    ("rar", "application/vnd.rar", "Архив RAR", "application-x-rar;application-vnd.rar;package-x-generic"),
    ("tar", "application/x-tar", "Архив TAR", "application-x-tar;package-x-generic"),
    ("tar.gz", "application/x-compressed-tar", "Архив TAR, сжатый GZip", "application-x-compressed-tar;application-x-tar;package-x-generic"),
    ("tar.xz", "application/x-xz-compressed-tar", "Архив TAR, сжатый XZ", "application-x-xz-compressed-tar;application-x-tar;package-x-generic"),
    ("tar.zst", "application/x-zstd-compressed-tar", "Архив TAR, сжатый Zstandard", "application-x-zstd-compressed-tar;application-x-tar;package-x-generic"),
    ("tar.bz2", "application/x-bzip-compressed-tar", "Архив TAR, сжатый BZip2", "application-x-bzip-compressed-tar;application-x-tar;package-x-generic"),
    ("gz", "application/gzip", "Архив GZip", "application-gzip;application-x-gzip;package-x-generic"),
    ("xz", "application/x-xz", "Архив XZ", "application-x-xz;package-x-generic"),
    ("zst", "application/zstd", "Архив Zstandard", "application-zstd;package-x-generic"),
    ("iso", "application/x-cd-image", "Образ диска ISO", "application-x-cd-image;media-optical"),
    ("img", "application/x-raw-disk-image", "Образ диска", "application-x-raw-disk-image;drive-harddisk"),
    ("odt", "application/vnd.oasis.opendocument.text", "Документ ODT", "application-vnd.oasis.opendocument.text;x-office-document"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet", "Электронная таблица ODS", "application-vnd.oasis.opendocument.spreadsheet;x-office-spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation", "Презентация ODP", "application-vnd.oasis.opendocument.presentation;x-office-presentation"),
    ("doc", "application/msword", "Документ Word 97–2003", "application-msword;x-office-document"),
    ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "Документ Word", "application-vnd.openxmlformats-officedocument.wordprocessingml.document;x-office-document"),
    ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "Лист Excel", "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet;x-office-spreadsheet"),
    ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation", "Презентация PowerPoint", "application-vnd.openxmlformats-officedocument.presentationml.presentation;x-office-presentation"),
    ("ttf", "font/ttf", "Шрифт TrueType", "font-ttf;font-x-generic"),
    ("otf", "font/otf", "Шрифт OpenType", "font-otf;font-x-generic"),
    ("appimage", "application/vnd.appimage", "Приложение AppImage", "application-vnd.appimage;application-x-executable"),
    ("exe", "application/x-ms-dos-executable", "Программа Windows", "application-x-ms-dos-executable;application-x-msdownload;application-x-executable"),
    ("msi", "application/x-msi", "Установщик Windows", "application-x-msi;application-x-executable"),
    ("desktop", "application/x-desktop", "Ярлык программы", "application-x-desktop"),
    ("deb", "application/vnd.debian.binary-package", "Пакет Debian", "application-x-deb;package-x-generic"),
    ("pkg.tar.zst", "application/x-zstd-compressed-tar", "Пакет Arch Linux", "package-x-generic"),
    ("torrent", "application/x-bittorrent", "Торрент", "application-x-bittorrent"),
];

pub fn type_for_ext(ext: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    TYPES.iter().find(|t| t.0 == ext)
}

fn kind_of(e: &Entry) -> Kind {
    if e.dir {
        return Kind { name: if e.link { "Ярлык на папку".into() } else { "Папка с файлами".into() }, icon: folder_icon(&e.path), mime: "inode/directory", image: false };
    }
    if let Some(t) = type_for_ext(&e.ext) {
        return Kind { name: t.2.into(), icon: t.3.into(), mime: t.1, image: t.1.starts_with("image/") };
    }
    if e.exec() {
        return Kind { name: "Программа".into(), icon: "application-x-executable".into(), mime: "application/x-executable", image: false };
    }
    let name = if e.ext.is_empty() { "Файл".into() } else { format!("Файл «{}»", e.ext.to_uppercase()) };
    Kind { name, icon: "application-octet-stream;unknown;text-x-generic".into(), mime: "application/octet-stream", image: false }
}

// ------------------------------------------------------------------ places

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// XDG user folders (user-dirs.dirs): (key, path). Missing ones fall back to the
/// English defaults when they exist.
pub fn user_dirs() -> Vec<(&'static str, PathBuf)> {
    let h = home();
    let cfg = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| h.join(".config"));
    let text = std::fs::read_to_string(cfg.join("user-dirs.dirs")).unwrap_or_default();
    let get = |key: &str| -> Option<PathBuf> {
        let line = text.lines().find(|l| l.trim_start().starts_with(&format!("XDG_{key}_DIR=")))?;
        let v = line.split_once('=')?.1.trim().trim_matches('"');
        let p = v.replace("$HOME", &h.to_string_lossy());
        let p = PathBuf::from(p);
        (p != h).then_some(p)
    };
    let defs = [("DESKTOP", "Desktop"), ("DOWNLOAD", "Downloads"), ("DOCUMENTS", "Documents"), ("PICTURES", "Pictures"), ("MUSIC", "Music"), ("VIDEOS", "Videos"), ("TEMPLATES", "Templates"), ("PUBLICSHARE", "Public")];
    let mut v = Vec::new();
    for (k, d) in defs {
        let p = get(k).unwrap_or_else(|| h.join(d));
        if p.is_dir() {
            v.push((k, p));
        }
    }
    v
}

fn folder_icon(p: &Path) -> String {
    let h = home();
    if p == h {
        return "user-home;folder".into();
    }
    if p.parent() == Some(h.as_path()) {
        for (k, d) in user_dirs() {
            if d == p {
                return match k {
                    "DESKTOP" => "user-desktop;folder",
                    "DOWNLOAD" => "folder-download;folder",
                    "DOCUMENTS" => "folder-documents;folder",
                    "PICTURES" => "folder-pictures;folder",
                    "MUSIC" => "folder-music;folder",
                    "VIDEOS" => "folder-videos;folder",
                    "TEMPLATES" => "folder-templates;folder",
                    _ => "folder-publicshare;folder",
                }
                .into();
            }
        }
    }
    "folder".into()
}

/// A mounted disk worth showing in «Этот компьютер».
#[derive(Clone, Debug, PartialEq)]
pub struct Mount {
    pub path: PathBuf,
    pub dev: String,
    pub fs: String,
    pub label: String,
    pub removable: bool,
    pub size: u64,
    pub free: u64,
}

/// `/` plus disks under /mnt, /media and /run/media (from /proc/self/mountinfo).
pub fn mounts() -> Vec<Mount> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut v: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(dash) = f.iter().position(|x| *x == "-") else { continue };
        if f.len() < dash + 3 || f.len() < 5 {
            continue;
        }
        let mp = unescape(f[4]);
        let fs = f[dash + 1].to_string();
        let dev = f[dash + 2].to_string();
        let root = mp == "/";
        let under = ["/mnt/", "/media/", "/run/media/"].iter().any(|p| mp.starts_with(p));
        if !(root || under) || !dev.starts_with('/') && !root || v.iter().any(|m| m.path == Path::new(&mp)) {
            continue;
        }
        let (size, free) = statvfs(&mp);
        let removable = mp.starts_with("/run/media/") || mp.starts_with("/media/");
        let name = Path::new(&mp).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let label = if root { "Локальный диск".into() } else { name };
        v.push(Mount { path: PathBuf::from(mp), dev, fs, label, removable, size, free });
    }
    v
}

/// The mount a path lies on (the longest prefix).
pub fn mount_of<'a>(ms: &'a [Mount], p: &Path) -> Option<&'a Mount> {
    ms.iter().filter(|m| p.starts_with(&m.path)).max_by_key(|m| m.path.as_os_str().len())
}

fn unescape(s: &str) -> String {
    // mountinfo writes spaces and the like as \040.
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            if let Some(n) = b.get(i + 1..i + 4).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| u8::from_str_radix(d, 8).ok()) {
                out.push(n);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn statvfs(p: &str) -> (u64, u64) {
    #[repr(C)]
    struct Vfs {
        bsize: u64,
        frsize: u64,
        blocks: u64,
        bfree: u64,
        bavail: u64,
        files: u64,
        ffree: u64,
        favail: u64,
        fsid: u64,
        flag: u64,
        namemax: u64,
        spare: [i32; 6],
    }
    extern "C" {
        fn statvfs(path: *const std::ffi::c_char, buf: *mut Vfs) -> i32;
    }
    let Ok(c) = std::ffi::CString::new(p) else { return (0, 0) };
    let mut v: Vfs = unsafe { std::mem::zeroed() };
    if unsafe { statvfs(c.as_ptr(), &mut v) } != 0 {
        return (0, 0);
    }
    (v.blocks * v.frsize, v.bavail * v.frsize)
}

// ------------------------------------------------------------------ Windows' formats

fn grp3(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

/// The «Размер» column: KB rounded up.
pub fn size_kb(b: u64) -> String {
    format!("{}\u{a0}КБ", grp3(b.div_ceil(1024)))
}

/// Three significant digits: «2,29 МБ».
pub fn size_short(b: u64) -> String {
    if b < 1024 {
        return format!("{b}\u{a0}{}", plural(b, ["байт", "байта", "байт"]));
    }
    let units = ["КБ", "МБ", "ГБ", "ТБ"];
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1000.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    let s = if v < 10.0 {
        format!("{:.2}", (v * 100.0).floor() / 100.0)
    } else if v < 100.0 {
        format!("{:.1}", (v * 10.0).floor() / 10.0)
    } else {
        format!("{}", v.floor())
    };
    format!("{}\u{a0}{}", s.replace('.', ","), units[i])
}

pub fn plural(n: u64, f: [&'static str; 3]) -> &'static str {
    let (a, b) = (n % 10, n % 100);
    if a == 1 && b != 11 {
        f[0]
    } else if (2..=4).contains(&a) && !(12..=14).contains(&b) {
        f[1]
    } else {
        f[2]
    }
}

/// Local time «25.11.2026 14:03» (the offset from /etc/localtime).
pub fn date(t: i64) -> String {
    let (y, mo, d, h, mi) = civil(t + tz_offset(t));
    format!("{d:02}.{mo:02}.{y} {h:02}:{mi:02}")
}

/// (year, month, day, hour, minute) of a Unix time.
pub fn civil(t: i64) -> (i64, u32, u32, u32, u32) {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // Howard Hinnant's days_from_civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32)
}

/// Seconds east of UTC at `t`, from the TZif file (the last transition before `t`).
pub fn tz_offset(t: i64) -> i64 {
    thread_local! {
        static TZ: std::cell::RefCell<Option<Vec<(i64, i64)>>> = const { std::cell::RefCell::new(None) };
    }
    TZ.with(|tz| {
        let mut tz = tz.borrow_mut();
        let list = tz.get_or_insert_with(load_tz);
        let mut off = list.first().map_or(0, |x| x.1);
        for &(at, o) in list.iter() {
            if at <= t {
                off = o;
            } else {
                break;
            }
        }
        off
    })
}

fn load_tz() -> Vec<(i64, i64)> {
    let Ok(b) = std::fs::read("/etc/localtime") else { return vec![(i64::MIN, 0)] };
    let be32 = |o: usize| -> i64 { b.get(o..o + 4).map_or(0, |s| i32::from_be_bytes([s[0], s[1], s[2], s[3]]) as i64) };
    let be64 = |o: usize| -> i64 { b.get(o..o + 8).map_or(0, |s| i64::from_be_bytes(s.try_into().unwrap())) };
    if b.len() < 44 || &b[0..4] != b"TZif" {
        return vec![(i64::MIN, 0)];
    }
    let hdr = |o: usize| [be32(o + 20), be32(o + 24), be32(o + 28), be32(o + 32), be32(o + 36), be32(o + 40)];
    let h1 = hdr(0);
    let (isut, isstd, leap, timecnt, typecnt, charcnt) = (h1[0], h1[1], h1[2], h1[3], h1[4], h1[5]);
    let v1len = (timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut) as usize;
    // Version 2+: the 64-bit block follows.
    let (o, wide) = if b[4] >= b'2' { (44 + v1len, true) } else { (0, false) };
    let h = hdr(o);
    let (_, _, _, timecnt, typecnt, _) = (h[0], h[1], h[2], h[3], h[4], h[5]);
    let base = o + 44;
    let tsz = if wide { 8 } else { 4 };
    let times: Vec<i64> = (0..timecnt as usize).map(|i| if wide { be64(base + i * 8) } else { be32(base + i * 4) }).collect();
    let idx_at = base + timecnt as usize * tsz;
    let types_at = idx_at + timecnt as usize;
    let off = |ti: usize| be32(types_at + ti * 6);
    let mut v = vec![(i64::MIN, if typecnt > 0 { off(0) } else { 0 })];
    for (i, t) in times.iter().enumerate() {
        let ti = b.get(idx_at + i).copied().unwrap_or(0) as usize;
        v.push((*t, off(ti)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_order() {
        let mut v = vec!["file10.txt", "file2.txt", "File1.txt", "ёлка", "ель", "жук"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["File1.txt", "file2.txt", "file10.txt", "ель", "ёлка", "жук"]);
    }
    #[test]
    fn sizes() {
        assert_eq!(size_kb(1), "1\u{a0}КБ");
        assert_eq!(size_kb(2_401_234), "2\u{a0}345\u{a0}КБ");
        assert_eq!(size_short(2_401_234), "2,28\u{a0}МБ");
        assert_eq!(ext_of("a.tar.gz"), "tar.gz");
        assert_eq!(ext_of(".bashrc"), "");
    }
    #[test]
    fn civil_time() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(1_764_079_380), (2025, 11, 25, 14, 3));
    }
}
