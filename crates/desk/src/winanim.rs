//! Windows as drops of liquid glass, after the approved prototypes (`morph` and `peek`): a
//! window grows out of its taskbar button's icon and shrinks back into it (opening,
//! minimizing, coming back), the preview card pops out of the button on hover and sinks back,
//! and a click on the card spreads it into the window.
//!
//! Time, not springs: every parameter rides its own cubic bezier over its own part [a, b] of
//! the animation (`Curve`, the prototypes' `bez`), and may overshoot. A drop goes between two
//! looks (`Look`: its box and corners, the icon in it, the content's insets, the title and the
//! caption buttons); turned around mid-way it starts back from where it is, so nothing jumps.
//! All four edges ride one curve: it flies and grows at once. At the button the drop is the
//! button's icon (30 px, corners 7): the window is hidden behind the icon there.
//!
//! What is drawn (the GPU path, gpu/mod.rs): the drop's glass (`shaders/resc/drop.frag`),
//! smooth-unioned with the taskbar's hover drop under the button by `k` (a bridge that swells
//! only while the drop lets go of the button or lands in it: nothing at rest); the window's
//! content, opaque, stretched into the drop (only its surfaces' destination rects change); on
//! the overlays' layer the icon, the title and the caption buttons, never stretched
//! (`draw_head`). The CPU path does not animate: windows appear and disappear.
use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{fade, Canvas, Xf};
use crate::shell::{self, col};
use crate::state::WKey;

/// A CSS cubic-bezier over the part [a, b] of the animation's time.
#[derive(Clone, Copy, Debug)]
pub struct Curve {
    p: [f32; 4],
    a: f32,
    b: f32,
}

pub const fn c(x1: f32, y1: f32, x2: f32, y2: f32, a: f32, b: f32) -> Curve {
    Curve { p: [x1, y1, x2, y2], a, b }
}

