//! Monitors: what each one does and how the picture gets onto it.
//!
//! - Each monitor's own settings (by connector name): on / off, its mode (size and
//!   refresh), rotation, VRR, HDR; which one is the main one; and how several are used
//!   («Win+P»: extend, duplicate, the main one only, the second one only). The config file
//!   keeps them (layout.rs), the backends apply them (`Backend::reconfigure`).
//! - The picture's way onto a panel: turned for a rotated monitor, scaled to fit for a
//!   mirror, and for HDR converted to PQ / BT.2020 in 10 bits (SDR white at 203 nits).
//! - The «Win+P» panel: four ways, chosen with the mouse or the keys (Win+P again moves
//!   down, Enter takes it, Esc goes away).

use std::collections::HashMap;

use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::shell::{self, col};

// ------------------------------------------------------------------ settings

/// One monitor's own settings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MonCfg {
    pub off: bool,
    /// Width, height, refresh (mHz); None: the monitor's preferred mode.
    pub mode: Option<(i32, i32, i32)>,
    /// Clockwise degrees: 0, 90, 180, 270.
    pub rot: u16,
    pub vrr: bool,
    pub hdr: bool,
}

impl MonCfg {
    pub fn to_line(&self, name: &str) -> String {
        let mode = self.mode.map_or("auto".to_string(), |(w, h, r)| format!("{w}x{h}@{r}"));
        format!("monitor {name} {} mode={mode} rot={} vrr={} hdr={}", if self.off { "off" } else { "on" }, self.rot, self.vrr as u8, self.hdr as u8)
    }

    pub fn parse(words: &[&str]) -> MonCfg {
        let mut c = MonCfg::default();
        for w in words {
            match w.split_once('=') {
                None if *w == "off" => c.off = true,
                Some(("mode", m)) => c.mode = parse_mode(m),
                Some(("rot", r)) => c.rot = r.parse::<u16>().ok().filter(|r| [0, 90, 180, 270].contains(r)).unwrap_or(0),
                Some(("vrr", v)) => c.vrr = v == "1",
                Some(("hdr", v)) => c.hdr = v == "1",
                _ => {}
            }
        }
        c
    }
}

/// `2560x1440@165000` (mHz) or `2560x1440@165` (Hz).
pub fn parse_mode(s: &str) -> Option<(i32, i32, i32)> {
    let (wh, r) = s.split_once('@').unwrap_or((s, "0"));
    let (w, h) = wh.split_once('x')?;
    let r: f64 = r.parse().ok()?;
    let mhz = if r < 1000.0 { (r * 1000.0).round() as i32 } else { r.round() as i32 };
    Some((w.parse().ok()?, h.parse().ok()?, mhz))
}

/// How several monitors are used (Win+P).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Project {
    PrimaryOnly,
    Duplicate,
    #[default]
    Extend,
    SecondOnly,
}

pub const PROJECTS: [Project; 4] = [Project::PrimaryOnly, Project::Duplicate, Project::Extend, Project::SecondOnly];

impl Project {
    pub fn key(self) -> &'static str {
        match self {
            Project::PrimaryOnly => "primary-only",
            Project::Duplicate => "duplicate",
            Project::Extend => "extend",
            Project::SecondOnly => "second-only",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Project::PrimaryOnly => "Только основной экран",
            Project::Duplicate => "Повторяющийся",
            Project::Extend => "Расширить",
            Project::SecondOnly => "Только второй экран",
        }
    }

    pub fn from_key(k: &str) -> Option<Project> {
        PROJECTS.into_iter().find(|p| p.key() == k)
    }
}

/// A connected monitor as the backend has it.
#[derive(Clone, Debug, Default)]
pub struct MonInfo {
    pub name: String,
    /// Its modes (w, h, mHz), and the one set.
    pub modes: Vec<(i32, i32, i32)>,
    /// The panel's mode now.
    pub cur: (i32, i32, i32),
    pub rot: u16,
    /// Showing anything (its own picture or a mirror).
    pub on: bool,
    pub mirror: Option<String>,
    pub vrr_cap: bool,
    pub vrr: bool,
    pub hdr_cap: bool,
    pub hdr: bool,
}

/// What a connected monitor does.
#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    /// A desk output of its own.
    Show,
    /// Shows that monitor's picture, scaled to fit.
    Mirror(String),
    Off,
}

