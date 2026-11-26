//! The pointer from the system's cursor theme (Xcursor files), the same one X programs
//! (Steam) and toolkits draw: `XCURSOR_THEME` / `XCURSOR_SIZE`, else the "default" theme
//! (whose index.theme names the real one, Adwaita here), 24 px. Themes inherit (index.theme
//! `Inherits=`); a cursor's other names are links in the theme's `cursors/`.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

pub struct Image {
    pub w: i32,
    pub h: i32,
    pub xhot: i32,
    pub yhot: i32,
    /// Premultiplied ARGB.
    pub px: Vec<u32>,
}

thread_local! {
    static CACHE: RefCell<HashMap<(u32, i32), Option<Rc<Image>>>> = RefCell::new(HashMap::new());
}

/// Xcursor names for a cursor-shape-v1 shape, best first.
fn names(shape: u32) -> &'static [&'static str] {
    match shape {
        2 => &["context-menu", "left_ptr"],
        3 => &["help", "question_arrow"],
        4 => &["pointer", "hand2", "hand1"],
        5 => &["progress", "left_ptr_watch", "watch"],
        6 => &["wait", "watch"],
        7 => &["cell", "plus"],
        8 => &["crosshair", "cross", "tcross"],
        9 => &["text", "xterm"],
        10 => &["vertical-text", "text", "xterm"],
        11 => &["alias", "dnd-link"],
        12 => &["copy", "dnd-copy"],
        13 => &["move", "fleur", "dnd-move"],
        14 => &["no-drop", "dnd-none"],
        15 => &["not-allowed", "crossed_circle"],
        16 => &["grab", "openhand", "hand1"],
        17 => &["grabbing", "closedhand", "fleur"],
        18 => &["e-resize", "right_side"],
        19 => &["n-resize", "top_side"],
        20 => &["ne-resize", "top_right_corner"],
        21 => &["nw-resize", "top_left_corner"],
        22 => &["s-resize", "bottom_side"],
        23 => &["se-resize", "bottom_right_corner"],
        24 => &["sw-resize", "bottom_left_corner"],
        25 => &["w-resize", "left_side"],
        26 => &["ew-resize", "sb_h_double_arrow", "h_double_arrow"],
        27 => &["ns-resize", "sb_v_double_arrow", "v_double_arrow"],
        28 => &["nesw-resize", "fd_double_arrow"],
        29 => &["nwse-resize", "bd_double_arrow"],
        30 => &["col-resize", "sb_h_double_arrow"],
        31 => &["row-resize", "sb_v_double_arrow"],
        32 => &["all-scroll", "fleur"],
        33 => &["zoom-in"],
        34 => &["zoom-out"],
        _ => &["default", "left_ptr", "arrow"],
    }
}

/// The nominal size at scale 1 (`XCURSOR_SIZE`, 24 by default).
pub fn base_size() -> i32 {
    std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).filter(|s| *s > 0 && *s <= 256).unwrap_or(24)
}

fn dirs() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("XCURSOR_PATH") {
        return p.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut v = vec![home.join(".local/share/icons"), home.join(".icons")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(data.split(':').filter(|s| !s.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    v.push("/usr/share/pixmaps".into());
    v
}

/// The cursor file `name` of `theme` or what it inherits (depth-limited).
fn find(theme: &str, name: &str, depth: u32, seen: &mut Vec<String>) -> Option<PathBuf> {
    if depth > 8 || seen.iter().any(|s| s == theme) {
        return None;
    }
    seen.push(theme.to_string());
    let ds = dirs();
    for d in &ds {
        let p = d.join(theme).join("cursors").join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    for d in &ds {
        let Ok(idx) = std::fs::read_to_string(d.join(theme).join("index.theme")) else { continue };
        for l in idx.lines() {
            if let Some(v) = l.trim().strip_prefix("Inherits") {
                let v = v.trim_start().trim_start_matches('=');
                for parent in v.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()) {
                    if let Some(p) = find(parent, name, depth + 1, seen) {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// The image of an Xcursor file nearest to `size` (the first frame of an animated one).
fn parse(b: &[u8], size: i32) -> Option<Image> {
    if b.get(0..4)? != b"Xcur" {
        return None;
    }
    let ntoc = u32le(b, 12)? as usize;
    let mut best: Option<(u32, usize)> = None;
    for i in 0..ntoc.min(1024) {
        let o = 16 + i * 12;
        let (ty, sub, pos) = (u32le(b, o)?, u32le(b, o + 4)?, u32le(b, o + 8)? as usize);
        if ty != 0xfffd_0002 {
            continue;
        }
        let d = (sub as i32 - size).unsigned_abs();
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, pos));
        }
    }
    let (_, pos) = best?;
    let (w, h) = (u32le(b, pos + 16)? as i32, u32le(b, pos + 20)? as i32);
    let (xhot, yhot) = (u32le(b, pos + 24)? as i32, u32le(b, pos + 28)? as i32);
    if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
        return None;
    }
    let start = pos + 36;
    let px: Vec<u32> = (0..(w * h) as usize).map(|i| u32le(b, start + i * 4)).collect::<Option<_>>()?;
    Some(Image { w, h, xhot, yhot, px })
}

fn theme() -> String {
    std::env::var("XCURSOR_THEME").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "default".into())
}

/// The theme's image for `shape` at `size` px (cached; None: no theme has it).
pub fn get(shape: u32, size: i32) -> Option<Rc<Image>> {
    CACHE.with(|c| {
        c.borrow_mut()
            .entry((shape, size))
            .or_insert_with(|| {
                let t = theme();
                names(shape).iter().find_map(|n| {
                    let p = find(&t, n, 0, &mut Vec::new()).or_else(|| find("Adwaita", n, 0, &mut Vec::new()))?;
                    parse(&std::fs::read(p).ok()?, size)
                }).map(Rc::new)
            })
            .clone()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_an_xcursor_file() {
        // One 2×1 image at nominal 24, hotspot (1, 0).
        let mut b = Vec::new();
        b.extend(b"Xcur");
        for v in [16u32, 0x10000, 1, 0xfffd_0002, 24, 28, 36, 0xfffd_0002, 24, 1, 2, 1, 1, 0, 0, 0xff00_0000, 0xffff_ffff] {
            b.extend(v.to_le_bytes());
        }
        let i = super::parse(&b, 24).unwrap();
        assert_eq!((i.w, i.h, i.xhot, i.yhot), (2, 1, 1, 0));
        assert_eq!(i.px, vec![0xff00_0000, 0xffff_ffff]);
    }
}