/// The prototypes' `bez`: cubic-bezier(x1, y1, x2, y2) at `x`, x(t) inverted by bisection.
/// The result may leave 0..1 (overshoot).
pub fn bez(p: [f32; 4], x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let [x1, y1, x2, y2] = p;
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let (mut lo, mut hi, mut t) = (0.0f32, 1.0f32, x);
    for _ in 0..30 {
        let xt = ((ax * t + bx) * t + cx) * t;
        if (xt - x).abs() < 1e-5 {
            break;
        }
        if xt < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    ((ay * t + by) * t + cy) * t
}

impl Curve {
    /// Its progress at `e`, the animation's time (0..1).
    pub fn at(&self, e: f32) -> f32 {
        bez(self.p, ((e - self.a) / (self.b - self.a).max(1e-4)).clamp(0.0, 1.0))
    }
}

/// One animation's curves (the prototypes' `defaults()`, whose four edges are linked).
struct Curves {
    edges: Curve,
    radius: Curve,
    /// The icon's size, and its way into the title bar's corner.
    icon: Curve,
    inset: Curve,
    /// The content's insets and opacity, and the glass going from a window's look to a panel's.
    thumb: Curve,
    /// The title (and the card's ✕).
    header: Curve,
    /// Minimize and maximize (and a window's ✕).
    buttons: Curve,
    /// The bridge to the hover drop: k = KMAX · 4n(1 − n).
    neck: Curve,
}

const NEVER: Curve = c(0.0, 0.0, 1.0, 1.0, 0.99, 1.0);
/// The bridge's widest (logical px).
const KMAX: f32 = 50.0;

/// Opening (morph `open`): all four edges together, a soft landing with a hint of overshoot.
/// The content's insets ride the icon's way into the corner; the title comes with the
/// prototype's content curve.
const OPEN: Curves = Curves {
    edges: c(0.25, 0.1, 0.25, 1.06, 0.0, 1.0),
    radius: c(0.3, 0.0, 0.3, 1.0, 0.1, 0.7),
    icon: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.9),
    inset: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.9),
    thumb: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.9),
    header: c(0.4, 0.0, 0.2, 1.0, 0.55, 0.95),
    buttons: c(0.3, 0.0, 0.3, 1.12, 0.5, 0.95),
    neck: c(0.25, 0.0, 0.4, 1.0, 0.0, 0.6),
};
/// Minimizing (morph `close`, progress toward the button): quick start, all together; the
/// caption buttons go first, the bridge swells as it lands.
const CLOSE: Curves = Curves {
    edges: c(0.45, 0.0, 0.7, 1.0, 0.0, 1.0),
    radius: c(0.4, 0.0, 0.6, 1.0, 0.3, 0.9),
    icon: c(0.4, 0.0, 0.6, 1.0, 0.1, 1.0),
    inset: c(0.4, 0.0, 0.6, 1.0, 0.1, 1.0),
    thumb: c(0.4, 0.0, 0.6, 1.0, 0.1, 1.0),
    header: c(0.3, 0.0, 0.6, 1.0, 0.0, 0.35),
    buttons: c(0.3, 0.0, 0.6, 1.0, 0.0, 0.3),
    neck: c(0.4, 0.0, 0.6, 1.0, 0.45, 1.0),
};
/// The preview card popping out of the button (peek `pop`).
const POP: Curves = Curves {
    edges: c(0.25, 0.1, 0.25, 1.08, 0.0, 1.0),
    radius: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.6),
    icon: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.8),
    inset: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.8),
    thumb: c(0.4, 0.0, 0.2, 1.0, 0.3, 0.9),
    header: c(0.4, 0.0, 0.2, 1.0, 0.45, 0.95),
    buttons: NEVER,
    neck: c(0.25, 0.0, 0.4, 1.0, 0.0, 0.6),
};
/// The card sinking back (peek `hide`).
const HIDE: Curves = Curves {
    edges: c(0.45, 0.0, 0.7, 1.0, 0.0, 1.0),
    radius: c(0.4, 0.0, 0.6, 1.0, 0.3, 0.9),
    icon: c(0.4, 0.0, 0.6, 1.0, 0.1, 1.0),
    inset: c(0.4, 0.0, 0.6, 1.0, 0.1, 1.0),
    thumb: c(0.3, 0.0, 0.6, 1.0, 0.0, 0.45),
    header: c(0.3, 0.0, 0.6, 1.0, 0.0, 0.3),
    buttons: NEVER,
    neck: c(0.4, 0.0, 0.6, 1.0, 0.45, 1.0),
};
/// The card spreading into the window (peek `expand`): the thumbnail's margins melt away
/// early, minimize and maximize grow in from 20%.
const EXPAND: Curves = Curves {
    edges: c(0.25, 0.1, 0.25, 1.06, 0.0, 1.0),
    radius: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.7),
    icon: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.9),
    inset: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.9),
    thumb: c(0.3, 0.0, 0.3, 1.0, 0.0, 0.45),
    header: c(0.3, 0.0, 0.6, 1.0, 0.0, 0.3),
    buttons: c(0.3, 0.0, 0.3, 1.12, 0.2, 0.7),
    neck: NEVER,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A window out of its button (opening, coming back).
    Open,
    /// A window into its button (minimizing).
    Close,
    /// The preview card out of the button.
    Pop,
    /// The card back into it.
    Hide,
    /// The card into the window (clicked).
    Expand,
}

impl Kind {
    fn curves(self) -> &'static Curves {
        match self {
            Kind::Open => &OPEN,
            Kind::Close => &CLOSE,
            Kind::Pop => &POP,
            Kind::Hide => &HIDE,
            Kind::Expand => &EXPAND,
        }
    }

    /// Seconds: the window's morph 650 ms, the preview's 600 ms.
    fn dur(self) -> f32 {
        match self {
            Kind::Open | Kind::Close => 0.65,
            _ => 0.6,
        }
    }
}

