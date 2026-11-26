//! Opening files: which program handles a type (mimeapps.list, then mimeinfo.cache /
//! the .desktop files' MimeType=), «Открыть с помощью» lists, and starting a program
//! with files (Exec= field codes) through the compositor, so the launch animates.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Handler {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Exec= as written, field codes included.
    pub exec: String,
    pub terminal: bool,
    pub mimes: Vec<String>,
}

fn app_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join(".local/share/applications"));
    }
    v.push("/usr/local/share/applications".into());
    v.push("/usr/share/applications".into());
    v
}

pub fn parse(text: &str, id: &str) -> Option<Handler> {
    let mut in_entry = false;
    let (mut name, mut name_ru, mut exec, mut icon) = (None, None, None, String::new());
    let (mut hidden, mut term, mut app) = (false, false, false);
    let mut mimes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "Name" => name = Some(v.to_string()),
            "Name[ru]" => name_ru = Some(v.to_string()),
            "Exec" => exec = Some(v.to_string()),
            "Icon" => icon = v.to_string(),
            "Hidden" => hidden |= v == "true",
            "Terminal" => term = v == "true",
            "Type" => app = v == "Application",
            "MimeType" => mimes = v.split(';').filter(|m| !m.is_empty()).map(str::to_string).collect(),
            _ => {}
        }
    }
    if hidden || !app {
        return None;
    }
    Some(Handler { id: id.to_string(), name: name_ru.or(name)?, icon, exec: exec?, terminal: term, mimes })
}

/// Every installed application (by desktop id; the user's own copies win).
pub struct Apps {
    pub all: HashMap<String, Handler>,
    defaults: HashMap<String, Vec<String>>,
    added: HashMap<String, Vec<String>>,
}

impl Apps {
    pub fn load() -> Apps {
        let mut all = HashMap::new();
        for dir in app_dirs().into_iter().rev() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(true, |x| x != "desktop") {
                    continue;
                }
                let id = p.file_name().unwrap().to_string_lossy().to_string();
                if let Some(h) = std::fs::read_to_string(&p).ok().and_then(|t| parse(&t, &id)) {
                    all.insert(id, h);
                }
            }
        }
        let mut defaults = HashMap::new();
        let mut added = HashMap::new();
        let mut lists = Vec::new();
        let cfg = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| crate::files::model::home().join(".config"));
        lists.push(cfg.join("mimeapps.list"));
        lists.push("/etc/xdg/mimeapps.list".into());
        for d in app_dirs() {
            lists.push(d.join("mimeapps.list"));
        }
        // Earlier files win: fill only what is still missing.
        for l in lists {
            let Ok(t) = std::fs::read_to_string(&l) else { continue };
            let mut sect = "";
            for line in t.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    sect = if line == "[Default Applications]" { "d" } else if line == "[Added Associations]" { "a" } else { "" };
                    continue;
                }
                let Some((m, v)) = line.split_once('=') else { continue };
                let ids: Vec<String> = v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect();
                match sect {
                    "d" => {
                        defaults.entry(m.trim().to_string()).or_insert(ids);
                    }
                    "a" => {
                        added.entry(m.trim().to_string()).or_insert_with(Vec::new).extend(ids);
                    }
                    _ => {}
                }
            }
        }
        Apps { all, defaults, added }
    }

    /// The programs that open `mime`, the default first.
    pub fn for_mime(&self, mime: &str) -> Vec<&Handler> {
        let mut ids: Vec<&str> = Vec::new();
        for m in [mime, generic(mime)] {
            for l in [self.defaults.get(m), self.added.get(m)].into_iter().flatten() {
                for i in l {
                    if !ids.contains(&i.as_str()) {
                        ids.push(i);
                    }
                }
            }
        }
        let mut rest: Vec<&Handler> = self.all.values().filter(|h| h.mimes.iter().any(|m| m == mime || m == generic(mime))).collect();
        rest.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let mut out: Vec<&Handler> = ids.iter().filter_map(|i| self.all.get(*i)).collect();
        for h in rest {
            if !out.iter().any(|o| o.id == h.id) {
                out.push(h);
            }
        }
        // Text editors take any text, image viewers any image.
        if mime.starts_with("text/") || mime.starts_with("application/x-shellscript") || mime == "application/json" {
            let mut extra: Vec<&Handler> = self.all.values().filter(|h| h.mimes.iter().any(|m| m == "text/plain")).collect();
            extra.sort_by(|a, b| a.name.cmp(&b.name));
            for h in extra {
                if !out.iter().any(|o| o.id == h.id) {
                    out.push(h);
                }
            }
        }
        out
    }

    pub fn default_for(&self, mime: &str) -> Option<&Handler> {
        self.for_mime(mime).into_iter().next()
    }
}

/// image/png → image/*, the wildcard some .desktop files use.
fn generic(mime: &str) -> &'static str {
    match mime.split('/').next() {
        Some("image") => "image/*",
        Some("text") => "text/*",
        Some("audio") => "audio/*",
        Some("video") => "video/*",
        _ => "",
    }
}

/// Shell-quotes one argument.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command line for `h` with `files` (Exec= field codes filled in).
pub fn command(h: &Handler, files: &[PathBuf]) -> String {
    let list = |uri: bool| files.iter().map(|f| quote(&if uri { uri_of(f) } else { f.to_string_lossy().to_string() })).collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    let mut used = false;
    let mut it = h.exec.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('%') => out.push('%'),
            Some('f') => {
                out.push_str(&files.first().map(|f| quote(&f.to_string_lossy())).unwrap_or_default());
                used = true;
            }
            Some('F') => {
                out.push_str(&list(false));
                used = true;
            }
            Some('u') => {
                out.push_str(&files.first().map(|f| quote(&uri_of(f))).unwrap_or_default());
                used = true;
            }
            Some('U') => {
                out.push_str(&list(true));
                used = true;
            }
            Some('c') => out.push_str(&quote(&h.name)),
            _ => {}
        }
    }
    if !used && !files.is_empty() {
        out.push(' ');
        out.push_str(&list(false));
    }
    if h.terminal {
        let term = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
        out = format!("{term} -e {out}");
    }
    out
}

pub fn uri_of(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Starts a command line the way «Пуск» does (through the compositor: launch animation,
/// its environment); straight from here when there is no compositor to ask.
/// Tests: nothing is started or put on the real clipboard; it is written down instead.
pub static DRY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static DRY_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn dry() -> bool {
    DRY.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn spawn(cmd: &str, cwd: Option<&Path>) {
    if dry() {
        DRY_LOG.lock().unwrap().push(format!("spawn {cmd}"));
        return;
    }
    let cmd = match cwd {
        Some(d) => format!("cd {} && {cmd}", quote(&d.to_string_lossy())),
        None => cmd.to_string(),
    };
    if crate::ipc::request(&format!("spawn {cmd}")).is_ok() {
        return;
    }
    let _ = std::process::Command::new("/bin/sh").arg("-c").arg(format!("{cmd} &")).spawn().and_then(|mut c| c.wait());
}

/// Whether a program is on $PATH.
pub fn have(prog: &str) -> bool {
    std::env::var_os("PATH").map_or(false, |p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}
