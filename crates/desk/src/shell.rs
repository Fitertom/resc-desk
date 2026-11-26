//! Built-in shell, Windows 11 look: server-side title bars, shadows, taskbar
//! (centered, per monitor), start menu with search, Alt+Tab with live thumbnails,
//! snap preview. Everything is drawn by the compositor itself (no extra clients).
use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, fade, Canvas, Xf};
use crate::state::*;
use crate::wm::{self, Hit};

pub const TITLE_H: i32 = 32;
pub const BORDER: i32 = 1;
pub const SHADOW: i32 = 36;
/// How far a glass panel paints around itself (its shadow quad, glass.rs), logical px:
/// what to damage when one appears, moves or goes away.
pub const GLASS_REACH: i32 = 64;
/// Reserved strip at the bottom of every monitor; the glass islands float in it.
pub const TASKBAR_H: i32 = 64;
pub const BTN_W: i32 = 46;
const EDGE: i32 = 6;

/// Liquify palette: neutrals biased toward the violet accent.
pub mod col {
    use crate::render::argb;
    pub const TITLE_ACTIVE: u32 = 0xff22_1f2c;
    pub const TITLE_INACTIVE: u32 = 0xff1a_1920;
    pub const TEXT: u32 = 0xffff_ffff;
    /// white at 0.55 (premultiplied)
    pub const TEXT_DIM: u32 = argb(140, 255, 255, 255);
    pub const CLOSE_HOVER: u32 = 0xffe1_3b3b;
    /// The ambient color (look.rs: the wallpaper's own).
    pub fn accent() -> u32 {
        crate::look::accent()
    }
    /// text on the accent
    pub fn accent_ink() -> u32 {
        crate::look::accent_ink()
    }
}

// ------------------------------------------------------------------ background

/// One picture over the whole monitor plane (see wallpaper.rs).
pub fn draw_background(d: &Desk, oi: usize, _xf: Xf, cv: &mut Canvas, clip: Rect) {
    crate::wallpaper::draw(d, oi, cv, clip);
}

// ------------------------------------------------------------------ shadows & decorations

/// Corner radius of floating windows with our frame (maximized, snapped and fullscreen
/// ones are square, as in Windows 11).
pub const RADIUS: i32 = 12;

pub fn window_radius(d: &Desk, wk: WKey) -> i32 {
    let w = &d.windows[wk];
    if w.ssd && w.tile == Tile::Floating { RADIUS } else { 0 }
}

/// A soft shadow of the window's rounded shape, a little below it (stronger for the
/// active window).
pub fn draw_shadow(d: &Desk, wk: WKey, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(f) = wm::frame_rect(d, wk) else { return };
    let s = xf.scale as f32;
    let fr = xf.r(f);
    let rad = window_radius(d, wk) as f32 * s;
    if fr.inflate(-(rad.ceil() as i32) - 1).contains_rect(&clip) {
        return; // all under the window
    }
    let (dy, sigma, strength) = if d.windows[wk].activated { (6.0 * s, 10.0 * s, 0.45) } else { (3.0 * s, 7.0 * s, 0.28) };
    let reach = SHADOW as f32 * s - dy;
    let area = fr.inflate(xf.len(SHADOW)).intersect(&clip).intersect(&cv.bounds());
    let k2 = 1.0 / (2.0 * sigma * sigma);
    for y in area.y..area.y1() {
        let row = y as usize * cv.stride;
        let py = y as f32 + 0.5;
        for x in area.x..area.x1() {
            let px = x as f32 + 0.5;
            let inside = crate::render::rrect_sdf(px, py, fr, rad);
            if inside < -1.0 {
                continue;
            }
            let ss = crate::render::rrect_sdf(px, py - dy, fr, rad).max(0.0);
            if ss >= reach {
                continue;
            }
            let fall = 1.0 - ss / reach;
            let a = (-ss * ss * k2).exp() * strength * fall * fall * (1.0 - (0.5 - inside).clamp(0.0, 1.0));
            let a = (a * 255.0) as u32;
            if a > 0 {
                let p = &mut cv.px[row + x as usize];
                *p = crate::render::blend(*p, a << 24);
            }
        }
    }
}

fn title_buttons(f: Rect) -> [(Hit, Rect); 3] {
    let y = f.y + BORDER;
    let x = f.x1() - BORDER;
    [
        (Hit::Close, Rect::new(x - BTN_W, y, BTN_W, TITLE_H)),
        (Hit::Maximize, Rect::new(x - 2 * BTN_W, y, BTN_W, TITLE_H)),
        (Hit::Minimize, Rect::new(x - 3 * BTN_W, y, BTN_W, TITLE_H)),
    ]
}

pub fn deco_hit(f: Rect, tile: Tile, x: f64, y: f64) -> Option<Hit> {
    deco_hit_m(f, tile, x, y, false)
}

/// `merged`: the program draws its own title row (only the resize edges are ours).
pub fn deco_hit_m(f: Rect, tile: Tile, x: f64, y: f64, merged: bool) -> Option<Hit> {
    if merged {
        if tile == Tile::Floating {
            if let Some(e) = edge_hit(f, x, y) {
                if !f.contains(x, y) || y < (f.y + 3) as f64 || x < (f.x + 3) as f64 || x >= (f.x1() - 3) as f64 || y >= (f.y1() - 3) as f64 {
                    return Some(Hit::Edge(e));
                }
            }
        }
        return None;
    }
    if tile == Tile::Floating {
        if let Some(e) = edge_hit(f, x, y) {
            // Inside the frame only the outer few pixels resize; title buttons win.
            let inside = f.contains(x, y);
            let on_btn = title_buttons(f).iter().any(|(_, r)| r.contains(x, y));
            if !inside || (!on_btn && (y < (f.y + 3) as f64 || x < (f.x + 3) as f64 || x >= (f.x1() - 3) as f64)) {
                return Some(Hit::Edge(e));
            }
        }
    }
    if !f.contains(x, y) {
        return None;
    }
    for (h, r) in title_buttons(f) {
        if r.contains(x, y) {
            return Some(h);
        }
    }
    if y < (f.y + BORDER + TITLE_H) as f64 {
        return Some(Hit::Title);
    }
    None
}

/// Resize edge hit in an invisible band of EDGE px around `r` (plus the outer pixels).
pub fn edge_hit(r: Rect, x: f64, y: f64) -> Option<u32> {
    let outer = r.inflate(EDGE);
    if !outer.contains(x, y) {
        return None;
    }
    let corner = 16.0;
    let mut e = 0;
    if y < r.y as f64 + 3.0 {
        e |= 1;
    } else if y >= r.y1() as f64 - 3.0 {
        e |= 2;
    }
    if x < r.x as f64 + 3.0 {
        e |= 4;
    } else if x >= r.x1() as f64 - 3.0 {
        e |= 8;
    }
    // Widen corners along the edges.
    if e == 1 || e == 2 {
        if x < r.x as f64 + corner {
            e |= 4;
        } else if x >= r.x1() as f64 - corner {
            e |= 8;
        }
    } else if e == 4 || e == 8 {
        if y < r.y as f64 + corner {
            e |= 1;
        } else if y >= r.y1() as f64 - corner {
            e |= 2;
        }
    }
    if e == 0 { None } else { Some(e) }
}

/// Our window frame: one rounded body in the title colour (the client covers all but the
/// title bar and the corners), a hairline rim lighter at the top like Liquify's glass, the
/// app icon and title, and Windows 11 caption buttons with a rounded hover.
pub fn draw_deco(d: &Desk, wk: WKey, xf: Xf, cv: &mut Canvas, clip: Rect, glass: bool) {
    let w = &d.windows[wk];
    let Some(f) = wm::frame_rect(d, wk) else { return };
    let active = w.activated;
    let s = xf.scale as f32;
    let pf = xf.r(f);
    let prad = xf.len(window_radius(d, wk));
    if glass {
        // The GPU draws Liquify glass under the frame; a light veil keeps the inactive
        // window a step back.
        if !active {
            cv.rrect(clip, pf, prad, argb(40, 10, 10, 14));
        }
    } else {
        cv.rrect(clip, pf, prad, if active { col::TITLE_ACTIVE } else { col::TITLE_INACTIVE });
    }
    if w.merged() {
        let (top, bottom) = if active { (60, 22) } else { (30, 14) };
        cv.rrect_stroke(clip, pf, prad, s.max(1.0), 0xffffff, top, bottom);
        return;
    }
    // App icon + title.
    let title = Rect::new(f.x + BORDER, f.y + BORDER, f.w - 2 * BORDER, TITLE_H);
    let icon = title_icon(f);
    draw_app_tile(cv, clip, xf, icon, &w.app_id, &w.title);
    let text_col = if active { col::TEXT } else { col::TEXT_DIM };
    let max_w = title.w - 3 * BTN_W - 52;
    let t = if w.title.is_empty() { &w.app_id } else { &w.title };
    let t = ellipsize(t, 12.5, font::W::Semi, max_w);
    font::draw_text_w(cv, clip, xf, title.x + 38, title.y + 21, &t, 12.5, text_col, max_w, font::W::Semi);
    // Caption buttons.
    let hover = d.seat.hover.filter(|(k, _)| *k == wk).map(|(_, h)| h);
    let pressed = match d.seat.grab {
        crate::input::Grab::Deco { wk: g, hit } if g == wk => Some(hit),
        _ => None,
    };
    for (h, r) in title_buttons(f) {
        let hot = hover == Some(h) || pressed == Some(h);
        if hot {
            let c = match (h, pressed == Some(h)) {
                (Hit::Close, true) => argb(255, 196, 43, 28),
                (Hit::Close, false) => col::CLOSE_HOVER,
                (_, true) => argb(20, 255, 255, 255),
                _ => argb(30, 255, 255, 255),
            };
            let pr = xf.r(r);
            if h == Hit::Close && prad > 0 {
                // Square, but cut by the window's corner: the frame's rounded shape, kept to
                // the button.
                let cl = clip.intersect(&pr);
                cv.rrect(cl, pf, prad, c);
            } else {
                cv.fill(clip, pr, c);
            }
        }
        let glyph = if hot && h == Hit::Close { col::TEXT } else { text_col };
        let (cx, cy) = r.center();
        let lw = 1.1 * s.max(1.0);
        let (fx, fy) = (cx as f32, cy as f32);
        match h {
            Hit::Minimize => cv.line(clip, phys(xf, fx - 5.0, fy + 0.5), phys(xf, fx + 5.0, fy + 0.5), lw, glyph),
            Hit::Maximize => {
                if w.tile == Tile::Maximized {
                    cv.rrect_stroke(clip, xf.r(Rect::new(cx - 5, cy - 3, 8, 8)), xf.len(2), lw, 0xffffff, glyph >> 24, glyph >> 24);
                    cv.line(clip, phys(xf, fx - 2.5, fy - 5.5), phys(xf, fx + 3.5, fy - 5.5), lw, glyph);
                    cv.line(clip, phys(xf, fx + 5.5, fy - 3.5), phys(xf, fx + 5.5, fy + 2.5), lw, glyph);
                } else {
                    cv.rrect_stroke(clip, xf.r(Rect::new(cx - 5, cy - 5, 10, 10)), xf.len(2), lw, 0xffffff, glyph >> 24, glyph >> 24);
                }
            }
            Hit::Close => draw_x(cv, clip, xf, cx, cy, 5, glyph),
            _ => {}
        }
    }
    // The hairline rim last: the window's edge stays whole over a hovered button.
    let (top, bottom) = if active { (60, 22) } else { (30, 14) };
    cv.rrect_stroke(clip, pf, prad, s.max(1.0), 0xffffff, top, bottom);
}

