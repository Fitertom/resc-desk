//! Drawing the window: tabs, the toolbar with the crumbs, the command bar, the
//! navigation pane, the items (table, icons, tiles), the status bar, then what floats
//! (drag ghost, the operations card with its speed graph, toasts, menus, dialogs).
use super::input::CmdBtn;
use super::ui::{self, card, glyph, text, text_w, tw, white, G, DIM, FAINT, FG};
use super::*;
use crate::font::W;
use crate::render::{argb, Canvas};

// The window's own darkness is its glass's (look.rs: «Стекло» → «Проводник»): only light
// veils here, so the parts still read apart.
const BG_TABS: u32 = argb(70, 0, 0, 0);
const BG_BAR: u32 = argb(10, 255, 255, 255);
const BG_NAV: u32 = argb(26, 0, 0, 0);
const BG_BODY: u32 = argb(0, 0, 0, 0);
/// Opaque, for what floats over the rest (a header being dragged).
const BG_SOLID: u32 = argb(255, 23, 24, 31);

pub fn draw(app: &App, cv: &mut Canvas) {
    let (w, h) = (app.w, app.h);
    let all = cv.bounds();
    cv.clear(all);
    if app.desk {
        // Only the icons, on the wallpaper (menus and dialogs: `overlay`).
        content(app, cv);
        drag_ghost(app, cv);
        ops_card(app, cv);
        toast(app, cv);
        return;
    }
    cv.fill(all, Rect::new(0, 0, w, TABS_H), BG_TABS);
    cv.fill(all, Rect::new(0, TABS_H, w, TOP - TABS_H), BG_BAR);
    cv.fill(all, Rect::new(0, TOP, NAV_W, h - TOP), BG_NAV);
    cv.fill(all, Rect::new(NAV_W, TOP, w - NAV_W, h - TOP), BG_BODY);
    cv.fill(all, Rect::new(NAV_W, TOP, 1, h - TOP - STATUS_H), white(10));
    cv.fill(all, Rect::new(0, TOP - 1, w, 1), white(12));
    tabs(app, cv);
    toolbar(app, cv);
    cmdbar(app, cv);
    nav(app, cv);
    content(app, cv);
    status(app, cv);
    drag_ghost(app, cv);
    ops_card(app, cv);
    toast(app, cv);
    if let Some(m) = &app.menu {
        menu(app, cv, m);
        if let Some((_, s)) = &m.sub {
            menu(app, cv, s);
        }
    }
    if let Some(d) = &app.dialog {
        dialog(app, cv, d);
    }
}

/// The desktop's menus and dialogs, drawn over every window.
pub fn overlay(app: &App, cv: &mut Canvas) {
    cv.clear(cv.bounds());
    if let Some(m) = &app.menu {
        menu(app, cv, m);
        if let Some((_, s)) = &m.sub {
            menu(app, cv, s);
        }
    }
    if let Some(d) = &app.dialog {
        dialog(app, cv, d);
    }
}

fn hov(app: &App, h: Hit) -> bool {
    app.hot == h && app.press.is_none() && app.menu.is_none() && app.dialog.is_none()
}

fn btn_bg(cv: &mut Canvas, clip: Rect, r: Rect, hot: bool, pressed: bool) {
    if pressed {
        cv.rrect(clip, r, 7, white(22));
    } else if hot {
        cv.rrect(clip, r, 7, white(14));
    }
}

/// An icon by theme name(s) (`a;b;c`), drawn into `r`; false when the theme has none.
pub fn icon(cv: &mut Canvas, clip: Rect, r: Rect, names: &str) -> bool {
    if names.is_empty() {
        return false;
    }
    let key = if names.contains(';') { names.to_string() } else { format!("{names};") };
    crate::icons::draw(cv, clip, ui::XF, r, &key)
}

fn item_icon(cv: &mut Canvas, clip: Rect, r: Rect, it: &Item) {
    if !icon(cv, clip, r, &it.icon) {
        if it.e.dir {
            ui::folder_shape(cv, clip, r);
        } else if !icon(cv, clip, r, "text-x-generic;application-octet-stream;unknown") {
            ui::page_shape(cv, clip, r, &it.e.ext);
        }
    }
    if it.e.link {
        let s = (r.w / 3).clamp(8, 32);
        let b = Rect::new(r.x, r.y1() - s, s, s);
        cv.rrect(clip, b, 3, argb(235, 240, 242, 248));
        glyph(cv, clip, b.inflate(-s / 6), G::Fwd, argb(255, 40, 90, 200));
    }
}

fn loc_icon(app: &App, l: &Loc) -> String {
    match l {
        Loc::Pc => "computer;".into(),
        Loc::Trash => "user-trash;".into(),
        Loc::Arc(..) => "package-x-generic;application-x-archive".into(),
        Loc::Search(..) => "system-search;edit-find".into(),
        Loc::Dir(p) => {
            if let Some(n) = app.nav.iter().find(|n| n.loc == *l) {
                return n.icon.clone();
            }
            if *p == model::home() { "user-home;folder".into() } else { "folder;".into() }
        }
    }
}

// ------------------------------------------------------------------ top

