//! Alt+Tab as drops of glass, after the approved prototype «Жидкий Alt+Tab» (scratchpad
//! `alttab/index.html`), variants П1 and В1: the panel grows out of a drop in the middle of the
//! screen, only its glass rim (no backing: the desktop shows through it as it is); the cards
//! come in as a wave from the chosen one outward, each a small drop that spreads into its card,
//! the window's picture settling into its thumbnail; the chosen card has a drop of clear glass
//! under it that flows to the next one, the edge in front first, the one behind catching up
//! (Liquify's Drop, like the taskbar's hover drop). Letting go of Alt spreads the chosen card
//! into its window (winanim.rs `expand`, as the preview card does) while the rest go back the
//! way they came, quickly, so nothing trails the window; Esc plays the coming in backwards.
//!
//! GPU path only (shell.rs keeps the still switcher for the CPU path). The glass: the panel's
//! rim (drop.frag with its inside left out, gpu/mod.rs) and the chosen card's drop, both under
//! the overlays' layer; the thumbnails are the windows' own textures (`thumbs`); the cards'
//! icons and titles are drawn on the overlays' layer (`draw`).
use crate::anim::Spring;
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::{Canvas, Xf};
use crate::state::WKey;
use crate::winanim::{bez, Look, Pose};

/// Coming in; the chosen window going (s); the wave's delay per card away from the chosen one.
const OPEN: f32 = 0.38;
const GO: f32 = 0.46;
const STAG: f32 = 0.026;
/// Let go of before it has come this far in: a quick Alt+Tab, no cards to spread from.
const QUICK: f32 = 0.35;
/// The chosen card's drop: its edges' springs (front, back) and how it pops up.
const LEAD: f32 = 0.26;
const TRAIL: f32 = 0.40;
const DAMP: f32 = 0.72;
const POP: (f32, f32) = (0.30, 0.62);
const PANEL_R: f32 = 22.0;
const CARD_R: f32 = 14.0;
const LENS_R: f32 = 16.0;
/// A card grows from a drop this big (logical px); the panel from this.
const SEED: f32 = 44.0;
const PANEL_SEED: (f32, f32) = (160.0, 70.0);

const EDGES: [f32; 4] = [0.25, 0.1, 0.25, 1.06];
const SOFT: [f32; 4] = [0.4, 0.0, 0.6, 1.0];
const BACK: [f32; 4] = [0.45, 0.0, 0.7, 1.0];