pub fn draw_x(cv: &mut Canvas, clip: Rect, xf: Xf, cx: i32, cy: i32, r: i32, c: u32) {
    // Anti-aliased diagonal cross.
    let pc = xf.r(Rect::new(cx - r, cy - r, 2 * r + 1, 2 * r + 1));
    let n = pc.w.max(1) as f32;
    let cc = clip.intersect(&pc).intersect(&cv.bounds());
    for y in cc.y..cc.y1() {
        for x in cc.x..cc.x1() {
            let fx = (x - pc.x) as f32 + 0.5;
            let fy = (y - pc.y) as f32 + 0.5;
            let d1 = (fx - fy).abs() / std::f32::consts::SQRT_2;
            let d2 = (fx + fy - n).abs() / std::f32::consts::SQRT_2;
            let dist = d1.min(d2);
            let cov = (0.85 * xf.scale as f32 - dist + 0.5).clamp(0.0, 1.0);
            if cov > 0.0 {
                let p = &mut cv.px[y as usize * cv.stride + x as usize];
                *p = crate::render::blend(*p, fade(c, (cov * 255.0) as u32));
            }
        }
    }
}

fn hash_color(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    const PALETTE: [u32; 8] =
        [0xff7c_5cf0, 0xff3b_82f6, 0xffdb_4c94, 0xffe0_8e0b, 0xff10_a37a, 0xff14_a8a0, 0xffe8_6a1c, 0xff58_5ee8];
    PALETTE[(h % 8) as usize]
}

/// App icon from the icon theme; without one, a coloured rounded tile with the app's initial.
pub fn draw_app_tile(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, app_id: &str, title: &str) {
    if crate::icons::draw(cv, clip, xf, r, app_id) {
        return;
    }
    let key = if app_id.is_empty() { title } else { app_id };
    let rad = xf.len((r.w * 3 / 10).max(3));
    let base = hash_color(key);
    cv.rrect(clip, xf.r(r), rad, base);
    // Rim light gives the tile a little volume, like the glass tiles in Liquify.
    cv.rrect_stroke(clip, xf.r(r), rad, xf.scale.max(1.0) as f32, 0xffffff, 90, 10);
    let name = key.rsplit('.').next().unwrap_or(key);
    if let Some(ch) = name.chars().find(|c| c.is_alphanumeric()) {
        let s: String = ch.to_uppercase().collect();
        let size = r.h as f32 * 0.5;
        let tw = font::text_width_w(&s, size, font::W::Bold);
        font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + (r.h as f32 * 0.68) as i32, &s, size, col::TEXT, r.w, font::W::Bold);
    }
}

// ------------------------------------------------------------------ taskbar
//
// Liquify look: floating glass islands in a reserved strip at the bottom. Centre: start,
// task view (the virtual desktops live there, Win+Tab) and the windows (an accent drop
// under the focused one). Right: layout, sound and the clock, with their flyouts (tray.rs).

const ISLAND_H: i32 = 50;
const BTN: i32 = 42;
const CHIP: i32 = 32;

pub fn taskbar_rect(d: &Desk, oi: usize) -> Rect {
    let r = d.outputs[oi].rect();
    if !has_taskbar(d, oi) {
        // None: an empty strip under the bottom edge (nothing hits or draws it).
        return Rect::new(r.x, r.y1(), r.w, 0);
    }
    Rect::new(r.x, r.y1() - TASKBAR_H, r.w, TASKBAR_H)
}

/// Centre and right islands of output `oi`.
pub fn islands(d: &Desk, oi: usize) -> [Rect; 2] {
    let tb = taskbar_rect(d, oi);
    let y = tb.y + (TASKBAR_H - ISLAND_H) / 2;
    // The program buttons each take as much of their place as they have grown into
    // (life.rs): the island widens and narrows with them.
    let apps: f32 = crate::life::bar(d).iter().map(|(_, w, _)| *w).sum();
    let n = 1 + taskview_button(d) as i32;
    let cw = 4 * 2 + n * BTN + (n - 1) * 4 + (apps * (BTN + 4) as f32).round() as i32;
    let rw = crate::tray::ISLAND_W;
    [Rect::new(tb.x + (tb.w - cw) / 2, y, cw, ISLAND_H), Rect::new(tb.x1() - 10 - rw, y, rw, ISLAND_H)]
}

/// Task view's button shows once there is more than one desktop (Win+Tab always works).
fn taskview_button(d: &Desk) -> bool {
    d.workspaces > 1
}

/// The flyout the right island's item under (`x`, `y`) opens, if any.
pub fn tray_item_at(d: &Desk, x: f64, y: f64) -> Option<crate::tray::Fly> {
    let oi = d.output_at(x, y)?;
    taskbar_layout(d, oi).into_iter().find(|(_, r)| r.contains(x, y)).and_then(|(i, r)| match i {
        TbItem::Net => Some(crate::tray::Fly::Net),
        TbItem::Sound => Some(crate::tray::Fly::Sound),
        TbItem::Clock => Some(clock_fly(r, y)),
        _ => None,
    })
}

/// The clock's two lines open two flyouts: the time the timer, the date the calendar.
pub fn clock_fly(r: Rect, y: f64) -> crate::tray::Fly {
    if y < (r.y + 23) as f64 {
        crate::tray::Fly::Timer
    } else {
        crate::tray::Fly::Calendar
    }
}

pub fn taskbar_contains(d: &Desk, x: f64, y: f64) -> bool {
    (0..d.outputs.len()).any(|i| !fullscreen_on(d, i) && islands(d, i).iter().any(|r| r.contains(x, y)))
        || peek_contains(d, x, y)
        || crate::tray::contains(d, x, y)
        || (crate::taskview::is_open(d) && d.output_at(x, y) == d.overlay.taskview.as_ref().map(|t| t.oi))
}

/// No taskbar on output `oi` now: a fullscreen window covers it, or the taskbar is on the
/// main monitor only.
fn fullscreen_on(d: &Desk, oi: usize) -> bool {
    !has_taskbar(d, oi) || wm::visible_stack(d).iter().any(|k| d.windows[*k].tile == Tile::Fullscreen && d.windows[*k].fullscreen_output == Some(oi))
}

/// Output `oi` has a taskbar (Параметры → Мониторы: on every monitor, or the main one only).
pub fn has_taskbar(d: &Desk, oi: usize) -> bool {
    d.display_cfg.taskbar_all || oi == crate::monitors::primary_index(d)
}

/// Where what opens from the taskbar (Пуск, flyouts, Win+V, Win+P) goes for output `oi`:
/// there if it has a taskbar, else the main monitor.
pub fn bar_output(d: &Desk, oi: usize) -> usize {
    if has_taskbar(d, oi) {
        oi
    } else {
        crate::monitors::primary_index(d)
    }
}

pub fn damage_taskbars(d: &mut Desk) {
    for i in 0..d.outputs.len() {
        // Islands change width: their shadow above the strip goes too.
        let r = taskbar_rect(d, i);
        d.damage(Rect::new(r.x, r.y - GLASS_REACH, r.w, r.h + GLASS_REACH));
    }
}

/// Once a minute: only the clock island changes.
pub fn damage_clocks(d: &mut Desk) {
    for i in (0..d.outputs.len()).filter(|i| has_taskbar(d, *i)).collect::<Vec<_>>() {
        let r = islands(d, i)[1];
        d.damage(r.inflate(GLASS_REACH));
    }
}

/// Every glass panel on output `oi` (logical): the renderer widens damage around them.
pub fn glass_rects(d: &Desk, oi: usize) -> Vec<Rect> {
    glass_panels(d, oi).into_iter().map(|g| g.0).collect()
}

