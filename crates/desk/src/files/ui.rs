//! The file manager's look (the web prototype's tokens), its glass drops and small
//! line-drawn glyphs (so the bars look the same under any icon theme).
use crate::anim::Spring;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};

pub const XF: Xf = Xf { ox: 0, oy: 0, scale: 1.0, px: 0, py: 0 };

pub const FG: u32 = argb(255, 0xec, 0xee, 0xf6);
pub const DIM: u32 = argb(255, 0x9b, 0xa1, 0xb5);
pub const FAINT: u32 = argb(255, 0x6b, 0x71, 0x86);
pub const ACCENT: u32 = argb(255, 0x7f, 0xb0, 0xff);
pub const ACCENT_RGB: (u32, u32, u32) = (0x7f, 0xb0, 0xff);
pub const WARN: u32 = argb(255, 0xf2, 0xb4, 0x5a);
pub const BAD: u32 = argb(255, 0xff, 0x6b, 0x6b);
pub const GOOD: u32 = argb(255, 0x5f, 0xd3, 0x9a);
pub const HOVER: u32 = argb(14, 255, 255, 255);
pub const SELBG: u32 = argb(38, 0x7f, 0xb0, 0xff);
pub const LINE: u32 = argb(20, 255, 255, 255);

pub fn white(a: u32) -> u32 {
    argb(a, 255, 255, 255)
}

pub fn accent(a: u32) -> u32 {
    argb(a, ACCENT_RGB.0, ACCENT_RGB.1, ACCENT_RGB.2)
}

pub fn text(cv: &mut Canvas, clip: Rect, x: i32, y: i32, s: &str, size: f32, color: u32, max_w: i32) {
    font::draw_text(cv, clip, XF, x, y, s, size, color, max_w);
}

pub fn text_w(cv: &mut Canvas, clip: Rect, x: i32, y: i32, s: &str, size: f32, color: u32, max_w: i32, w: font::W) {
    font::draw_text_w(cv, clip, XF, x, y, s, size, color, max_w, w);
}

pub fn tw(s: &str, size: f32) -> i32 {
    font::text_width(s, size)
}

/// Text centred in `r` (vertically by the baseline guess).
pub fn text_c(cv: &mut Canvas, clip: Rect, r: Rect, s: &str, size: f32, color: u32, w: font::W) {
    let tw = font::text_width_w(s, size, w).min(r.w);
    font::draw_text_w(cv, clip, XF, r.x + (r.w - tw) / 2, r.y + r.h / 2 + (size * 0.36) as i32, s, size, color, r.w, w);
}

/// A glass card: dark tint, hairline rim lit from above, soft shadow.
pub fn card(cv: &mut Canvas, clip: Rect, r: Rect, radius: i32, tint: u32) {
    for i in 1..=6 {
        let g = r.inflate(i).translate(0, 3);
        cv.rrect_stroke(clip, g, radius + i, 1.5, 0x000000, (26 - i * 4) as u32, (34 - i * 5) as u32);
    }
    cv.rrect(clip, r, radius, tint);
    cv.rrect_stroke(clip, r, radius, 1.0, 0xffffff, 46, 14);
}

/// Liquify's drop as a rectangle: each edge a spring; the edges in front of the motion
/// lead (quick), the ones behind trail, so it stretches toward the target and pulls
/// together there with a wobble. `a`: how visible it is.
#[derive(Clone, Copy)]
pub struct RectDrop {
    pub l: Spring,
    pub t: Spring,
    pub r: Spring,
    pub b: Spring,
    pub a: Spring,
    pub placed: bool,
}

const LEAD: (f32, f32) = (0.26, 0.72);
const TRAIL: (f32, f32) = (0.40, 0.72);

impl Default for RectDrop {
    fn default() -> Self {
        RectDrop {
            l: Spring::new(0.0, LEAD.0, LEAD.1),
            t: Spring::new(0.0, LEAD.0, LEAD.1),
            r: Spring::new(0.0, LEAD.0, LEAD.1),
            b: Spring::new(0.0, LEAD.0, LEAD.1),
            a: Spring::new(0.0, 0.22, 1.0),
            placed: false,
        }
    }
}

