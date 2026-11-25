//! Application icons: which icon a window or menu entry has (its .desktop file:
//! id, StartupWMClass, Exec), where it lies (freedesktop icon themes, pixmaps), and
//! the picture at the size asked (PNG, SVG via svg.rs, XPM), cached per size.
use crate::geom::Rect;
use crate::render::{Canvas, Xf};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Pic {
    pub w: i32,
    pub h: i32,
    /// Premultiplied ARGB.
    pub px: Vec<u32>,
}

#[derive(Default)]
struct Cache {
    /// Lower-cased app id / desktop id / WM class → icon name.
    names: Option<HashMap<String, String>>,
    files: HashMap<String, Option<PathBuf>>,
    pics: HashMap<(String, i32), Option<Rc<Pic>>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// Forget everything (new apps installed, theme changed).
pub fn reset() {
    CACHE.with(|c| *c.borrow_mut() = Cache::default());
}

fn name_index() -> HashMap<String, String> {
    let mut m = HashMap::new();
    for a in crate::apps::load() {
        if a.icon.is_empty() {
            continue;
        }
        let id = a.id.to_lowercase();
        // org.gnome.Nautilus → also "nautilus".
        if let Some(last) = id.rsplit('.').next() {
            m.entry(last.to_string()).or_insert_with(|| a.icon.clone());
        }
        let exe = a.exec.split_whitespace().next().unwrap_or("");
        let exe = exe.rsplit('/').next().unwrap_or(exe).to_lowercase();
        if !exe.is_empty() && !exe.contains('=') {
            m.entry(exe).or_insert_with(|| a.icon.clone());
        }
        if !a.wmclass.is_empty() {
            m.insert(a.wmclass.to_lowercase(), a.icon.clone());
        }
        m.insert(id, a.icon.clone());
    }
    m
}

/// Icon name for a window's app_id (or a desktop id).
fn icon_name(c: &mut Cache, key: &str) -> String {
    let names = c.names.get_or_insert_with(name_index);
    let k = key.to_lowercase();
    if let Some(n) = names.get(&k) {
        return n.clone();
    }
    if let Some(n) = k.rsplit('.').next().and_then(|l| names.get(l)) {
        return n.clone();
    }
    k
}

const SIZES: [u32; 10] = [16, 22, 24, 32, 48, 64, 96, 128, 256, 512];

fn themes() -> Vec<String> {
    let mut t = Vec::new();
    if let Ok(v) = std::env::var("RESC_ICON_THEME") {
        t.push(v);
    }
    for n in ["Papirus-Dark", "Papirus", "breeze-dark", "breeze", "Adwaita", "hicolor"] {
        t.push(n.to_string());
    }
    t
}

fn bases() -> Vec<PathBuf> {
    let mut b = Vec::new();
    if let Ok(h) = std::env::var("HOME") {
        b.push(PathBuf::from(format!("{h}/.local/share/icons")));
        b.push(PathBuf::from(format!("{h}/.icons")));
    }
    b.push(PathBuf::from("/usr/local/share/icons"));
    b.push(PathBuf::from("/usr/share/icons"));
    b
}

/// The file for icon `name` best suited to `px` pixels; `name` may list fallbacks
/// separated by ';'.
fn find_file(name: &str, px: i32) -> Option<PathBuf> {
    if name.contains(';') {
        return name.split(';').filter(|n| !n.is_empty()).find_map(|n| find_file(n, px));
    }
    if name.starts_with('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let name = name.trim_end_matches(".png").trim_end_matches(".svg").trim_end_matches(".xpm");
    if name.is_empty() || name.contains('/') {
        return None;
    }
    // Sizes at or above the wanted one first (scale down), then scalable, then smaller.
    let mut order: Vec<String> = SIZES.iter().filter(|s| **s as i32 >= px).map(|s| s.to_string()).collect();
    order.push("scalable".into());
    order.extend(SIZES.iter().rev().filter(|s| (**s as i32) < px).map(|s| s.to_string()));
    for theme in themes() {
        for base in bases() {
            let dir = base.join(&theme);
            if !dir.is_dir() {
                continue;
            }
            for s in &order {
                let sized = if s == "scalable" { s.clone() } else { format!("{s}x{s}") };
                let cands = [
                    dir.join(&sized).join("apps").join(format!("{name}.svg")),
                    dir.join(&sized).join("apps").join(format!("{name}.png")),
                    // Breeze: apps/48/name.svg
                    dir.join("apps").join(s).join(format!("{name}.svg")),
                    dir.join(&sized).join("devices").join(format!("{name}.svg")),
                    dir.join(&sized).join("devices").join(format!("{name}.png")),
                    dir.join(&sized).join("categories").join(format!("{name}.svg")),
                    dir.join(&sized).join("places").join(format!("{name}.svg")),
                    dir.join(&sized).join("actions").join(format!("{name}.svg")),
                    // File types (the file manager).
                    dir.join(&sized).join("mimetypes").join(format!("{name}.svg")),
                    dir.join(&sized).join("mimetypes").join(format!("{name}.png")),
                ];
                if let Some(f) = cands.into_iter().find(|f| f.is_file()) {
                    return Some(f);
                }
            }
        }
    }
    for ext in ["svg", "png", "xpm"] {
        let f = PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}"));
        if f.is_file() {
            return Some(f);
        }
    }
    None
}

fn load(path: &Path, px: i32) -> Option<Pic> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let px = px.clamp(4, 512);
    match ext.as_str() {
        "svg" => {
            let text = std::fs::read_to_string(path).ok()?;
            let img = crate::svg::render(&text, px as usize)?;
            Some(Pic { w: px, h: px, px: img })
        }
        "png" => {
            let data = std::fs::read(path).ok()?;
            let img = crate::wallpaper::decode_png_alpha(&data)?;
            Some(fit(&img.px, img.w, img.h, px))
        }
        "xpm" => {
            let text = std::fs::read_to_string(path).ok()?;
            let (w, h, img) = parse_xpm(&text)?;
            Some(fit(&img, w, h, px))
        }
        _ => None,
    }
}

