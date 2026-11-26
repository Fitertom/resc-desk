//! The look the desk and its own apps share:
//! - the ambient color: the wallpaper's own, taken the way Liquify takes a cover's
//!   (`background::accent_of`: the average at 60×60, HSL saturation ×1.9 up to 1, lightness
//!   ×1.25 up to .76). Accents, folders and the places' icons wear it;
//! - each glass place's settings: how dark the glass is (dim), how much it blurs what is
//!   under it, and how wide its refracting edge is. Places: «Проводник», «Параметры», the
//!   calendar, the timer, and the frame of every other window.
//!
//! The compositor owns both (its config file keeps the places; the accent follows the
//! wallpaper) and writes them to $XDG_RUNTIME_DIR/resc-desk-look for its apps, which read
//! it again when it changes (`poll`).

use std::sync::atomic::{AtomicU32, Ordering};

/// Liquify's default accent, #a78bfa (no wallpaper picture to take it from).
pub const DEFAULT_ACCENT: u32 = 0xa78bfa;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    Files,
    Settings,
    Calendar,
    Timer,
    /// The title bar and frame of every other window.
    Windows,
}

pub const PLACES: [Place; 5] = [Place::Files, Place::Settings, Place::Calendar, Place::Timer, Place::Windows];

impl Place {
    pub fn key(self) -> &'static str {
        match self {
            Place::Files => "files",
            Place::Settings => "settings",
            Place::Calendar => "calendar",
            Place::Timer => "timer",
            Place::Windows => "windows",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Place::Files => "Проводник",
            Place::Settings => "Параметры",
            Place::Calendar => "Календарь",
            Place::Timer => "Таймер",
            Place::Windows => "Окна",
        }
    }

    pub fn from_key(k: &str) -> Option<Place> {
        PLACES.into_iter().find(|p| p.key() == k)
    }

    /// The place of a window's glass, by its app id.
    pub fn of_app(app_id: &str) -> Place {
        match app_id {
            "resc.files" => Place::Files,
            "resc.settings" => Place::Settings,
            _ => Place::Windows,
        }
    }

    fn index(self) -> usize {
        PLACES.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// Programs that draw no frame of their own and are see-through on purpose (Liquify): the
/// desk puts the glass of «Окна» under all of such a window, rounded like a framed one.
pub fn glass_under(app_id: &str) -> bool {
    matches!(app_id, "liquify")
}

/// One place's glass.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Glass {
    /// How dark: the tint's opacity, 0..1.
    pub dim: f32,
    /// The frost's sigma, CSS px (0: none, the glass only refracts and tints).
    pub blur: f32,
    /// The refracting band along the edge, CSS px.
    pub bezel: f32,
}

pub const MAX_DIM: f32 = 1.0;
pub const MAX_BLUR: f32 = crate::glass::MAX_BLUR;
pub const MAX_BEZEL: f32 = crate::glass::MAX_BEZEL;

pub fn default_glass(p: Place) -> Glass {
    match p {
        // About as dark as they were with their own backgrounds.
        Place::Files => Glass { dim: 0.88, blur: crate::glass::FROST, bezel: crate::glass::WIN_BEZEL },
        Place::Settings => Glass { dim: 0.75, blur: crate::glass::FROST, bezel: crate::glass::WIN_BEZEL },
        Place::Calendar | Place::Timer => Glass { dim: 0.55, blur: crate::glass::FROST, bezel: crate::glass::BEZEL },
        Place::Windows => Glass { dim: 0.62, blur: crate::glass::FROST, bezel: crate::glass::WIN_BEZEL },
    }
}

impl Glass {
    pub fn clamped(self) -> Glass {
        Glass { dim: self.dim.clamp(0.0, MAX_DIM), blur: self.blur.clamp(0.0, MAX_BLUR), bezel: self.bezel.clamp(0.0, MAX_BEZEL) }
    }
}

static ACCENT: AtomicU32 = AtomicU32::new(DEFAULT_ACCENT);
static SETS: [[AtomicU32; 3]; 5] = [const { [const { AtomicU32::new(u32::MAX) }; 3] }; 5];