fn tabs(app: &App, cv: &mut Canvas) {
    let clip = Rect::new(0, 0, app.w, TABS_H);
    let (rects, plus) = app.tab_rects();
    for (i, r) in rects.iter().enumerate() {
        let act = i == app.cur;
        if act {
            // Joined to the bar below: rounded on top only.
            cv.rrect(clip, Rect::new(r.x, r.y, r.w, r.h + 10), 9, BG_BAR);
            cv.rrect_stroke(clip, Rect::new(r.x, r.y, r.w, r.h + 10), 9, 1.0, 0xffffff, 40, 0);
        } else if hov(app, Hit::Tab(i)) || hov(app, Hit::TabClose(i)) {
            cv.rrect(clip, r.inflate(-2), 8, white(12));
        }
        if i > 0 && !act && i != app.cur + 1 {
            cv.fill(clip, Rect::new(r.x - 1, r.y + 9, 1, 16), white(26));
        }
        let t = &app.tabs[i];
        let title = if act { app.title.clone() } else { app.loc_title(&t.loc) };
        let ic = Rect::new(r.x + 12, r.y + 9, 16, 16);
        if !icon(cv, clip, ic, &loc_icon(app, &t.loc)) {
            ui::folder_shape(cv, clip, ic);
        }
        text(cv, clip, ic.x1() + 9, r.y + 22, &title, 12.5, if act { FG } else { DIM }, r.w - 70);
        let x = Rect::new(r.x1() - 30, r.y + 5, 24, 24);
        if hov(app, Hit::TabClose(i)) {
            cv.rrect(clip, x, 6, white(20));
        }
        if act || hov(app, Hit::Tab(i)) || hov(app, Hit::TabClose(i)) {
            glyph(cv, clip, x.inflate(-6), G::Close, DIM);
        }
    }
    // A tab from another window over the strip: where it would go.
    if let Some(at) = app.tab_drop_at {
        let x = rects.get(at).map_or(plus.x - 2, |r| r.x - 2);
        cv.rrect(clip, Rect::new(x, 6, 3, TABS_H - 12), 1, ui::accent(255));
    }
    btn_bg(cv, clip, plus, hov(app, Hit::NewTab), false);
    glyph(cv, clip, plus.inflate(-8), G::Plus, DIM);
    // ─ □ ✕, Windows 11 style (the close button turns red).
    for (i, r) in app.caption_rects().iter().enumerate() {
        let hot = hov(app, Hit::Caption(i));
        let down = app.press.as_ref().map_or(false, |p| p.hit == Hit::Caption(i));
        if hot || down {
            let c = match (i, down) {
                (2, true) => argb(255, 196, 43, 28),
                (2, false) => argb(255, 0xe1, 0x3b, 0x3b),
                (_, true) => white(14),
                _ => white(22),
            };
            if i == 2 && !app.maximized {
                // Cut by the window's rounded corner.
                cv.rrect(clip.intersect(r), Rect::new(0, 0, app.w, app.h), crate::shell::RADIUS - 1, c);
            } else {
                cv.fill(clip, *r, c);
            }
        }
        let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        let col = FG;
        match i {
            0 => cv.line(clip, (cx - 5.0, cy + 0.5), (cx + 5.0, cy + 0.5), 1.1, col),
            1 if app.maximized => {
                cv.rrect_stroke(clip, Rect::new(cx as i32 - 5, cy as i32 - 3, 8, 8), 2, 1.1, 0xffffff, 255, 255);
                cv.line(clip, (cx - 2.5, cy - 5.5), (cx + 3.5, cy - 5.5), 1.1, col);
                cv.line(clip, (cx + 5.5, cy - 3.5), (cx + 5.5, cy + 2.5), 1.1, col);
            }
            1 => cv.rrect_stroke(clip, Rect::new(cx as i32 - 5, cy as i32 - 5, 10, 10), 2, 1.1, 0xffffff, 255, 255),
            _ => {
                cv.line(clip, (cx - 5.0, cy - 5.0), (cx + 5.0, cy + 5.0), 1.1, col);
                cv.line(clip, (cx + 5.0, cy - 5.0), (cx - 5.0, cy + 5.0), 1.1, col);
            }
        }
    }
}

fn toolbar(app: &App, cv: &mut Canvas) {
    let clip = Rect::new(0, TABS_H, app.w, TOOL_H);
    let t = &app.tabs[app.cur];
    let can = [!t.back.is_empty(), !t.fwd.is_empty(), app.parent().is_some(), true];
    for (i, (h, g)) in [(Hit::Back, G::Back), (Hit::Fwd, G::Fwd), (Hit::Up, G::Up), (Hit::Refresh, G::Refresh)].into_iter().enumerate() {
        let r = app.tool_btn(i as i32);
        if can[i] {
            btn_bg(cv, clip, r, hov(app, h), app.press.as_ref().map_or(false, |p| p.hit == h));
        }
        glyph(cv, clip, r.inflate(-8), g, if can[i] { FG } else { FAINT });
    }
    // The address bar.
    let ar = app.addr_rect();
    let editing = app.addr.is_some();
    cv.rrect(clip, ar, 8, argb(if editing { 235 } else { 160 }, 12, 13, 18));
    cv.rrect_stroke(clip, ar, 8, 1.0, if editing { 0x7fb0ff } else { 0xffffff }, if editing { 200 } else { 34 }, if editing { 200 } else { 14 });
    let ic = Rect::new(ar.x + 10, ar.y + 8, 16, 16);
    if !icon(cv, clip, ic, &loc_icon(app, app.loc())) {
        ui::folder_shape(cv, clip, ic);
    }
    let inner = Rect::from_edges(ar.x + 2, ar.y, ar.x1() - 2, ar.y1());
    if let Some(tf) = &app.addr {
        tf.draw(cv, inner.intersect(&clip), ui::XF, ar.x + 34, ar.y + 21, ar.w - 44, 13.0, FG, W::Text);
    } else {
        let (crumbs, cut) = app.crumb_rects();
        let cs = app.crumbs();
        if cut {
            glyph(cv, clip, Rect::new(ar.x + 32, ar.y + 9, 14, 14), G::Back, DIM);
        }
        for (k, (r, i)) in crumbs.iter().enumerate() {
            if app.crumb_drop == Some(*i) {
                cv.rrect(clip, *r, 6, if app.drag_to.is_some() { argb(70, 0x60, 0xcd, 0xff) } else { white(26) });
            } else if hov(app, Hit::Crumb(*i)) {
                cv.rrect(clip, *r, 6, white(16));
            }
            let last = k + 1 == crumbs.len();
            text(cv, inner, r.x + 8, ar.y + 21, &cs[*i].0, 13.0, if last { FG } else { argb(255, 0xc8, 0xcc, 0xd8) }, r.w - 8);
            if !last {
                glyph(cv, inner, Rect::new(r.x1() + 3, ar.y + 11, 12, 12), G::ChevR, DIM);
            }
        }
    }
    // Search.
    let sr = app.search_rect();
    let on = app.search_on;
    cv.rrect(clip, sr, 8, argb(if on { 235 } else { 160 }, 12, 13, 18));
    cv.rrect_stroke(clip, sr, 8, 1.0, if on { 0x7fb0ff } else { 0xffffff }, if on { 200 } else { 34 }, if on { 200 } else { 14 });
    glyph(cv, clip, Rect::new(sr.x + 10, sr.y + 9, 14, 14), G::Search, DIM);
    if app.search.is_empty() && !on {
        let place = format!("Поиск: {}", match app.loc() {
            Loc::Search(b, _) => app.disp_name(b),
            l => app.loc_title(l),
        });
        text(cv, clip, sr.x + 32, sr.y + 21, &place, 13.0, FAINT, sr.w - 44);
    } else if on {
        app.search.draw(cv, sr.intersect(&clip), ui::XF, sr.x + 32, sr.y + 21, sr.w - 64, 13.0, FG, W::Text);
    } else {
        text(cv, clip, sr.x + 32, sr.y + 21, &app.search.text(), 13.0, FG, sr.w - 64);
    }
    if !app.search.is_empty() {
        let x = Rect::new(sr.x1() - 28, sr.y + 5, 22, 22);
        if hov(app, Hit::SearchClear) {
            cv.rrect(clip, x, 5, white(20));
        }
        glyph(cv, clip, x.inflate(-6), G::Close, DIM);
    }
    if app.searching {
        // A thin moving bar under the search box.
        let t = (sys::now_ns() / 4_000_000 % 1000) as f32 / 1000.0;
        let bw = sr.w / 3;
        let x = sr.x + ((sr.w + bw) as f32 * t) as i32 - bw;
        cv.fill(sr, Rect::new(x, sr.y1() - 2, bw, 2), ui::accent(255));
    }
}

