//! Installed applications from .desktop files (for the start menu) and a
//! US-layout keycode → char map for start menu search.

pub struct App {
    pub name: String,
    pub exec: String,
    pub id: String,
    /// Icon= (a theme name or an absolute path).
    pub icon: String,
    /// StartupWMClass=, how X programs (and some Wayland ones) name their windows.
    pub wmclass: String,
}

fn dirs() -> Vec<String> {
    let mut dirs = vec!["/usr/share/applications".to_string(), "/usr/local/share/applications".to_string()];
    if let Ok(h) = std::env::var("HOME") {
        dirs.push(format!("{h}/.local/share/applications"));
    }
    dirs
}

/// When the folders of .desktop files last changed (a program installed or removed): the
/// start menu reads them again when this is not what it read them at.
pub fn stamp() -> Vec<Option<std::time::SystemTime>> {
    dirs().iter().map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok()).collect()
}

pub fn load() -> Vec<App> {
    let dirs = dirs();
    let terminal = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
    let mut apps: Vec<App> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map_or(true, |x| x != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let id = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if let Some(a) = parse(&text, &id, &terminal) {
                apps.retain(|x| x.id != a.id);
                apps.push(a);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    // Built-in settings first.
    apps.insert(
        0,
        App {
            name: "Параметры".into(),
            exec: "@settings".into(),
            id: "resc.settings".into(),
            icon: "org.gnome.Settings;preferences-system".into(),
            wmclass: String::new(),
        },
    );
    apps.insert(
        1,
        App {
            name: "Проводник".into(),
            exec: "@files".into(),
            id: "resc.files".into(),
            icon: "system-file-manager;org.gnome.Nautilus;folder".into(),
            wmclass: String::new(),
        },
    );
    apps
}

fn parse(text: &str, id: &str, terminal: &str) -> Option<App> {
    let mut in_entry = false;
    let (mut name, mut name_ru, mut exec) = (None, None, None);
    let (mut icon, mut wmclass) = (String::new(), String::new());
    let mut hidden = false;
    let mut term = false;
    let mut is_app = false;
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
        match k.trim() {
            "Name" => name = Some(v.trim().to_string()),
            "Name[ru]" => name_ru = Some(v.trim().to_string()),
            "Exec" => exec = Some(v.trim().to_string()),
            "Icon" => icon = v.trim().to_string(),
            "StartupWMClass" => wmclass = v.trim().to_string(),
            "NoDisplay" | "Hidden" => hidden |= v.trim() == "true",
            "Terminal" => term = v.trim() == "true",
            "Type" => is_app = v.trim() == "Application",
            _ => {}
        }
    }
    if hidden || !is_app {
        return None;
    }
    let exec = exec?;
    // Drop field codes (%f %U ...).
    let mut cmd = String::new();
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        if c == '%' {
            if it.peek() == Some(&'%') {
                cmd.push('%');
            }
            it.next();
        } else {
            cmd.push(c);
        }
    }
    let cmd = cmd.trim().to_string();
    let cmd = if term { format!("{terminal} -e {cmd}") } else { cmd };
    Some(App { name: name_ru.or(name)?, exec: cmd, id: id.to_string(), icon, wmclass })
}