/// The ambient color, opaque ARGB.
pub fn accent() -> u32 {
    0xff00_0000 | ACCENT.load(Ordering::Relaxed)
}

/// The ambient color at opacity `a` (0..255), premultiplied ARGB.
pub fn accent_a(a: u32) -> u32 {
    let c = ACCENT.load(Ordering::Relaxed);
    let m = |v: u32| v * a / 255;
    a << 24 | m(c >> 16 & 255) << 16 | m(c >> 8 & 255) << 8 | m(c & 255)
}

/// The ambient color, lighter by `k` (0..1) toward white: hovers.
pub fn accent_light(k: f32) -> u32 {
    let c = ACCENT.load(Ordering::Relaxed);
    let l = |v: u32| (v as f32 + (255.0 - v as f32) * k).round() as u32;
    0xff00_0000 | l(c >> 16 & 255) << 16 | l(c >> 8 & 255) << 8 | l(c & 255)
}

/// Text on the accent: near black on a light one, near white on a dark one.
pub fn accent_ink() -> u32 {
    let c = ACCENT.load(Ordering::Relaxed);
    let luma = 0.2126 * (c >> 16 & 255) as f32 + 0.7152 * (c >> 8 & 255) as f32 + 0.0722 * (c & 255) as f32;
    if luma > 110.0 {
        0xff15_101f
    } else {
        0xfff4_f2fa
    }
}

/// True when it changed.
pub fn set_accent(rgb: u32) -> bool {
    ACCENT.swap(rgb & 0xff_ffff, Ordering::Relaxed) != rgb & 0xff_ffff
}

pub fn glass(p: Place) -> Glass {
    let s = &SETS[p.index()];
    let v = [0, 1, 2].map(|i| s[i].load(Ordering::Relaxed));
    if v[0] == u32::MAX {
        return default_glass(p);
    }
    Glass { dim: f32::from_bits(v[0]), blur: f32::from_bits(v[1]), bezel: f32::from_bits(v[2]) }
}

pub fn set_glass(p: Place, g: Glass) {
    let g = g.clamped();
    let s = &SETS[p.index()];
    s[1].store(g.blur.to_bits(), Ordering::Relaxed);
    s[2].store(g.bezel.to_bits(), Ordering::Relaxed);
    s[0].store(g.dim.to_bits(), Ordering::Relaxed);
}

/// The widest blur any place asks for, CSS px.
pub fn max_blur() -> f32 {
    let apps = APPS.lock().map(|a| a.iter().map(|(_, g)| g.blur).fold(0.0, f32::max)).unwrap_or(0.0);
    PLACES.iter().map(|p| glass(*p).blur).fold(apps, f32::max)
}

// ------------------------------------------------------------------ programs' own glass

/// Programs with glass of their own (resc_glass_v1, Liquify) set apart from «Окна»: their
/// app id and look. Keys `app:ID` in the config and over ctl.
static APPS: std::sync::Mutex<Vec<(String, Glass)>> = std::sync::Mutex::new(Vec::new());

/// The look a program's glass takes: its own place's for the desk's apps, its own if set,
/// else that of «Окна».
pub fn glass_of_app(app_id: &str) -> Glass {
    let p = Place::of_app(app_id);
    if p != Place::Windows {
        return glass(p);
    }
    app_glass(app_id).unwrap_or_else(|| glass(Place::Windows))
}

pub fn app_glass(app_id: &str) -> Option<Glass> {
    APPS.lock().ok()?.iter().find(|(a, _)| a == app_id).map(|(_, g)| *g)
}

/// A program's own look (None: as «Окна» again).
pub fn set_app_glass(app_id: &str, g: Option<Glass>) {
    let Ok(mut a) = APPS.lock() else { return };
    a.retain(|(x, _)| x != app_id);
    if let Some(g) = g {
        a.push((app_id.to_string(), g.clamped()));
    }
}

/// The programs with a look of their own.
pub fn app_glasses() -> Vec<(String, Glass)> {
    APPS.lock().map(|a| a.clone()).unwrap_or_default()
}