/// The roles of the connected monitors `names` (in the backend's order).
pub fn roles(names: &[String], cfgs: &HashMap<String, MonCfg>, primary: Option<&str>, project: Project) -> Vec<Role> {
    let n = names.len();
    if n == 0 {
        return Vec::new();
    }
    let p = primary.and_then(|p| names.iter().position(|x| x == p)).unwrap_or(0);
    let mut v: Vec<Role> = match project {
        Project::Extend => names.iter().map(|nm| if cfgs.get(nm).map_or(false, |c| c.off) { Role::Off } else { Role::Show }).collect(),
        Project::Duplicate => (0..n).map(|i| if i == p { Role::Show } else { Role::Mirror(names[p].clone()) }).collect(),
        Project::PrimaryOnly => (0..n).map(|i| if i == p { Role::Show } else { Role::Off }).collect(),
        Project::SecondOnly => {
            let s = (0..n).find(|i| *i != p).unwrap_or(p);
            (0..n).map(|i| if i == s { Role::Show } else { Role::Off }).collect()
        }
    };
    // Never everything dark.
    if !v.iter().any(|r| *r == Role::Show) {
        v[p] = Role::Show;
    }
    v
}

/// The mode to set among `modes` (w, h, mHz) for `want`: that one, else the same size at
/// the nearest refresh, else `preferred`.
pub fn pick_mode(modes: &[(i32, i32, i32)], want: Option<(i32, i32, i32)>, preferred: usize) -> usize {
    let Some((w, h, r)) = want else { return preferred.min(modes.len().saturating_sub(1)) };
    if let Some(i) = modes.iter().position(|m| *m == (w, h, r)) {
        return i;
    }
    modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.0 == w && m.1 == h)
        .min_by_key(|(_, m)| (m.2 - r).abs())
        .map(|(i, _)| i)
        .unwrap_or(preferred.min(modes.len().saturating_sub(1)))
}

// ------------------------------------------------------------------ the picture onto a panel

/// The desk's size of a panel `w`×`h` turned by `rot`.
pub fn rot_dims(w: i32, h: i32, rot: u16) -> (i32, i32) {
    if rot % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    }
}

/// Where the desk's pixel (`x`, `y`) of an output `ow`×`oh` lands on the panel. The
/// monitor turned clockwise by `rot`: its picture turns the other way.
#[inline]
pub fn to_panel(x: i32, y: i32, ow: i32, oh: i32, rot: u16) -> (i32, i32) {
    match rot {
        90 => (oh - 1 - y, x),
        180 => (ow - 1 - x, oh - 1 - y),
        270 => (y, ow - 1 - x),
        _ => (x, y),
    }
}

/// The panel's rect for the desk's rect `r`.
pub fn rect_to_panel(r: Rect, ow: i32, oh: i32, rot: u16) -> Rect {
    if r.is_empty() {
        return r;
    }
    let (ax, ay) = to_panel(r.x, r.y, ow, oh, rot);
    let (bx, by) = to_panel(r.x1() - 1, r.y1() - 1, ow, oh, rot);
    Rect::from_edges(ax.min(bx), ay.min(by), ax.max(bx) + 1, ay.max(by) + 1)
}

/// Copies the desk's rect `r` of `src` (`ow`×`oh`) onto a panel (`pitch` bytes a row),
/// turned by `rot`, in PQ 10-bit if `hdr`.
///
/// # Safety
/// `dst` must hold the whole panel (`rot_dims(ow, oh)` × `pitch`).
pub unsafe fn copy_out(src: &[u32], ow: i32, oh: i32, r: Rect, rot: u16, dst: *mut u8, pitch: usize, hdr: bool) {
    let r = r.intersect(&Rect::new(0, 0, ow, oh));
    if r.is_empty() {
        return;
    }
    if rot == 0 && !hdr {
        for y in r.y..r.y1() {
            let s = &src[(y * ow + r.x) as usize..(y * ow + r.x1()) as usize];
            std::ptr::copy_nonoverlapping(s.as_ptr(), dst.add(y as usize * pitch + r.x as usize * 4) as *mut u32, s.len());
        }
        return;
    }
    for y in r.y..r.y1() {
        for x in r.x..r.x1() {
            let p = src[(y * ow + x) as usize];
            let (u, v) = to_panel(x, y, ow, oh, rot);
            *(dst.add(v as usize * pitch + u as usize * 4) as *mut u32) = if hdr { pq10(p) } else { p };
        }
    }
}