/// A drop at rest or anywhere between (logical px).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// The drop: x0, y0, x1, y1, and its corners.
    pub b: [f32; 4],
    pub r: f32,
    /// The app icon: from the drop's top left, and its size.
    pub icon: [f32; 3],
    /// The window's content: insets from the drop's edges (left, top, right, bottom), its
    /// opacity, its top and bottom corners.
    pub img: [f32; 4],
    pub img_a: f32,
    pub img_r: [f32; 2],
    /// The title; the ✕; minimize and maximize (0..1, the last may overshoot).
    pub header: f32,
    pub close: f32,
    pub buttons: f32,
    /// The glass's look: 0 a window's (deep tint, its shadow, the rim light), 1 a panel's.
    pub panel: f32,
    /// How much of its shadow it casts (none under the button's icon).
    pub sh: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn edges(r: Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.x1() as f32, r.y1() as f32]
}

/// One drop's animation.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub kind: Kind,
    /// Where it started: a resting look, or where the last one had got to.
    pub from: Look,
    /// Seconds so far (the shell's clock: slow motion slows it).
    pub t: f32,
    /// The bridge it had when it turned around: let go of as it moves.
    pub k0: f32,
    /// Where it comes from and bridges to, when not its taskbar button (a clicked icon,
    /// life.rs).
    pub anchor: Option<Rect>,
}

/// A window's animation (state.rs `Window::morph`).
pub type Morph = Anim;

impl Anim {
    pub fn new(kind: Kind, from: Look, k0: f32) -> Anim {
        Anim { kind, from, t: 0.0, k0, anchor: None }
    }

    /// 0..1 of its time.
    pub fn progress(&self) -> f32 {
        (self.t / self.kind.dur()).clamp(0.0, 1.0)
    }

    pub fn done(&self) -> bool {
        self.t >= self.kind.dur()
    }

    /// Where it is now on its way to `to`: the look and the bridge's k (logical px).
    pub fn now(&self, to: &Look) -> (Look, f32) {
        let e = self.progress();
        let cv = self.kind.curves();
        let (a, z) = (&self.from, to);
        let pe = cv.edges.at(e);
        let mut b = [0.0; 4];
        for i in 0..4 {
            b[i] = lerp(a.b[i], z.b[i], pe);
        }
        b[2] = b[2].max(b[0] + 1.0);
        b[3] = b[3].max(b[1] + 1.0);
        let r = lerp(a.r, z.r, cv.radius.at(e).clamp(0.0, 1.0)).min((b[2] - b[0]) * 0.5).min((b[3] - b[1]) * 0.5).max(0.0);
        let (pi, ps) = (cv.inset.at(e), cv.icon.at(e));
        let icon = [lerp(a.icon[0], z.icon[0], pi), lerp(a.icon[1], z.icon[1], pi), lerp(a.icon[2], z.icon[2], ps).max(0.0)];
        let pt = cv.thumb.at(e).clamp(0.0, 1.0);
        let mut img = [0.0; 4];
        for i in 0..4 {
            img[i] = lerp(a.img[i], z.img[i], pt);
        }
        let ph = cv.header.at(e).clamp(0.0, 1.0);
        let pb = cv.buttons.at(e);
        // The ✕ comes with the title on the card, with the caption buttons on a window.
        let pc = if matches!(self.kind, Kind::Pop | Kind::Hide) { ph } else { pb.clamp(0.0, 1.0) };
        let look = Look {
            b,
            r,
            icon,
            img,
            img_a: lerp(a.img_a, z.img_a, pt).clamp(0.0, 1.0),
            img_r: [lerp(a.img_r[0], z.img_r[0], pt), lerp(a.img_r[1], z.img_r[1], pt)],
            header: lerp(a.header, z.header, ph).clamp(0.0, 1.0),
            close: lerp(a.close, z.close, pc).clamp(0.0, 1.0),
            buttons: lerp(a.buttons, z.buttons, pb),
            panel: lerp(a.panel, z.panel, pt).clamp(0.0, 1.0),
            sh: lerp(a.sh, z.sh, pe.clamp(0.0, 1.0)).clamp(0.0, 1.0),
        };
        let n = cv.neck.at(e).clamp(0.0, 1.0);
        let k = KMAX * 4.0 * n * (1.0 - n) + self.k0 * (1.0 - pe.clamp(0.0, 1.0));
        (look, k)
    }
}