/// Every glass panel on output `oi` in drawing order: logical rect, corner radius, and the
/// GPU layer it lies under (1: the taskbar's content, 2: the overlays'). The draw functions
/// below draw the same panels; this list is what the GPU renderer draws instead.
pub fn glass_panels(d: &Desk, oi: usize) -> Vec<(Rect, i32, usize)> {
    let mut v = Vec::new();
    // No backing under the taskbar's buttons: glass only under the hovered one (the lens).
    if !fullscreen_on(d, oi) {
        if let Some(r) = lens_rect(d, oi) {
            v.push((r, lens_radius(r), 1));
        }
        // The drops of their own under buttons coming and going (life.rs).
        v.extend(crate::life::button_drops(d, oi).into_iter().map(|r| (r, lens_radius(r), 1)));
    }
    // A program loading under the icon it was started from: over «Пуск».
    v.extend(crate::life::origin_drops(d, oi).into_iter().map(|r| (r, lens_radius(r), 2)));
    if let Some(r) = crate::taskview::glass(d, oi) {
        v.push((r, 0, 2));
    }
    if d.overlay.start_menu == Some(oi) {
        v.push((crate::start::rect(d, oi), crate::start::RADIUS, 2));
    }
    v.extend(crate::tray::glass(d, oi).map(|r| (r, crate::tray::FLY_R, 2)));
    v.extend(crate::clipboard::glass_rect(d, oi).map(|r| (r, crate::clipboard::RADIUS, 2)));
    v.extend(crate::monitors::glass_rect(d, oi).map(|r| (r, 20, 2)));
    // (On the GPU path Alt+Tab is drops of its own, alttab.rs.)
    if let Some((list, _)) = d.overlay.switcher.as_ref().filter(|_| d.overlay.alttab.is_none()) {
        if d.output_at(d.seat.x, d.seat.y).unwrap_or(0) == oi {
            v.push((switcher_layout(d, oi, list.len()).0, SWITCHER_R, 2));
        }
    }
    // The preview card at rest, under the taskbar's layer like the hover drop it pops out of
    // (its thumbnail is the GPU's, drawn right over it); moving, it is a drop (winanim.rs).
    if let Some(l) = shown_peek(d).filter(|p| p.oi == oi && p.anim.is_none()).and_then(|p| peek_layout(d, &p)) {
        v.push((l.panel, PEEK_R, 1));
    }
    v
}

const SWITCHER_R: i32 = 22;

thread_local! {
    /// The GPU draws the glass: the CPU only draws what lies on it.
    static GLASS_ON_GPU: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Drawing the GPU path's CPU layers now (the moving drops' heads are drawn only there).
pub fn glass_on_gpu() -> bool {
    GLASS_ON_GPU.with(|g| g.get())
}

/// Runs `f` with glass left to the GPU (render.rs `render_layers`).
pub fn without_glass<T>(f: impl FnOnce() -> T) -> T {
    GLASS_ON_GPU.with(|g| g.set(true));
    let r = f();
    GLASS_ON_GPU.with(|g| g.set(false));
    r
}

/// Glass panel: Liquify's glass card (glass.rs), tinted like kit.rs `glass_deep`.
pub fn draw_glass(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, radius: i32) {
    if GLASS_ON_GPU.with(|g| g.get()) {
        return;
    }
    let s = xf.scale as f32;
    crate::glass::draw(cv, clip, xf.r(r), radius as f32 * s, s, crate::glass::TINT_DEEP);
}