/// A glass key (`files`… or `app:ID`) set to `g`; false: no such place.
pub fn set_glass_key(k: &str, g: Glass) -> bool {
    if let Some(id) = k.strip_prefix("app:").filter(|id| !id.is_empty()) {
        set_app_glass(id, Some(g));
        return true;
    }
    match Place::from_key(k) {
        Some(p) => {
            set_glass(p, g);
            true
        }
        None => false,
    }
}

// ------------------------------------------------------------------ the ambient color

/// The ambient color of a picture (opaque ARGB, `w`×`h`), as Liquify's `accent_of`.
pub fn accent_of(px: &[u32], w: usize, h: usize) -> u32 {
    if w == 0 || h == 0 || px.len() < w * h {
        return DEFAULT_ACCENT;
    }
    let mut sum = [0f64; 3];
    let n = 60usize;
    for j in 0..n {
        for i in 0..n {
            // The middle of each of 60×60 boxes (the wallpaper is opaque).
            let (x, y) = (((i * 2 + 1) * w) / (2 * n), ((j * 2 + 1) * h) / (2 * n));
            let p = px[y.min(h - 1) * w + x.min(w - 1)];
            sum[0] += (p >> 16 & 255) as f64 / 255.0;
            sum[1] += (p >> 8 & 255) as f64 / 255.0;
            sum[2] += (p & 255) as f64 / 255.0;
        }
    }
    let k = (n * n) as f64;
    let [r, g, b] = boost([sum[0] / k, sum[1] / k, sum[2] / k].map(|v| v as f32), 1.9, 1.25);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    q(r) << 16 | q(g) << 8 | q(b)
}

fn to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    if mx <= mn {
        return (0.0, 0.0, l);
    }
    let d = mx - mn;
    let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
    let h = if mx == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s == 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let h2r = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [h2r(h + 1.0 / 3.0), h2r(h), h2r(h - 1.0 / 3.0)]
}

/// Liquify's `boost`: HSL saturation × `sm` (at most 1), lightness × `lm` (at most .76).
fn boost(c: [f32; 3], sm: f32, lm: f32) -> [f32; 3] {
    let (h, s, l) = to_hsl(c);
    from_hsl(h, (s * sm).min(1.0), (l * lm).min(0.76))
}

/// Recolors a picture (premultiplied ARGB) into the ambient color: every colored pixel
/// takes the accent's hue and saturation, keeping its own lightness (greys stay grey).
pub fn tint_pixels(px: &mut [u32]) {
    let c = ACCENT.load(Ordering::Relaxed);
    let (ah, asat, _) = to_hsl([(c >> 16 & 255) as f32 / 255.0, (c >> 8 & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]);
    for p in px.iter_mut() {
        let a = *p >> 24;
        if a == 0 {
            continue;
        }
        let af = a as f32 / 255.0;
        let un = |v: u32| ((v & 255) as f32 / 255.0 / af).min(1.0);
        let rgb = [un(*p >> 16), un(*p >> 8), un(*p)];
        let (_, s, l) = to_hsl(rgb);
        if s < 0.12 {
            continue;
        }
        let [r, g, b] = from_hsl(ah, (s * 0.5 + asat * 0.5).min(1.0), l);
        let q = |v: f32| ((v.clamp(0.0, 1.0) * af) * 255.0).round() as u32;
        *p = a << 24 | q(r) << 16 | q(g) << 8 | q(b);
    }
}

/// Whether an icon (by its theme name or file) is a place: a folder, home, the desktop,
/// a user folder, the trash, a drive. Those wear the ambient color.
pub fn is_place_icon(name: &str) -> bool {
    let n = name.rsplit('/').next().unwrap_or(name);
    n.starts_with("folder") || n.starts_with("user-") || n.starts_with("inode-directory") || n.starts_with("network-workgroup")
}

// ------------------------------------------------------------------ sharing with the apps

fn file() -> std::path::PathBuf {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(rt).join("resc-desk-look")
}

pub fn to_text() -> String {
    let mut s = format!("accent {:06x}\n", ACCENT.load(Ordering::Relaxed));
    for p in PLACES {
        let g = glass(p);
        s += &format!("glass {} {} {} {}\n", p.key(), g.dim, g.blur, g.bezel);
    }
    for (a, g) in app_glasses() {
        s += &format!("glass app:{a} {} {} {}\n", g.dim, g.blur, g.bezel);
    }
    s
}

pub fn apply_text(text: &str) {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["accent", c] => {
                if let Ok(v) = u32::from_str_radix(c, 16) {
                    set_accent(v);
                }
            }
            ["glass", k, dim, blur, bezel] => {
                if let (Ok(dim), Ok(blur), Ok(bezel)) = (dim.parse(), blur.parse(), bezel.parse()) {
                    set_glass_key(k, Glass { dim, blur, bezel });
                }
            }
            _ => {}
        }
    }
}