/// A window where it belongs: the drop is its frame, the content 1 px in from its edges (the
/// glass rim shows all the way round) below the title bar, the icon where the title bar has
/// it. Without our frame the content fills the drop and the icon shrinks away in the corner.
pub fn window_look(d: &Desk, wk: WKey) -> Option<Look> {
    let w = d.windows.get(wk)?;
    let f = crate::wm::frame_rect(d, wk)?;
    let (bd, t) = w.frame_insets();
    let deco = bd + t > 0;
    let r = shell::window_radius(d, wk) as f32;
    let ti = shell::title_icon(f);
    let (b, top) = (bd as f32, (bd + t) as f32);
    let on = deco as u8 as f32;
    Some(Look {
        b: edges(f),
        r,
        icon: if deco { [(ti.x - f.x) as f32, (ti.y - f.y) as f32, ti.w as f32] } else { [12.0, 8.0, 0.0] },
        img: [b, top, b, b],
        img_a: 1.0,
        img_r: [0.0, (r - b).max(0.0)],
        header: on,
        close: on,
        buttons: on,
        panel: 0.0,
        sh: on,
    })
}

/// In the taskbar button `btn`: the drop is the button's icon. A window's content is opaque all
/// the way (it only stretches), a little inside the icon there: theme icons leave a margin it
/// would show through. The card's thumbnail comes in from nothing.
pub fn button_look(btn: Rect, card: bool) -> Look {
    let i = shell::button_icon(btn);
    Look {
        b: edges(i),
        r: 7.0,
        icon: [0.0, 0.0, i.w as f32],
        img: [3.0; 4],
        img_a: if card { 0.0 } else { 1.0 },
        img_r: [5.0, 5.0],
        panel: card as u8 as f32,
        ..Look::default()
    }
}

/// An app icon `i` somewhere on the screen (logical), as the drop a window grows out of: like
/// a taskbar button's.
pub fn icon_look(i: Rect) -> Look {
    Look {
        b: edges(i),
        r: (i.w as f32 * 0.24).round(),
        icon: [0.0, 0.0, i.w as f32],
        img: [3.0; 4],
        img_a: 1.0,
        img_r: [5.0, 5.0],
        ..Look::default()
    }
}

/// The preview card at rest: the card `panel`, the thumbnail `thumb` in it (insets 8 / 34 / 8 /
/// 8, corners 9), the icon and title in its header with the ✕.
pub fn card_look(panel: Rect, thumb: Rect) -> Look {
    Look {
        b: edges(panel),
        r: shell::PEEK_R as f32,
        icon: [10.0, 8.0, 18.0],
        img: [(thumb.x - panel.x) as f32, (thumb.y - panel.y) as f32, (panel.x1() - thumb.x1()) as f32, (panel.y1() - thumb.y1()) as f32],
        img_a: 1.0,
        img_r: [9.0, 9.0],
        header: 1.0,
        close: 1.0,
        buttons: 0.0,
        panel: 1.0,
        sh: 1.0,
    }
}

/// A drop as drawn now.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub look: Look,
    /// The bridge to the hover drop (smooth-union radius, logical px).
    pub k: f32,
    /// The taskbar button it comes out of / goes into.
    pub btn: Rect,
    /// The window's content where it is at rest (logical): it is mapped onto the drop's
    /// content rect.
    pub content: Rect,
}

impl Pose {
    /// The content rect now (logical x0, y0, x1, y1).
    pub fn img(&self) -> [f32; 4] {
        let (b, i) = (self.look.b, self.look.img);
        let (x0, y0) = (b[0] + i[0], b[1] + i[1]);
        [x0, y0, (b[2] - i[2]).max(x0), (b[3] - i[3]).max(y0)]
    }