fn cmdbar(app: &App, cv: &mut Canvas) {
    let clip = Rect::new(0, TABS_H + TOOL_H, app.w, CMD_H);
    let btns: Vec<CmdBtn> = app.cmd_btns();
    let mut prev_x1: Option<i32> = None;
    for (i, b) in btns.iter().enumerate() {
        if let Some(px) = prev_x1 {
            if b.r.x - px > 6 && b.cmd != Cmd::More {
                cv.fill(clip, Rect::new(px + (b.r.x - px) / 2, b.r.y + 6, 1, 20), white(30));
            }
        }
        prev_x1 = Some(b.r.x1());
        let open = app.menu.is_some() && app.press.is_none() && false;
        if b.on {
            btn_bg(cv, clip, b.r, hov(app, Hit::Cmd(i)) || open, app.press.as_ref().map_or(false, |p| p.hit == Hit::Cmd(i)));
        }
        let col = if b.on { FG } else { FAINT };
        let gc = if b.on {
            match b.cmd {
                Cmd::New => ui::accent(255),
                Cmd::Delete | Cmd::Empty => argb(255, 0xff, 0x8a, 0x8a),
                _ => FG,
            }
        } else {
            FAINT
        };
        let gx = if b.label.is_empty() { b.r.x + (b.r.w - 18) / 2 } else { b.r.x + 9 };
        glyph(cv, clip, Rect::new(gx, b.r.y + 7, 18, 18), b.g, gc);
        if !b.label.is_empty() {
            text(cv, clip, b.r.x + 34, b.r.y + 21, b.label, 13.0, col, b.r.w - 34 - if b.drop { 18 } else { 4 });
            if b.drop {
                glyph(cv, clip, Rect::new(b.r.x1() - 18, b.r.y + 11, 10, 10), G::ChevD, DIM);
            }
        }
    }
}

// ------------------------------------------------------------------ the navigation pane

fn nav(app: &App, cv: &mut Canvas) {
    let clip = Rect::from_edges(0, TOP, NAV_W, app.h - STATUS_H);
    let rects = app.nav_rects();
    let dy = -(app.nav_scroll.value.round() as i32);
    // Hover drop (follows the pointer between entries).
    if app.ndrop.a.value > 0.01 {
        let r = app.ndrop.rect().translate(0, dy);
        let a = (app.ndrop.a.value * if app.drag_to.is_some() { 40.0 } else { 14.0 }) as u32;
        cv.rrect(clip, r, 7, if app.drag_to.is_some() { ui::accent(a * 2) } else { white(a) });
    }
    for (i, n) in app.nav.iter().enumerate() {
        let r = rects[i];
        if n.sep {
            cv.fill(clip, Rect::new(r.x + 10, r.y - 7, r.w - 20, 1), white(18));
        }
        if !r.overlaps(&clip) {
            continue;
        }
        let here = *app.loc() == n.loc;
        if here {
            cv.rrect(clip, r, 7, ui::accent(36));
            cv.rrect(clip, Rect::new(r.x, r.y + 9, 3, r.h - 18), 1, ui::accent(255));
        }
        let ic = Rect::new(r.x + 12 + n.indent, r.y + 7, 18, 18);
        if !icon(cv, clip, ic, &n.icon) {
            match n.glyph {
                Some(g) => glyph(cv, clip, ic, g, FG),
                None => ui::folder_shape(cv, clip, ic),
            }
        }
        text(cv, clip, ic.x1() + 10, r.y + 21, &n.label, 13.0, FG, r.x1() - ic.x1() - 16 - if n.eject { 28 } else { 0 });
        if n.eject {
            let e = Rect::new(r.x1() - 30, r.y + 4, 24, 24);
            if hov(app, Hit::Eject(i)) {
                cv.rrect(clip, e, 6, white(20));
            }
            glyph(cv, clip, e.inflate(-6), G::Eject, DIM);
        }
    }
}

// ------------------------------------------------------------------ items

pub fn cell_text(app: &App, it: &Item, c: Col) -> String {
    let _ = app;
    match c {
        Col::Name => it.disp.clone(),
        Col::Date => if it.e.mtime > 0 { model::date(it.e.mtime) } else { String::new() },
        Col::Type => it.kind.clone(),
        Col::Size => if it.e.dir { String::new() } else { model::size_kb(it.e.size) },
        Col::Orig => it.orig.as_ref().map(|o| o.display().to_string()).unwrap_or_default(),
        Col::Deleted => if it.deleted > 0 { model::date(it.deleted) } else { String::new() },
        Col::Packed => if it.e.dir || it.packed == 0 { String::new() } else { model::size_kb(it.packed) },
        Col::Folder => it.folder.clone(),
        Col::Perms => if it.e.mode == 0 { String::new() } else { super::perm_str(it.e.mode) },
        Col::Owner => if it.e.mode == 0 { String::new() } else { super::user_name(it.e.uid) },
    }
}

fn header(app: &App, cv: &mut Canvas) {
    let vr = app.view_rect();
    let clip = Rect::new(vr.x, TOP, vr.w, HEAD_H);
    for (i, (c, r)) in app.head_cols().iter().enumerate() {
        if hov(app, Hit::Head(i)) {
            cv.rrect(clip, r.inflate(-2), 6, white(12));
        }
        let x = if c.right() { r.x1() - 12 - tw(c.title(), 12.5) } else { r.x + 10 };
        let x = if *c == Col::Name { r.x + 36 } else { x };
        text(cv, clip, x, r.y + 21, c.title(), 12.5, DIM, r.w - 20);
        if app.view.sort == *c {
            // Windows: a small caret at the top centre of the sorted column.
            let g = Rect::new(r.x + r.w / 2 - 5, r.y + 1, 10, 10);
            if app.view.desc {
                glyph(cv, clip, g, G::ChevD, DIM);
            } else {
                // Up: the chevron flipped, by hand.
                let (cx, cy) = (g.x as f32 + 5.0, g.y as f32 + 5.0);
                cv.line(clip, (cx - 4.0, cy + 2.0), (cx, cy - 2.0), 1.5, DIM);
                cv.line(clip, (cx, cy - 2.0), (cx + 4.0, cy + 2.0), 1.5, DIM);
            }
        }
        cv.fill(clip, Rect::new(r.x1() - 1, r.y + 8, 1, r.h - 16), white(if hov(app, Hit::HeadEdge(i)) { 90 } else { 22 }));
    }
    cv.fill(clip, Rect::new(vr.x, TOP + HEAD_H - 1, vr.w, 1), white(14));
    // A header being dragged: where it would land, and the header under the pointer.
    if let Some((from, x)) = app.col_drag {
        let cols = app.head_cols();
        if let Some((c, r)) = cols.get(from).copied() {
            let to = app.col_drop_at(from, x);
            let lx = if to < from { cols[to].1.x } else { cols[to].1.x1() };
            cv.fill(clip, Rect::new(lx - 1, TOP + 4, 3, HEAD_H - 8), ui::accent(255));
            let g = Rect::new(x as i32 - r.w / 2, r.y + 2, r.w, r.h - 4);
            cv.rrect(clip, g, 6, BG_SOLID);
            cv.rrect(clip, g, 6, ui::accent(60));
            cv.rrect_stroke(clip, g, 6, 1.0, ui::accent_rgb(), 200, 200);
            text(cv, clip, g.x + 10, r.y + 21, c.title(), 12.5, FG, g.w - 20);
        }
    }
}

