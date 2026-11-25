//! Task view (Win+Tab, the taskbar's button), like Windows 11: frosted glass over the
//! desktop, the windows of a desktop as live cards, and a bar of the virtual desktops
//! along the bottom with one to make a new one.
//! - click a card: that window comes to the front; its × (or the middle button) closes it;
//! - drag a card onto a desktop: the window moves there;
//! - hover a desktop: its windows show; click it: switch there; its ×: it closes (its
//!   windows go to the one before);
//! - Esc, Win+Tab or a click on the empty glass: back.

use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::shell::{self, col};
use crate::state::WKey;
use crate::wm;

const CARD_R: i32 = 12;
const HEAD: i32 = 32;
const GAP: i32 = 28;
const TILE_W: i32 = 168;
const LABEL_H: i32 = 26;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hot {
    Win(WKey),
    WinClose(WKey),
    Desk(u32),
    DeskClose(u32),
    NewDesk,
}

pub struct TaskView {
    pub oi: usize,
    /// The desktop whose windows show (the hovered one, else the current).
    shown: u32,
    hot: Option<Hot>,
    /// A card pressed (window, where) and whether it is being dragged.
    press: Option<(WKey, f64, f64, bool)>,
}

struct Card {
    wk: WKey,
    card: Rect,
    thumb: Rect,
    close: Rect,
}

struct Lay {
    veil: Rect,
    cards: Vec<Card>,
    bar: Rect,
    desks: Vec<(u32, Rect, Rect)>,
    new_desk: Rect,
}

pub fn is_open(d: &Desk) -> bool {
    d.overlay.taskview.is_some()
}

pub fn toggle(d: &mut Desk) {
    if d.overlay.taskview.is_some() {
        close(d);
        return;
    }
    shell::close_menus(d);
    crate::tray::close(d);
    d.overlay.switcher = None;
    let oi = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
    d.overlay.taskview = Some(TaskView { oi, shown: d.workspace, hot: None, press: None });
    d.damage_all();
}

pub fn close(d: &mut Desk) {
    if d.overlay.taskview.take().is_some() {
        d.damage_all();
    }
}

fn windows_of(d: &Desk, ws: u32) -> Vec<WKey> {
    // Most recently used first, then the rest (never focused).
    let ok = |k: &WKey| {
        let w = &d.windows[*k];
        w.workspace == ws && w.parent.is_none() && !w.desktop() && d.surfaces.get(w.surf).map_or(false, |s| s.mapped)
    };
    let mut v: Vec<WKey> = d.mru.iter().copied().filter(|k| d.windows.contains(*k) && ok(k)).collect();
    for (k, _) in d.windows.iter() {
        if ok(&k) && !v.contains(&k) {
            v.push(k);
        }
    }
    v
}