/// Windows shown on the taskbar: current workspace, in the order their buttons came.
fn taskbar_windows(d: &Desk) -> Vec<WKey> {
    crate::life::bar(d)
        .into_iter()
        .filter_map(|(k, _, _)| match k {
            crate::life::Key::Win(wk) => Some(wk),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum TbItem {
    Start,
    TaskView,
    Win(WKey),
    /// A program loading; a closed window's button going (life.rs).
    Launch(u32),
    Ghost(u32),
    Lang,
    Net,
    Sound,
    Clock,
}

fn taskbar_layout(d: &Desk, oi: usize) -> Vec<(TbItem, Rect)> {
    let [c, r] = islands(d, oi);
    let mut v = Vec::new();
    let by = c.y + (ISLAND_H - BTN) / 2;
    let mut x = c.x + 4;
    v.push((TbItem::Start, Rect::new(x, by, BTN, BTN)));
    if taskview_button(d) {
        x += BTN + 4;
        v.push((TbItem::TaskView, Rect::new(x, by, BTN, BTN)));
    }
    // Program buttons: each its share of a place; the button itself stays full size, centred
    // in it (its icon is what grows and shrinks).
    let mut xe = (x + BTN) as f32;
    for (key, w, _) in crate::life::bar(d) {
        let cx = xe + 4.0 * w + BTN as f32 * w * 0.5;
        xe += (BTN + 4) as f32 * w;
        let item = match key {
            crate::life::Key::Win(wk) => TbItem::Win(wk),
            crate::life::Key::Launch(id) => TbItem::Launch(id),
            crate::life::Key::Ghost(id) => TbItem::Ghost(id),
        };
        v.push((item, Rect::new((cx - BTN as f32 * 0.5).round() as i32, by, BTN, BTN)));
    }
    let mut x = r.x + 4;
    v.push((TbItem::Lang, Rect::new(x, by, 36, BTN)));
    x += 36 + 2;
    v.push((TbItem::Net, Rect::new(x, by, 40, BTN)));
    x += 40 + 2;
    v.push((TbItem::Sound, Rect::new(x, by, 40, BTN)));
    x += 40 + 2;
    v.push((TbItem::Clock, Rect::new(x, by, r.x1() - 4 - x, BTN)));
    v
}

/// The app icon in the title bar of a window whose frame is `f` (logical).
pub fn title_icon(f: Rect) -> Rect {
    Rect::new(f.x + BORDER + 12, f.y + BORDER + 8, 16, 16)
}

/// The app icon inside a window's taskbar button `b` (logical).
pub fn button_icon(b: Rect) -> Rect {
    Rect::new(b.x + 6, b.y + 6, 30, 30)
}

/// The taskbar button of `key` (life.rs) on output `oi` (logical).
pub fn slot_button(d: &Desk, oi: usize, key: crate::life::Key) -> Option<Rect> {
    if fullscreen_on(d, oi) {
        return None;
    }
    let item = match key {
        crate::life::Key::Win(wk) => TbItem::Win(wk),
        crate::life::Key::Launch(id) => TbItem::Launch(id),
        crate::life::Key::Ghost(id) => TbItem::Ghost(id),
    };
    taskbar_layout(d, oi).into_iter().find(|(i, _)| *i == item).map(|(_, r)| r)
}

/// A program button's icon at `k` of its size (growing in, popping out).
fn scaled_icon(b: Rect, k: f32) -> Rect {
    let i = button_icon(b);
    let s = (i.w as f32 * k).round() as i32;
    Rect::new(i.x + (i.w - s) / 2, i.y + (i.h - s) / 2, s, s)
}

/// The start button on output `oi` (logical).
pub fn start_button(d: &Desk, oi: usize) -> Option<Rect> {
    taskbar_layout(d, oi).into_iter().find(|(i, _)| *i == TbItem::Start).map(|(_, r)| r)
}

/// Window `wk`'s taskbar button (logical): on the output the window stands on now, else the first.
pub fn button_rect(d: &Desk, wk: WKey) -> Option<Rect> {
    d.windows.get(wk)?;
    let own = wm::window_output(d, wk).min(d.outputs.len().saturating_sub(1));
    std::iter::once(own).chain(0..d.outputs.len()).filter(|oi| *oi < d.outputs.len() && !fullscreen_on(d, *oi)).find_map(|oi| {
        taskbar_layout(d, oi).into_iter().find(|(i, _)| *i == TbItem::Win(wk)).map(|(_, r)| r)
    })
}

pub fn phys(xf: Xf, x: f32, y: f32) -> (f32, f32) {
    (xf.fx(x as f64) as f32, xf.fy(y as f64) as f32)
}

pub fn draw_taskbar(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if !GLASS_ON_GPU.with(|g| g.get()) {
        let s = xf.scale as f32;
        for r in lens_rect(d, oi).into_iter().chain(crate::life::button_drops(d, oi)) {
            crate::glass::draw(cv, clip, xf.r(r), lens_radius(r) as f32 * s, s, crate::glass::TINT_LENS);
        }
    }
    let focused = wm::focused(d);
    let anim = d.overlay.tb_anim.get(oi).copied().unwrap_or_default();
    let lay = taskbar_layout(d, oi);
    let bar = crate::life::bar(d);
    let icon_k = |key: crate::life::Key| bar.iter().find(|(k, _, _)| *k == key).map_or(1.0, |(_, _, s)| *s);

    for (item, r) in lay.iter().copied() {
        match item {
            TbItem::TaskView => {
                if crate::taskview::is_open(d) {
                    cv.rrect(clip, xf.r(r), xf.len(12), argb(22, 255, 255, 255));
                }
                draw_taskview_icon(cv, clip, xf, r, d.workspaces);
            }
            TbItem::Lang | TbItem::Net | TbItem::Sound => {
                let open = match item {
                    TbItem::Sound => crate::tray::is_open(d, crate::tray::Fly::Sound),
                    TbItem::Net => crate::tray::is_open(d, crate::tray::Fly::Net),
                    _ => false,
                };
                if open {
                    cv.rrect(clip, xf.r(r), xf.len(12), argb(22, 255, 255, 255));
                }
                let (cx, cy) = r.center();
                if item == TbItem::Lang {
                    let lang = if crate::input::layout_group(d) == 1 { "RU" } else { "EN" };
                    let tw = font::text_width_w(lang, 12.0, font::W::Bold);
                    font::draw_text_w(cv, clip, xf, cx - tw / 2, cy + 5, lang, 12.0, col::TEXT, 40, font::W::Bold);
                } else if item == TbItem::Net {
                    crate::tray::draw_net_icon(d, cv, clip, xf, cx as f32, cy as f32, col::TEXT);
                } else {
                    // The speaker with its waves is lopsided: its middle, not its body, on
                    // the button's centre.
                    crate::tray::draw_speaker(cv, clip, xf, cx as f32 - 2.0, cy as f32, d.tray.audio.sink(), col::TEXT);
                }
            }
            TbItem::Start => {
                // On the GPU path the logo's squares are the menu's drops while it is shown
                // (startanim.rs): they have left the button.
                if GLASS_ON_GPU.with(|g| g.get()) {
                    if crate::startanim::shown(d) != Some(oi) {
                        draw_logo(cv, clip, xf, r);
                    }
                } else {
                    if d.overlay.start_menu == Some(oi) {
                        cv.rrect(clip, xf.r(r), xf.len(12), argb(22, 255, 255, 255));
                    }
                    draw_logo(cv, clip, xf, r);
                }
            }
            TbItem::Win(wk) => {
                let w = &d.windows[wk];
                let k = icon_k(crate::life::Key::Win(wk));
                let ir = scaled_icon(r, k);
                if ir.w > 1 {
                    draw_app_tile(cv, clip, xf, ir, &w.app_id, &w.title);
                }
                if focused != Some(wk) && k > 0.3 {
                    let dw = (6.0 * k.min(1.0)).round() as i32;
                    let dot = Rect::new(r.x + (r.w - dw) / 2, r.y1() - 3, dw, 3);
                    cv.rrect(clip, xf.r(dot), xf.len(1), argb(150, 255, 255, 255));
                }
            }
            TbItem::Launch(_) | TbItem::Ghost(_) => {
                let key = match item {
                    TbItem::Launch(id) => crate::life::Key::Launch(id),
                    TbItem::Ghost(id) => crate::life::Key::Ghost(id),
                    _ => unreachable!(),
                };
                let ir = scaled_icon(r, icon_k(key));
                if let Some((app, title)) = crate::life::slot_icon(d, key).filter(|_| ir.w > 1) {
                    draw_app_tile(cv, clip, xf, ir, app, title);
                }
            }
            TbItem::Clock => {
                // The half whose flyout is open (the time: the timer, the date: the calendar).
                for (f, part) in [(crate::tray::Fly::Timer, Rect::new(r.x, r.y, r.w, 23)), (crate::tray::Fly::Calendar, Rect::new(r.x, r.y + 23, r.w, r.h - 23))] {
                    if crate::tray::is_open(d, f) {
                        cv.rrect(clip, xf.r(part), xf.len(10), argb(22, 255, 255, 255));
                    }
                }
                let (hh, mm, day, mon, year) = crate::clock::local_now();
                let t = format!("{hh:02}:{mm:02}");
                // The timer counting down or what went off, else the date.
                let (dt, dc) = crate::cal::clock_line(d).unwrap_or_else(|| (format!("{day:02}.{mon:02}.{year}"), col::TEXT_DIM));
                let dt = ellipsize(&dt, 11.5, font::W::Text, r.w - 14);
                let tw = font::text_width_w(&t, 13.0, font::W::Bold);
                let dw = font::text_width(&dt, 11.5);
                font::draw_text_w(cv, clip, xf, r.x1() - tw - 10, r.y + 19, &t, 13.0, col::TEXT, r.w, font::W::Bold);
                font::draw_text(cv, clip, xf, r.x1() - dw - 10, r.y + 35, &dt, 11.5, dc, r.w);
            }
        }
    }
    // Accent drop under the focused window's button.
    if let Some((_, r)) = lay.iter().find(|(i, _)| matches!(i, TbItem::Win(k) if Some(*k) == focused)) {
        let cx = if anim.focus.init { anim.focus.x.round() as i32 } else { r.x + r.w / 2 };
        let bar = Rect::new(cx - 8, r.y1() - 3, 16, 3);
        cv.rrect(clip, xf.r(bar.inflate(2)), xf.len(3), crate::look::accent_a(60));
        cv.rrect(clip, xf.r(bar), xf.len(1), col::accent());
    }
}

/// Task view: a window card with the edge of another behind it; the desktops' count in a
/// small badge when there are several.
fn draw_taskview_icon(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, desktops: u32) {
    let (cx, cy) = r.center();
    let back = Rect::new(cx - 7, cy - 9, 16, 12);
    let front = Rect::new(cx - 10, cy - 5, 16, 13);
    let w = 1.6 * xf.scale as f32;
    cv.rrect_stroke(clip, xf.r(back), xf.len(3), w, 0xffffff, 130, 130);
    cv.rrect(clip, xf.r(front.inflate(1)), xf.len(4), argb(255, 30, 27, 40));
    cv.rrect_stroke(clip, xf.r(front), xf.len(3), w, 0xffffff, 255, 255);
    if desktops > 1 {
        let b = Rect::new(r.x1() - 15, r.y + 4, 13, 13);
        cv.rrect(clip, xf.r(b), xf.len(7), col::accent());
        let s = format!("{}", desktops.min(9));
        let tw = font::text_width_w(&s, 9.5, font::W::Bold);
        font::draw_text_w(cv, clip, xf, b.x + (b.w - tw) / 2 + 1, b.y + 10, &s, 9.5, col::accent_ink(), b.w, font::W::Bold);
    }
}

/// resc logo: four rounded squares from white to the accent.
fn draw_logo(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect) {
    let (cx, cy) = r.center();
    let s = 9;
    let cells = [(-s - 1, -s - 1, 0u32, 3), (1, -s - 1, 110, s / 2), (-s - 1, 1, 110, 3), (1, 1, 200, 3)];
    for (dx, dy, t, rad) in cells {
        let c = crate::render::lerp(0xfff4_f0ff, col::accent(), t);
        cv.rrect(clip, xf.r(Rect::new(cx + dx, cy + dy, s, s)), xf.len(rad), c);
    }
}

pub fn taskbar_click(d: &mut Desk, x: f64, y: f64, button: u32) -> bool {
    let Some(oi) = (0..d.outputs.len()).find(|i| !fullscreen_on(d, *i) && islands(d, *i).iter().any(|r| r.contains(x, y))) else {
        return false;
    };
    for (item, r) in taskbar_layout(d, oi) {
        if !r.contains(x, y) {
            continue;
        }
        // A click puts the preview away until the pointer leaves the button.
        if let Some(p) = d.overlay.peek.as_mut() {
            p.dismissed = true;
        }
        hide_peek(d);
        match item {
            TbItem::Start => {
                crate::taskview::close(d);
                toggle_start_on(d, oi)
            }
            TbItem::TaskView => crate::taskview::toggle(d),
            TbItem::Lang => crate::input::switch_layout(d),
            TbItem::Net => crate::tray::toggle(d, oi, crate::tray::Fly::Net),
            TbItem::Sound if button == crate::input::BTN_MIDDLE => crate::tray::toggle_mute(d, false),
            TbItem::Sound => crate::tray::toggle(d, oi, crate::tray::Fly::Sound),
            TbItem::Clock => crate::tray::toggle(d, oi, clock_fly(r, y)),
            TbItem::Launch(_) | TbItem::Ghost(_) => {}
            TbItem::Win(wk) => {
                close_menus(d);
                if button == crate::input::BTN_MIDDLE {
                    let c = d.windows[wk].app_id.clone();
                    if !c.is_empty() {
                        crate::life::launch(d, &c, &c, &[&c], None);
                    }
                } else {
                    wm::taskbar_activate(d, wk);
                }
            }
        }
        damage_taskbars(d);
        return true;
    }
    close_menus(d);
    true
}

pub fn taskbar_nth(d: &mut Desk, n: usize) {
    if let Some(wk) = taskbar_windows(d).get(n).copied() {
        wm::taskbar_activate(d, wk);
    }
}

// ------------------------------------------------------------------ window preview
//
// Hovering a window's taskbar button for a moment shows a glass card above it: the
// window's icon, title and close button over its live content. The card is a drop
// (winanim.rs): it pops out of the button and sinks back into it; a click on it spreads it
// into the window, which comes to the front. Once a card is up, sliding along the buttons
// moves it to the next one at once, like Windows.

const PEEK_DELAY_MS: u32 = 380;
const PEEK_TW: i32 = 240;
const PEEK_TH: i32 = 150;
const PEEK_HEAD: i32 = 34;
const PEEK_PAD: i32 = 8;
pub const PEEK_R: i32 = 18;

#[derive(Clone, Copy)]
pub struct Peek {
    pub wk: WKey,
    pub oi: usize,
    /// When the pointer came onto the button.
    pub since: u32,
    pub shown: bool,
    /// Clicked away: stays hidden until the pointer leaves the button.
    pub dismissed: bool,
    /// 0 nothing, 1 the card, 2 its close button.
    pub hot: u8,
    /// Popping up or sinking back (winanim.rs); None: at rest.
    pub anim: Option<crate::winanim::Anim>,
    /// Sinking back into the button: it goes when it gets there.
    pub leaving: bool,
}

struct PeekLayout {
    panel: Rect,
    thumb: Rect,
    close: Rect,
    /// Panel plus the gap down to the button: the pointer may cross it.
    zone: Rect,
    /// The window's taskbar button.
    btn: Rect,
}

/// The preview card as drawn now: a drop (winanim.rs).
pub struct Card {
    pub wk: WKey,
    pub oi: usize,
    pub pose: crate::winanim::Pose,
    pub moving: bool,
}

fn shown_peek(d: &Desk) -> Option<Peek> {
    d.overlay.peek.filter(|p| p.shown && d.windows.contains(p.wk))
}

/// The card the pointer can use: shown, not on its way back.
fn live_peek(d: &Desk) -> Option<Peek> {
    shown_peek(d).filter(|p| !p.leaving)
}

fn peek_layout(d: &Desk, p: &Peek) -> Option<PeekLayout> {
    if p.oi >= d.outputs.len() || fullscreen_on(d, p.oi) {
        return None;
    }
    let w = d.windows.get(p.wk)?;
    let s = d.surfaces.get(w.surf)?;
    let (_, btn) = taskbar_layout(d, p.oi).into_iter().find(|(i, _)| *i == TbItem::Win(p.wk))?;
    let g = s.geom();
    let (gw, gh) = if g.w > 0 && g.h > 0 { (g.w as f64, g.h as f64) } else { (16.0, 10.0) };
    let k = (PEEK_TW as f64 / gw).min(PEEK_TH as f64 / gh);
    let (tw, th) = (((gw * k) as i32).max(24), ((gh * k) as i32).max(24));
    let pw = tw.max(180) + 2 * PEEK_PAD;
    let ph = PEEK_HEAD + th + PEEK_PAD;
    let o = d.outputs[p.oi].rect();
    let island_top = taskbar_rect(d, p.oi).y + (TASKBAR_H - ISLAND_H) / 2;
    let x = (btn.x + btn.w / 2 - pw / 2).clamp(o.x + 8, (o.x1() - 8 - pw).max(o.x + 8));
    let panel = Rect::new(x, island_top - 10 - ph, pw, ph);
    // The content fills the card but for the header and the margins (a narrow window: wider
    // margins, never stretched).
    let thumb = Rect::new(panel.x + (pw - tw) / 2, panel.y + PEEK_HEAD, tw, th);
    // Its ✕ 18 px in from the card's edge, where the title bar's lands (winanim.rs `draw_head`).
    let close = Rect::new(panel.x1() - 32, panel.y + 5, 28, 24);
    let zone = Rect::from_edges(panel.x, panel.y, panel.x1(), btn.y);
    Some(PeekLayout { panel, thumb, close, zone, btn })
}

/// The card where it is now (at rest, popping up, sinking back).
pub fn card(d: &Desk) -> Option<Card> {
    let p = shown_peek(d)?;
    let l = peek_layout(d, &p)?;
    let rest = crate::winanim::card_look(l.panel, l.thumb);
    let (look, k) = match &p.anim {
        Some(a) if p.leaving => a.now(&crate::winanim::button_look(l.btn, true)),
        Some(a) => a.now(&rest),
        None => (rest, 0.0),
    };
    let content = crate::winanim::content_rect(d, p.wk)?;
    Some(Card { wk: p.wk, oi: p.oi, pose: crate::winanim::Pose { look, k, btn: l.btn, content }, moving: p.anim.is_some() })
}

fn peek_contains(d: &Desk, x: f64, y: f64) -> bool {
    live_peek(d).and_then(|p| peek_layout(d, &p)).map_or(false, |l| l.panel.contains(x, y))
}

fn damage_peek(d: &mut Desk) {
    let mut r = shown_peek(d).and_then(|p| peek_layout(d, &p)).map_or(Rect::default(), |l| l.panel.inflate(GLASS_REACH));
    if let Some(c) = card(d) {
        r = r.union(&c.pose.bounds().inflate(GLASS_REACH));
    }
    d.damage(r);
}

/// Starts the card moving (`kind`) from where it is now (`from`: its look and bridge).
fn peek_anim(d: &mut Desk, kind: crate::winanim::Kind, from: (crate::winanim::Look, f32)) {
    if !d.overlay.animating {
        d.overlay.anim_ms = d.now_ms();
    }
    if let Some(p) = d.overlay.peek.as_mut() {
        p.anim = Some(crate::winanim::Anim::new(kind, from.0, from.1));
        d.overlay.animating = true;
    }
}

/// Puts the card away: it sinks back into its button (it comes back after the delay, unless
/// it was dismissed).
fn hide_peek(d: &mut Desk) {
    let Some(p) = d.overlay.peek else { return };
    if p.leaving {
        return;
    }
    match card(d).filter(|_| d.gpu.is_some()) {
        Some(c) => {
            damage_peek(d);
            peek_anim(d, crate::winanim::Kind::Hide, (c.pose.look, c.pose.k));
            let p = d.overlay.peek.as_mut().unwrap();
            p.leaving = true;
            p.hot = 0;
        }
        None => drop_peek(d),
    }
}

/// Puts the card away at once.
fn drop_peek(d: &mut Desk) {
    damage_peek(d);
    match d.overlay.peek.as_mut() {
        Some(p) if p.dismissed => {
            p.shown = false;
            p.hot = 0;
            p.anim = None;
            p.leaving = false;
        }
        _ => d.overlay.peek = None,
    }
}

fn peek_hover(d: &mut Desk, x: f64, y: f64, h: Option<(usize, usize)>) {
    let busy = d.overlay.start_menu.is_some()
        || d.overlay.switcher.is_some()
        || d.overlay.taskview.is_some()
        || d.tray.fly.is_some()
        || !matches!(d.seat.grab, crate::input::Grab::None);
    if busy {
        if d.overlay.peek.is_some() {
            d.overlay.peek.as_mut().unwrap().dismissed = false;
            hide_peek(d);
        }
        return;
    }
    // On the card (or on the way up to it): it stays.
    if let Some(p) = live_peek(d) {
        if let Some(l) = peek_layout(d, &p) {
            if l.zone.contains(x, y) {
                let hot = if l.close.contains(x, y) { 2 } else if l.panel.contains(x, y) { 1 } else { 0 };
                if hot != p.hot {
                    d.overlay.peek.as_mut().unwrap().hot = hot;
                    d.damage(l.panel);
                }
                return;
            }
        }
    }
    let on = h.and_then(|(oi, i)| match taskbar_layout(d, oi).get(i) {
        Some((TbItem::Win(wk), _)) => Some((oi, *wk)),
        _ => None,
    });
    match (on, d.overlay.peek) {
        (Some((oi, wk)), Some(p)) if p.wk == wk && p.oi == oi => {
            // Back on the button while the card sinks: it comes up again from where it is.
            if p.leaving && !p.dismissed {
                if let Some(c) = card(d) {
                    peek_anim(d, crate::winanim::Kind::Pop, (c.pose.look, c.pose.k));
                    d.overlay.peek.as_mut().unwrap().leaving = false;
                }
            }
        }
        (Some((oi, wk)), old) => {
            // A card already up moves over to this button's window at once.
            let from = old.filter(|p| p.shown).and_then(|_| card(d)).map(|c| c.pose.look);
            damage_peek(d);
            d.overlay.peek = Some(Peek { wk, oi, since: d.now_ms(), shown: from.is_some(), dismissed: false, hot: 0, anim: None, leaving: false });
            if let Some(f) = from.filter(|_| d.gpu.is_some()) {
                peek_anim(d, crate::winanim::Kind::Pop, (f, 0.0));
            }
            damage_peek(d);
        }
        (None, Some(_)) => {
            d.overlay.peek.as_mut().unwrap().dismissed = false;
            hide_peek(d);
        }
        (None, None) => {}
    }
}

/// The delay before the card shows, then its way up or down; true while either runs.
fn peek_tick(d: &mut Desk, dt: f32) -> bool {
    let Some(p) = d.overlay.peek else { return false };
    if !d.windows.contains(p.wk) {
        d.overlay.peek = None;
        d.damage_all();
        return false;
    }
    if !p.shown {
        if p.dismissed {
            return false;
        }
        if d.now_ms().wrapping_sub(p.since) < PEEK_DELAY_MS {
            return true;
        }
        d.overlay.peek.as_mut().unwrap().shown = true;
        // Out of the button's icon (the CPU path: there at once).
        let from = peek_layout(d, &p).map(|l| crate::winanim::button_look(l.btn, true)).filter(|_| d.gpu.is_some());
        if let Some(f) = from {
            peek_anim(d, crate::winanim::Kind::Pop, (f, 0.0));
        }
        damage_peek(d);
        return from.is_some();
    }
    if p.anim.is_none() {
        return false;
    }
    damage_peek(d);
    let pk = d.overlay.peek.as_mut().unwrap();
    let a = pk.anim.as_mut().unwrap();
    a.t += dt;
    let done = a.done();
    if !done {
        damage_peek(d);
        return true;
    }
    pk.anim = None;
    if pk.leaving {
        // Back in the button: gone (or hidden until the pointer leaves, when dismissed).
        pk.leaving = false;
        if pk.dismissed {
            pk.shown = false;
            pk.hot = 0;
        } else {
            d.overlay.peek = None;
        }
    }
    damage_peek(d);
    false
}

/// A surface got new content: refresh the card's thumbnail if it shows that window.
pub fn peek_commit(d: &mut Desk, sk: SKey) {
    if let Some(wk) = d.window_of(sk) {
        crate::taskview::commit(d, wk);
    }
    let Some(p) = shown_peek(d) else { return };
    if d.windows[p.wk].surf != sk {
        return;
    }
    if let Some(l) = peek_layout(d, &p) {
        d.damage(l.thumb);
    }
}

fn peek_click(d: &mut Desk, x: f64, y: f64, button: u32) -> bool {
    let Some(p) = live_peek(d) else { return false };
    let Some(l) = peek_layout(d, &p) else { return false };
    if !l.panel.contains(x, y) {
        return false;
    }
    let look = card(d).map(|c| c.pose.look);
    d.overlay.peek.as_mut().unwrap().dismissed = true;
    if l.close.contains(x, y) || button == crate::input::BTN_MIDDLE {
        hide_peek(d);
        wm::close(d, p.wk);
    } else {
        // The card becomes the window: gone from here at once, the window spreads out of
        // where it was.
        drop_peek(d);
        if d.windows[p.wk].workspace != d.workspace {
            wm::switch_workspace(d, d.windows[p.wk].workspace);
        }
        wm::unminimize(d, p.wk);
        if let Some(look) = look {
            crate::winanim::expand(d, p.wk, look);
        }
    }
    damage_taskbars(d);
    true
}

/// Live thumbnail of a window straight from its client buffer (scaled blit), fitted
/// and centred in `area`.
pub fn draw_thumb(d: &Desk, wk: WKey, area: Rect, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(w) = d.windows.get(wk) else { return };
    let s = &d.surfaces[w.surf];
    let Some((b, _)) = &s.buffer else { return };
    let g = s.geom();
    let sc = s.scale.max(1);
    let src = Rect::new(g.x * sc, g.y * sc, g.w * sc, g.h * sc).intersect(&Rect::new(0, 0, b.w, b.h));
    if src.is_empty() {
        return;
    }
    let k = (area.w as f64 / src.w as f64).min(area.h as f64 / src.h as f64);
    let (tw, th) = ((src.w as f64 * k) as i32, (src.h as f64 * k) as i32);
    let dst = Rect::new(area.x + (area.w - tw) / 2, area.y + (area.h - th) / 2, tw, th);
    let pd = xf.r(dst);
    let opaque = s.is_opaque();
    b.with_pixels(|px, stride| cv.blit(clip, pd, px, stride, src, opaque, 255));
}

/// The card's header (icon, title, ✕) where the card is now. Its glass and content are the
/// GPU's; the CPU path draws the card at rest, glass and thumbnail too.
fn draw_peek(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(p) = shown_peek(d).filter(|p| p.oi == oi) else { return };
    let (Some(l), Some(c)) = (peek_layout(d, &p), card(d)) else { return };
    let w = &d.windows[p.wk];
    if !glass_on_gpu() {
        draw_glass(cv, clip, xf, l.panel, PEEK_R);
        // Backdrop under the thumbnail (shows through transparent windows).
        cv.rrect(clip, xf.r(l.thumb.inflate(1)), xf.len(9), argb(40, 0, 0, 0));
        if w.minimized {
            cv.rrect(clip, xf.r(l.thumb), xf.len(9), argb(14, 255, 255, 255));
        }
        draw_thumb(d, p.wk, l.thumb, xf, cv, clip);
    }
    let rest = !c.moving;
    if rest && p.hot != 0 {
        cv.rrect(clip, xf.r(l.panel), xf.len(PEEK_R), argb(16, 255, 255, 255));
    }
    let room = l.close.x - (l.panel.x + 36);
    crate::winanim::draw_head(cv, clip, xf, &c.pose, &w.app_id, &w.title, room, true, rest && p.hot == 2);
}

/// Hover highlight on taskbar items / menu entries.
pub fn pointer_hover(d: &mut Desk) {
    let (x, y) = (d.seat.x, d.seat.y);
    let mut h = None;
    for oi in 0..d.outputs.len() {
        if taskbar_rect(d, oi).contains(x, y) {
            for (i, (_, r)) in taskbar_layout(d, oi).into_iter().enumerate() {
                if r.contains(x, y) {
                    h = Some((oi, i));
                }
            }
        }
    }
    if h != d.overlay_hover {
        d.overlay_hover = h;
        damage_taskbars(d);
    }
    peek_hover(d, x, y, h);
    crate::tray::hover(d, x, y);
    crate::taskview::hover(d, x, y);
    crate::clipboard::hover(d, x, y);
    crate::monitors::hover(d, x, y);
    crate::start::hover(d, x, y);
}

// ------------------------------------------------------------------ animation

/// Spring with a little overshoot, like Liquify's drops.
#[derive(Clone, Copy, Default)]
pub struct Spring {
    pub x: f64,
    pub v: f64,
    pub init: bool,
}

impl Spring {
    /// Returns true while still moving.
    fn step(&mut self, target: f64, dt: f64) -> bool {
        if !self.init {
            *self = Spring { x: target, v: 0.0, init: true };
            return false;
        }
        let mut t = dt;
        while t > 0.0 {
            let h = t.min(0.004);
            let a = 520.0 * (target - self.x) - 36.0 * self.v;
            self.v += a * h;
            self.x += self.v * h;
            t -= h;
        }
        if (target - self.x).abs() < 0.25 && self.v.abs() < 4.0 {
            self.x = target;
            self.v = 0.0;
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Default)]
pub struct TbAnim {
    pub focus: Spring,
    pub lens: Lens,
}

/// The glass drop under the hovered taskbar button: Liquify's `Drop` in pixels. Its edges
/// are springs; the one in front moves first and quicker, the one behind follows, so it
/// stretches toward the next button and pulls together there. `k`: its size, popping up
/// from nothing where the pointer comes in and shrinking away when it leaves.
#[derive(Clone, Copy, Default)]
pub struct Lens {
    l: crate::anim::Spring,
    r: crate::anim::Spring,
    k: crate::anim::Spring,
    y: i32,
    h: i32,
}

const LENS_LEAD: (f32, f32) = (0.26, 0.68);
const LENS_TRAIL: (f32, f32) = (0.40, 0.80);
const LENS_POP: (f32, f32) = (0.30, 0.62);

/// The drop on output `oi` now (logical), if it shows.
pub fn lens_rect(d: &Desk, oi: usize) -> Option<Rect> {
    let l = d.overlay.tb_anim.get(oi)?.lens;
    let k = l.k.value;
    if k <= 0.04 || l.h <= 0 {
        return None;
    }
    let (x0, x1) = (l.l.value.min(l.r.value), l.l.value.max(l.r.value));
    // Stretched between two buttons, it thins a little, like a drop pulled along.
    let stretch = ((x1 - x0) / BTN as f32 - 1.0).clamp(0.0, 1.5);
    let w = (x1 - x0) * k;
    let h = l.h as f32 * k * (1.0 - 0.12 * stretch);
    let (cx, cy) = ((x0 + x1) * 0.5, l.y as f32 + l.h as f32 * 0.5);
    let r = Rect::from_edges((cx - w * 0.5).round() as i32, (cy - h * 0.5).round() as i32, (cx + w * 0.5).round() as i32, (cy + h * 0.5).round() as i32);
    (!r.is_empty()).then_some(r)
}

pub fn lens_radius(r: Rect) -> i32 {
    (r.h / 2).min(13).max(1)
}

/// Moves the drops toward the hovered buttons; true while any moves.
fn lens_tick(d: &mut Desk, dt: f32) -> bool {
    let mut moving = false;
    for oi in 0..d.outputs.len() {
        let target = match d.overlay_hover {
            Some((o, i)) if o == oi && !fullscreen_on(d, oi) => taskbar_layout(d, oi).get(i).map(|x| x.1),
            _ => None,
        };
        let before = lens_rect(d, oi);
        let l = &mut d.overlay.tb_anim[oi].lens;
        (l.k.response, l.k.damping) = LENS_POP;
        match target {
            Some(r) => {
                let (x0, x1) = (r.x as f32, r.x1() as f32);
                if l.k.value < 0.05 {
                    // Coming in: it pops up right there.
                    l.l.snap(x0);
                    l.r.snap(x1);
                } else if l.l.target != x0 || l.r.target != x1 {
                    let ahead = x0 > l.l.target;
                    let (front, back) = if ahead { (&mut l.r, &mut l.l) } else { (&mut l.l, &mut l.r) };
                    (front.response, front.damping) = LENS_LEAD;
                    (back.response, back.damping) = LENS_TRAIL;
                    l.l.to(x0);
                    l.r.to(x1);
                }
                l.y = r.y;
                l.h = r.h;
                l.k.to(1.0);
            }
            None => l.k.to(0.0),
        }
        let m = l.l.step(dt) | l.r.step(dt) | l.k.step(dt);
        if m || before != lens_rect(d, oi) {
            damage_taskbars(d);
        }
        moving |= m;
    }
    moving
}

/// Advances the shell's springs; true while something still moves.
pub fn tick(d: &mut Desk) -> bool {
    let now = d.now_ms();
    let dt = (now.wrapping_sub(d.overlay.anim_ms) as f64 / 1000.0).clamp(0.0, 0.05);
    let dt = if d.overlay.slowmo > 0.0 { dt * d.overlay.slowmo as f64 } else { dt };
    d.overlay.anim_ms = now;
    if d.overlay.tb_anim.len() != d.outputs.len() {
        d.overlay.tb_anim.resize(d.outputs.len(), TbAnim::default());
    }
    let focused = wm::focused(d);
    let mut moving = false;
    for oi in 0..d.outputs.len() {
        let lay = taskbar_layout(d, oi);
        let ft = lay.iter().find(|(i, _)| matches!(i, TbItem::Win(k) if Some(*k) == focused)).map(|(_, r)| (r.x + r.w / 2) as f64);
        let a = &mut d.overlay.tb_anim[oi];
        let mut m = false;
        match ft {
            Some(t) => m |= a.focus.step(t, dt),
            None => a.focus.init = false,
        }
        if m {
            let [c, _] = islands(d, oi);
            d.damage(c);
            moving = true;
        }
    }
    moving |= peek_tick(d, dt as f32);
    moving |= crate::winanim::tick(d, dt as f32);
    moving |= crate::life::tick(d, dt as f32);
    moving |= crate::alttab::tick(d, dt as f32);
    moving |= crate::startanim::tick(d, dt as f32);
    moving |= crate::boot::tick(d, dt as f32);
    moving |= crate::osd::tick(d, dt as f32);
    moving |= lens_tick(d, dt as f32);
    moving
}

// ------------------------------------------------------------------ start menu

pub fn toggle_start(d: &mut Desk) {
    let oi = bar_output(d, d.output_at(d.seat.x, d.seat.y).unwrap_or(0));
    toggle_start_on(d, oi);
}

fn toggle_start_on(d: &mut Desk, oi: usize) {
    if let Some(old) = d.overlay.start_menu.take() {
        let r = crate::start::rect(d, old);
        d.damage(r.inflate(GLASS_REACH));
        damage_taskbars(d);
        crate::startanim::begin(d, old, false);
        return;
    }
    d.overlay.switcher = None;
    crate::tray::close(d);
    if d.apps.is_empty() {
        d.apps = crate::apps::load();
    }
    d.start_field.clear();
    d.start_sel = 0;
    d.start_hover = None;
    crate::start::opened(d);
    d.overlay.start_menu = Some(oi);
    let r = crate::start::rect(d, oi);
    d.damage(r.inflate(GLASS_REACH));
    damage_taskbars(d);
    crate::startanim::begin(d, oi, true);
}

pub fn damage_start(d: &mut Desk) {
    if let Some(oi) = d.overlay.start_menu {
        let r = crate::start::rect(d, oi);
        d.damage(r);
    }
}

/// «Параметры»: the open window comes to the front, or the app starts.
pub fn open_settings(d: &mut Desk) {
    open_settings_page(d, "");
}

/// «Параметры» on a page (look, displays, sound, …; "" its first).
pub fn open_settings_page(d: &mut Desk, page: &str) {
    close_menus(d);
    crate::taskview::close(d);
    let found = d.windows.iter().find(|(_, w)| w.app_id == "resc.settings").map(|(k, _)| k);
    if let Some(wk) = found {
        if d.windows[wk].workspace != d.workspace {
            wm::switch_workspace(d, d.windows[wk].workspace);
        }
        wm::unminimize(d, wk);
        return;
    }
    // (After an update the running file is "<path> (deleted)": the new one is at <path>.)
    let exe = crate::update::exe_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "resc-desk".into());
    d.spawn(&format!("'{}' settings {page}", exe.replace('\'', r"'\''")));
}

/// «Проводник» (Win+E, «Пуск»): our own file manager unless RESC_FILES names another.
pub fn open_files(d: &mut Desk, origin: Option<Rect>) {
    close_menus(d);
    crate::taskview::close(d);
    let c = d.cfg.files.clone();
    crate::life::launch(d, &c, "resc.files", &["resc.files"], origin);
}

pub fn close_menus(d: &mut Desk) {
    if d.overlay.start_menu.is_some() {
        toggle_start_on(d, 0);
    }
}

pub fn ellipsize(s: &str, size: f32, w: font::W, max: i32) -> String {
    let mut name = s.to_string();
    while font::text_width_w(&name, size, w) > max && name.chars().count() > 3 {
        name = name.trim_end_matches('…').to_string();
        name.pop();
        name.push('…');
    }
    name
}

/// Keyboard input while a shell menu is open. Returns true if consumed.
pub fn menu_key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    if d.calib.is_some() {
        if pressed {
            crate::layout::calib_key(d, code);
        }
        return true;
    }
    if crate::clipboard::key(d, code, pressed) {
        return true;
    }
    if crate::monitors::key(d, code, pressed) {
        return true;
    }
    if crate::tray::wants_keys(d) && crate::tray::key(d, code, pressed) {
        return true;
    }
    if code == crate::input::key::ESC && (d.overlay.taskview.is_some() || d.tray.fly.is_some()) && d.overlay.start_menu.is_none() {
        if pressed {
            d.seat.swallowed.push(code);
            crate::taskview::close(d);
            crate::tray::close(d);
        }
        return true;
    }
    let Some(oi) = d.overlay.start_menu else { return false };
    if !pressed {
        return true;
    }
    if code == crate::input::key::ESC {
        // Its release must not reach the window under the menu either.
        d.seat.swallowed.push(code);
        if d.start.power {
            d.start.power = false;
            damage_start(d);
        } else if d.start.all && d.start_field.is_empty() {
            d.start.all = false;
            d.start_sel = 0;
            damage_start(d);
        } else {
            close_menus(d);
        }
        return true;
    }
    crate::start::key(d, oi, code);
    true
}

// ------------------------------------------------------------------ Alt+Tab

pub fn switcher_step(d: &mut Desk, dir: i32) {
    close_menus(d);
    let (list, idx) = match d.overlay.switcher.take() {
        Some((l, i)) => (l, i as i32 + dir),
        None => {
            let l: Vec<WKey> = d
                .mru
                .iter()
                .copied()
                .filter(|k| d.windows[*k].workspace == d.workspace && !d.windows[*k].desktop() && d.surfaces.get(d.windows[*k].surf).map_or(false, |s| s.mapped))
                .collect();
            (l, dir)
        }
    };
    if list.is_empty() {
        return;
    }
    let n = list.len() as i32;
    let idx = ((idx % n) + n) % n;
    d.overlay.switcher = Some((list, idx as usize));
    d.damage_all();
    crate::alttab::stepped(d);
}

pub fn switcher_commit(d: &mut Desk) {
    if let Some((list, idx)) = d.overlay.switcher.take() {
        let wk = list.get(idx).copied().filter(|k| d.windows.contains(*k));
        if let Some(wk) = wk {
            wm::unminimize(d, wk);
        }
        // Its card spreads into it (alttab.rs).
        crate::alttab::commit(d, wk);
        d.damage_all();
    }
}

/// A switcher card's size (logical).
pub const SWITCHER_TW: i32 = 232;
pub const SWITCHER_TH: i32 = 168;

pub fn switcher_layout(d: &Desk, oi: usize, n: usize) -> (Rect, Vec<Rect>) {
    let o = d.outputs[oi].rect();
    let (tw, th) = (SWITCHER_TW, SWITCHER_TH);
    let per_row = ((o.w - 80) / (tw + 12)).max(1) as usize;
    let cols = n.min(per_row) as i32;
    let rows = ((n + per_row - 1) / per_row) as i32;
    let pw = cols * (tw + 12) + 30;
    let ph = rows * (th + 12) + 30;
    let panel = Rect::new(o.x + (o.w - pw) / 2, o.y + (o.h - ph) / 2, pw, ph);
    let cells = (0..n)
        .map(|i| {
            let (cx, cy) = ((i % per_row) as i32, (i / per_row) as i32);
            Rect::new(panel.x + 20 + cx * (tw + 12), panel.y + 20 + cy * (th + 12), tw, th)
        })
        .collect();
    (panel, cells)
}

fn draw_switcher(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some((list, sel)) = &d.overlay.switcher else { return };
    let (panel, cells) = switcher_layout(d, oi, list.len());
    draw_glass(cv, clip, xf, panel, SWITCHER_R);
    for (i, (wk, cell)) in list.iter().zip(cells).enumerate() {
        let Some(w) = d.windows.get(*wk) else { continue };
        let tile = cell.inflate(3);
        cv.rrect(clip, xf.r(tile), xf.len(14), if i == *sel { argb(30, 255, 255, 255) } else { argb(12, 255, 255, 255) });
        if i == *sel {
            cv.rrect_stroke(clip, xf.r(tile), xf.len(14), 2.0 * xf.scale as f32, crate::look::accent() & 0xff_ffff, 255, 255);
        }
        let t = if w.title.is_empty() { &w.app_id } else { &w.title };
        draw_app_tile(cv, clip, xf, Rect::new(cell.x + 6, cell.y + 5, 18, 18), &w.app_id, &w.title);
        font::draw_text_w(cv, clip, xf, cell.x + 30, cell.y + 19, t, 12.5, col::TEXT, cell.w - 36, font::W::Semi);
        draw_thumb(d, *wk, Rect::new(cell.x + 6, cell.y + 30, cell.w - 12, cell.h - 36), xf, cv, clip);
    }
}

// ------------------------------------------------------------------ overlays

// ------------------------------------------------------------------ monitor calibration

/// Rulers and a millimetre grid in PLANE coordinates: when the layout is right,
/// every line runs on straight across the bezels between monitors.
fn draw_calibration(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(cal) = &d.calib else { return };
    let o = &d.outputs[oi];
    let r = o.rect();
    let k = d.mm_k;
    let (mx, my) = (d.mm_origin.0 + o.shift_mm, d.mm_origin.1);
    cv.fill(clip, xf.r(r), argb(170, 6, 10, 18));
    // Plane mm → physical pixel on this output.
    let px_x = |mm: f64| xf.fx((mm - mx) * k).round() as i32;
    let px_y = |mm: f64| xf.fy((mm - my) * k).round() as i32;
    let pr = xf.r(r);
    let mm_x0 = (o.plane[0]).floor() as i64;
    let mm_x1 = (o.plane[0] + o.plane[2]).ceil() as i64;
    let mm_y0 = (o.plane[1]).floor() as i64;
    let mm_y1 = (o.plane[1] + o.plane[3]).ceil() as i64;
    let line = |m: i64| -> Option<u32> {
        if m % 100 == 0 {
            Some(crate::look::accent_a(230))
        } else if m % 50 == 0 {
            Some(crate::look::accent_a(150))
        } else if m % 10 == 0 {
            Some(argb(45, 255, 255, 255))
        } else {
            None
        }
    };
    // Grid.
    for m in mm_x0..=mm_x1 {
        if let Some(c) = line(m) {
            let x = px_x(m as f64);
            cv.fill(clip, Rect::new(x, pr.y, if m % 100 == 0 { 2 } else { 1 }, pr.h), c);
        }
    }
    for m in mm_y0..=mm_y1 {
        if let Some(c) = line(m) {
            let y = px_y(m as f64);
            cv.fill(clip, Rect::new(pr.x, y, pr.w, if m % 100 == 0 { 2 } else { 1 }), c);
        }
    }
    // Rulers along all four edges, 1 mm ticks.
    let tick = |m: i64| -> i32 {
        if m % 10 == 0 {
            26
        } else if m % 5 == 0 {
            15
        } else {
            8
        }
    };
    for m in mm_x0..=mm_x1 {
        let x = px_x(m as f64);
        let t = tick(m);
        cv.fill(clip, Rect::new(x, pr.y, 1, t), col::TEXT);
        cv.fill(clip, Rect::new(x, pr.y1() - t, 1, t), col::TEXT);
        if m % 10 == 0 {
            let lx = (m as f64 - mx) * k + 3.0;
            font::draw_text(cv, clip, xf, lx as i32, r.y + 38, &format!("{}", m / 10), 10.0, col::TEXT, 40);
        }
    }
    for m in mm_y0..=mm_y1 {
        let y = px_y(m as f64);
        let t = tick(m);
        cv.fill(clip, Rect::new(pr.x, y, t, 1), col::TEXT);
        cv.fill(clip, Rect::new(pr.x1() - t, y, t, 1), col::TEXT);
        if m % 10 == 0 {
            let ly = (m as f64 - my) * k + 12.0;
            font::draw_text(cv, clip, xf, r.x + 34, ly as i32, &format!("{}", m / 10), 10.0, col::TEXT, 40);
            font::draw_text(cv, clip, xf, r.x1() - 52, ly as i32, &format!("{}", m / 10), 10.0, col::TEXT, 40);
        }
    }
    // A bank card (ISO/IEC 7810 ID-1: 85.60 × 53.98 mm) to check the size with.
    let ccx = o.plane[0] + o.plane[2] / 2.0;
    let ccy = o.plane[1] + o.plane[3] / 2.0 + 40.0;
    let card = Rect::from_edges(px_x(ccx - 42.8), px_y(ccy - 26.99), px_x(ccx + 42.8), px_y(ccy + 26.99));
    let t = xf.len(2).max(2);
    for e in [
        Rect::new(card.x, card.y, card.w, t),
        Rect::new(card.x, card.y1() - t, card.w, t),
        Rect::new(card.x, card.y, t, card.h),
        Rect::new(card.x1() - t, card.y, t, card.h),
    ] {
        cv.fill(clip, e, col::accent());
    }
    let (clx, cly) = ((ccx - 40.0 - mx) * k, (ccy + 2.0 - my) * k);
    font::draw_text(cv, clip, xf, clx as i32, cly as i32, "банковская карта 85.6 × 54 мм", 11.0, col::accent(), 400);
    // Info + help.
    let sel = cal.sel == oi;
    if sel {
        let b = xf.len(4).max(3);
        for e in [Rect::new(pr.x, pr.y, pr.w, b), Rect::new(pr.x, pr.y1() - b, pr.w, b), Rect::new(pr.x, pr.y, b, pr.h), Rect::new(pr.x1() - b, pr.y, b, pr.h)] {
            cv.fill(clip, e, col::accent());
        }
    }
    let lines = [
        format!("{}{}", o.name, if sel { "  — выбран" } else { "" }),
        format!("{}×{} px   {:.1} × {:.1} мм   {:.2} px/мм   масштаб {:.3}", o.pw, o.ph, o.plane[2], o.plane[3], o.pw as f64 / o.plane[2], o.scale),
        format!("позиция {:.1}, {:.1} мм", o.plane[0], o.plane[1]),
        "Tab — следующий монитор   стрелки — сдвиг 0.5 мм (Shift — 5 мм)".into(),
        format!("+ / − — размер (по карте)   G — склеить: {}   L — из LittleBigMouse", if d.display_cfg.glue { "вкл" } else { "выкл" }),
        "Enter — сохранить   Esc — отменить".into(),
        cal.note.clone(),
    ];
    let (bx, by) = (r.x + r.w / 2 - 260, r.y + 70);
    let panel = Rect::new(bx - 16, by - 22, 552, 22 * lines.len() as i32 + 16);
    cv.rrect(clip, xf.r(panel), xf.len(8), argb(220, 28, 28, 28));
    for (i, l) in lines.iter().enumerate() {
        let c = if i == 0 { col::TEXT } else if i >= 3 { col::TEXT_DIM } else { col::TEXT };
        font::draw_text(cv, clip, xf, bx, by + i as i32 * 22, l, if i == 0 { 14.0 } else { 11.0 }, c, 530);
    }
}

pub fn draw_overlays(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if d.calib.is_some() {
        draw_calibration(d, oi, xf, cv, clip);
    }
    if let Some(r) = d.overlay.snap_preview {
        if r.overlaps(&d.outputs[oi].rect()) {
            cv.rrect(clip, xf.r(r), xf.len(14), crate::look::accent_a(46));
            cv.rrect_stroke(clip, xf.r(r), xf.len(14), 1.5 * xf.scale as f32, crate::look::accent() & 0xff_ffff, 200, 140);
        }
    }
    if glass_on_gpu() {
        crate::winanim::draw(d, oi, xf, cv, clip);
        crate::life::draw(d, oi, xf, cv, clip);
    }
    crate::taskview::draw(d, oi, xf, cv, clip);
    // (Still while it closes, on the GPU path.)
    if crate::startanim::shown(d) == Some(oi) {
        crate::start::draw(d, oi, xf, cv, clip);
    }
    crate::tray::draw(d, oi, xf, cv, clip);
    if d.clip.open == Some(oi) {
        crate::clipboard::draw(d, oi, xf, cv, clip);
    }
    crate::monitors::draw(d, oi, xf, cv, clip);
    if d.overlay.switcher.is_some() && d.overlay.alttab.is_none() {
        let pointer_out = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
        if pointer_out == oi {
            draw_switcher(d, oi, xf, cv, clip);
        }
    }
    if glass_on_gpu() {
        crate::alttab::draw(d, oi, xf, cv, clip);
    }
    crate::osd::draw(d, oi, xf, cv, clip);
    crate::snip::draw(d, oi, xf, cv, clip);
    draw_peek(d, oi, xf, cv, clip);
    // Over everything while the desk starts.
    crate::boot::draw(d, oi, xf, cv, clip);
}

/// Clicks on overlays (start menu entries, switcher tiles).
pub fn overlay_click(d: &mut Desk, x: f64, y: f64, button: u32) -> bool {
    if peek_click(d, x, y, button) {
        return true;
    }
    if crate::tray::click(d, x, y, button) {
        return true;
    }
    if crate::clipboard::click(d, x, y) {
        return true;
    }
    if crate::monitors::click(d, x, y) {
        return true;
    }
    if crate::taskview::click(d, x, y, button) {
        return true;
    }
    if let Some(oi) = d.overlay.start_menu {
        if crate::start::click(d, oi, x, y, button) {
            return true;
        }
        if !taskbar_contains(d, x, y) {
            close_menus(d);
            return true;
        }
    }
    if let Some((list, _)) = d.overlay.switcher.clone() {
        let oi = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
        let (_, cells) = switcher_layout(d, oi, list.len());
        if let Some(i) = cells.iter().position(|r| r.contains(x, y)) {
            d.overlay.switcher = Some((list, i));
            switcher_commit(d);
        } else {
            d.overlay.switcher = None;
            d.damage_all();
        }
        return true;
    }
    false
}

// ------------------------------------------------------------------ cursor shapes

/// Rect for the built-in cursor of `shape` with hotspot at (x, y).
pub fn cursor_shape_rect(shape: u32, x: i32, y: i32) -> Rect {
    match shape {
        18..=25 | 26 | 27 | 28 | 29 | 30 | 31 => Rect::new(x - 12, y - 12, 25, 25),
        9 => Rect::new(x - 5, y - 10, 11, 21),
        _ => Rect::new(x, y, 12, 21),
    }
}

/// Draws non-arrow built-in cursors. Returns false to fall back to the arrow.
pub fn draw_cursor_shape(shape: u32, x: i32, y: i32, xf: Xf, cv: &mut Canvas, clip: Rect) -> bool {
    let line = |cv: &mut Canvas, r: Rect| {
        cv.fill(clip, xf.r(r.inflate(1)), 0xff00_0000);
    };
    let white = |cv: &mut Canvas, r: Rect| {
        cv.fill(clip, xf.r(r), 0xffff_ffff);
    };
    match shape {
        // Text I-beam.
        9 => {
            for r in [Rect::new(x - 3, y - 9, 7, 1), Rect::new(x - 3, y + 9, 7, 1), Rect::new(x, y - 9, 1, 19)] {
                line(cv, r);
            }
            for r in [Rect::new(x - 3, y - 9, 7, 1), Rect::new(x - 3, y + 9, 7, 1), Rect::new(x, y - 9, 1, 19)] {
                white(cv, r);
            }
            true
        }
        // Horizontal / vertical / diagonal resize arrows (drawn as bars with heads).
        18 | 22 | 27 | 29 => {
            let bars = [Rect::new(x, y - 10, 1, 21), Rect::new(x - 3, y - 7, 7, 1), Rect::new(x - 3, y + 7, 7, 1)];
            for r in bars {
                line(cv, r);
            }
            for r in bars {
                white(cv, r);
            }
            true
        }
        20 | 24 | 26 | 28 => {
            let bars = [Rect::new(x - 10, y, 21, 1), Rect::new(x - 7, y - 3, 1, 7), Rect::new(x + 7, y - 3, 1, 7)];
            for r in bars {
                line(cv, r);
            }
            for r in bars {
                white(cv, r);
            }
            true
        }
        19 | 21 | 23 | 25 | 30 | 31 => {
            let flip = matches!(shape, 19 | 23 | 31);
            for i in -8..=8 {
                let px = if flip { x + i } else { x + i };
                let py = if flip { y - i } else { y + i };
                line(cv, Rect::new(px, py, 1, 1));
            }
            for i in -8..=8 {
                let px = x + i;
                let py = if flip { y - i } else { y + i };
                white(cv, Rect::new(px, py, 1, 1));
            }
            true
        }
        _ => false,
    }
}