    /// A rect of the window's content (logical, as at rest) where it is drawn now, physical on
    /// output `xf`: stretched with the drop, each axis on its own.
    pub fn map_px(&self, xf: Xf, r: Rect) -> Rect {
        let [x0, y0, x1, y1] = self.img();
        let c = self.content;
        let (sx, sy) = ((x1 - x0) / c.w.max(1) as f32, (y1 - y0) / c.h.max(1) as f32);
        let px = |x: i32| xf.fx((x0 + (x - c.x) as f32 * sx) as f64).round() as i32;
        let py = |y: i32| xf.fy((y0 + (y - c.y) as f32 * sy) as f64).round() as i32;
        Rect::from_edges(px(r.x), py(r.y), px(r.x1()), py(r.y1()))
    }

    /// surface.frag's clip: the content rect with its corners (physical x, y, w, h, top
    /// corners, bottom corners).
    pub fn clip_px(&self, xf: Xf) -> [f32; 6] {
        let [x0, y0, x1, y1] = self.img();
        let s = xf.scale as f32;
        let (px0, py0) = (xf.fx(x0 as f64) as f32, xf.fy(y0 as f64) as f32);
        let (w, h) = ((x1 - x0) * s, (y1 - y0) * s);
        let cap = |r: f32| (r * s).min(w * 0.5).min(h * 0.5).max(0.0);
        // Never "no clip": the content stays inside its rect even with square corners.
        [px0, py0, w.max(1.0), h.max(1.0), cap(self.look.img_r[0]).max(0.01), cap(self.look.img_r[1]).max(0.01)]
    }

    /// The drop (logical, rounded out).
    pub fn body(&self) -> Rect {
        let b = self.look.b;
        Rect::from_edges(b[0].floor() as i32, b[1].floor() as i32, b[2].ceil() as i32, b[3].ceil() as i32)
    }

    /// The flying icon (logical).
    pub fn icon_rect(&self) -> Rect {
        let [dx, dy, size] = self.look.icon;
        let s = size.round() as i32;
        Rect::new((self.look.b[0] + dx).round() as i32, (self.look.b[1] + dy).round() as i32, s, s)
    }

    /// Everything it covers (logical): the drop with its shadow, the bridge, the icon.
    pub fn bounds(&self) -> Rect {
        let mut r = self.body().inflate(shell::SHADOW);
        if self.k > 0.0 {
            r = r.union(&self.btn.inflate(8));
        }
        r.union(&self.icon_rect().inflate(2))
    }
}

/// Where a window's content is at rest (its geometry, logical).
pub fn content_rect(d: &Desk, wk: WKey) -> Option<Rect> {
    let w = d.windows.get(wk)?;
    let g = d.surfaces.get(w.surf)?.geom();
    Some(Rect::new(w.x, w.y, g.w, g.h))
}

/// Window `wk` as drawn now, while it grows, shrinks or spreads.
pub fn pose(d: &Desk, wk: WKey) -> Option<Pose> {
    let m = d.windows.get(wk)?.morph?;
    let btn = match m.anchor {
        Some(a) => a,
        None => shell::button_rect(d, wk)?,
    };
    let to = match m.kind {
        Kind::Close | Kind::Hide => button_look(btn, false),
        _ => window_look(d, wk)?,
    };
    let (look, k) = m.now(&to);
    Some(Pose { look, k, btn, content: content_rect(d, wk)? })
}

/// The window grows out of its taskbar button (opening, coming back from minimized). One
/// shrinking away right now turns around where it is.
pub fn open(d: &mut Desk, wk: WKey) {
    start(d, wk, Kind::Open, None);
}

/// The window grows out of a clicked icon `from` (life.rs, «из значка»), bridged to the drop
/// `anchor` under it.
pub fn open_from(d: &mut Desk, wk: WKey, from: Look, anchor: Rect) {
    start(d, wk, Kind::Open, Some(from));
    if let Some(m) = d.windows.get_mut(wk).and_then(|w| w.morph.as_mut()) {
        m.anchor = Some(anchor);
    }
}