impl RectDrop {
    pub fn go(&mut self, r: Rect) {
        let (l, t, rr, b) = (r.x as f32, r.y as f32, r.x1() as f32, r.y1() as f32);
        if !self.placed || self.a.value < 0.02 {
            self.l.snap(l);
            self.t.snap(t);
            self.r.snap(rr);
            self.b.snap(b);
            self.placed = true;
        }
        let set = |s: &mut Spring, lead: bool, v: f32| {
            (s.response, s.damping) = if lead { LEAD } else { TRAIL };
            s.to(v);
        };
        let right = l > self.l.target;
        let down = t > self.t.target;
        set(&mut self.r, right, rr);
        set(&mut self.l, !right, l);
        set(&mut self.b, down, b);
        set(&mut self.t, !down, t);
        self.a.to(1.0);
    }

    /// Jumps there (scrolled lists, relayout).
    pub fn snap(&mut self, r: Rect) {
        self.l.snap(r.x as f32);
        self.t.snap(r.y as f32);
        self.r.snap(r.x1() as f32);
        self.b.snap(r.y1() as f32);
        self.placed = true;
    }

    pub fn hide(&mut self) {
        self.a.to(0.0);
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let mut m = false;
        for s in [&mut self.l, &mut self.t, &mut self.r, &mut self.b, &mut self.a] {
            m |= s.step(dt);
        }
        m
    }

    pub fn rect(&self) -> Rect {
        Rect::from_edges(self.l.value.round() as i32, self.t.value.round() as i32, self.r.value.round() as i32, self.b.value.round() as i32)
    }
}

/// Draws a selection drop: the tint (when `fill`) and the glass rim.
pub fn drop(cv: &mut Canvas, clip: Rect, r: Rect, a: f32, fill: bool) {
    if a < 0.01 || r.w < 2 || r.h < 2 {
        return;
    }
    let k = |v: f32| (v * a).round().clamp(0.0, 255.0) as u32;
    if fill {
        cv.rrect(clip, r, 7, accent(k(40.0)));
    }
    cv.rrect_stroke(clip, r, 7, 1.2, 0xbcd6ff, k(150.0), k(60.0));
}

// ------------------------------------------------------------------ glyphs

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum G {
    Back,
    Fwd,
    Up,
    Refresh,
    ChevR,
    ChevD,
    Close,
    Plus,
    Search,
    More,
    Cut,
    Copy,
    Paste,
    Rename,
    Delete,
    Sort,
    View,
    Details,
    Grid,
    Check,
    Pause,
    Play,
    Home,
    Pc,
    Trash,
    Archive,
    Eject,
    Star,
}