fn content(app: &App, cv: &mut Canvas) {
    let vr = app.view_rect();
    let clip = vr.intersect(&cv.bounds());
    if app.view.mode == Mode::Details {
        header(app, cv);
    }
    let sy = app.scroll.value.round() as i32;
    let off = |r: Rect| Rect::new(vr.x + r.x, vr.y + r.y - sy, r.w, r.h);
    if let Some(e) = &app.load_err {
        ui::text_c(cv, clip, Rect::new(vr.x, vr.y + 30, vr.w, 30), e, 13.5, DIM, W::Text);
        return;
    }
    if app.items.is_empty() && !app.desk {
        let msg = match app.loc() {
            Loc::Search(..) if app.searching => "Поиск…",
            Loc::Search(..) => "Ничего не найдено.",
            Loc::Trash => "Корзина пуста.",
            _ => "Эта папка пуста.",
        };
        ui::text_c(cv, clip, Rect::new(vr.x, vr.y + 24, vr.w, 30), msg, 13.0, DIM, W::Text);
    }
    for (r, label) in &app.heads {
        let r = off(*r);
        if !r.overlaps(&clip) {
            continue;
        }
        text_w(cv, clip, r.x + 6, r.y + 23, label, 13.5, FG, r.w - 20, W::Semi);
        let lw = ui::tw(label, 13.5) + 18;
        cv.fill(clip, Rect::new(r.x + lw, r.y + r.h / 2 + 2, r.w - lw - 6, 1), white(20));
    }
    // Hover, selection, focus drop.
    if app.hdrop.a.value > 0.01 {
        let a = (app.hdrop.a.value * if app.drag_to.is_some() { 60.0 } else { 14.0 }) as u32;
        let r = off(app.hdrop.rect());
        cv.rrect(clip, r, 7, if app.drag_to.is_some() { ui::accent(a) } else { white(a) });
    }
    let first = app.rects.iter().position(|r| r.y1() - sy >= 0).unwrap_or(0);
    for i in first..app.items.len() {
        let r = off(app.rects[i]);
        if r.y > clip.y1() {
            break;
        }
        if app.sel[i] && app.focus != Some(i) {
            cv.rrect(clip, r, 7, ui::selbg());
        }
    }
    if let Some(f) = app.focus.filter(|f| *f < app.items.len()) {
        let r = off(app.fdrop.rect());
        let a = app.fdrop.a.value.max(0.0);
        if app.sel[f] {
            cv.rrect(clip, r, 7, ui::accent((38.0 * a) as u32));
        }
        ui::drop(cv, clip, r, a * if app.sel[f] { 1.0 } else { 0.7 }, false);
    }
    let cut: Vec<&PathBuf> = app.clip.as_ref().filter(|c| c.1).map(|c| c.0.iter().collect()).unwrap_or_default();
    for i in first..app.items.len() {
        let r = off(app.rects[i]);
        if r.y > clip.y1() {
            break;
        }
        let it = &app.items[i];
        let faded = it.e.hidden || cut.contains(&&it.e.path);
        let renaming = app.rename.as_ref().filter(|x| x.0 == i).map(|x| &x.1);
        match app.view.mode {
            Mode::Details => row(app, cv, clip, r, it, faded, renaming),
            Mode::Icons(s) => {
                // Pictures and videos as themselves from the normal size up.
                let th = if s >= 48 && !it.e.dir && it.inner.is_empty() { thumbs::kind(&it.e.ext).and_then(|v| app.thumbs.borrow_mut().get(&it.e.path, it.e.mtime, v)) } else { None };
                tile_icon(cv, clip, r, s, it, faded, app.desk, renaming, th.as_deref())
            }
            Mode::Tiles => tile_pc(cv, clip, r, it),
        }
    }
    if let Some((x0, y0, x1, y1)) = app.band {
        let r = Rect::from_edges(x0.min(x1) as i32, (y0.min(y1)) as i32 - sy, x0.max(x1) as i32, (y0.max(y1)) as i32 - sy);
        cv.fill(clip, r, ui::accent(40));
        cv.rrect_stroke(clip, r, 2, 1.0, 0x7fb0ff, 200, 200);
    }
    if let Some((track, thumb)) = app.scrollbar() {
        let hot = hov(app, Hit::Scroll) || app.press.as_ref().map_or(false, |p| p.hit == Hit::Scroll);
        if hot {
            cv.rrect(clip, track, 5, white(10));
        }
        cv.rrect(clip, thumb, 3, white(if hot { 110 } else { 60 }));
    }
}

fn fade_col(c: u32, faded: bool) -> u32 {
    if faded { crate::render::fade(c, 130) } else { c }
}

fn row(app: &App, cv: &mut Canvas, clip: Rect, r: Rect, it: &Item, faded: bool, renaming: Option<&TextField>) {
    let cols = app.head_cols();
    let ic = Rect::new(r.x + 10, r.y + 6, 20, 20);
    item_icon(cv, clip, ic, it);
    if faded {
        cv.fill(clip, ic, argb(110, 23, 24, 31));
    }
    for (c, cr) in &cols {
        let cell = Rect::new(cr.x, r.y, cr.w, r.h).intersect(&clip);
        if *c == Col::Name {
            if let Some(tf) = renaming {
                let fr = Rect::new(r.x + 34, r.y + 3, cr.w - 38, r.h - 6);
                cv.rrect(clip, fr, 5, argb(255, 12, 13, 18));
                cv.rrect_stroke(clip, fr, 5, 1.0, 0x7fb0ff, 220, 220);
                tf.draw(cv, fr.intersect(&clip), ui::XF, fr.x + 6, r.y + 21, fr.w - 12, 13.0, FG, W::Text);
            } else {
                text(cv, cell, r.x + 38, r.y + 21, &it.disp, 13.0, fade_col(FG, faded), cr.x1() - r.x - 46);
            }
            continue;
        }
        let t = cell_text(app, it, *c);
        if t.is_empty() {
            continue;
        }
        let x = if c.right() { cr.x1() - 12 - tw(&t, 12.5) } else { cr.x + 10 };
        text(cv, cell, x, r.y + 21, &t, 12.5, fade_col(DIM, faded), cr.w - 20);
    }
}

/// Up to two lines of a name under an icon (the second ellipsized), as in Windows.
fn two_lines(s: &str, size: f32, w: i32) -> Vec<String> {
    if tw(s, size) <= w {
        return vec![s.to_string()];
    }
    let chars: Vec<char> = s.chars().collect();
    let mut cut = 0;
    let mut last_break = None;
    for i in 1..=chars.len() {
        let part: String = chars[..i].iter().collect();
        if tw(&part, size) > w {
            break;
        }
        cut = i;
        if matches!(chars[i - 1], ' ' | '-' | '_' | '.') {
            last_break = Some(i);
        }
    }
    let cut = last_break.filter(|b| *b > cut / 2).unwrap_or(cut.max(1));
    let a: String = chars[..cut].iter().collect();
    let b: String = chars[cut..].iter().collect();
    vec![a.trim_end().to_string(), b.trim_start().to_string()]
}