/// The window shrinks into its taskbar button (minimizing). It is minimized already: it is
/// drawn until it gets there (`shown`).
pub fn close(d: &mut Desk, wk: WKey) {
    start(d, wk, Kind::Close, None);
}

/// The window spreads out of the preview card, which looked like `card` when it was clicked.
pub fn expand(d: &mut Desk, wk: WKey, card: Look) {
    start(d, wk, Kind::Expand, Some(card));
}

fn start(d: &mut Desk, wk: WKey, kind: Kind, from: Option<Look>) {
    if !d.windows.contains(wk) {
        return;
    }
    // The CPU path does not animate; nor does a window without a button (a dialog, a
    // taskbar under a fullscreen window): it just shows or goes.
    let Some(btn) = shell::button_rect(d, wk).filter(|_| d.gpu.is_some()) else {
        d.windows[wk].morph = None;
        return;
    };
    let now = pose(d, wk);
    let (from, k0) = match (from, now) {
        (Some(f), _) => (f, 0.0),
        (None, Some(p)) => (p.look, p.k),
        (None, None) => match kind {
            Kind::Close => match window_look(d, wk) {
                Some(l) => (l, 0.0),
                None => return,
            },
            _ => (button_look(btn, false), 0.0),
        },
    };
    if let Some(p) = now {
        d.damage(p.bounds());
    }
    // A clock that stood still would make the first step a long one.
    if !d.overlay.animating {
        d.overlay.anim_ms = d.now_ms();
    }
    d.windows[wk].morph = Some(Anim::new(kind, from, k0));
    d.overlay.animating = true;
    if let Some(p) = pose(d, wk) {
        d.damage(p.bounds());
    }
}

/// A minimized window still on its way into the button: drawn.
pub fn shown(d: &Desk, wk: WKey) -> bool {
    d.windows.get(wk).map_or(false, |w| w.morph.map_or(false, |m| m.kind == Kind::Close))
}

/// Advances every window's animation by `dt` seconds; true while any moves.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let keys: Vec<WKey> = d.windows.iter().filter(|(_, w)| w.morph.is_some()).map(|(k, _)| k).collect();
    let mut moving = false;
    for wk in keys {
        let before = pose(d, wk).map(|p| p.bounds());
        let m = d.windows[wk].morph.as_mut().unwrap();
        m.t += dt;
        let done = m.done();
        let after = pose(d, wk).map(|p| p.bounds());
        if done {
            d.windows[wk].morph = None;
        }
        for r in [before, after].into_iter().flatten() {
            d.damage(r);
        }
        if done {
            // At rest: the window as it is (or gone), its frame and surfaces in full.
            crate::wm::damage_window(d, wk);
        }
        moving |= !done;
    }
    moving
}

/// What the overlays' layer draws for the drops on output `oi` (logical): the windows' heads
/// and icons, the preview card (render.rs `layer_extents`).
pub fn layer_rects(d: &Desk, oi: usize) -> Vec<Rect> {
    let orect = d.outputs[oi].rect();
    let mut v: Vec<Rect> = d
        .windows
        .iter()
        .filter(|(_, w)| w.morph.is_some())
        .filter_map(|(k, _)| pose(d, k))
        .map(|p| {
            let b = p.body();
            Rect::new(b.x, b.y, b.w, b.h.min(40)).inflate(2).union(&p.icon_rect().inflate(2))
        })
        .filter(|r| r.overlaps(&orect))
        .collect();
    if let Some(c) = shell::card(d).filter(|c| c.oi == oi) {
        v.push(c.pose.body().inflate(2).union(&c.pose.icon_rect().inflate(2)));
    }
    v
}