/// A line glyph in a square `r` (logical = physical here), `col` premultiplied.
pub fn glyph(cv: &mut Canvas, clip: Rect, r: Rect, g: G, col: u32) {
    let s = r.w.min(r.h) as f32;
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let u = s / 16.0;
    let p = |x: f32, y: f32| (cx + x * u, cy + y * u);
    let w = 1.5f32.max(u * 1.4);
    let mut ln = |cv: &mut Canvas, a: (f32, f32), b: (f32, f32)| cv.line(clip, p(a.0, a.1), p(b.0, b.1), w, col);
    match g {
        G::Back => {
            ln(cv, (-5.0, 0.0), (5.0, 0.0));
            ln(cv, (-5.0, 0.0), (-1.0, -4.0));
            ln(cv, (-5.0, 0.0), (-1.0, 4.0));
        }
        G::Fwd => {
            ln(cv, (-5.0, 0.0), (5.0, 0.0));
            ln(cv, (5.0, 0.0), (1.0, -4.0));
            ln(cv, (5.0, 0.0), (1.0, 4.0));
        }
        G::Up => {
            ln(cv, (0.0, 5.0), (0.0, -5.0));
            ln(cv, (0.0, -5.0), (-4.0, -1.0));
            ln(cv, (0.0, -5.0), (4.0, -1.0));
        }
        G::Refresh => {
            cv.ring(clip, (cx, cy), 5.0 * u, w, col);
            // Gap and arrow head at the top right.
            ln(cv, (2.0, -6.5), (5.0, -5.0));
            ln(cv, (5.0, -5.0), (4.0, -1.8));
        }
        G::ChevR => {
            ln(cv, (-2.0, -4.0), (2.0, 0.0));
            ln(cv, (2.0, 0.0), (-2.0, 4.0));
        }
        G::ChevD => {
            ln(cv, (-4.0, -2.0), (0.0, 2.0));
            ln(cv, (0.0, 2.0), (4.0, -2.0));
        }
        G::Close => {
            ln(cv, (-4.0, -4.0), (4.0, 4.0));
            ln(cv, (4.0, -4.0), (-4.0, 4.0));
        }
        G::Plus => {
            ln(cv, (-5.0, 0.0), (5.0, 0.0));
            ln(cv, (0.0, -5.0), (0.0, 5.0));
        }
        G::Search => {
            cv.ring(clip, p(-1.5, -1.5), 4.0 * u, w, col);
            ln(cv, (1.5, 1.5), (5.5, 5.5));
        }
        G::More => {
            for x in [-5.0, 0.0, 5.0] {
                cv.ring(clip, p(x, 0.0), 0.6 * u, w, col);
            }
        }
        G::Cut => {
            cv.ring(clip, p(-3.5, 4.0), 2.2 * u, w, col);
            cv.ring(clip, p(3.5, 4.0), 2.2 * u, w, col);
            ln(cv, (-2.0, 2.2), (4.0, -6.0));
            ln(cv, (2.0, 2.2), (-4.0, -6.0));
        }
        G::Copy => {
            cv.rrect_stroke(clip, Rect::new((cx - 3.0 * u) as i32, (cy - 3.0 * u) as i32, (9.0 * u) as i32, (10.0 * u) as i32), (2.0 * u) as i32, w, col & 0xffffff, col >> 24, col >> 24);
            ln(cv, (-5.5, 3.0), (-5.5, -5.0));
            ln(cv, (-5.5, -6.0), (1.5, -6.0));
        }
        G::Paste => {
            cv.rrect_stroke(clip, Rect::new((cx - 5.0 * u) as i32, (cy - 5.5 * u) as i32, (10.0 * u) as i32, (12.5 * u) as i32), (2.0 * u) as i32, w, col & 0xffffff, col >> 24, col >> 24);
            ln(cv, (-2.0, -6.5), (2.0, -6.5));
            ln(cv, (-2.5, -1.0), (2.5, -1.0));
            ln(cv, (-2.5, 2.5), (1.0, 2.5));
        }
        G::Rename => {
            cv.rrect_stroke(clip, Rect::new((cx - 7.0 * u) as i32, (cy - 3.5 * u) as i32, (11.0 * u) as i32, (7.0 * u) as i32), (1.5 * u) as i32, w, col & 0xffffff, col >> 24, col >> 24);
            ln(cv, (6.0, -6.0), (6.0, 6.0));
            ln(cv, (4.0, -6.0), (8.0, -6.0));
            ln(cv, (4.0, 6.0), (8.0, 6.0));
        }
        G::Delete => {
            ln(cv, (-6.0, -4.0), (6.0, -4.0));
            ln(cv, (-2.0, -6.0), (2.0, -6.0));
            ln(cv, (-4.5, -4.0), (-3.5, 6.0));
            ln(cv, (4.5, -4.0), (3.5, 6.0));
            ln(cv, (-3.5, 6.0), (3.5, 6.0));
        }
        G::Sort => {
            ln(cv, (-3.0, -6.0), (-3.0, 6.0));
            ln(cv, (-3.0, 6.0), (-6.0, 3.0));
            ln(cv, (3.0, 6.0), (3.0, -6.0));
            ln(cv, (3.0, -6.0), (6.0, -3.0));
        }
        G::View | G::Details => {
            for y in [-4.5, 0.0, 4.5] {
                ln(cv, (-6.0, y), (-4.5, y));
                ln(cv, (-2.0, y), (6.0, y));
            }
        }
        G::Grid => {
            for (x, y) in [(-6.0, -6.0), (1.0, -6.0), (-6.0, 1.0), (1.0, 1.0)] {
                cv.rrect_stroke(clip, Rect::new((cx + x * u) as i32, (cy + y * u) as i32, (5.0 * u) as i32, (5.0 * u) as i32), (1.0 * u) as i32, w * 0.9, col & 0xffffff, col >> 24, col >> 24);
            }
        }
        G::Check => {
            ln(cv, (-5.0, 0.0), (-1.5, 4.0));
            ln(cv, (-1.5, 4.0), (5.5, -4.5));
        }
        G::Pause => {
            ln(cv, (-2.5, -5.0), (-2.5, 5.0));
            ln(cv, (2.5, -5.0), (2.5, 5.0));
        }
        G::Play => {
            ln(cv, (-3.0, -5.0), (4.0, 0.0));
            ln(cv, (4.0, 0.0), (-3.0, 5.0));
            ln(cv, (-3.0, 5.0), (-3.0, -5.0));
        }
        G::Home => {
            ln(cv, (-7.0, -0.5), (0.0, -6.5));
            ln(cv, (0.0, -6.5), (7.0, -0.5));
            ln(cv, (-5.0, -2.0), (-5.0, 6.0));
            ln(cv, (5.0, -2.0), (5.0, 6.0));
            ln(cv, (-5.0, 6.0), (5.0, 6.0));
        }
        G::Pc => {
            cv.rrect_stroke(clip, Rect::new((cx - 7.0 * u) as i32, (cy - 6.0 * u) as i32, (14.0 * u) as i32, (9.5 * u) as i32), (1.5 * u) as i32, w, col & 0xffffff, col >> 24, col >> 24);
            ln(cv, (-3.0, 6.0), (3.0, 6.0));
            ln(cv, (0.0, 3.5), (0.0, 6.0));
        }
        G::Trash => {
            ln(cv, (-6.0, -4.0), (6.0, -4.0));
            ln(cv, (-2.0, -6.0), (2.0, -6.0));
            ln(cv, (-4.5, -4.0), (-3.5, 6.0));
            ln(cv, (4.5, -4.0), (3.5, 6.0));
            ln(cv, (-3.5, 6.0), (3.5, 6.0));
            ln(cv, (-1.0, -1.0), (-1.0, 3.5));
            ln(cv, (1.5, -1.0), (1.5, 3.5));
        }
        G::Archive => {
            cv.rrect_stroke(clip, Rect::new((cx - 6.0 * u) as i32, (cy - 6.5 * u) as i32, (12.0 * u) as i32, (13.0 * u) as i32), (2.0 * u) as i32, w, col & 0xffffff, col >> 24, col >> 24);
            for y in [-4.0, -1.5, 1.0] {
                ln(cv, (-0.8, y), (0.8, y));
            }
            ln(cv, (-1.5, 3.0), (1.5, 3.0));
        }
        G::Eject => {
            ln(cv, (-5.0, 1.5), (0.0, -4.5));
            ln(cv, (0.0, -4.5), (5.0, 1.5));
            ln(cv, (5.0, 1.5), (-5.0, 1.5));
            ln(cv, (-5.0, 5.0), (5.0, 5.0));
        }
        G::Star => {
            let pts: Vec<(f32, f32)> = (0..5).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU * 2.0 / 5.0;
                (a.cos() * 6.0, a.sin() * 6.0)
            }).collect();
            for i in 0..5 {
                ln(cv, pts[i], pts[(i + 1) % 5]);
            }
        }
    }
}