fn tile_icon(cv: &mut Canvas, clip: Rect, r: Rect, s: i32, it: &Item, faded: bool, wall: bool, renaming: Option<&TextField>, th: Option<&thumbs::Thumb>) {
    let ic = Rect::new(r.x + (r.w - s) / 2, r.y + 8, s, s);
    match th {
        Some(t) => preview(cv, clip, ic, t),
        None => item_icon(cv, clip, ic, it),
    }
    if faded {
        cv.fill(clip, ic, argb(110, 23, 24, 31));
    }
    let ty = ic.y1() + 18;
    if let Some(tf) = renaming {
        let fr = Rect::new(r.x + 2, ic.y1() + 4, r.w - 4, 22);
        cv.rrect(clip, fr, 5, argb(255, 12, 13, 18));
        cv.rrect_stroke(clip, fr, 5, 1.0, 0x7fb0ff, 220, 220);
        tf.draw(cv, fr.intersect(&clip), ui::XF, fr.x + 4, fr.y + 16, fr.w - 8, 12.5, FG, W::Text);
        return;
    }
    let lines = two_lines(&it.disp, 12.5, r.w - 8);
    for (k, l) in lines.iter().enumerate() {
        let lw = tw(l, 12.5).min(r.w - 8);
        let (x, y) = (r.x + (r.w - lw) / 2, ty + k as i32 * 17);
        if wall {
            // On the wallpaper: white with a soft shadow, as Windows does.
            text(cv, clip, x + 1, y + 1, l, 12.5, argb(170, 0, 0, 0), r.w - 8);
            text(cv, clip, x, y + 2, l, 12.5, argb(60, 0, 0, 0), r.w - 8);
            text(cv, clip, x, y, l, 12.5, fade_col(argb(255, 255, 255, 255), faded), r.w - 8);
        } else {
            text(cv, clip, x, y, l, 12.5, fade_col(FG, faded), r.w - 8);
        }
    }
}

/// A preview fitted into the icon's square, standing on its bottom edge, with a thin rim; a
/// video's gets a play mark.
fn preview(cv: &mut Canvas, clip: Rect, ic: Rect, t: &thumbs::Thumb) {
    let k = (ic.w as f64 / t.w as f64).min(ic.h as f64 / t.h as f64);
    let (w, h) = (((t.w as f64 * k).round() as i32).max(1), ((t.h as f64 * k).round() as i32).max(1));
    let d = Rect::new(ic.x + (ic.w - w) / 2, ic.y1() - h, w, h);
    cv.fill(clip, d.inflate(1), argb(90, 0, 0, 0));
    cv.blit(clip, d, &t.px, t.w, Rect::new(0, 0, t.w as i32, t.h as i32), true, 255);
    if t.video {
        let s = (ic.w / 4).clamp(12, 28);
        let b = Rect::new(d.x + (d.w - s) / 2, d.y + (d.h - s) / 2, s, s);
        cv.rrect(clip, b, s / 2, argb(150, 0, 0, 0));
        // A triangle pointing right, row by row.
        let (tx, th) = (b.x + s * 2 / 5, s / 2);
        for i in 0..th {
            let half = (th / 2 - (i - th / 2).abs()).max(0);
            cv.fill(clip, Rect::new(tx, b.y + s / 4 + i, half * 2 * 7 / 8 + 1, 1), argb(235, 255, 255, 255));
        }
    }
}

fn tile_pc(cv: &mut Canvas, clip: Rect, r: Rect, it: &Item) {
    let ic = Rect::new(r.x + 10, r.y + (r.h - 44) / 2, 44, 44);
    item_icon(cv, clip, ic, it);
    let x = ic.x1() + 12;
    let w = r.x1() - x - 12;
    match &it.drive {
        Some(m) => {
            text_w(cv, clip, x, r.y + 22, &it.disp, 13.0, FG, w, W::Semi);
            let bar = Rect::new(x, r.y + 32, w, 7);
            cv.rrect(clip, bar, 3, white(26));
            let used = if m.size > 0 { 1.0 - m.free as f64 / m.size as f64 } else { 0.0 };
            let col = if used > 0.9 { ui::BAD } else { ui::accent(255) };
            cv.rrect(clip, Rect::new(bar.x, bar.y, (bar.w as f64 * used).round() as i32, bar.h), 3, col);
            let t = format!("{} свободно из {}", model::size_short(m.free), model::size_short(m.size));
            text(cv, clip, x, r.y + 58, &t, 12.0, DIM, w);
        }
        None => {
            text_w(cv, clip, x, r.y + 25, &it.disp, 13.0, FG, w, W::Semi);
            text(cv, clip, x, r.y + 43, "Папка", 12.0, DIM, w);
        }
    }
}

// ------------------------------------------------------------------ status

fn status(app: &App, cv: &mut Canvas) {
    let y = app.h - STATUS_H;
    let clip = Rect::new(0, y, app.w, STATUS_H);
    cv.fill(cv.bounds(), Rect::new(0, y, app.w, 1), white(10));
    let n = app.items.len() as u64;
    let mut s = format!("{n} {}", model::plural(n, ["элемент", "элемента", "элементов"]));
    let sel = app.selected();
    if !sel.is_empty() {
        let k = sel.len() as u64;
        s.push_str(&format!("    Выбран{} {k} {}", if k == 1 { "" } else { "о" }, model::plural(k, ["элемент", "элемента", "элементов"])));
        if sel.iter().all(|i| !app.items[*i].e.dir) {
            let b: u64 = sel.iter().map(|i| app.items[*i].e.size).sum();
            s.push_str(&format!(": {}", model::size_short(b)));
        }
    }
    if app.searching {
        s.push_str("    Поиск…");
    }
    text(cv, clip, 14, y + 20, &s, 12.0, DIM, app.w - 120);
    let (a, b) = app.status_btns();
    let det = app.view.mode == Mode::Details;
    for (r, on, g, h) in [(a, det, G::Details, Hit::ViewDetails), (b, !det, G::Grid, Hit::ViewIcons)] {
        if on {
            cv.rrect(clip, r, 6, white(22));
        } else if hov(app, h) {
            cv.rrect(clip, r, 6, white(12));
        }
        glyph(cv, clip, Rect::new(r.x + 7, r.y + 4, 16, 16), g, if on { FG } else { DIM });
    }
}

// ------------------------------------------------------------------ floating

/// The picture under the pointer when items leave the window (the system drag).
pub fn drag_icon(app: &App) -> Option<(Vec<u32>, i32, i32, i32, i32)> {
    let sel = app.selected();
    let &first = sel.first()?;
    let (w, h) = (84, 66);
    let mut px = vec![0u32; (w * h) as usize];
    {
        let mut cv = Canvas { px: &mut px, w, h, stride: w as usize };
        let clip = cv.bounds();
        let ic = Rect::new(16, 20, 40, 40);
        card(&mut cv, clip, ic.inflate(4), 10, argb(200, 30, 32, 43));
        item_icon(&mut cv, clip, ic, &app.items[first]);
        if sel.len() > 1 {
            let t = sel.len().to_string();
            let bw = tw(&t, 11.0) + 12;
            let b = Rect::new(ic.x1() - bw / 2, ic.y - 10, bw, 18);
            cv.rrect(clip, b, 9, ui::accent(255));
            ui::text_c(&mut cv, clip, b, &t, 11.0, argb(255, 12, 22, 40), W::Bold);
        }
    }
    Some((px, w, h, 36, 34))
}