/// A mirror's fit: scale and offset of a `sw`×`sh` picture centred on `dw`×`dh`.
pub fn fit(sw: i32, sh: i32, dw: i32, dh: i32) -> (f64, i32, i32) {
    let k = (dw as f64 / sw.max(1) as f64).min(dh as f64 / sh.max(1) as f64);
    let (w, h) = ((sw as f64 * k).round() as i32, (sh as f64 * k).round() as i32);
    (k, (dw - w) / 2, (dh - h) / 2)
}

/// Copies the source's rect `r` (in `src`, `sw`×`sh`) onto a mirror panel `dw`×`dh`,
/// scaled to fit (nearest pixel). Returns the panel's rect it wrote.
///
/// # Safety
/// `dst` must hold the whole panel (`dh` rows of `pitch` bytes).
pub unsafe fn copy_mirror(src: &[u32], sw: i32, sh: i32, r: Rect, dst: *mut u8, pitch: usize, dw: i32, dh: i32, hdr: bool) -> Rect {
    let (k, ox, oy) = fit(sw, sh, dw, dh);
    let r = r.intersect(&Rect::new(0, 0, sw, sh));
    if r.is_empty() {
        return r;
    }
    let pr = Rect::from_edges(
        ox + (r.x as f64 * k).floor() as i32,
        oy + (r.y as f64 * k).floor() as i32,
        ox + (r.x1() as f64 * k).ceil() as i32,
        oy + (r.y1() as f64 * k).ceil() as i32,
    )
    .intersect(&Rect::new(0, 0, dw, dh));
    for v in pr.y..pr.y1() {
        let sy = (((v - oy) as f64 + 0.5) / k) as i32;
        if sy < 0 || sy >= sh {
            continue;
        }
        let row = dst.add(v as usize * pitch) as *mut u32;
        for u in pr.x..pr.x1() {
            let sx = (((u - ox) as f64 + 0.5) / k) as i32;
            if sx < 0 || sx >= sw {
                continue;
            }
            let p = src[(sy * sw + sx) as usize];
            *row.add(u as usize) = if hdr { pq10(p) } else { p };
        }
    }
    pr
}

/// SDR white in HDR, nits (ITU-R BT.2408).
pub const SDR_WHITE: f32 = 203.0;
const PQ_STEPS: usize = 16384;

struct Luts {
    lin: [f32; 256],
    pq: Vec<u16>,
}

fn luts() -> &'static Luts {
    static L: std::sync::OnceLock<Luts> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut lin = [0f32; 256];
        for (i, v) in lin.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        let pq = (0..PQ_STEPS)
            .map(|i| {
                let l = i as f32 / (PQ_STEPS - 1) as f32 * SDR_WHITE / 10000.0;
                (pq_encode(l) * 1023.0).round() as u16
            })
            .collect();
        Luts { lin, pq }
    })
}

/// SMPTE ST 2084: linear (1 = 10000 nits) → signal 0..1.
pub fn pq_encode(l: f32) -> f32 {
    let (m1, m2, c1, c2, c3) = (0.159_301_76, 78.843_75, 0.835_937_5, 18.851_563, 18.6875);
    let lp = l.max(0.0).powf(m1);
    ((c1 + c2 * lp) / (1.0 + c3 * lp)).powf(m2)
}

/// An sRGB pixel (XRGB8888) as PQ BT.2020 (XRGB2101010), SDR white at 203 nits.
#[inline]
pub fn pq10(p: u32) -> u32 {
    let t = luts();
    let (r, g, b) = (t.lin[(p >> 16 & 255) as usize], t.lin[(p >> 8 & 255) as usize], t.lin[(p & 255) as usize]);
    // BT.709 → BT.2020 primaries (ITU-R BT.2087).
    let r2 = 0.6274 * r + 0.3293 * g + 0.0433 * b;
    let g2 = 0.0691 * r + 0.9195 * g + 0.0114 * b;
    let b2 = 0.0164 * r + 0.0880 * g + 0.8956 * b;
    let q = |v: f32| t.pq[((v.clamp(0.0, 1.0) * (PQ_STEPS - 1) as f32) + 0.5) as usize] as u32;
    q(r2) << 20 | q(g2) << 10 | q(b2)
}

// ------------------------------------------------------------------ the desk's side