fn seg(p: [f32; 4], u: f32, a: f32, b: f32) -> f32 {
    bez(p, ((u - a) / (b - a)).clamp(0.0, 1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

type F4 = [f32; 4];

fn f4(r: Rect) -> F4 {
    [r.x as f32, r.y as f32, r.x1() as f32, r.y1() as f32]
}

fn lerp4(a: F4, b: F4, t: f32) -> F4 {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), lerp(a[3], b[3], t)]
}

fn centred(r: F4, w: f32, h: f32) -> F4 {
    let (cx, cy) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
    [cx - w * 0.5, cy - h * 0.5, cx + w * 0.5, cy + h * 0.5]
}

fn rect_of(r: F4) -> Rect {
    Rect::from_edges(r[0].floor() as i32, r[1].floor() as i32, r[2].ceil() as i32, r[3].ceil() as i32)
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Opening,
    Open,
    Cancel,
    Commit,
}

pub struct At {
    list: Vec<WKey>,
    sel: usize,
    mode: Mode,
    /// How far it has come in (0..1 of `OPEN`); how far the chosen window has gone (of `GO`).
    o: f32,
    c: f32,
    /// The output it shows on.
    oi: usize,
    /// The chosen card's drop: its edges (x0, x1, y0, y1) and its size.
    lens: [Spring; 4],
    k: Spring,
    /// Spreading into its window (no card of its own then).
    chosen: Option<WKey>,
}

fn layout(d: &Desk, a: &At) -> (Rect, Vec<Rect>) {
    crate::shell::switcher_layout(d, a.oi.min(d.outputs.len().saturating_sub(1)), a.list.len())
}

/// The output shown on: the pointer's.
fn pointer_output(d: &Desk) -> usize {
    d.output_at(d.seat.x, d.seat.y).unwrap_or(0)
}

/// Where the chosen card's drop belongs: the card, 3 px out.
fn lens_target(d: &Desk, a: &At) -> Option<F4> {
    layout(d, a).1.get(a.sel).map(|c| f4(c.inflate(3)))
}

fn aim(d: &Desk, a: &mut At) {
    let Some(t) = lens_target(d, a) else { return };
    let ahead = t[0] > a.lens[0].target;
    let (fi, bi) = if ahead { (1, 0) } else { (0, 1) };
    (a.lens[fi].response, a.lens[fi].damping) = (LEAD, DAMP);
    (a.lens[bi].response, a.lens[bi].damping) = (TRAIL, DAMP);
    for i in 2..4 {
        (a.lens[i].response, a.lens[i].damping) = (LEAD, DAMP);
    }
    let [x0, y0, x1, y1] = t;
    for (s, v) in a.lens.iter_mut().zip([x0, x1, y0, y1]) {
        s.to(v);
    }
}

fn snap_lens(d: &Desk, a: &mut At) {
    if let Some([x0, y0, x1, y1]) = lens_target(d, a) {
        for (s, v) in a.lens.iter_mut().zip([x0, x1, y0, y1]) {
            s.snap(v);
        }
    }
}

fn start_clock(d: &mut Desk) {
    if !d.overlay.animating {
        d.overlay.anim_ms = d.now_ms();
    }
    d.overlay.animating = true;
}

/// Alt+Tab or Shift+Alt+Tab (shell.rs `switcher_step`): it comes in, or the chosen card's
/// drop flows to the next one.
pub fn stepped(d: &mut Desk) {
    let Some((list, sel)) = d.overlay.switcher.clone() else { return };
    if d.gpu.is_none() {
        d.overlay.alttab = None;
        return;
    }
    let oi = pointer_output(d);
    let fresh = match d.overlay.alttab.as_mut() {
        Some(a) if a.mode != Mode::Commit && a.oi == oi => {
            a.list = list.clone();
            a.sel = sel;
            if a.mode == Mode::Cancel {
                a.mode = Mode::Opening;
            }
            false
        }
        _ => true,
    };
    if fresh {
        let spring = || Spring::new(0.0, LEAD, DAMP);
        let mut a = At { list, sel, mode: Mode::Opening, o: 0.0, c: 0.0, oi, lens: [spring(), spring(), spring(), spring()], k: Spring::new(0.0, POP.0, POP.1), chosen: None };
        snap_lens(d, &mut a);
        d.overlay.alttab = Some(Box::new(a));
    }
    let mut a = d.overlay.alttab.take().unwrap();
    aim(d, &mut a);
    d.overlay.alttab = Some(a);
    start_clock(d);
    damage(d);
}

/// Alt let go of with `wk` chosen (shell.rs `switcher_commit`, after it is raised): its card
/// spreads into it. A quick Alt+Tab (hardly come in) just goes.
pub fn commit(d: &mut Desk, wk: Option<WKey>) {
    let Some(mut a) = d.overlay.alttab.take() else { return };
    let card = wk.and_then(|wk| a.list.iter().position(|k| *k == wk)).and_then(|i| cards(d, &a).into_iter().find(|c| c.i == i));
    match (wk, card) {
        (Some(wk), Some(card)) if a.o >= QUICK && d.windows.contains(wk) => {
            // From the chosen card's drop (its glass is the drop now) with the thumbnail in it.
            let b = lens_now(&a).map_or(card.r, |(r, _)| r);
            let fit = card.content(card.pt);
            let look = Look {
                b,
                r: LENS_R,
                icon: [card.r[0] + 6.0 - b[0], card.r[1] + 5.0 - b[1], 18.0],
                img: [fit[0] - b[0], fit[1] - b[1], b[2] - fit[2], b[3] - fit[3]],
                img_a: card.alpha,
                img_r: [6.0, 6.0],
                header: card.ph,
                close: 0.0,
                buttons: 0.0,
                panel: 1.0,
                sh: 1.0,
            };
            crate::winanim::expand(d, wk, look);
            a.chosen = Some(wk);
            a.mode = Mode::Commit;
            a.c = 0.0;
        }
        _ => a.mode = Mode::Cancel,
    }
    a.k.to(0.0);
    d.overlay.alttab = Some(a);
    start_clock(d);
    damage(d);
}

/// A card as drawn now (logical).
pub struct Card {
    pub i: usize,
    pub wk: WKey,
    /// Its drop, corners.
    pub r: F4,
    pub rad: f32,
    /// How far the picture has settled into its thumbnail; the header; the picture's opacity.
    pub pt: f32,
    pub ph: f32,
    pub alpha: f32,
    /// The thumbnail at rest: the window's content fitted under the header.
    fit: F4,
}

impl Card {
    /// The picture's rect at `pt`: the whole drop, settling into the thumbnail.
    fn content(&self, pt: f32) -> F4 {
        lerp4(self.r, self.fit, pt)
    }
}

/// How far card `i` has come in at `o`: a wave from the chosen card outward.
fn card_u(a: &At, i: usize, o: f32) -> f32 {
    let rank = (i as i32 - a.sel as i32).abs() as f32;
    let max_rank = a.sel.max(a.list.len().saturating_sub(1 + a.sel)) as f32;
    let span = (OPEN - STAG * max_rank).max(0.05);
    ((o * OPEN - STAG * rank) / span).clamp(0.0, 1.0)
}

fn cards(d: &Desk, a: &At) -> Vec<Card> {
    let (_, cells) = layout(d, a);
    let mut v = Vec::new();
    for (i, (&wk, cell)) in a.list.iter().zip(cells).enumerate() {
        if Some(wk) == a.chosen {
            continue;
        }
        let Some(content) = crate::winanim::content_rect(d, wk) else { continue };
        let u = match a.mode {
            Mode::Commit => 1.0 - seg(BACK, a.c, 0.0, 0.45),
            _ => card_u(a, i, a.o),
        };
        if u <= 0.001 {
            continue;
        }
        let c = f4(cell);
        // The thumbnail: the content fitted into the card under its 30 px header.
        let bx = [c[0] + 6.0, c[1] + 30.0, c[2] - 6.0, c[3] - 6.0];
        let k = ((bx[2] - bx[0]) / content.w.max(1) as f32).min((bx[3] - bx[1]) / content.h.max(1) as f32);
        let fit = centred(bx, content.w as f32 * k, content.h as f32 * k);
        let e = bez(EDGES, u);
        v.push(Card {
            i,
            wk,
            r: lerp4(centred(c, SEED, SEED), c, e),
            rad: lerp(SEED * 0.5, CARD_R, seg(SOFT, u, 0.0, 0.7)),
            pt: seg(SOFT, u, 0.25, 0.9),
            ph: seg(SOFT, u, 0.5, 1.0),
            alpha: seg(SOFT, u, 0.05, 0.5),
            fit,
        });
    }
    v
}

/// The panel now: its drop and corners (logical).
fn panel_now(d: &Desk, a: &At) -> Option<(F4, f32)> {
    let u = match a.mode {
        Mode::Commit => 1.0 - seg(BACK, a.c, 0.0, 0.4),
        _ => a.o,
    };
    if u <= 0.001 {
        return None;
    }
    let p = f4(layout(d, a).0);
    let e = bez(EDGES, u);
    Some((lerp4(centred(p, PANEL_SEED.0, PANEL_SEED.1), p, e), lerp(PANEL_SEED.1 * 0.5, PANEL_R, seg(SOFT, u, 0.0, 0.7))))
}

/// The chosen card's drop now (logical), if it shows.
fn lens_now(a: &At) -> Option<(F4, f32)> {
    let k = a.k.value;
    if k < 0.03 {
        return None;
    }
    let (x0, x1) = (a.lens[0].value.min(a.lens[1].value), a.lens[0].value.max(a.lens[1].value));
    let (y0, y1) = (a.lens[2].value, a.lens[3].value);
    // Pulled out between two cards it thins a little, like a drop.
    let w0 = (crate::shell::SWITCHER_TW + 6) as f32;
    let stretch = ((x1 - x0) / w0 - 1.0).clamp(0.0, 1.0);
    let h = (y1 - y0) * (1.0 - 0.1 * stretch);
    let cy = (y0 + y1) * 0.5;
    let r = centred([x0, cy - h * 0.5, x1, cy + h * 0.5], (x1 - x0) * k, h * k);
    Some((r, LENS_R * k))
}

/// The panel's glass on output `oi`: its rim only (gpu/mod.rs leaves its inside out).
pub fn panel_drop(d: &Desk, oi: usize) -> Option<crate::startanim::DropSpec> {
    let a = d.overlay.alttab.as_ref().filter(|a| a.oi == oi)?;
    let (r, rad) = panel_now(d, a)?;
    let g = crate::glass::SHADOW * 0.6;
    Some(crate::startanim::DropSpec {
        a: r,
        ra: rad,
        b: None,
        k: 0.0,
        tint: [crate::glass::TINT_DEEP; 2],
        bezel: crate::glass::bezel(),
        sh: [g, 20.0, 6.0, 54.0],
        layer: 2,
        bridge: None,
    })
}

/// The chosen card's drop on output `oi` (logical) and its corners.
pub fn lens(d: &Desk, oi: usize) -> Option<(Rect, f32)> {
    let a = d.overlay.alttab.as_ref().filter(|a| a.oi == oi)?;
    lens_now(a).map(|(r, rad)| (rect_of(r), rad))
}

/// The cards' thumbnails on output `oi`: each window's content mapped into its card (the GPU
/// draws its surfaces there, `Pose::map_px`).
pub fn thumbs(d: &Desk, oi: usize) -> Vec<(WKey, Pose)> {
    let Some(a) = d.overlay.alttab.as_ref().filter(|a| a.oi == oi) else { return Vec::new() };
    cards(d, a)
        .into_iter()
        .filter_map(|c| {
            let content = crate::winanim::content_rect(d, c.wk)?;
            let img = c.content(c.pt);
            let look = Look {
                b: c.r,
                r: c.rad,
                img: [img[0] - c.r[0], img[1] - c.r[1], c.r[2] - img[2], c.r[3] - img[3]],
                img_a: c.alpha,
                img_r: [lerp(c.rad, 6.0, c.pt); 2],
                ..Look::default()
            };
            Some((c.wk, Pose { look, k: 0.0, btn: Rect::default(), content }))
        })
        .collect()
}

/// Everything it covers on output `oi` (logical): the panel at rest with its shadow.
pub fn bounds(d: &Desk, oi: usize) -> Option<Rect> {
    let a = d.overlay.alttab.as_ref().filter(|a| a.oi == oi)?;
    Some(layout(d, a).0.inflate(60))
}

fn damage(d: &mut Desk) {
    if let Some(r) = d.overlay.alttab.as_ref().map(|a| a.oi).and_then(|oi| bounds(d, oi)) {
        d.damage(r);
    }
}

/// The cards' icons and titles (the overlays' layer, GPU path).
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(a) = d.overlay.alttab.as_ref().filter(|a| a.oi == oi) else { return };
    for c in cards(d, a) {
        if c.ph <= 0.01 {
            continue;
        }
        let Some(w) = d.windows.get(c.wk) else { continue };
        let (x, y) = (c.r[0].round() as i32, c.r[1].round() as i32);
        let s = (18.0 * c.ph).round() as i32;
        if s > 1 {
            crate::shell::draw_app_tile(cv, clip, xf, Rect::new(x + 6 + (18 - s) / 2, y + 5 + (18 - s) / 2, s, s), &w.app_id, &w.title);
        }
        let t = if w.title.is_empty() { &w.app_id } else { &w.title };
        let room = (c.r[2] - c.r[0]) as i32 - 36;
        if room > 8 {
            let col = crate::render::fade(crate::shell::col::TEXT, (c.ph * 255.0) as u32);
            crate::font::draw_text_w(cv, clip.intersect(&xf.r(rect_of(c.r))), xf, x + 30, y + 19, t, 12.5, col, room, crate::font::W::Semi);
        }
    }
}