/// The compositor: tells the apps.
pub fn publish() {
    let path = file();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, to_text()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

thread_local! {
    static SEEN: std::cell::Cell<Option<std::time::SystemTime>> = const { std::cell::Cell::new(None) };
}

/// An app: reads the file again if it changed. True when it did (redraw).
pub fn poll() -> bool {
    let path = file();
    let Ok(m) = std::fs::metadata(&path).and_then(|m| m.modified()) else { return false };
    if SEEN.with(|s| s.get()) == Some(m) {
        return false;
    }
    SEEN.with(|s| s.set(Some(m)));
    match std::fs::read_to_string(&path) {
        Ok(t) => {
            let before = to_text();
            apply_text(&t);
            before != to_text()
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_like_liquify() {
        // A dull blue picture comes out vivid, a grey one stays grey.
        let blue = vec![0xff40_5070u32; 64 * 64];
        let a = accent_of(&blue, 64, 64);
        let (h, s, l) = to_hsl([(a >> 16) as f32 / 255.0, (a >> 8 & 255) as f32 / 255.0, (a & 255) as f32 / 255.0]);
        assert!((h - 0.6).abs() < 0.05 && s > 0.45 && l <= 0.77, "{a:06x}");
        let grey = vec![0xff80_8080u32; 64 * 64];
        let g = accent_of(&grey, 64, 64);
        assert_eq!(g >> 16, g & 255);
        assert_eq!(accent_of(&[], 0, 0), DEFAULT_ACCENT);
    }

    #[test]
    fn places_round_trip_through_the_file_text() {
        set_glass(Place::Timer, Glass { dim: 0.3, blur: 20.0, bezel: 12.0 });
        set_glass(Place::Files, Glass { dim: 7.0, blur: -3.0, bezel: 1000.0 });
        assert_eq!(glass(Place::Files), Glass { dim: 1.0, blur: 0.0, bezel: MAX_BEZEL });
        let t = to_text();
        set_glass(Place::Timer, default_glass(Place::Timer));
        apply_text(&t);
        assert_eq!(glass(Place::Timer), Glass { dim: 0.3, blur: 20.0, bezel: 12.0 });
        assert_eq!(Place::of_app("resc.files"), Place::Files);
        assert_eq!(Place::of_app("firefox"), Place::Windows);
        set_glass(Place::Files, default_glass(Place::Files));
        set_glass(Place::Timer, default_glass(Place::Timer));
    }

    #[test]
    fn icons_take_the_hue_keep_the_light() {
        set_accent(0xe05050);
        // Papirus blue folder, premultiplied, opaque; and a grey pixel; and a transparent one.
        let mut px = vec![0xff52_94e2, 0xff80_8080, 0];
        tint_pixels(&mut px);
        let (r, g, b) = (px[0] >> 16 & 255, px[0] >> 8 & 255, px[0] & 255);
        assert!(r > g && r > b, "{:08x}", px[0]);
        assert_eq!(px[1], 0xff80_8080);
        assert_eq!(px[2], 0);
        assert!(is_place_icon("folder-documents") && is_place_icon("/usr/share/icons/Papirus/48x48/places/user-home.svg"));
        assert!(!is_place_icon("application-pdf"));
        set_accent(DEFAULT_ACCENT);
    }
}