#[derive(Default)]
pub struct State {
    /// The Win+P panel: on which output, the row picked.
    pub panel: Option<(usize, usize)>,
    pub hover: Option<usize>,
    /// The backend has to apply the settings anew (the main loop does it).
    pub reconfigure: bool,
}

/// The main monitor's output index (the reference for the scale and the plane's origin).
pub fn primary_index(d: &Desk) -> usize {
    d.display_cfg.primary.as_ref().and_then(|p| d.outputs.iter().position(|o| &o.name == p)).unwrap_or(0)
}

pub fn set_project(d: &mut Desk, p: Project) {
    d.display_cfg.project = p;
    // Saved once the backend has applied it (main loop).
    d.mons.reconfigure = true;
}

const PW: i32 = 360;
const ROW: i32 = 60;
const TOP: i32 = 58;
const RADIUS: i32 = 20;

fn panel(d: &Desk, oi: usize) -> (Rect, Vec<Rect>) {
    let o = d.outputs[oi].rect();
    let tb = shell::taskbar_rect(d, oi);
    let h = TOP + PROJECTS.len() as i32 * ROW + 12;
    let p = Rect::new(o.x1() - 12 - PW, tb.y - 10 - h, PW, h);
    let rows = (0..PROJECTS.len() as i32).map(|i| Rect::new(p.x + 10, p.y + TOP + i * ROW, PW - 20, ROW - 6)).collect();
    (p, rows)
}

pub fn glass_rect(d: &Desk, oi: usize) -> Option<Rect> {
    d.mons.panel.filter(|(o, _)| *o == oi && oi < d.outputs.len()).map(|(o, _)| panel(d, o).0)
}

fn damage(d: &mut Desk) {
    if let Some((oi, _)) = d.mons.panel.filter(|(o, _)| *o < d.outputs.len()) {
        let r = panel(d, oi).0;
        d.damage(r.inflate(shell::GLASS_REACH));
    }
}

/// Win+P: opens the panel, or moves to the next way when it is open.
pub fn toggle(d: &mut Desk) {
    if let Some((oi, sel)) = d.mons.panel {
        d.mons.panel = Some((oi, (sel + 1) % PROJECTS.len()));
        damage(d);
        return;
    }
    shell::close_menus(d);
    crate::tray::close(d);
    let oi = shell::bar_output(d, d.output_at(d.seat.x, d.seat.y).unwrap_or(0));
    let cur = PROJECTS.iter().position(|p| *p == d.display_cfg.project).unwrap_or(2);
    d.mons.panel = Some((oi, cur));
    d.mons.hover = None;
    damage(d);
}

pub fn close(d: &mut Desk) {
    damage(d);
    d.mons.panel = None;
}

fn choose(d: &mut Desk, i: usize) {
    close(d);
    set_project(d, PROJECTS[i]);
}

pub fn key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    let Some((oi, sel)) = d.mons.panel else { return false };
    if !pressed {
        return true;
    }
    use crate::input::key;
    let n = PROJECTS.len();
    match code {
        key::ESC => close(d),
        key::ENTER | key::SPACE => {
            d.seat.swallowed.push(code);
            choose(d, sel);
            return true;
        }
        key::UP => d.mons.panel = Some((oi, (sel + n - 1) % n)),
        key::DOWN | key::P => d.mons.panel = Some((oi, (sel + 1) % n)),
        _ => return true,
    }
    damage(d);
    true
}

pub fn click(d: &mut Desk, x: f64, y: f64) -> bool {
    let Some((oi, _)) = d.mons.panel else { return false };
    let (p, rows) = panel(d, oi);
    if !p.contains(x, y) {
        close(d);
        return true;
    }
    if let Some(i) = rows.iter().position(|r| r.contains(x, y)) {
        choose(d, i);
    }
    true
}

pub fn hover(d: &mut Desk, x: f64, y: f64) {
    let Some((oi, _)) = d.mons.panel else { return };
    let h = panel(d, oi).1.iter().position(|r| r.contains(x, y));
    if h != d.mons.hover {
        d.mons.hover = h;
        damage(d);
    }
}