fn lay(d: &Desk, tv: &TaskView) -> Lay {
    let o = d.outputs[tv.oi].rect();
    let veil = Rect::new(o.x, o.y, o.w, o.h - (o.y1() - crate::shell::taskbar_rect(d, tv.oi).y));
    // The desktops bar.
    let tile_h = TILE_W * o.h / o.w.max(1);
    let n = d.workspaces as i32 + 1;
    let bw = n * TILE_W + (n - 1) * 16 + 2 * 18;
    let bh = 16 + tile_h + LABEL_H + 8;
    let bar = Rect::new(o.x + (o.w - bw) / 2, veil.y1() - 20 - bh, bw, bh);
    let mut desks = Vec::new();
    for i in 0..d.workspaces {
        let t = Rect::new(bar.x + 18 + i as i32 * (TILE_W + 16), bar.y + 16, TILE_W, tile_h);
        desks.push((i, t, Rect::new(t.x1() - 26, t.y + 4, 22, 22)));
    }
    let new_desk = Rect::new(bar.x + 18 + d.workspaces as i32 * (TILE_W + 16), bar.y + 16, TILE_W, tile_h);
    // The windows: the grid with the largest cards that fits.
    let area = Rect::from_edges(veil.x + 48, veil.y + 40, veil.x1() - 48, bar.y - 28);
    let wins = windows_of(d, tv.shown);
    let mut cards = Vec::new();
    let nw = wins.len() as i32;
    if nw > 0 && area.w > 100 && area.h > 100 {
        let aspect = |wk: WKey| {
            let g = d.surfaces[d.windows[wk].surf].geom();
            if g.w > 0 && g.h > 0 { g.w as f64 / g.h as f64 } else { 1.6 }
        };
        let mut best = (1, 0.0);
        for cols in 1..=nw {
            let rows = (nw + cols - 1) / cols;
            let cw = (area.w - (cols - 1) * GAP) as f64 / cols as f64;
            let ch = (area.h - (rows - 1) * GAP) as f64 / rows as f64 - HEAD as f64;
            let size = cw.min(ch * 1.6).min(440.0);
            if size > best.1 {
                best = (cols, size);
            }
        }
        let cols = best.0;
        let rows = (nw + cols - 1) / cols;
        let cw = ((area.w - (cols - 1) * GAP) / cols).min(440);
        let th_max = ((area.h - (rows - 1) * GAP) / rows - HEAD).min(300);
        let block_h = rows * (th_max + HEAD) + (rows - 1) * GAP;
        let mut y = area.y + (area.h - block_h) / 2;
        for r in 0..rows {
            let row: Vec<WKey> = wins.iter().copied().skip((r * cols) as usize).take(cols as usize).collect();
            let sizes: Vec<(i32, i32)> = row
                .iter()
                .map(|wk| {
                    let a = aspect(*wk);
                    let (mut tw, mut th) = (cw as f64, cw as f64 / a);
                    if th > th_max as f64 {
                        th = th_max as f64;
                        tw = th * a;
                    }
                    ((tw as i32).max(150), (th as i32).max(60))
                })
                .collect();
            let total: i32 = sizes.iter().map(|s| s.0).sum::<i32>() + (row.len() as i32 - 1) * GAP;
            let mut x = area.x + (area.w - total) / 2;
            for (wk, (tw, th)) in row.into_iter().zip(sizes) {
                let ty = y + HEAD + (th_max - th) / 2;
                let card = Rect::new(x, ty - HEAD, tw, th + HEAD);
                cards.push(Card { wk, card, thumb: Rect::new(x, ty, tw, th), close: Rect::new(card.x1() - 30, card.y + 2, 28, 26) });
                x += tw + GAP;
            }
            y += th_max + HEAD + GAP;
        }
    }
    Lay { veil, cards, bar, desks, new_desk }
}

fn hit(d: &Desk, tv: &TaskView, l: &Lay, x: f64, y: f64) -> Option<Hot> {
    for (i, t, c) in &l.desks {
        if t.contains(x, y) {
            return Some(if d.workspaces > 1 && c.contains(x, y) { Hot::DeskClose(*i) } else { Hot::Desk(*i) });
        }
    }
    if l.new_desk.contains(x, y) {
        return Some(Hot::NewDesk);
    }
    let _ = tv;
    for c in l.cards.iter().rev() {
        if c.card.contains(x, y) {
            return Some(if c.close.contains(x, y) { Hot::WinClose(c.wk) } else { Hot::Win(c.wk) });
        }
    }
    None
}

/// The glass under it all (shell::glass_panels): the veil.
pub fn glass(d: &Desk, oi: usize) -> Option<Rect> {
    let tv = d.overlay.taskview.as_ref().filter(|t| t.oi == oi)?;
    Some(lay(d, tv).veil)
}

pub fn hover(d: &mut Desk, x: f64, y: f64) {
    let Some(tv) = d.overlay.taskview.as_ref() else { return };
    let l = lay(d, tv);
    let hot = hit(d, tv, &l, x, y);
    // Over a desktop: its windows show (and stay when the pointer goes up to them).
    let shown = match hot {
        Some(Hot::Desk(i) | Hot::DeskClose(i)) => i,
        _ => tv.shown,
    };
    let tv = d.overlay.taskview.as_mut().unwrap();
    if tv.hot != hot || tv.shown != shown {
        tv.hot = hot;
        tv.shown = shown;
        let v = l.veil;
        d.damage(v);
    }
}

/// A press. Everything is ours while it is open.
pub fn click(d: &mut Desk, x: f64, y: f64, button: u32) -> bool {
    let Some(tv) = d.overlay.taskview.as_ref() else { return false };
    let oi = tv.oi;
    let l = lay(d, tv);
    if !d.outputs[oi].rect().contains(x, y) || (y >= l.veil.y1() as f64) {
        // The taskbar keeps working (its task view button closes this).
        return false;
    }
    match hit(d, tv, &l, x, y) {
        Some(Hot::Win(wk)) if button == crate::input::BTN_MIDDLE => wm::close(d, wk),
        Some(Hot::WinClose(wk)) => wm::close(d, wk),
        Some(Hot::Win(wk)) => {
            d.overlay.taskview.as_mut().unwrap().press = Some((wk, x, y, false));
        }
        Some(Hot::Desk(i)) => {
            wm::switch_workspace(d, i);
            close(d);
        }
        Some(Hot::DeskClose(i)) => {
            wm::switch_workspace(d, i);
            wm::close_workspace(d);
            if let Some(tv) = d.overlay.taskview.as_mut() {
                tv.shown = d.workspace;
                tv.hot = None;
            }
        }
        Some(Hot::NewDesk) => {
            wm::new_workspace(d);
            if let Some(tv) = d.overlay.taskview.as_mut() {
                tv.shown = d.workspace;
            }
        }
        None => close(d),
    }
    d.damage_all();
    true
}