/// Advances it by `dt` seconds; true while it moves.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let Some(mut a) = d.overlay.alttab.take() else { return false };
    let before = bounds_of(d, &a);
    // Closed under it (Esc, a menu, its windows gone): it goes back.
    match &d.overlay.switcher {
        Some((list, sel)) if a.mode != Mode::Commit => {
            if a.list != *list || a.sel != *sel {
                a.list = list.clone();
                a.sel = *sel;
                aim(d, &mut a);
            }
        }
        None if matches!(a.mode, Mode::Opening | Mode::Open) => {
            a.mode = Mode::Cancel;
            a.k.to(0.0);
        }
        _ => {}
    }
    match a.mode {
        Mode::Opening => {
            a.o = (a.o + dt / OPEN).min(1.0);
            if a.o >= 1.0 {
                a.mode = Mode::Open;
            }
        }
        Mode::Cancel => a.o = (a.o - dt / OPEN).max(0.0),
        Mode::Commit => a.c = (a.c + dt / GO).min(1.0),
        Mode::Open => {}
    }
    if matches!(a.mode, Mode::Open) || (a.mode == Mode::Opening && a.o > 0.55) {
        a.k.to(1.0);
    }
    let mut moving = a.mode != Mode::Open;
    for s in a.lens.iter_mut() {
        moving |= s.step(dt);
    }
    moving |= a.k.step(dt);
    let done = (a.mode == Mode::Cancel && a.o <= 0.0) || (a.mode == Mode::Commit && a.c >= 1.0);
    let after = bounds_of(d, &a);
    for r in [before, after].into_iter().flatten() {
        d.damage(r);
    }
    if !done {
        d.overlay.alttab = Some(a);
    }
    moving && !done
}

fn bounds_of(d: &Desk, a: &At) -> Option<Rect> {
    (a.oi < d.outputs.len()).then(|| layout(d, a).0.inflate(60))
}