/// Two little screens for way `p`: lit (the picture is there) or dark.
fn glyph(cv: &mut Canvas, clip: Rect, xf: Xf, x: i32, cy: i32, p: Project) {
    let s = xf.scale as f32;
    let scr = |cv: &mut Canvas, r: Rect, lit: bool| {
        if lit {
            cv.rrect(clip, xf.r(r), xf.len(3), col::accent());
        } else {
            cv.rrect_stroke(clip, xf.r(r), xf.len(3), 1.5 * s, 0xffffff, 150, 150);
        }
    };
    let a = Rect::new(x, cy - 9, 22, 16);
    let b = Rect::new(x + 26, cy - 9, 22, 16);
    match p {
        Project::PrimaryOnly => {
            scr(cv, a, true);
            scr(cv, b, false);
        }
        Project::Duplicate => {
            scr(cv, a, true);
            scr(cv, b, true);
            // The same picture: an equals sign between them.
            cv.fill(clip, xf.r(Rect::new(x + 22, cy - 3, 4, 1)), col::TEXT);
            cv.fill(clip, xf.r(Rect::new(x + 22, cy + 1, 4, 1)), col::TEXT);
        }
        Project::Extend => {
            scr(cv, Rect::new(x, cy - 9, 24, 16), true);
            scr(cv, Rect::new(x + 24, cy - 9, 24, 16), true);
        }
        Project::SecondOnly => {
            scr(cv, a, false);
            scr(cv, b, true);
        }
    }
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some((poi, sel)) = d.mons.panel else { return };
    if poi != oi {
        return;
    }
    let (p, rows) = panel(d, oi);
    shell::draw_glass(cv, clip, xf, p, RADIUS);
    font::draw_text_w(cv, clip, xf, p.x + 20, p.y + 36, "Проецировать", 16.0, col::TEXT, PW - 40, font::W::Bold);
    let n = d.outputs.len();
    let note = if n < 2 { "подключён один монитор" } else { "Win+P — дальше, Enter — выбрать" };
    let nw = font::text_width(note, 11.5);
    font::draw_text(cv, clip, xf, p.x1() - 18 - nw, p.y + 35, note, 11.5, col::TEXT_DIM, PW);
    for (i, r) in rows.iter().enumerate() {
        let pr = PROJECTS[i];
        let fill = if i == sel {
            crate::look::accent_a(40)
        } else if d.mons.hover == Some(i) {
            argb(24, 255, 255, 255)
        } else {
            argb(10, 255, 255, 255)
        };
        cv.rrect(clip, xf.r(*r), xf.len(12), fill);
        if i == sel {
            cv.rrect_stroke(clip, xf.r(*r), xf.len(12), 1.5 * xf.scale as f32, crate::look::accent() & 0xff_ffff, 220, 220);
        }
        glyph(cv, clip, xf, r.x + 14, r.y + r.h / 2, pr);
        font::draw_text_w(cv, clip, xf, r.x + 78, r.y + r.h / 2 + 5, pr.name(), 14.0, col::TEXT, r.w - 120, font::W::Semi);
        if d.display_cfg.project == pr {
            let s = "сейчас";
            let sw = font::text_width(s, 11.5);
            font::draw_text(cv, clip, xf, r.x1() - 14 - sw, r.y + r.h / 2 + 4, s, 11.5, col::TEXT_DIM, 80);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_by_way() {
        let names: Vec<String> = ["DP-1", "HDMI-A-1", "DP-2"].iter().map(|s| s.to_string()).collect();
        let mut cfgs = HashMap::new();
        cfgs.insert("DP-2".to_string(), MonCfg { off: true, ..Default::default() });
        let show = Role::Show;
        assert_eq!(roles(&names, &cfgs, Some("HDMI-A-1"), Project::Extend), vec![show.clone(), show.clone(), Role::Off]);
        assert_eq!(
            roles(&names, &cfgs, Some("HDMI-A-1"), Project::Duplicate),
            vec![Role::Mirror("HDMI-A-1".into()), show.clone(), Role::Mirror("HDMI-A-1".into())]
        );
        assert_eq!(roles(&names, &cfgs, Some("HDMI-A-1"), Project::PrimaryOnly), vec![Role::Off, show.clone(), Role::Off]);
        assert_eq!(roles(&names, &cfgs, Some("HDMI-A-1"), Project::SecondOnly), vec![show.clone(), Role::Off, Role::Off]);
        // An unknown main one: the first. Everything off in the settings: the main one stays.
        let all_off: HashMap<String, MonCfg> = names.iter().map(|n| (n.clone(), MonCfg { off: true, ..Default::default() })).collect();
        assert_eq!(roles(&names, &all_off, Some("nope"), Project::Extend), vec![show.clone(), Role::Off, Role::Off]);
        assert_eq!(roles(&names[..1], &cfgs, None, Project::SecondOnly), vec![show]);
    }

    #[test]
    fn config_lines_round_trip() {
        let c = MonCfg { off: false, mode: Some((2560, 1440, 165000)), rot: 90, vrr: true, hdr: false };
        let line = c.to_line("DP-1");
        let w: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(&w[..2], ["monitor", "DP-1"]);
        assert_eq!(MonCfg::parse(&w[2..]), c);
        assert_eq!(MonCfg::parse(&["off", "rot=45"]), MonCfg { off: true, ..Default::default() });
        assert_eq!(parse_mode("1920x1080@60"), Some((1920, 1080, 60000)));
    }

    #[test]
    fn modes_are_picked_sensibly() {
        let m = [(2560, 1440, 165000), (2560, 1440, 59951), (1920, 1080, 60000)];
        assert_eq!(pick_mode(&m, None, 0), 0);
        assert_eq!(pick_mode(&m, Some((2560, 1440, 59951)), 0), 1);
        assert_eq!(pick_mode(&m, Some((2560, 1440, 60000)), 0), 1, "nearest refresh of that size");
        assert_eq!(pick_mode(&m, Some((800, 600, 60000)), 0), 0, "gone: the preferred one");
    }

    #[test]
    fn rotation_maps_every_pixel_once() {
        for rot in [0u16, 90, 180, 270] {
            let (pw, ph) = (5, 3);
            let (ow, oh) = rot_dims(pw, ph, rot);
            let mut seen = vec![0; (pw * ph) as usize];
            for y in 0..oh {
                for x in 0..ow {
                    let (u, v) = to_panel(x, y, ow, oh, rot);
                    assert!(u >= 0 && u < pw && v >= 0 && v < ph, "rot {rot}: {x},{y} → {u},{v}");
                    seen[(v * pw + u) as usize] += 1;
                }
            }
            assert!(seen.iter().all(|n| *n == 1), "rot {rot}");
            // A rect goes where its pixels go.
            let r = Rect::new(1, 0, 2, 2);
            let pr = rect_to_panel(r, ow, oh, rot);
            let mut src = vec![0u32; (ow * oh) as usize];
            for y in r.y..r.y1() {
                for x in r.x..r.x1() {
                    src[(y * ow + x) as usize] = 0xff00_0001 + (y * ow + x) as u32;
                }
            }
            let mut dst = vec![0u32; (pw * ph) as usize];
            unsafe { copy_out(&src, ow, oh, r, rot, dst.as_mut_ptr() as *mut u8, pw as usize * 4, false) };
            for v in 0..ph {
                for u in 0..pw {
                    assert_eq!(dst[(v * pw + u) as usize] != 0, pr.contains(u as f64 + 0.5, v as f64 + 0.5), "rot {rot} at {u},{v}");
                }
            }
        }
    }

    #[test]
    fn mirror_fits_with_bars() {
        assert_eq!(fit(1920, 1080, 1280, 1024), (1280.0 / 1920.0, 0, 152));
        let src = vec![0xffff_ffffu32; 4 * 2];
        let mut dst = vec![0u32; 8 * 8];
        let pr = unsafe { copy_mirror(&src, 4, 2, Rect::new(0, 0, 4, 2), dst.as_mut_ptr() as *mut u8, 32, 8, 8, false) };
        assert_eq!(pr, Rect::new(0, 2, 8, 4));
        assert!(dst[..16].iter().all(|p| *p == 0) && dst[16..48].iter().all(|p| *p == 0xffff_ffff));
    }

    #[test]
    fn pq_puts_sdr_white_at_203_nits() {
        // 203 nits is about 58 % of the PQ signal.
        let w = pq10(0xffff_ffff);
        let (r, g, b) = (w >> 20 & 1023, w >> 10 & 1023, w & 1023);
        assert!((r as i32 - g as i32).abs() <= 1 && (g as i32 - b as i32).abs() <= 1, "{r} {g} {b}");
        assert!((585..=600).contains(&g), "{g}");
        assert_eq!(pq10(0xff00_0000), 0);
        // Monotonic.
        let mut last = 0;
        for v in 0..=255u32 {
            let q = pq10(0xff00_0000 | v << 8) >> 10 & 1023;
            assert!(q >= last);
            last = q;
        }
    }
}