/// The pointer moved with the button held: a pressed card follows it once it moved enough.
pub fn drag(d: &mut Desk, x: f64, y: f64) {
    let Some(tv) = d.overlay.taskview.as_mut() else { return };
    let Some((wk, sx, sy, active)) = tv.press else { return };
    if !active && (x - sx).hypot(y - sy) < 8.0 {
        return;
    }
    tv.press = Some((wk, sx, sy, true));
    let r = d.outputs[tv.oi].rect();
    d.damage(r);
}

pub fn release(d: &mut Desk, x: f64, y: f64) {
    let Some(tv) = d.overlay.taskview.as_mut() else { return };
    let Some((wk, _, _, active)) = tv.press.take() else { return };
    let tv = d.overlay.taskview.as_ref().unwrap();
    let l = lay(d, tv);
    if !active {
        // A click: to that window.
        if d.windows.contains(wk) {
            let ws = d.windows[wk].workspace;
            wm::switch_workspace(d, ws);
            wm::unminimize(d, wk);
        }
        close(d);
        return;
    }
    let target = l.desks.iter().find(|(_, t, _)| t.contains(x, y)).map(|(i, _, _)| *i).or_else(|| l.new_desk.contains(x, y).then_some(u32::MAX));
    match target {
        Some(u32::MAX) => {
            d.workspaces += 1;
            let new = d.workspaces - 1;
            wm::move_to_workspace(d, wk, new);
        }
        Some(i) => wm::move_to_workspace(d, wk, i),
        None => {}
    }
    d.damage_all();
}

