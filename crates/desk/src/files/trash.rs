//! «Корзина»: the freedesktop Trash (what every Linux file manager shares).
//! Home: ~/.local/share/Trash; other disks: <mount>/.Trash-<uid>. Each trashed thing
//! is files/<name> plus info/<name>.trashinfo (original path, deletion date).
use std::path::{Path, PathBuf};

pub struct Item {
    /// Where it lies in the trash now.
    pub file: PathBuf,
    pub orig: PathBuf,
    /// Seconds since the epoch (local time as written, read back as such).
    pub deleted: i64,
}

pub fn home_trash() -> PathBuf {
    let data = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| super::model::home().join(".local/share"));
    data.join("Trash")
}

fn dev_of(p: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).ok().map(|m| m.dev())
}

/// The trash for `p`: the home one when on the same disk, else the disk's own.
fn trash_for(p: &Path) -> PathBuf {
    let home = home_trash();
    let _ = std::fs::create_dir_all(&home);
    let pd = p.parent().and_then(dev_of);
    if pd.is_none() || pd == dev_of(&home) {
        return home;
    }
    let ms = super::model::mounts();
    match super::model::mount_of(&ms, p) {
        Some(m) if m.path != Path::new("/") => m.path.join(format!(".Trash-{}", sys::uid())),
        _ => home,
    }
}

fn enc(p: &Path) -> String {
    let mut s = String::new();
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Moves `p` to the trash; returns where it went. Fails (EXDEV and the like) when the
/// trash is on another disk: the caller deletes for good after asking.
pub fn put(p: &Path) -> std::io::Result<PathBuf> {
    let t = trash_for(p);
    std::fs::create_dir_all(t.join("files"))?;
    std::fs::create_dir_all(t.join("info"))?;
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (y, mo, d, h, mi) = super::model::civil(now + super::model::tz_offset(now));
    let date = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{:02}", now.rem_euclid(60));
    let mut n = 1;
    loop {
        let tn = if n == 1 { name.clone() } else { format!("{stem}.{n}{ext}") };
        let info = t.join("info").join(format!("{tn}.trashinfo"));
        // create_new: the info file reserves the name (the spec's way against races).
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&info) {
            Ok(mut f) => {
                use std::io::Write;
                write!(f, "[Trash Info]\nPath={}\nDeletionDate={date}\n", enc(p))?;
                let dst = t.join("files").join(&tn);
                if let Err(e) = std::fs::rename(p, &dst) {
                    let _ = std::fs::remove_file(&info);
                    return Err(e);
                }
                return Ok(dst);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}

fn parse_date(s: &str) -> i64 {
    // YYYY-MM-DDThh:mm:ss
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d) = (num(0, 4), num(5, 7), num(8, 10));
    let (h, mi, se) = (num(11, 13), num(14, 16), num(17, 19));
    // days_from_civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let local = days * 86400 + h * 3600 + mi * 60 + se;
    local - super::model::tz_offset(local)
}

fn trashes() -> Vec<PathBuf> {
    let mut v = vec![home_trash()];
    for m in super::model::mounts() {
        let t = m.path.join(format!(".Trash-{}", sys::uid()));
        if m.path != Path::new("/") && t.is_dir() {
            v.push(t);
        }
    }
    v
}

pub fn list() -> Vec<Item> {
    let mut v = Vec::new();
    for t in trashes() {
        let Ok(rd) = std::fs::read_dir(t.join("files")) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let info = std::fs::read_to_string(t.join("info").join(format!("{name}.trashinfo"))).unwrap_or_default();
            let mut orig = PathBuf::from(&name);
            let mut deleted = 0;
            for l in info.lines() {
                if let Some(p) = l.strip_prefix("Path=") {
                    let p = dec(p);
                    orig = if p.starts_with('/') { PathBuf::from(p) } else { t.parent().unwrap_or(Path::new("/")).join(p) };
                } else if let Some(d) = l.strip_prefix("DeletionDate=") {
                    deleted = parse_date(d);
                }
            }
            v.push(Item { file: e.path(), orig, deleted });
        }
    }
    v
}

pub fn info_of(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?.to_string_lossy().to_string();
    let t = file.parent()?.parent()?;
    Some(t.join("info").join(format!("{name}.trashinfo")))
}

/// The original place of a trashed file.
pub fn orig_of(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(info_of(file)?).ok()?;
    let p = dec(text.lines().find_map(|l| l.strip_prefix("Path="))?);
    Some(PathBuf::from(p))
}

/// Puts a trashed file back (into a free name when its place is taken); where it went.
pub fn restore(file: &Path) -> std::io::Result<PathBuf> {
    let orig = orig_of(file).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "нет сведений о файле"))?;
    if let Some(dir) = orig.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let dst = if orig.exists() { super::ops::free_name(&orig, false) } else { orig };
    std::fs::rename(file, &dst)?;
    if let Some(i) = info_of(file) {
        let _ = std::fs::remove_file(i);
    }
    Ok(dst)
}

/// Deletes a trashed file for good.
pub fn erase(file: &Path) -> std::io::Result<()> {
    let r = match std::fs::symlink_metadata(file) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(file),
        Ok(_) => std::fs::remove_file(file),
        Err(e) => Err(e),
    };
    if let Some(i) = info_of(file) {
        let _ = std::fs::remove_file(i);
    }
    r
}