/// The moving windows' heads on output `oi` (the overlays' layer, GPU path).
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let orect = d.outputs[oi].rect();
    for (wk, w) in d.windows.iter() {
        if w.morph.is_none() || w.workspace != d.workspace {
            continue;
        }
        let Some(p) = pose(d, wk) else { continue };
        if !p.bounds().overlaps(&orect) || !xf.r(p.bounds()).overlaps(&clip) {
            continue;
        }
        let room = crate::wm::frame_rect(d, wk).map_or(0, |f| f.w - 2 * shell::BORDER - 3 * shell::BTN_W - 52);
        draw_head(cv, clip, xf, &p, &w.app_id, &w.title, room, w.activated, false);
    }
}

/// The parts of a drop that are never stretched: the title (and the ✕) and the caption
/// buttons along its top, clipped to it, then the app icon, the anchor. On the window they
/// land exactly where its frame draws them (shell.rs `draw_deco`). `room`: the title's width
/// where it is going; `active`: a window's focus (dims its title); `close_hot`: the card's ✕
/// under the pointer.
pub fn draw_head(cv: &mut Canvas, clip: Rect, xf: Xf, p: &Pose, app_id: &str, title: &str, room: i32, active: bool, close_hot: bool) {
    let l = &p.look;
    let s = xf.scale as f32;
    let hclip = clip.intersect(&xf.r(p.body()));
    let tb = l.buttons.clamp(0.0, 1.0);
    let (x1, y0) = (l.b[2], l.b[1]);
    let cy = y0 + 17.0;
    // The ✕: the card's 18 px from its edge, the window's 24 (the middle of its 46 px button).
    let close_x = x1 - lerp(18.0, 24.0, tb);
    let text = if active { col::TEXT } else { col::TEXT_DIM };
    let ir = p.icon_rect();
    if l.header > 0.01 {
        let tx = (l.b[0] + l.icon[0] + l.icon[2] + lerp(8.0, 10.0, tb)).round();
        let right = close_x - 14.0 - tb * 2.0 * shell::BTN_W as f32;
        if right > tx + 1.0 {
            let name = if title.is_empty() { app_id } else { title };
            let name = shell::ellipsize(name, 12.5, font::W::Semi, room.max(1));
            let band = Rect::from_edges(tx as i32, y0.floor() as i32, right.ceil() as i32, (y0 + 34.0).ceil() as i32);
            let tclip = hclip.intersect(&xf.r(band));
            let c = fade(text, (l.header * 255.0) as u32);
            font::draw_text_w(cv, tclip, xf, tx as i32, (y0 + 22.0).round() as i32, &name, 12.5, c, room.max(1), font::W::Semi);
        }
    }
    let lw = 1.1 * s.max(1.0);
    if l.close > 0.01 {
        if close_hot {
            let r = Rect::new((close_x - 14.0).round() as i32, (y0 + 5.0).round() as i32, 28, 24);
            cv.rrect(hclip, xf.r(r), xf.len(7), col::CLOSE_HOVER);
        }
        let g = if close_hot { col::TEXT } else { text };
        let half = 5.5 * lerp(0.9, 1.0, tb) * lerp(0.4, 1.0, l.close);
        let a = l.close * lerp(0.85, 1.0, tb);
        // Drawn like shell.rs `draw_x` at rest: an 11 px cross, lines 1.7 px.
        let (cx, cyy) = (close_x + 0.5, cy + 0.5);
        let c = fade(g, (a * 255.0) as u32);
        let wd = 1.7 * s;
        cv.line(hclip, shell::phys(xf, cx - half, cyy - half), shell::phys(xf, cx + half, cyy + half), wd, c);
        cv.line(hclip, shell::phys(xf, cx + half, cyy - half), shell::phys(xf, cx - half, cyy + half), wd, c);
    }
    if l.buttons > 0.005 {
        // Minimize grows out of a point both ways; maximize starts as a dot as thick as its
        // line, swells as a circle and squares off.
        let t = l.buttons.max(0.0);
        let (mx, xx) = (close_x - 2.0 * shell::BTN_W as f32, close_x - shell::BTN_W as f32);
        let hw = 5.0 * t;
        cv.line(hclip, shell::phys(xf, mx - hw, cy + 0.5), shell::phys(xf, mx + hw, cy + 0.5), lw, text);
        let lwl = lw / s;
        let half = ((5.0 - lwl * 0.5) * t).max(0.0);
        let corner = lerp(half, 2.0 - lwl * 0.5, ((t - 0.35) / 0.65).clamp(0.0, 1.0)).min(half).max(0.0);
        let (pcx, pcy) = shell::phys(xf, xx, cy);
        stroke_box(cv, hclip, pcx, pcy, half * s, corner * s, lw, text);
    }
    shell::draw_app_tile(cv, clip, xf, ir, app_id, title);
}