/// A window's content changed: its card follows.
pub fn commit(d: &mut Desk, wk: WKey) {
    let Some(tv) = d.overlay.taskview.as_ref() else { return };
    if d.windows.get(wk).map_or(true, |w| w.workspace != tv.shown) {
        return;
    }
    let l = lay(d, tv);
    if let Some(c) = l.cards.iter().find(|c| c.wk == wk) {
        let r = c.thumb;
        d.damage(r);
    }
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some(tv) = d.overlay.taskview.as_ref().filter(|t| t.oi == oi) else { return };
    let l = lay(d, tv);
    shell::draw_glass(cv, clip, xf, l.veil, 0);
    // A little deeper than the panels: the cards stand out.
    cv.fill(clip, xf.r(l.veil), argb(if crate::glass::frost() >= 8.0 { 70 } else { 150 }, 6, 4, 12));
    let dragging = tv.press.filter(|p| p.3).map(|p| p.0);
    for c in &l.cards {
        let hot = matches!(tv.hot, Some(Hot::Win(k) | Hot::WinClose(k)) if k == c.wk);
        let w = &d.windows[c.wk];
        let frame = c.card.inflate(6);
        cv.rrect(clip, xf.r(frame), xf.len(CARD_R + 4), if hot { argb(40, 255, 255, 255) } else { argb(18, 255, 255, 255) });
        if hot {
            cv.rrect_stroke(clip, xf.r(frame), xf.len(CARD_R + 4), 1.5 * xf.scale as f32, 0xffffff, 90, 60);
        }
        shell::draw_app_tile(cv, clip, xf, Rect::new(c.card.x + 4, c.card.y + 6, 18, 18), &w.app_id, &w.title);
        let t = if w.title.is_empty() { &w.app_id } else { &w.title };
        let tmax = c.card.w - 30 - 34;
        font::draw_text_w(cv, clip, xf, c.card.x + 30, c.card.y + 20, &shell::ellipsize(t, 12.5, font::W::Semi, tmax), 12.5, col::TEXT, tmax, font::W::Semi);
        if hot {
            let ch = tv.hot == Some(Hot::WinClose(c.wk));
            if ch {
                cv.rrect(clip, xf.r(c.close), xf.len(7), col::CLOSE_HOVER);
            }
            let (cx, cy) = c.close.center();
            shell::draw_x(cv, clip, xf, cx, cy, 4, col::TEXT);
        }
        cv.rrect(clip, xf.r(c.thumb), xf.len(8), argb(50, 0, 0, 0));
        if dragging == Some(c.wk) {
            continue;
        }
        shell::draw_thumb(d, c.wk, c.thumb, xf, cv, clip);
        if w.minimized {
            cv.rrect(clip, xf.r(c.thumb), xf.len(8), argb(70, 10, 8, 16));
        }
    }
    // The desktops.
    cv.rrect(clip, xf.r(l.bar), xf.len(22), argb(30, 255, 255, 255));
    cv.rrect_stroke(clip, xf.r(l.bar), xf.len(22), xf.scale as f32, 0xffffff, 40, 20);
    let o = d.outputs[oi].rect();
    for (i, t, cl) in &l.desks {
        let cur = *i == d.workspace;
        let hot = matches!(tv.hot, Some(Hot::Desk(k) | Hot::DeskClose(k)) if k == *i);
        let pt = xf.r(*t);
        crate::wallpaper::draw_scaled(d, oi, cv, clip.intersect(&pt), pt);
        // Its windows, in miniature.
        let k = t.w as f64 / o.w.max(1) as f64;
        for wk in windows_of(d, *i).into_iter().rev() {
            let w = &d.windows[wk];
            if w.minimized {
                continue;
            }
            let Some(f) = wm::frame_rect(d, wk) else { continue };
            let m = Rect::new(t.x + ((f.x - o.x) as f64 * k) as i32, t.y + ((f.y - o.y) as f64 * k) as i32, ((f.w as f64 * k) as i32).max(4), ((f.h as f64 * k) as i32).max(3));
            let m = m.intersect(t);
            if !m.is_empty() {
                cv.rrect(clip, xf.r(m), xf.len(2), argb(170, 34, 31, 44));
                cv.rrect_stroke(clip, xf.r(m), xf.len(2), xf.scale as f32, 0xffffff, 60, 40);
            }
        }
        let ring = if cur { ((crate::look::accent() & 0xff_ffff), 255) } else if hot { (0xffffff, 120) } else { (0xffffff, 40) };
        cv.rrect_stroke(clip, xf.r(t.inflate(2)), xf.len(10), if cur { 2.5 } else { 1.5 } * xf.scale as f32, ring.0, ring.1, ring.1);
        let name = format!("Рабочий стол {}", i + 1);
        let tw = font::text_width_w(&name, 12.5, font::W::Semi);
        font::draw_text_w(cv, clip, xf, t.x + (t.w - tw) / 2, t.y1() + 18, &name, 12.5, if cur { col::TEXT } else { col::TEXT_DIM }, t.w, font::W::Semi);
        if hot && d.workspaces > 1 {
            let ch = tv.hot == Some(Hot::DeskClose(*i));
            cv.rrect(clip, xf.r(*cl), xf.len(6), if ch { col::CLOSE_HOVER } else { argb(160, 20, 18, 28) });
            let (cx, cy) = cl.center();
            shell::draw_x(cv, clip, xf, cx, cy, 4, col::TEXT);
        }
    }
    let nd = l.new_desk;
    let hot = tv.hot == Some(Hot::NewDesk);
    cv.rrect(clip, xf.r(nd), xf.len(10), if hot { argb(36, 255, 255, 255) } else { argb(16, 255, 255, 255) });
    cv.rrect_stroke(clip, xf.r(nd), xf.len(10), xf.scale as f32, 0xffffff, if hot { 90 } else { 40 }, if hot { 90 } else { 40 });
    let (cx, cy) = nd.center();
    let (cx, cy) = (cx as f32, cy as f32);
    let lw = 2.0 * xf.scale as f32;
    cv.line(clip, shell::phys(xf, cx - 9.0, cy), shell::phys(xf, cx + 9.0, cy), lw, col::TEXT);
    cv.line(clip, shell::phys(xf, cx, cy - 9.0), shell::phys(xf, cx, cy + 9.0), lw, col::TEXT);
    let label = "Создать";
    let tw = font::text_width_w(label, 12.5, font::W::Semi);
    font::draw_text_w(cv, clip, xf, nd.x + (nd.w - tw) / 2, nd.y1() + 18, label, 12.5, col::TEXT_DIM, nd.w, font::W::Semi);
    // The dragged card under the pointer.
    if let Some(wk) = dragging {
        let (px, py) = (d.seat.x as i32, d.seat.y as i32);
        let area = Rect::new(px - 110, py - 70, 220, 140);
        cv.rrect(clip, xf.r(area.inflate(4)), xf.len(10), argb(120, 20, 18, 28));
        shell::draw_thumb(d, wk, area, xf, cv, clip);
    }
}