/// A folder drawn by hand (when the icon theme has none).
pub fn folder_shape(cv: &mut Canvas, clip: Rect, r: Rect) {
    let s = r.w.min(r.h);
    let (x, y) = (r.x + (r.w - s) / 2, r.y + (r.h - s) / 2);
    let tab = Rect::new(x + s / 16, y + s * 3 / 16, s * 7 / 16, s / 4);
    cv.rrect(clip, tab, (s / 16).max(1), argb(255, 0xd9, 0x9a, 0x2b));
    let body = Rect::new(x + s / 16, y + s * 5 / 16, s * 14 / 16, s * 9 / 16);
    cv.rrect(clip, body, (s / 12).max(1), argb(255, 0xf6, 0xc3, 0x4a));
}

/// A page with a folded corner and the extension on it.
pub fn page_shape(cv: &mut Canvas, clip: Rect, r: Rect, ext: &str) {
    let s = r.w.min(r.h);
    let (x, y) = (r.x + (r.w - s) / 2, r.y + (r.h - s) / 2);
    let pg = Rect::new(x + s * 3 / 16, y + s / 16, s * 10 / 16, s * 14 / 16);
    cv.rrect(clip, pg, (s / 14).max(1), argb(255, 0xe8, 0xea, 0xf2));
    if s >= 40 && !ext.is_empty() {
        let e: String = ext.chars().take(4).collect::<String>().to_uppercase();
        text_c(cv, clip, Rect::new(pg.x, pg.y + pg.h / 2, pg.w, pg.h / 2), &e, (s as f32 / 6.0).max(8.0), argb(255, 0x55, 0x5a, 0x6e), font::W::Bold);
    }
}