/// A tab being dragged out: the tab itself.
pub fn tab_icon(app: &App, i: usize) -> Option<(Vec<u32>, i32, i32, i32, i32)> {
    let t = app.tabs.get(i)?;
    let title = app.loc_title(&t.loc);
    let w = (tw(&title, 12.5) + 64).clamp(150, 280);
    let h = 40;
    let mut px = vec![0u32; (w * h) as usize];
    {
        let mut cv = Canvas { px: &mut px, w, h, stride: w as usize };
        let clip = cv.bounds();
        let r = Rect::new(2, 2, w - 4, h - 4);
        cv.rrect(clip, r, 9, argb(245, 40, 42, 55));
        cv.rrect_stroke(clip, r, 9, 1.0, 0xffffff, 50, 30);
        let ic = Rect::new(r.x + 12, r.y + 10, 16, 16);
        if !icon(&mut cv, clip, ic, &loc_icon(app, &t.loc)) {
            ui::folder_shape(&mut cv, clip, ic);
        }
        text(&mut cv, clip, ic.x1() + 9, r.y + 23, &title, 12.5, FG, r.w - 44);
    }
    Some((px, w, h, 60, 20))
}

/// Over the window during a system drag: only what a drop would do (the picture is the
/// compositor's).
fn drop_label(app: &App, cv: &mut Canvas) {
    let Some((x, y)) = app.ptr else { return };
    let Some((to, eff)) = &app.drag_to else { return };
    let name = app.loc_title(to);
    let label = match eff {
        Act::Trash => format!("Удалить в «{name}»"),
        Act::DropMove => format!("→ Переместить в «{name}»"),
        Act::DropLink => format!("↗ Создать ярлык в «{name}»"),
        _ => format!("+ Копировать в «{name}»"),
    };
    let clip = cv.bounds();
    let lw = tw(&label, 12.0) + 20;
    let lr = Rect::new(x as i32 + 8, y as i32 + 36, lw, 26);
    card(cv, clip, lr, 7, argb(235, 30, 32, 43));
    text(cv, clip, lr.x + 10, lr.y + 18, &label, 12.0, FG, lw);
}

fn drag_ghost(app: &App, cv: &mut Canvas) {
    if app.outgoing.is_some() || app.incoming.as_ref().map_or(false, |i| !i.own) {
        drop_label(app, cv);
        return;
    }
    let Some(p) = &app.press else { return };
    if !p.dragging {
        return;
    }
    let Some((x, y)) = app.ptr else { return };
    let sel = app.selected();
    let Some(&first) = sel.first() else { return };
    let clip = cv.bounds();
    // On the desktop: the cells the icons will land in.
    if app.desk && app.drag_to.is_none() && matches!(p.hit, Hit::Item(_)) {
        for (_, c) in app.desk_plan((p.x, p.y), (x, y)) {
            let r = app.screen(app.desk_cell_rect(c));
            cv.rrect(clip, r, 8, ui::accent(40));
        }
    }
    let (x, y) = (x as i32, y as i32);
    let ic = Rect::new(x - 20, y - 14, 40, 40);
    card(cv, clip, ic.inflate(4), 10, argb(180, 30, 32, 43));
    item_icon(cv, clip, ic, &app.items[first]);
    if sel.len() > 1 {
        let t = sel.len().to_string();
        let bw = tw(&t, 11.0) + 12;
        let b = Rect::new(ic.x1() - bw / 2, ic.y - 8, bw, 18);
        cv.rrect(clip, b, 9, ui::accent(255));
        ui::text_c(cv, clip, b, &t, 11.0, argb(255, 12, 22, 40), W::Bold);
    }
    let label = match &app.drag_to {
        Some((to, eff)) => {
            let name = app.loc_title(to);
            match eff {
                Act::Trash => format!("Удалить в «{name}»"),
                Act::DropMove => format!("→ Переместить в «{name}»"),
                Act::DropLink => format!("↗ Создать ярлык в «{name}»"),
                _ => format!("+ Копировать в «{name}»"),
            }
        }
        None => return,
    };
    let lw = tw(&label, 12.0) + 20;
    let lr = Rect::new(x + 8, ic.y1() + 10, lw, 26);
    card(cv, clip, lr, 7, argb(235, 30, 32, 43));
    text(cv, clip, lr.x + 10, lr.y + 18, &label, 12.0, FG, lw);
}

fn speed_str(b: f32) -> String {
    format!("{}/с", model::size_short(b.max(0.0) as u64))
}

fn left_str(s: f64) -> String {
    let s = s.max(1.0) as u64;
    if s < 60 {
        format!("{s} с")
    } else if s < 3600 {
        format!("{} мин", s.div_ceil(60))
    } else {
        format!("{} ч {} мин", s / 3600, (s % 3600) / 60)
    }
}