/// Scales a premultiplied image into a px×px square (aspect kept, centred), box filter.
fn fit(src: &[u32], sw: usize, sh: usize, px: i32) -> Pic {
    let n = px as usize;
    let k = n as f32 / sw.max(sh) as f32;
    let (dw, dh) = (((sw as f32 * k).round() as usize).clamp(1, n), ((sh as f32 * k).round() as usize).clamp(1, n));
    let (ox, oy) = ((n - dw) / 2, (n - dh) / 2);
    let mut out = vec![0u32; n * n];
    let ch = |p: u32, s: u32| ((p >> s) & 0xff) as f32;
    for dy in 0..dh {
        let (y0, y1) = (dy as f32 * sh as f32 / dh as f32, (dy + 1) as f32 * sh as f32 / dh as f32);
        for dx in 0..dw {
            let (x0, x1) = (dx as f32 * sw as f32 / dw as f32, (dx + 1) as f32 * sw as f32 / dw as f32);
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0;
            let mut y = y0.floor() as usize;
            while (y as f32) < y1 && y < sh {
                let wy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
                let mut x = x0.floor() as usize;
                while (x as f32) < x1 && x < sw {
                    let wx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                    let w = wx * wy;
                    let p = src[y * sw + x];
                    acc[0] += ch(p, 24) * w;
                    acc[1] += ch(p, 16) * w;
                    acc[2] += ch(p, 8) * w;
                    acc[3] += ch(p, 0) * w;
                    wsum += w;
                    x += 1;
                }
                y += 1;
            }
            if wsum > 0.0 {
                let c = |v: f32| ((v / wsum).round().clamp(0.0, 255.0)) as u32;
                out[(oy + dy) * n + ox + dx] = c(acc[0]) << 24 | c(acc[1]) << 16 | c(acc[2]) << 8 | c(acc[3]);
            }
        }
    }
    Pic { w: px, h: px, px: out }
}

