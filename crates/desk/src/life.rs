//! Programs coming and going, after the approved prototype «Жизнь окна» (scratchpad
//! `apps/index.html`): the taskbar's buttons grow in and pop out while their neighbours make
//! room and close up, a program that is loading breathes as a drop of glass, and a closed
//! window goes away as a drop too instead of vanishing.
//!
//! Starting a program (`launch`): «из кнопки» (О1) — its button grows in on the taskbar right
//! away, a drop under it breathing until the first window shows, which then grows out of the
//! button (winanim.rs `open`); «из значка» (О2) — the drop breathes under the icon that was
//! clicked (in «Пуск»), and the window flies out of it, bridged to it (`winanim::open_from`),
//! while its button grows in on the taskbar.
//!
//! Closing (`gone`): the window's last picture stays (its surfaces' textures on the GPU,
//! `holds`; where it was, `Snap`) and goes as a «ghost»: into its button, which then pops (З1);
//! squeezed into a round drop that evaporates (З2); or sucked into the ✕ it was closed with,
//! the far edges first (З3).
//!
//! All of it is timed like winanim.rs: cubic beziers over parts of a phase. The CPU path keeps
//! the taskbar's part (it is cheap) and leaves the windows out.
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::{Canvas, Xf};
use crate::state::{SKey, WKey};
use crate::winanim::{self, bez, Anim, Kind, Look, Pose};

/// A button growing in; popping out (s).
const ROOM: f32 = 0.32;
const POP: f32 = 0.34;
/// The window's morph (winanim.rs `Kind::Open`/`Close`).
const MORPH: f32 = 0.65;
/// The drop under a clicked icon growing in; one breath while the program loads (s).
const ORIGIN_IN: f32 = 0.24;
const BREATH: f32 = 0.9;
/// A program that shows no window by then is given up on (s).
const WAIT_MAX: f32 = 10.0;
/// З2 and З3's whole time (s).
const EVAP: f32 = 0.55;
const SUCK: f32 = 0.77;

const EASE: [f32; 4] = [0.3, 0.0, 0.2, 1.0];
const SOFT: [f32; 4] = [0.4, 0.0, 0.6, 1.0];
/// Grows in with a little overshoot; pops out with a breath in first.
const POPIN: [f32; 4] = [0.3, 0.0, 0.3, 1.25];
const POPOUT: [f32; 4] = [0.45, -0.6, 0.7, 1.0];