fn ops_card(app: &App, cv: &mut Canvas) {
    if app.ops.is_empty() {
        return;
    }
    let clip = cv.bounds();
    let (r, rows) = app.ops_rects();
    card(cv, clip, r, 12, argb(238, 28, 30, 40));
    let n = app.ops.len();
    let head = if n == 1 { app.ops[0].verb.to_string() } else { format!("Выполняется операций: {n}") };
    let total_pct: f32 = app.ops.iter().map(|o| {
        let p = o.prog.lock().unwrap();
        if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 }
    }).sum::<f32>() / n as f32;
    text_w(cv, clip, r.x + 16, r.y + 26, &format!("{head} — {}%", (total_pct * 100.0).floor()), 13.5, FG, r.w - 60, W::Semi);
    glyph(cv, clip, Rect::new(r.x1() - 30, r.y + 13, 14, 14), if app.ops_open { G::ChevD } else { G::ChevR }, DIM);
    if !app.ops_open {
        let bar = Rect::new(r.x + 16, r.y + 36, r.w - 32, 3);
        cv.rrect(clip, bar, 1, white(24));
        cv.rrect(clip, Rect::new(bar.x, bar.y, (bar.w as f32 * total_pct) as i32, 3), 1, ui::accent(255));
        return;
    }
    for (i, rr) in rows.iter().enumerate() {
        let op = &app.ops[i];
        let p = op.prog.lock().unwrap();
        let paused = op.pause.load(std::sync::atomic::Ordering::Relaxed);
        let frac = if p.total > 0 { (p.done as f64 / p.total as f64).min(1.0) } else { 0.0 };
        text(cv, clip, rr.x, rr.y + 16, &op.title, 12.0, DIM, rr.w - 72);
        for (k, (g, h)) in [(if paused { G::Play } else { G::Pause }, Hit::OpPause(i)), (G::Close, Hit::OpCancel(i))].into_iter().enumerate() {
            let b = Rect::new(rr.x1() - 64 + k as i32 * 34, rr.y, 30, 28);
            if hov(app, h) {
                cv.rrect(clip, b, 6, white(20));
            }
            glyph(cv, clip, b.inflate(-8), g, FG);
        }
        let big = if p.scanning { "Подсчёт…".to_string() } else if paused { format!("{}% — пауза", (frac * 100.0).floor()) } else { format!("{}% выполнено", (frac * 100.0).floor()) };
        text_w(cv, clip, rr.x, rr.y + 44, &big, 15.0, FG, rr.w, W::Semi);
        // The speed graph (Windows' green area; ours in the accent), only for byte copies.
        let g = Rect::new(rr.x, rr.y + 54, rr.w, 46);
        cv.rrect(clip, g, 5, white(8));
        let done_w = (g.w as f64 * frac).round() as i32;
        cv.rrect(clip, Rect::new(g.x, g.y, done_w, g.h), 5, ui::accent(26));
        if p.bytes && op.speed.len() > 1 {
            let max = op.speed.iter().cloned().fold(1.0f32, f32::max);
            let n = op.speed.len();
            let pts: Vec<(f32, f32)> = op.speed.iter().enumerate().map(|(k, s)| {
                let x = g.x as f32 + done_w as f32 * k as f32 / (n - 1) as f32;
                let y = g.y1() as f32 - 2.0 - (s / max) * (g.h - 8) as f32;
                (x, y)
            }).collect();
            // The area under the line: one column per pixel.
            for x in g.x..g.x + done_w {
                let fx = x as f32 + 0.5;
                let k = pts.partition_point(|p| p.0 < fx).clamp(1, pts.len() - 1);
                let (a, b) = (pts[k - 1], pts[k]);
                let t = if b.0 > a.0 { ((fx - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
                let y = a.1 + (b.1 - a.1) * t;
                cv.fill(clip, Rect::from_edges(x, y.round() as i32, x + 1, g.y1()), ui::accent(60));
            }
            for k in 1..pts.len() {
                cv.line(clip, pts[k - 1], pts[k], 1.5, ui::accent(255));
            }
        } else if !p.bytes {
            cv.rrect(clip, Rect::new(g.x, g.y1() - 3, done_w, 3), 1, ui::accent(255));
        }
        let mut info = String::new();
        if p.bytes && !p.scanning {
            let sp = op.speed.iter().rev().take(8).sum::<f32>() / op.speed.len().clamp(1, 8) as f32;
            info = format!("Скорость: {}", speed_str(sp));
            if sp > 1.0 && p.total > p.done {
                info.push_str(&format!("   Осталось: {}", left_str((p.total - p.done) as f64 / sp as f64)));
            }
        } else if !p.bytes && p.items_total > 0 && op.zip.is_none() {
            info = format!("{} из {}", p.items_done, p.items_total);
        }
        text(cv, clip, rr.x, rr.y + 118, &info, 12.0, DIM, rr.w);
        if !p.current.is_empty() {
            text(cv, clip, rr.x, rr.y + 136, &format!("Имя: {}", p.current), 12.0, DIM, rr.w);
        }
    }
}

fn toast(app: &App, cv: &mut Canvas) {
    let Some((t, _)) = &app.toast else { return };
    let clip = cv.bounds();
    let w = tw(t, 13.0) + 36;
    let r = Rect::new(NAV_W + (app.w - NAV_W - w) / 2, app.h - STATUS_H - 60, w, 38);
    card(cv, clip, r, 10, argb(240, 30, 32, 43));
    text(cv, clip, r.x + 18, r.y + 24, t, 13.0, FG, w);
}

// ------------------------------------------------------------------ menus

fn menu(app: &App, cv: &mut Canvas, m: &Menu) {
    let full = m.rect();
    let t = m.t.value.clamp(0.0, 1.2);
    // Windows 11: the menu unrolls from the top edge.
    let h = ((full.h as f32) * t.min(1.0)).round() as i32;
    let clip = Rect::new(full.x - 12, full.y - 12, full.w + 24, h + 24).intersect(&cv.bounds());
    if app.desk {
        // Over other windows (the desktop's overlay): solid, not see-through.
        cv.rrect(clip, full, 9, argb(255, 30, 32, 43));
    }
    card(cv, clip, full, 9, argb(244, 30, 32, 43));
    let inner = Rect::new(full.x, full.y, full.w, h);
    for (i, it) in m.items.iter().enumerate() {
        let r = m.item_rect(i);
        if it.is_sep() {
            cv.fill(inner, Rect::new(r.x + 8, r.y + 4, r.w - 16, 1), white(22));
            continue;
        }
        let hot = m.hot == Some(i) || m.sub.as_ref().map_or(false, |s| s.0 == i);
        if hot && it.on {
            cv.rrect(inner, r, 6, white(18));
        }
        let col = if it.on { FG } else { FAINT };
        let ic = Rect::new(r.x + 10, r.y + 8, 16, 16);
        if let Some(c) = it.check {
            if c {
                glyph(cv, inner, ic, G::Check, ui::accent(255));
            }
        } else if !it.icon.is_empty() {
            if !icon(cv, inner, ic, &it.icon) {
                if let Some(g) = it.glyph {
                    glyph(cv, inner, ic, g, col);
                }
            }
        } else if let Some(g) = it.glyph {
            glyph(cv, inner, ic, g, col);
        }
        let wt = if it.bold { W::Semi } else { W::Text };
        text_w(cv, inner, r.x + 36, r.y + 21, &it.label, 13.0, col, r.w - 60 - tw(&it.hint, 12.0), wt);
        if !it.hint.is_empty() {
            let hw = tw(&it.hint, 12.0);
            text(cv, inner, r.x1() - 14 - hw, r.y + 21, &it.hint, 12.0, FAINT, hw + 2);
        }
        if !it.sub.is_empty() {
            glyph(cv, inner, Rect::new(r.x1() - 20, r.y + 11, 10, 10), G::ChevR, DIM);
        }
    }
}

// ------------------------------------------------------------------ dialogs

pub struct DlgLayout {
    pub fields: Vec<Rect>,
    pub choices: Vec<Vec<Rect>>,
    pub check: Option<Rect>,
    pub text_y: i32,
}

fn wrap(s: &str, size: f32, w: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split(' ') {
        let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if tw(&cand, size) <= w || line.is_empty() {
            line = cand;
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn dialog_w(d: &Dialog) -> i32 {
    if d.wide { 560 } else { 460 }
}

fn text_lines(d: &Dialog) -> Vec<String> {
    let w = dialog_w(d) - 48 - if d.icon.is_empty() { 0 } else { 60 };
    d.lines.iter().flat_map(|l| wrap(l, 13.0, w)).collect()
}

pub fn dialog_rect(d: &Dialog, ww: i32, wh: i32) -> Rect {
    let w = dialog_w(d);
    let mut h = 64 + text_lines(d).len() as i32 * 20;
    if !d.icon.is_empty() {
        h = h.max(64 + 56);
    }
    h += d.fields.len() as i32 * 58 + d.choices.len() as i32 * 58;
    if d.check.is_some() {
        h += 34;
    }
    h += 72;
    Rect::new((ww - w) / 2, ((wh - h) / 2).max(10), w, h)
}

pub fn dialog_layout(d: &Dialog, r: Rect) -> DlgLayout {
    let mut y = r.y + 64 + text_lines(d).len() as i32 * 20;
    if !d.icon.is_empty() {
        y = y.max(r.y + 64 + 56);
    }
    let mut fields = Vec::new();
    let mut choices = Vec::new();
    // Fields and choices in the order: first field, choices, other fields (Add to archive).
    let mut fi = 0;
    if !d.fields.is_empty() {
        fields.push(Rect::new(r.x + 24, y + 22, r.w - 48, 32));
        y += 58;
        fi = 1;
    }
    for (_, opts, _) in &d.choices {
        let mut x = r.x + 24;
        let mut v = Vec::new();
        for o in opts {
            let w = tw(o, 12.5) + 22;
            v.push(Rect::new(x, y + 22, w, 30));
            x += w + 4;
        }
        choices.push(v);
        y += 58;
    }
    for _ in fi..d.fields.len() {
        fields.push(Rect::new(r.x + 24, y + 22, r.w - 48, 32));
        y += 58;
    }
    let check = d.check.as_ref().map(|_| {
        let c = Rect::new(r.x + 24, y + 6, r.w - 48, 24);
        y += 34;
        c
    });
    DlgLayout { fields, choices, check, text_y: r.y + 64 }
}

pub fn dialog_buttons(d: &Dialog, r: Rect) -> Vec<Rect> {
    let mut x = r.x1() - 24;
    let mut v = Vec::new();
    for b in d.buttons.iter().rev() {
        let w = (tw(b, 13.0) + 32).max(96);
        x -= w;
        v.push(Rect::new(x, r.y1() - 52, w, 34));
        x -= 8;
    }
    v.reverse();
    v
}

fn dialog(app: &App, cv: &mut Canvas, d: &Dialog) {
    let all = cv.bounds();
    let t = d.t.value.clamp(0.0, 1.0);
    if !app.desk {
        cv.fill(all, all, argb((90.0 * t) as u32, 0, 0, 0));
    }
    let r0 = dialog_rect(d, app.w, app.h);
    // Rises a little while it comes.
    let r = r0.translate(0, ((1.0 - d.t.value) * 18.0).round() as i32);
    if app.desk {
        // Over other windows: solid, like a window of its own.
        cv.rrect(all, r, 12, argb(255, 30, 32, 43));
    }
    card(cv, all, r, 12, argb(248, 30, 32, 43));
    cv.fill(all, Rect::new(r.x, r.y1() - 70, r.w, 1), white(14));
    cv.rrect(all, Rect::new(r.x + 1, r.y1() - 69, r.w - 2, 68), 11, argb(60, 0, 0, 0));
    text_w(cv, all, r.x + 24, r.y + 38, &d.title, 16.0, FG, r.w - 48, W::Semi);
    let mut tx = r.x + 24;
    if !d.icon.is_empty() {
        let ic = Rect::new(r.x + 24, r.y + 56, 48, 48);
        if !icon(cv, all, ic, &d.icon) {
            ui::page_shape(cv, all, ic, "");
        }
        tx += 60;
    }
    for (i, l) in text_lines(d).iter().enumerate() {
        text(cv, all, tx, r.y + 72 + i as i32 * 20, l, 13.0, argb(255, 0xd6, 0xd9, 0xe4), r.x1() - tx - 24);
    }
    let lay = dialog_layout(d, r0);
    let dy = r.y - r0.y;
    let mut fi_order: Vec<usize> = (0..d.fields.len()).collect();
    fi_order.sort();
    for (i, fr) in lay.fields.iter().enumerate() {
        let fr = fr.translate(0, dy);
        let f = &d.fields[i];
        text(cv, all, fr.x, fr.y - 7, &f.label, 12.0, DIM, fr.w);
        let focus = d.focus == i;
        cv.rrect(all, fr, 6, argb(255, 14, 15, 20));
        cv.rrect_stroke(all, fr, 6, 1.0, if focus { 0x7fb0ff } else { 0xffffff }, if focus { 220 } else { 40 }, if focus { 220 } else { 20 });
        if f.secret {
            let dots: String = "•".repeat(f.tf.len());
            text(cv, all, fr.x + 10, fr.y + 21, &dots, 13.0, FG, fr.w - 20);
            if focus {
                let cx = fr.x + 10 + tw(&dots, 13.0);
                cv.fill(all, Rect::new(cx, fr.y + 8, 2, 16), ui::accent(255));
            }
        } else if focus {
            f.tf.draw(cv, fr, ui::XF, fr.x + 10, fr.y + 21, fr.w - 20, 13.0, FG, W::Text);
        } else {
            text(cv, all, fr.x + 10, fr.y + 21, &f.tf.text(), 13.0, FG, fr.w - 20);
        }
    }
    for (ci, opts) in lay.choices.iter().enumerate() {
        let (label, names, chosen) = &d.choices[ci];
        if let Some(first) = opts.first() {
            text(cv, all, first.x, first.y + dy - 7, label, 12.0, DIM, r.w);
        }
        for (oi, orc) in opts.iter().enumerate() {
            let orc = orc.translate(0, dy);
            let on = *chosen == oi;
            cv.rrect(all, orc, 6, if on { ui::accent(60) } else { white(10) });
            if on {
                cv.rrect_stroke(all, orc, 6, 1.0, 0x7fb0ff, 200, 120);
            }
            ui::text_c(cv, all, orc, &names[oi], 12.5, if on { FG } else { DIM }, W::Text);
        }
    }
    if let (Some(cr), Some((label, on))) = (lay.check, &d.check) {
        let cr = cr.translate(0, dy);
        let b = Rect::new(cr.x, cr.y + 3, 18, 18);
        cv.rrect(all, b, 4, if *on { ui::accent(255) } else { white(10) });
        cv.rrect_stroke(all, b, 4, 1.2, 0xffffff, if *on { 0 } else { 110 }, if *on { 0 } else { 110 });
        if *on {
            glyph(cv, all, b.inflate(-3), G::Check, argb(255, 12, 22, 40));
        }
        text(cv, all, cr.x + 28, cr.y + 17, label, 13.0, FG, cr.w - 30);
    }
    for (i, b) in dialog_buttons(d, r0).iter().enumerate() {
        let b = b.translate(0, dy);
        let def = i == d.default;
        let hot = d.hot == Some(i);
        if def {
            cv.rrect(all, b, 7, if hot { crate::look::accent_light(0.25) } else { ui::accent(255) });
            ui::text_c(cv, all, b, &d.buttons[i], 13.0, argb(255, 12, 22, 40), W::Semi);
        } else {
            cv.rrect(all, b, 7, white(if hot { 30 } else { 18 }));
            cv.rrect_stroke(all, b, 7, 1.0, 0xffffff, 40, 16);
            ui::text_c(cv, all, b, &d.buttons[i], 13.0, FG, W::Text);
        }
    }
    let _ = lay.text_y;
}