/// XPM (X PixMap, a C source file): the old icon format of xterm and friends.
fn parse_xpm(text: &str) -> Option<(usize, usize, Vec<u32>)> {
    let mut strs = Vec::new();
    let mut rest = text;
    while let Some(a) = rest.find('"') {
        let b = rest[a + 1..].find('"')? + a + 1;
        strs.push(&rest[a + 1..b]);
        rest = &rest[b + 1..];
    }
    let head: Vec<usize> = strs.first()?.split_whitespace().take(4).filter_map(|v| v.parse().ok()).collect();
    let [w, h, nc, cpp] = head[..] else { return None };
    if w == 0 || h == 0 || w > 1024 || h > 1024 || cpp == 0 || cpp > 4 || strs.len() < 1 + nc + h {
        return None;
    }
    let mut colors: HashMap<&str, u32> = HashMap::new();
    for line in &strs[1..1 + nc] {
        if line.len() < cpp {
            continue;
        }
        let (key, rest) = line.split_at(cpp);
        let toks: Vec<&str> = rest.split_whitespace().collect();
        let mut c = 0u32;
        let mut i = 0;
        while i + 1 < toks.len() {
            if toks[i] == "c" {
                let v = toks[i + 1];
                c = if v.eq_ignore_ascii_case("none") {
                    0
                } else if v.starts_with('#') && v.len() == 13 {
                    // #RRRRGGGGBBBB
                    let x = |o: usize| u32::from_str_radix(&v[o..o + 2], 16).unwrap_or(0);
                    0xff00_0000 | x(1) << 16 | x(5) << 8 | x(9)
                } else {
                    crate::svg::color(v).unwrap_or(0xff00_0000)
                };
                break;
            }
            i += 2;
        }
        colors.insert(key, c);
    }
    let mut px = Vec::with_capacity(w * h);
    for row in &strs[1 + nc..1 + nc + h] {
        for x in 0..w {
            let k = row.get(x * cpp..(x + 1) * cpp).unwrap_or("");
            px.push(*colors.get(k).unwrap_or(&0));
        }
    }
    Some((w, h, px))
}

/// The icon for `key` (a window's app_id, or a desktop id) at `px` physical pixels.
pub fn get(key: &str, px: i32) -> Option<Rc<Pic>> {
    if key.is_empty() {
        return None;
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let ck = (key.to_string(), px);
        if let Some(p) = c.pics.get(&ck) {
            return p.clone();
        }
        let name = if key.contains(';') { key.to_string() } else { icon_name(&mut c, key) };
        let file = match c.files.get(&name) {
            Some(f) => f.clone(),
            None => {
                let f = find_file(&name, px);
                c.files.insert(name.clone(), f.clone());
                f
            }
        };
        let pic = file.and_then(|f| load(&f, px)).map(Rc::new);
        c.pics.insert(ck, pic.clone());
        pic
    })
}

/// Draws the icon for `key` into logical rect `r`; false when there is none.
pub fn draw(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, key: &str) -> bool {
    let pr = xf.r(r);
    let px = pr.w.max(pr.h);
    let Some(pic) = get(key, px) else { return false };
    cv.blit(clip, pr, &pic.px, pic.w as usize, Rect::new(0, 0, pic.w, pic.h), false, 255);
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn xpm() {
        let t = "static char *x[] = {\n\"2 2 2 1\",\n\"  c None\",\n\". c #ff0000\",\n\" .\",\n\". \"\n};";
        let (w, h, px) = super::parse_xpm(t).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(px, vec![0, 0xffff0000, 0xffff0000, 0]);
    }

    /// Contact sheet of real icons, for looking at by eye:
    /// ICON_DIR=/usr/share/icons/Papirus/64x64/apps ICON_SHEET=/tmp/sheet.png cargo test -- --ignored sheet
    #[test]
    #[ignore]
    fn sheet() {
        let dir = std::env::var("ICON_DIR").unwrap();
        let out = std::env::var("ICON_SHEET").unwrap();
        let size: usize = std::env::var("ICON_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        let step = (files.len() / 120).max(1);
        let files: Vec<_> = files.into_iter().step_by(step).take(120).collect();
        let cols = 12;
        let cell = size + 8;
        let (w, h) = (cols * cell, files.len().div_ceil(cols) * cell);
        let mut px = vec![0xff2a_2833u32; w * h];
        let t = std::time::Instant::now();
        for (i, f) in files.iter().enumerate() {
            let Some(p) = super::load(f, size as i32) else {
                eprintln!("failed: {}", f.display());
                continue;
            };
            let (ox, oy) = ((i % cols) * cell + 4, (i / cols) * cell + 4);
            for y in 0..size {
                for x in 0..size {
                    let s = p.px[y * size + x];
                    let d = &mut px[(oy + y) * w + ox + x];
                    *d = crate::render::blend(*d, s);
                }
            }
        }
        eprintln!("{} icons in {:?}", files.len(), t.elapsed());
        std::fs::write(out, crate::png::encode(&px, w as i32, h as i32, w, 1)).unwrap();
    }

    #[test]
    fn fit_keeps_aspect() {
        let src = vec![0xffffffffu32; 4 * 2];
        let p = super::fit(&src, 4, 2, 4);
        assert_eq!(p.px[0], 0); // top row empty (centred 4×2)
        assert_eq!(p.px[4], 0xffffffff);
    }
}