/// A rounded square's outline centred on (`cx`, `cy`), physical: half size `half`, corners
/// `r`, line `lw` centred on the path (half 0: a dot as thick as the line).
fn stroke_box(cv: &mut Canvas, clip: Rect, cx: f32, cy: f32, half: f32, r: f32, lw: f32, color: u32) {
    let pad = (half + lw).ceil() as i32 + 2;
    let bb = Rect::new(cx as i32 - pad, cy as i32 - pad, 2 * pad + 1, 2 * pad + 1).intersect(&clip).intersect(&cv.bounds());
    let r = r.min(half);
    for y in bb.y..bb.y1() {
        for x in bb.x..bb.x1() {
            let (qx, qy) = ((x as f32 + 0.5 - cx).abs() - (half - r), (y as f32 + 0.5 - cy).abs() - (half - r));
            let sd = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let cov = (lw * 0.5 - sd.abs() + 0.5).clamp(0.0, 1.0);
            if cov > 0.0 {
                let p = &mut cv.px[y as usize * cv.stride + x as usize];
                *p = crate::render::blend(*p, fade(color, (cov * 255.0) as u32));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_ends_and_overshoot() {
        let p = [0.25, 0.1, 0.25, 1.06];
        assert_eq!(bez(p, 0.0), 0.0);
        assert_eq!(bez(p, 1.0), 1.0);
        // Overshoots before landing, as the prototypes' curve does.
        assert!((0..100).map(|i| bez(p, i as f32 / 100.0)).fold(0.0f32, f32::max) > 1.0);
        // Linear is linear.
        assert!((bez([0.0, 0.0, 1.0, 1.0], 0.3) - 0.3).abs() < 1e-3);
    }

    #[test]
    fn curve_waits_for_its_window() {
        let cv = c(0.3, 0.0, 0.3, 1.0, 0.5, 0.95);
        assert_eq!(cv.at(0.2), 0.0);
        assert_eq!(cv.at(0.96), 1.0);
        assert!(cv.at(0.7) > 0.0 && cv.at(0.7) < 1.0);
    }

    #[test]
    fn turning_around_is_continuous() {
        let btn = button_look(Rect::new(100, 800, 42, 42), false);
        let win = Look { b: [200.0, 100.0, 900.0, 600.0], r: 10.0, icon: [13.0, 9.0, 16.0], img: [1.0, 33.0, 1.0, 1.0], img_a: 1.0, img_r: [0.0, 9.0], header: 1.0, close: 1.0, buttons: 1.0, panel: 0.0, sh: 1.0 };
        let mut a = Anim::new(Kind::Open, btn, 0.0);
        a.t = 0.3;
        let (mid, k) = a.now(&win);
        assert!(mid.b[0] > 106.0 && mid.b[0] < 200.0 * 1.1);
        // Minimized again right there: the way back starts where it is.
        let back = Anim::new(Kind::Close, mid, k);
        let (start, k1) = back.now(&btn);
        assert_eq!(start.b, mid.b);
        assert!((k1 - k).abs() < 1e-3);
        // It lands on the resting look.
        a.t = 10.0;
        let end = a.now(&win).0;
        assert!(end.b.iter().zip(win.b).all(|(x, y)| (x - y).abs() < 1e-3));
        assert!(end.icon.iter().zip(win.icon).all(|(x, y)| (x - y).abs() < 1e-3));
    }
}