/// `p` over the part [a, b] of `u` (0..1).
fn seg(p: [f32; 4], u: f32, a: f32, b: f32) -> f32 {
    bez(p, ((u - a) / (b - a)).clamp(0.0, 1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A loading drop's size `t` s into breathing: 1 at first, up to 1.12 and back.
fn breath(t: f32) -> f32 {
    1.06 + 0.06 * (t / BREATH * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2).sin()
}

/// What a taskbar button belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Win(WKey),
    /// A program that is loading (О1).
    Launch(u32),
    /// A window that has closed and is going away.
    Ghost(u32),
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    In,
    Rest,
    Out,
}

/// A taskbar button: where it is in the row and how it comes and goes.
pub struct Slot {
    pub key: Key,
    phase: Phase,
    t: f32,
    /// The drop of its own under it (not the hover one): its size, 0 none.
    pub drop: f32,
    /// Its program's window has just shown: seconds since, and the drop's size then.
    after: Option<(f32, f32)>,
}

impl Slot {
    fn new(key: Key, grow: bool) -> Slot {
        Slot { key, phase: if grow { Phase::In } else { Phase::Rest }, t: 0.0, drop: 0.0, after: None }
    }

    /// How much of its place in the row it takes (0..1).
    pub fn weight(&self) -> f32 {
        match self.phase {
            Phase::In => seg(EASE, self.t / ROOM, 0.0, 1.0),
            Phase::Rest => 1.0,
            Phase::Out => 1.0 - seg(EASE, self.t / POP, 0.25, 1.0),
        }
    }

    /// Its icon's size (may overshoot).
    pub fn icon(&self) -> f32 {
        match self.phase {
            Phase::In => bez(POPIN, (self.t / ROOM).min(1.0)),
            Phase::Rest => 1.0,
            Phase::Out => (1.0 - bez(POPOUT, (self.t / POP).min(1.0))).max(0.0),
        }
    }

    fn out(&mut self) {
        if self.phase != Phase::Out {
            self.phase = Phase::Out;
            self.t = 0.0;
        }
    }
}

/// A program started from the desktop, until its first window shows.
pub struct Launch {
    id: u32,
    /// Its icon (an app id or icon name).
    pub icon: String,
    /// What its windows may be called (lower case).
    names: Vec<String>,
    /// The icon that was clicked (logical); None: it starts from its taskbar button.
    origin: Option<Rect>,
    t: f32,
    /// Its window flies out of the clicked icon: which, seconds since, the drop's size then.
    flying: Option<(WKey, f32, f32)>,
    /// Given up on: seconds since, the drop's size then.
    giving_up: Option<(f32, f32)>,
}

impl Launch {
    /// The drop under the clicked icon: its size now.
    fn origin_drop(&self) -> f32 {
        if let Some((_, t, d0)) = self.flying {
            let u = t / MORPH;
            return lerp(d0, 1.0, (u * 4.0).min(1.0)) * (1.0 - seg(SOFT, u, 0.5, 1.0));
        }
        if let Some((t, d0)) = self.giving_up {
            return d0 * (1.0 - bez(POPOUT, (t / POP).min(1.0))).max(0.0);
        }
        if self.t < ORIGIN_IN { bez(POPIN, self.t / ORIGIN_IN) } else { breath(self.t - ORIGIN_IN) }
    }

    fn waiting(&self) -> bool {
        self.flying.is_none() && self.giving_up.is_none()
    }
}

/// A closed window going away: its last picture.
pub struct Ghost {
    id: u32,
    /// Its surfaces where they were (logical); their textures stay on the GPU (`holds`).
    pub surfs: Vec<(SKey, Rect)>,
    content: Rect,
    /// What it looked like when it closed, and its bridge then.
    from: Look,
    k0: f32,
    pub app_id: String,
    pub title: String,
    pub active: bool,
    /// 0: into its button (З1), 1: evaporates (З2), 2: into the ✕ (З3).
    kind: u8,
    t: f32,
    /// The title's width at rest.
    pub room: i32,
}

impl Ghost {
    fn total(&self) -> f32 {
        match self.kind {
            0 => MORPH,
            1 => EVAP,
            _ => SUCK,
        }
    }

    /// When its button starts to pop (s).
    fn button_goes(&self) -> f32 {
        match self.kind {
            0 => MORPH,
            1 => 0.45 * EVAP,
            _ => 0.7 * SUCK,
        }
    }
}

/// What a window looked like when it last drew (state.rs `Window::snap`), relative to its
/// content's origin: what a closed window goes away as.
pub struct Snap {
    pub surfs: Vec<(SKey, Rect)>,
    pub content: Rect,
    pub look: Look,
}

#[derive(Default)]
pub struct Life {
    pub slots: Vec<Slot>,
    pub launches: Vec<Launch>,
    pub ghosts: Vec<Ghost>,
    next: u32,
}

fn shift(r: Rect, dx: i32, dy: i32) -> Rect {
    Rect::new(r.x + dx, r.y + dy, r.w, r.h)
}

fn shift_look(mut l: Look, dx: f32, dy: f32) -> Look {
    l.b = [l.b[0] + dx, l.b[1] + dy, l.b[2] + dx, l.b[3] + dy];
    l
}

fn start_clock(d: &mut Desk) {
    if !d.overlay.animating {
        d.overlay.anim_ms = d.now_ms();
    }
    d.overlay.animating = true;
}

// ------------------------------------------------------------------ starting

/// Starts `cmd`, a program whose icon is `icon` and whose windows may be called one of
/// `names`, clicked at `origin` (its icon there, logical).
pub fn launch(d: &mut Desk, cmd: &str, icon: &str, names: &[&str], origin: Option<Rect>) {
    let mut names: Vec<String> = names.iter().filter(|n| !n.is_empty()).map(|n| n.to_lowercase()).collect();
    names.dedup();
    // Clicked again while it is still loading: once is enough (a second Steam would only
    // leave a drop breathing for nothing).
    if !names.is_empty() && d.overlay.life.launches.iter().any(|l| l.waiting() && l.names == names) {
        return;
    }
    // Already running: its window comes forward. The command still runs (a program that
    // opens more windows, a terminal, gets its new one), but no drop waits for a window:
    // one-instance programs (Steam, Telegram) only wake the one they have.
    // («Проводник» opens a window each time, with its drop: not this way.)
    let multi = names.iter().any(|n| n == "resc.files");
    let running = if multi { None } else { d.windows.iter().filter(|(_, w)| w.parent.is_none() && names_match(&names, &w.app_id)).map(|(k, _)| k).last() };
    if let Some(wk) = running {
        d.spawn(cmd);
        crate::wm::unminimize(d, wk);
        crate::wm::focus_window(d, Some(wk));
        return;
    }
    d.spawn(cmd);
    if d.outputs.is_empty() {
        return;
    }
    let origin = origin.filter(|_| d.display_cfg.anim_launch == 1 && d.gpu.is_some());
    let l = &mut d.overlay.life;
    l.next += 1;
    let id = l.next;
    l.launches.push(Launch { id, icon: icon.to_string(), names, origin, t: 0.0, flying: None, giving_up: None });
    if origin.is_none() {
        l.slots.push(Slot::new(Key::Launch(id), true));
    }
    start_clock(d);
    crate::shell::damage_taskbars(d);
}

/// A command typed or bound to a key: its program's name is its icon.
pub fn launch_cmd(d: &mut Desk, cmd: &str) {
    let first = cmd.split_whitespace().find(|w| !w.contains('=')).unwrap_or("");
    let name = first.rsplit('/').next().unwrap_or(first).to_string();
    launch(d, cmd, &name, &[&name], None);
}

/// Whether a window with app id `app` belongs to a program called `names` (lowercase).
fn names_match(names: &[String], app: &str) -> bool {
    let a = app.to_lowercase();
    !a.is_empty() && names.iter().any(|n| *n == a || n.rsplit('.').next() == Some(a.as_str()) || a.rsplit('.').next() == Some(n.as_str()))
}

/// The launch window `wk` comes from: one whose names match its app id, else the oldest
/// still waiting.
fn pick_launch(d: &Desk, wk: WKey) -> Option<usize> {
    let w = d.windows.get(wk)?;
    let a = w.app_id.to_lowercase();
    let ls = &d.overlay.life.launches;
    let named = |l: &Launch| names_match(&l.names, &a);
    ls.iter().position(|l| l.waiting() && named(l)).or_else(|| ls.iter().position(|l| l.waiting()))
}

/// Window `wk` shows (wm.rs `map_window`): it gets its button and, if it was started from
/// the desktop, comes out of where it was started. True when it already grows from somewhere
/// (no `winanim::open` then).
pub fn mapped(d: &mut Desk, wk: WKey) -> bool {
    if d.windows.get(wk).map_or(true, |w| w.parent.is_some()) {
        return false;
    }
    if d.overlay.life.slots.iter().any(|s| s.key == Key::Win(wk)) {
        return false;
    }
    start_clock(d);
    crate::shell::damage_taskbars(d);
    let Some(i) = pick_launch(d, wk) else {
        d.overlay.life.slots.push(Slot::new(Key::Win(wk), true));
        return false;
    };
    let l = &mut d.overlay.life.launches[i];
    match l.origin {
        Some(o) => {
            let d0 = l.origin_drop();
            l.flying = Some((wk, 0.0, d0));
            d.overlay.life.slots.push(Slot::new(Key::Win(wk), true));
            winanim::open_from(d, wk, winanim::icon_look(o), origin_rect(o, 1.0));
            d.windows.get(wk).map_or(false, |w| w.morph.is_some())
        }
        None => {
            let id = l.id;
            d.overlay.life.launches.remove(i);
            let life = &mut d.overlay.life;
            match life.slots.iter_mut().find(|s| s.key == Key::Launch(id)) {
                Some(s) => {
                    s.key = Key::Win(wk);
                    s.after = Some((0.0, s.drop.max(1.0)));
                }
                None => life.slots.push(Slot::new(Key::Win(wk), true)),
            }
            false
        }
    }
}

/// The drop under a clicked icon `o` at size `k` (logical).
fn origin_rect(o: Rect, k: f32) -> Rect {
    scaled(o.inflate(8), k)
}

fn scaled(r: Rect, k: f32) -> Rect {
    let (cx, cy) = (r.x as f32 + r.w as f32 * 0.5, r.y as f32 + r.h as f32 * 0.5);
    let (hw, hh) = (r.w as f32 * 0.5 * k, r.h as f32 * 0.5 * k);
    Rect::from_edges((cx - hw).round() as i32, (cy - hh).round() as i32, (cx + hw).round() as i32, (cy + hh).round() as i32)
}

// ------------------------------------------------------------------ closing

/// Remembers how window `wk` looks now (wm.rs `toplevel_commit`): its picture is what it
/// goes away as when it closes.
pub fn snapshot(d: &mut Desk, wk: WKey) {
    let Some(w) = d.windows.get(wk) else { return };
    if w.parent.is_some() || w.desktop() || d.surfaces.get(w.surf).map_or(true, |s| s.buffer.is_none() || !s.mapped) {
        return;
    }
    let (ox, oy) = (w.x, w.y);
    let (Some(content), Some(look)) = (winanim::content_rect(d, wk), winanim::window_look(d, wk)) else { return };
    let surfs = crate::render::window_surfaces(d, wk).into_iter().map(|(k, r)| (k, shift(r, -ox, -oy))).collect();
    let snap = Snap { surfs, content: shift(content, -ox, -oy), look: shift_look(look, -ox as f32, -oy as f32) };
    d.windows[wk].snap = Some(Box::new(snap));
}

/// Window `wk` closes or hides (wm.rs `unmap_window`, `destroy_window`): its button goes,
/// and on the GPU path the window goes away as a drop with its last picture.
pub fn gone(d: &mut Desk, wk: WKey) {
    let Some(si) = d.overlay.life.slots.iter().position(|s| s.key == Key::Win(wk)) else { return };
    let Some(w) = d.windows.get(wk) else { return };
    let shown = !w.minimized && w.workspace == d.workspace && d.surfaces.get(w.surf).map_or(false, |s| s.mapped);
    let snap = w.snap.as_ref().filter(|_| shown && d.gpu.is_some() && crate::shell::button_rect(d, wk).is_some());
    let Some(s) = snap else {
        d.overlay.life.slots[si].out();
        start_clock(d);
        return;
    };
    let (ox, oy) = (w.x, w.y);
    let mut from = shift_look(s.look, ox as f32, oy as f32);
    let mut k0 = 0.0;
    if w.morph.is_some() {
        if let Some(p) = winanim::pose(d, wk) {
            from = p.look;
            k0 = p.k;
        }
    }
    let room = (s.look.b[2] - s.look.b[0]) as i32 - 2 * crate::shell::BORDER - 3 * crate::shell::BTN_W - 52;
    let ghost = Ghost {
        id: 0,
        surfs: s.surfs.iter().map(|(k, r)| (*k, shift(*r, ox, oy))).collect(),
        content: shift(s.content, ox, oy),
        from,
        k0,
        app_id: w.app_id.clone(),
        title: w.title.clone(),
        active: w.activated,
        kind: d.display_cfg.anim_close.min(2),
        t: 0.0,
        room,
    };
    let l = &mut d.overlay.life;
    l.next += 1;
    let id = l.next;
    l.ghosts.push(Ghost { id, ..ghost });
    l.slots[si].key = Key::Ghost(id);
    d.windows[wk].morph = None;
    start_clock(d);
}

/// A closed window's drop now (None: it is gone, or its button pops alone).
pub fn ghost_pose(d: &Desk, g: &Ghost) -> Option<Pose> {
    let btn = slot_rect(d, Key::Ghost(g.id));
    let f = &g.from;
    match g.kind {
        0 => {
            if g.t >= MORPH {
                return None;
            }
            let btn = btn?;
            let mut a = Anim::new(Kind::Close, g.from, g.k0);
            a.t = g.t;
            let (look, k) = a.now(&winanim::button_look(btn, false));
            Some(Pose { look, k, btn, content: g.content })
        }
        kind => {
            let (fw, fh) = ((f.b[2] - f.b[0]).max(1.0), (f.b[3] - f.b[1]).max(1.0));
            let (b, r, ca, cap) = if kind == 1 {
                // Width first, then height: a round drop in the middle, which evaporates.
                let u = g.t / EVAP;
                let (cx, cy) = ((f.b[0] + f.b[2]) * 0.5, (f.b[1] + f.b[3]) * 0.5);
                let dd = 64.0;
                let pw = seg([0.55, 0.0, 0.6, 1.0], u, 0.0, 0.6);
                let ph = seg([0.45, 0.0, 0.55, 1.0], u, 0.06, 0.66);
                let pop = (1.0 - seg([0.45, -0.8, 0.7, 1.0], u, 0.62, 1.0)).max(0.0);
                let (w, h) = (lerp(fw, dd, pw) * pop, lerp(fh, dd, ph) * pop);
                let r = lerp(f.r, w.min(h) * 0.5, seg(SOFT, u, 0.0, 0.6));
                ([cx - w * 0.5, cy - h * 0.5, cx + w * 0.5, cy + h * 0.5], r, 1.0 - (u / 0.45).clamp(0.0, 1.0), 1.0 - (u / 0.15).clamp(0.0, 1.0))
            } else {
                // Into the ✕: the far edges first, the near ones last, then the little drop pops.
                let u = g.t / SUCK;
                let (xx, xy, d0) = (f.b[2] - 24.0, f.b[1] + 17.0, 24.0);
                let t = [xx - d0 * 0.5, xy - d0 * 0.5, xx + d0 * 0.5, xy + d0 * 0.5];
                let e = |a: f32, b: f32| seg([0.5, 0.0, 0.6, 1.0], u, a, b);
                let b0 = [lerp(f.b[0], t[0], e(0.0, 0.55)), lerp(f.b[1], t[1], e(0.12, 0.62)), lerp(f.b[2], t[2], e(0.18, 0.65)), lerp(f.b[3], t[3], e(0.05, 0.6))];
                let pop = (1.0 - seg([0.45, -0.8, 0.7, 1.0], u, 0.66, 0.86)).max(0.0);
                let b = [lerp(xx, b0[0], pop), lerp(xy, b0[1], pop), lerp(xx, b0[2], pop), lerp(xy, b0[3], pop)];
                let r = lerp(f.r, (b[2] - b[0]).min(b[3] - b[1]) * 0.5, e(0.0, 0.6));
                (b, r, 1.0 - ((u - 0.25) / 0.3).clamp(0.0, 1.0), 1.0 - (u / 0.15).clamp(0.0, 1.0))
            };
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            if w < 0.5 || h < 0.5 {
                return None;
            }
            let (sx, sy) = (w / fw, h / fh);
            let s = sx.min(sy);
            let look = Look {
                b,
                r: r.max(0.0),
                icon: [f.icon[0] * sx, f.icon[1] * sy, f.icon[2] * s],
                img: [f.img[0] * sx, f.img[1] * sy, f.img[2] * sx, f.img[3] * sy],
                img_a: ca,
                img_r: [f.img_r[0] * s, f.img_r[1] * s],
                header: f.header * cap,
                close: f.close * cap,
                buttons: f.buttons * cap,
                panel: 0.0,
                sh: f.sh * ca,
            };
            Some(Pose { look, k: 0.0, btn: btn.unwrap_or_default(), content: g.content })
        }
    }
}

/// The surfaces the closed windows still show (the GPU keeps their textures).
pub fn holds(d: &Desk, sk: SKey) -> bool {
    d.overlay.life.ghosts.iter().any(|g| g.surfs.iter().any(|(k, _)| *k == sk))
}

/// The closed windows going away on output `oi`, as drawn now.
pub fn ghosts(d: &Desk, oi: usize) -> Vec<(&Ghost, Pose)> {
    let orect = d.outputs[oi].rect();
    d.overlay.life.ghosts.iter().filter_map(|g| ghost_pose(d, g).map(|p| (g, p))).filter(|(_, p)| p.bounds().overlaps(&orect)).collect()
}

// ------------------------------------------------------------------ the taskbar

/// The taskbar's program buttons in order: what each is, how much of its place it takes, its
/// icon's size. Windows that show and have no slot (a desktop switched to) are at rest.
pub fn bar(d: &Desk) -> Vec<(Key, f32, f32)> {
    let mut wins: Vec<WKey> = d
        .windows
        .iter()
        .filter(|(_, w)| w.workspace == d.workspace && d.surfaces.get(w.surf).map_or(false, |s| s.mapped) && w.parent.is_none() && !w.desktop())
        .map(|(k, _)| k)
        .collect();
    let mut v = Vec::with_capacity(wins.len() + 2);
    for s in &d.overlay.life.slots {
        match s.key {
            Key::Win(wk) => {
                if let Some(i) = wins.iter().position(|k| *k == wk) {
                    wins.remove(i);
                    v.push((s.key, s.weight(), s.icon()));
                }
            }
            _ => v.push((s.key, s.weight(), s.icon())),
        }
    }
    wins.sort_by_key(|k| (k.idx, k.gen));
    v.extend(wins.into_iter().map(|k| (Key::Win(k), 1.0, 1.0)));
    v
}

/// The button of `key` (logical), on the first output that shows it.
pub fn slot_rect(d: &Desk, key: Key) -> Option<Rect> {
    (0..d.outputs.len()).find_map(|oi| crate::shell::slot_button(d, oi, key))
}

/// The icon a launch or ghost button shows.
pub fn slot_icon(d: &Desk, key: Key) -> Option<(&str, &str)> {
    let l = &d.overlay.life;
    match key {
        Key::Launch(id) => l.launches.iter().find(|x| x.id == id).map(|x| (x.icon.as_str(), x.icon.as_str())),
        Key::Ghost(id) => l.ghosts.iter().find(|g| g.id == id).map(|g| (g.app_id.as_str(), g.title.as_str())),
        Key::Win(_) => None,
    }
}

/// The drops of their own under taskbar buttons on output `oi` (logical): a program loading,
/// one whose window has just come out, one closing.
pub fn button_drops(d: &Desk, oi: usize) -> Vec<Rect> {
    d.overlay
        .life
        .slots
        .iter()
        .filter(|s| s.drop > 0.02)
        .filter_map(|s| crate::shell::slot_button(d, oi, s.key).map(|r| scaled(r, s.drop)))
        .filter(|r| r.w > 1 && r.h > 1)
        .collect()
}

/// The drops under clicked icons (logical), on output `oi`.
pub fn origin_drops(d: &Desk, oi: usize) -> Vec<Rect> {
    let orect = d.outputs[oi].rect();
    d.overlay
        .life
        .launches
        .iter()
        .filter_map(|l| l.origin.map(|o| origin_rect(o, l.origin_drop())))
        .filter(|r| r.w > 1 && r.h > 1 && r.overlaps(&orect))
        .collect()
}

/// What the overlays' layer draws for all this on output `oi` (logical): the clicked icons,
/// the closing windows' heads.
pub fn layer_rects(d: &Desk, oi: usize) -> Vec<Rect> {
    let mut v: Vec<Rect> = origin_drops(d, oi).into_iter().map(|r| r.inflate(2)).collect();
    for (_, p) in ghosts(d, oi) {
        let b = p.body();
        v.push(Rect::new(b.x, b.y, b.w, b.h.min(40)).inflate(2).union(&p.icon_rect().inflate(2)));
    }
    v
}

/// The overlays' layer: the icons waiting under their drops, the closing windows' heads (GPU
/// path).
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let orect = d.outputs[oi].rect();
    for l in &d.overlay.life.launches {
        let Some(o) = l.origin else { continue };
        if l.flying.is_some() || !o.overlaps(&orect) {
            continue;
        }
        let k = if l.giving_up.is_some() { l.origin_drop() } else { 1.0 };
        let r = scaled(o, k.min(1.0));
        if r.w > 1 {
            crate::shell::draw_app_tile(cv, clip, xf, r, &l.icon, &l.icon);
        }
    }
    for (g, p) in ghosts(d, oi) {
        if !xf.r(p.bounds()).overlaps(&clip) {
            continue;
        }
        winanim::draw_head(cv, clip, xf, &p, &g.app_id, &g.title, g.room, g.active, false);
    }
}

// ------------------------------------------------------------------ time

/// Advances it all by `dt` seconds; true while anything moves.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let l = &d.overlay.life;
    if l.slots.iter().all(|s| s.phase == Phase::Rest && s.after.is_none() && !matches!(s.key, Key::Launch(_))) && l.launches.is_empty() && l.ghosts.is_empty() {
        return false;
    }
    // Before: what to damage where things were.
    let mut dmg: Vec<Rect> = Vec::new();
    for oi in 0..d.outputs.len() {
        dmg.extend(origin_drops(d, oi).into_iter().map(|r| r.inflate(4)));
        dmg.extend(ghosts(d, oi).into_iter().map(|(_, p)| p.bounds()));
    }
    // Windows gone without a word: their buttons go too.
    let alive: Vec<bool> = d.overlay.life.slots.iter().map(|s| if let Key::Win(wk) = s.key { d.windows.contains(wk) } else { true }).collect();
    let cpu = d.gpu.is_none();
    let life = &mut d.overlay.life;
    for (s, a) in life.slots.iter_mut().zip(alive) {
        if !a {
            s.out();
        }
    }
    for g in &mut life.ghosts {
        g.t += dt;
    }
    for ln in &mut life.launches {
        ln.t += dt;
        if let Some((_, t, _)) = ln.flying.as_mut() {
            *t += dt;
        }
        if let Some((t, _)) = ln.giving_up.as_mut() {
            *t += dt;
        }
        if ln.waiting() && ln.t >= WAIT_MAX {
            ln.giving_up = Some((0.0, ln.origin_drop()));
        }
    }
    for s in &mut life.slots {
        s.t += dt;
        if s.phase == Phase::In && s.t >= ROOM {
            s.phase = Phase::Rest;
        }
        if let Some((t, _)) = s.after.as_mut() {
            *t += dt;
        }
        // The drop under it.
        s.drop = match s.key {
            Key::Launch(id) => match life.launches.iter().find(|x| x.id == id) {
                Some(ln) if ln.giving_up.is_some() => {
                    s.out();
                    s.icon()
                }
                Some(ln) => if s.phase == Phase::In { s.icon() } else { breath(ln.t - ROOM) },
                None => s.icon(),
            },
            Key::Ghost(id) => match life.ghosts.iter().find(|g| g.id == id) {
                Some(g) => {
                    if g.t >= g.button_goes() {
                        s.out();
                    }
                    if s.phase == Phase::Out {
                        s.icon()
                    } else if g.kind == 0 {
                        seg(EASE, g.t / MORPH, 0.0, 0.35)
                    } else {
                        seg(EASE, g.t / g.total(), 0.2, 0.5)
                    }
                }
                None => {
                    s.out();
                    s.icon()
                }
            },
            Key::Win(_) => match s.after {
                Some((t, d0)) if t < MORPH => {
                    let u = t / MORPH;
                    lerp(d0, 1.0, (u * 4.0).min(1.0)) * (1.0 - seg(SOFT, u, 0.55, 1.0))
                }
                _ => {
                    s.after = None;
                    0.0
                }
            },
        };
        if cpu {
            // No glass drops of their own on the CPU path: the buttons just come and go.
            s.drop = 0.0;
        }
    }
    life.slots.retain(|s| !(s.phase == Phase::Out && s.t >= POP));
    let slots = &life.slots;
    life.ghosts.retain(|g| g.t < g.total() || slots.iter().any(|s| s.key == Key::Ghost(g.id)));
    life.launches.retain(|ln| {
        if let Some((_, t, _)) = ln.flying {
            return t < MORPH;
        }
        if let Some((t, _)) = ln.giving_up {
            return t < POP;
        }
        true
    });
    // A launch given up on (О1) leaves its button popping, with nothing behind it.
    for oi in 0..d.outputs.len() {
        dmg.extend(origin_drops(d, oi).into_iter().map(|r| r.inflate(4)));
        dmg.extend(ghosts(d, oi).into_iter().map(|(_, p)| p.bounds()));
    }
    for r in dmg {
        d.damage(r);
    }
    crate::shell::damage_taskbars(d);
    true
}
