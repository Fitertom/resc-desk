//! «Пуск», as in the Liquify demo: a search field, the pinned apps and the recently
//! started ones on the home page, every app (scrolling) on the second page, the user
//! and a power button at the bottom. Right click pins / unpins an app. Pins and the
//! recent list live in ~/.config/resc-desk/start.conf.
use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::shell::{self, col};

const COLS: i32 = 6;
const CELL_W: i32 = 100;
const CELL_H: i32 = 88;
const GRID_TOP: i32 = 108;
const FOOT_H: i32 = 60;
const PIN_ROWS: i32 = 2;
const REC_H: i32 = 52;
const REC_MAX: usize = 6;
pub const RADIUS: i32 = 20;
/// Headings and the recent list stand this far in from the glass's edge (its rim bends 16 px
/// of what is behind it: text nearer looks squeezed against it).
const MARGIN: i32 = 48;

#[derive(Default)]
pub struct Prefs {
    pub loaded: bool,
    /// Desktop ids.
    pub pinned: Vec<String>,
    /// Desktop id and when it was started (unix seconds), newest first.
    pub recent: Vec<(String, u64)>,
}

#[derive(Default)]
pub struct Start {
    pub prefs: Prefs,
    /// The "all apps" page.
    pub all: bool,
    /// Scroll offset of the all-apps grid (logical px).
    pub scroll: i32,
    /// The power menu is open.
    pub power: bool,
    /// The selection was moved with the keyboard (show it).
    pub keyboard: bool,
    /// Opening or closing (the GPU path): the menu as a drop out of the start button.
    pub anim: Option<crate::startanim::Anim>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum It {
    App(usize),
    Recent(usize),
    All,
    Back,
    Power,
    Gear,
    Act(u8),
}

impl It {
    fn nav(self) -> bool {
        matches!(self, It::App(_) | It::Recent(_))
    }
}

fn conf_path() -> String {
    let base = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    format!("{base}/resc-desk/start.conf")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn load_prefs(d: &mut Desk) {
    if d.start.prefs.loaded {
        return;
    }
    let mut p = Prefs { loaded: true, ..Default::default() };
    match std::fs::read_to_string(conf_path()) {
        Ok(text) => {
            for line in text.lines() {
                let mut it = line.split_whitespace();
                match (it.next(), it.next()) {
                    (Some("pin"), Some(id)) => p.pinned.push(id.to_string()),
                    (Some("recent"), Some(id)) => p.recent.push((id.to_string(), it.next().and_then(|t| t.parse().ok()).unwrap_or(0))),
                    _ => {}
                }
            }
        }
        Err(_) => {
            // First start: the usual suspects that are installed.
            let term = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
            let wanted = [
                term.as_str(),
                "org.gnome.Nautilus",
                "thunar",
                "org.kde.dolphin",
                "pcmanfm",
                "firefox",
                "chromium",
                "google-chrome",
                "code",
                "steam",
                "org.gnome.TextEditor",
                "resc.settings",
            ];
            for w in wanted {
                if d.apps.iter().any(|a| a.id == w) && p.pinned.len() < (COLS * PIN_ROWS) as usize {
                    p.pinned.push(w.to_string());
                }
            }
        }
    }
    d.start.prefs = p;
}

fn save_prefs(d: &Desk) {
    let p = &d.start.prefs;
    let mut s = String::from("# resc-desk start menu: pinned apps and recently started ones\n");
    for id in &p.pinned {
        s.push_str(&format!("pin {id}\n"));
    }
    for (id, t) in &p.recent {
        s.push_str(&format!("recent {id} {t}\n"));
    }
    let path = conf_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, s);
}

fn app_index(d: &Desk, id: &str) -> Option<usize> {
    d.apps.iter().position(|a| a.id == id)
}

/// Called when the menu opens.
pub fn opened(d: &mut Desk) {
    load_prefs(d);
    d.start.all = false;
    d.start.scroll = 0;
    d.start.power = false;
    d.start.keyboard = false;
}

pub fn rect(d: &Desk, oi: usize) -> Rect {
    let wa = crate::wm::workarea(d, oi);
    let (w, h) = (660.min(wa.w - 24), 600.min(wa.h - 24));
    Rect::new(wa.x + (wa.w - w) / 2, wa.y1() - h - 8, w, h)
}

fn searching(d: &Desk) -> bool {
    !d.start_field.is_empty()
}

fn filtered_apps(d: &Desk) -> Vec<usize> {
    let q = d.start_field.text().to_lowercase();
    let mut v: Vec<usize> = (0..d.apps.len()).filter(|i| q.is_empty() || d.apps[*i].name.to_lowercase().contains(&q)).collect();
    if !q.is_empty() {
        v.sort_by_key(|i| !d.apps[*i].name.to_lowercase().starts_with(&q));
    }
    v
}

/// Where the scrolling grid lives (all apps / search results).
fn grid_area(r: Rect) -> Rect {
    Rect::new(r.x + 10, r.y + GRID_TOP, r.w - 20, r.h - GRID_TOP - FOOT_H - 6)
}

fn grid_cells(r: Rect, n: usize, y0: i32) -> impl Iterator<Item = Rect> {
    let gx = r.x + (r.w - COLS * CELL_W) / 2;
    (0..n as i32).map(move |i| Rect::new(gx + (i % COLS) * CELL_W, y0 + (i / COLS) * CELL_H, CELL_W, CELL_H))
}

pub fn power_btn(r: Rect) -> Rect {
    Rect::new(r.x1() - 22 - 36, r.y1() - FOOT_H + 12, 36, 36)
}

pub fn gear_btn(r: Rect) -> Rect {
    power_btn(r).translate(-46, 0)
}

/// The search pill (logical).
pub fn search_rect(r: Rect) -> Rect {
    Rect::new(r.x + 22, r.y + 22, r.w - 44, 42)
}

/// The user's avatar in the footer (logical).
pub fn avatar_rect(r: Rect) -> Rect {
    Rect::new(r.x + 24, r.y1() - FOOT_H + 15, 30, 30)
}

const ACTS: [&str; 3] = ["Выйти из сеанса", "Перезагрузка", "Завершение работы"];

fn power_menu(r: Rect) -> Rect {
    let b = power_btn(r);
    let h = ACTS.len() as i32 * 36 + 12;
    Rect::new(b.x1() - 210, b.y - 8 - h, 210, h)
}

/// Every clickable thing in the menu, in reading order.
pub fn items(d: &Desk, oi: usize) -> Vec<(It, Rect)> {
    let r = rect(d, oi);
    let mut v = Vec::new();
    if d.start.power {
        let m = power_menu(r);
        for i in 0..ACTS.len() {
            v.push((It::Act(i as u8), Rect::new(m.x + 6, m.y + 6 + i as i32 * 36, m.w - 12, 36)));
        }
    }
    if searching(d) || d.start.all {
        if !searching(d) {
            v.push((It::Back, Rect::new(r.x1() - MARGIN - 96, r.y + 80, 96, 24)));
        }
        let y0 = r.y + GRID_TOP - d.start.scroll;
        let apps = filtered_apps(d);
        for (a, c) in apps.iter().zip(grid_cells(r, apps.len(), y0)) {
            v.push((It::App(*a), c));
        }
    } else {
        v.push((It::All, Rect::new(r.x1() - MARGIN - 150, r.y + 80, 150, 24)));
        let pins: Vec<usize> = d.start.prefs.pinned.iter().filter_map(|id| app_index(d, id)).take((COLS * PIN_ROWS) as usize).collect();
        for (a, c) in pins.iter().zip(grid_cells(r, pins.len(), r.y + GRID_TOP)) {
            v.push((It::App(*a), c));
        }
        let ry = r.y + GRID_TOP + PIN_ROWS * CELL_H + 42;
        let cw = (r.w - 2 * (MARGIN - 10)) / 2;
        let recent: Vec<usize> = d.start.prefs.recent.iter().filter_map(|(id, _)| app_index(d, id)).take(REC_MAX).collect();
        for (i, a) in recent.into_iter().enumerate() {
            let (cx, cy) = (i as i32 % 2, i as i32 / 2);
            v.push((It::Recent(a), Rect::new(r.x + MARGIN - 10 + cx * cw, ry + cy * REC_H, cw - 4, REC_H - 4)));
        }
    }
    v.push((It::Gear, gear_btn(r)));
    v.push((It::Power, power_btn(r)));
    v
}

/// The item under (x, y); grid cells count only inside the visible grid.
fn item_at(d: &Desk, oi: usize, x: f64, y: f64) -> Option<usize> {
    let r = rect(d, oi);
    let area = grid_area(r);
    let scrolling = searching(d) || d.start.all;
    let its = items(d, oi);
    // The power menu lies over everything else.
    if let Some(i) = its.iter().position(|(it, rr)| matches!(it, It::Act(_)) && rr.contains(x, y)) {
        return Some(i);
    }
    its.iter().position(|(it, rr)| rr.contains(x, y) && !(scrolling && matches!(it, It::App(_)) && !area.contains(x, y)))
}

fn nav_items(d: &Desk, oi: usize) -> Vec<(It, Rect)> {
    items(d, oi).into_iter().filter(|(i, _)| i.nav()).collect()
}

fn ago(t: u64) -> String {
    let s = now_secs().saturating_sub(t);
    match s {
        0..=59 => "только что".into(),
        60..=3599 => format!("{} мин назад", s / 60),
        3600..=86399 => format!("{} ч назад", s / 3600),
        86400..=172799 => "вчера".into(),
        _ => format!("{} дн назад", s / 86400),
    }
}

fn pill_button(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, text: &str, hot: bool) {
    cv.rrect(clip, xf.r(r), xf.len(r.h / 2), if hot { argb(40, 255, 255, 255) } else { argb(18, 255, 255, 255) });
    let tw = font::text_width_w(text, 12.0, font::W::Semi);
    font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + 16, text, 12.0, col::TEXT, r.w, font::W::Semi);
}

fn draw_cell(d: &Desk, cv: &mut Canvas, clip: Rect, xf: Xf, cell: Rect, app: usize, hot: bool, sel: bool) {
    let sc = xf.scale as f32;
    if hot || sel {
        cv.rrect(clip, xf.r(cell.inflate(-3)), xf.len(12), argb(22, 255, 255, 255));
    }
    if sel {
        cv.rrect_stroke(clip, xf.r(cell.inflate(-3)), xf.len(12), 1.5 * sc, crate::look::accent() & 0xff_ffff, 200, 200);
    }
    let a = &d.apps[app];
    let icon = Rect::new(cell.x + (cell.w - 40) / 2, cell.y + 10, 40, 40);
    shell::draw_app_tile(cv, clip, xf, icon, &a.id, &a.name);
    let name = shell::ellipsize(&a.name, 12.0, font::W::Semi, cell.w - 10);
    let tw = font::text_width_w(&name, 12.0, font::W::Semi);
    font::draw_text_w(cv, clip, xf, cell.x + (cell.w - tw) / 2, cell.y + 70, &name, 12.0, col::TEXT, cell.w, font::W::Semi);
}

/// The whole menu, as one picture (the CPU path; on the GPU path the overlays' layer, where
/// the menu is a drop of glass with drops on it: startanim.rs draws it, layer by layer).
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if shell::glass_on_gpu() {
        crate::startanim::draw_layer1(d, oi, xf, cv, clip);
        return;
    }
    let r = rect(d, oi);
    shell::draw_glass(cv, clip, xf, r, RADIUS);
    let sc = xf.scale as f32;
    let sb = search_rect(r);
    cv.rrect(clip, xf.r(sb), xf.len(21), argb(20, 255, 255, 255));
    cv.rrect_stroke(clip, xf.r(sb), xf.len(21), sc.max(1.0), 0xffffff, 34, 22);
    draw_search(d, sb, xf, cv, clip);
    draw_content(d, oi, xf, cv, clip);
    let av = avatar_rect(r);
    cv.rrect(clip, xf.r(av), xf.len(15), col::accent());
    draw_initial(cv, clip, xf, av);
    draw_user(cv, clip, xf, av.x1() + 12, av.y + 20);
    draw_footer_buttons(d, oi, xf, cv, clip, true);
    draw_power_menu(d, oi, xf, cv, clip);
}

/// What lies on the search pill `sb`: the magnifier, the placeholder or the query.
pub fn draw_search(d: &Desk, sb: Rect, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let sc = xf.scale as f32;
    let (mx, my) = (sb.x as f32 + 22.0, sb.y as f32 + 20.0);
    cv.ring(clip, shell::phys(xf, mx, my), 6.0 * sc, 1.8 * sc, col::TEXT_DIM);
    cv.line(clip, shell::phys(xf, mx + 4.5, my + 4.5), shell::phys(xf, mx + 9.0, my + 9.0), 1.8 * sc, col::TEXT_DIM);
    if d.start_field.is_empty() {
        font::draw_text_w(cv, clip, xf, sb.x + 42, sb.y + 26, "Поиск приложений", 14.0, col::TEXT_DIM, sb.w - 60, font::W::Semi);
    }
    d.start_field.draw(cv, clip, xf, sb.x + 42, sb.y + 26, sb.w - 60, 14.0, col::TEXT, font::W::Semi);
}

pub fn user_name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".into())
}

/// The user's name, its baseline's left end at (x, y).
pub fn draw_user(cv: &mut Canvas, clip: Rect, xf: Xf, x: i32, y: i32) {
    font::draw_text_w(cv, clip, xf, x, y, &user_name(), 13.0, col::TEXT, 220, font::W::Semi);
}

/// The initial on the avatar `av`.
pub fn draw_initial(cv: &mut Canvas, clip: Rect, xf: Xf, av: Rect) {
    let ini: String = user_name().chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let iw = font::text_width_w(&ini, 13.0, font::W::Bold);
    font::draw_text_w(cv, clip, xf, av.x + (av.w - iw) / 2, av.y + av.h / 2 + 5, &ini, 13.0, col::accent_ink(), av.w.max(1), font::W::Bold);
}

/// Gear and power: their glyphs, lit when hovered (power also while its menu is open), with
/// their round backs when `backs` (the CPU path; on the GPU path the backs are drops).
pub fn draw_footer_buttons(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect, backs: bool) {
    let sc = xf.scale as f32;
    for (i, (it, b)) in items(d, oi).into_iter().enumerate() {
        let hot = d.start_hover == Some(i);
        let on = match it {
            It::Gear => hot,
            It::Power => hot || d.start.power,
            _ => continue,
        };
        if backs {
            cv.rrect(clip, xf.r(b), xf.len(18), if on { argb(40, 255, 255, 255) } else { argb(16, 255, 255, 255) });
            cv.rrect_stroke(clip, xf.r(b), xf.len(18), sc.max(1.0), 0xffffff, 40, 20);
        } else if on {
            cv.rrect(clip, xf.r(b), xf.len(18), argb(26, 255, 255, 255));
        }
        if it == It::Gear {
            draw_gear_glyph(cv, clip, xf, b, col::TEXT);
        } else {
            draw_power_glyph(cv, clip, xf, b, col::TEXT);
        }
    }
}

/// The power menu, when open.
pub fn draw_power_menu(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if !d.start.power {
        return;
    }
    let m = power_menu(rect(d, oi));
    cv.rrect(clip, xf.r(m.inflate(1)), xf.len(13), argb(60, 255, 255, 255));
    cv.rrect(clip, xf.r(m), xf.len(12), 0xf01c_1a26);
    for (i, (it, rr)) in items(d, oi).into_iter().enumerate() {
        if let It::Act(k) = it {
            if d.start_hover == Some(i) {
                let c = if k == 2 { argb(220, 225, 59, 59) } else { argb(30, 255, 255, 255) };
                cv.rrect(clip, xf.r(rr), xf.len(8), c);
            }
            font::draw_text_w(cv, clip, xf, rr.x + 12, rr.y + 23, ACTS[k as usize], 13.0, col::TEXT, rr.w - 20, font::W::Semi);
        }
    }
}

/// What lies on the menu's glass itself: the headings, the grid, the recent list, the
/// footer's line (not the search, the avatar and the footer's buttons: drops on the GPU path).
pub fn draw_content(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let r = rect(d, oi);
    let sc = xf.scale as f32;
    let its = items(d, oi);
    let nav: Vec<usize> = (0..its.len()).filter(|i| its[*i].0.nav()).collect();
    let sel_item = nav.get(d.start_sel).copied().filter(|_| searching(d) || d.start.keyboard);
    let hot = |i: usize| d.start_hover == Some(i);
    let scrolling = searching(d) || d.start.all;
    let head = if searching(d) {
        "Лучшие совпадения"
    } else if d.start.all {
        "Все приложения"
    } else {
        "Закреплённые"
    };
    font::draw_text_w(cv, clip, xf, r.x + MARGIN, r.y + 97, head, 14.0, col::TEXT, 300, font::W::Bold);
    if searching(d) {
        let count = format!("{}", filtered_apps(d).len());
        let cw = font::text_width(&count, 12.0);
        font::draw_text(cv, clip, xf, r.x1() - MARGIN - cw, r.y + 97, &count, 12.0, col::TEXT_DIM, 80);
    }
    if scrolling {
        let area = grid_area(r);
        let gclip = clip.intersect(&xf.r(area));
        let mut any = false;
        for (i, (it, cell)) in its.iter().enumerate() {
            match it {
                It::App(a) => {
                    any = true;
                    if xf.r(*cell).overlaps(&gclip) {
                        draw_cell(d, cv, gclip, xf, *cell, *a, hot(i), sel_item == Some(i));
                        if d.start.prefs.pinned.iter().any(|p| *p == d.apps[*a].id) {
                            let dot = Rect::new(cell.x1() - 22, cell.y + 10, 6, 6);
                            cv.rrect(gclip, xf.r(dot), xf.len(3), col::accent());
                        }
                    }
                }
                It::Back => pill_button(cv, clip, xf, *cell, "‹  Назад", hot(i)),
                _ => {}
            }
        }
        if !any {
            let msg = if d.apps.is_empty() { "Нет .desktop-файлов в /usr/share/applications" } else { "Ничего не найдено. Enter запустит набранную команду" };
            font::draw_text(cv, clip, xf, r.x + MARGIN, r.y + GRID_TOP + 30, msg, 13.0, col::TEXT_DIM, r.w - 2 * MARGIN);
        }
        // Scroll bar.
        let n = filtered_apps(d).len() as i32;
        let total = (n + COLS - 1) / COLS * CELL_H;
        if total > area.h {
            let th = (area.h * area.h / total).max(24);
            let ty = area.y + (area.h - th) * d.start.scroll / (total - area.h).max(1);
            cv.rrect(clip, xf.r(Rect::new(r.x1() - 9, ty, 4, th)), xf.len(2), argb(70, 255, 255, 255));
        }
    } else {
        let mut pins = 0;
        let mut recents = 0;
        for (i, (it, cell)) in its.iter().enumerate() {
            match it {
                It::All => pill_button(cv, clip, xf, *cell, "Все приложения  ›", hot(i)),
                It::App(a) => {
                    pins += 1;
                    draw_cell(d, cv, clip, xf, *cell, *a, hot(i), sel_item == Some(i));
                }
                It::Recent(a) => {
                    recents += 1;
                    if hot(i) || sel_item == Some(i) {
                        cv.rrect(clip, xf.r(*cell), xf.len(12), argb(22, 255, 255, 255));
                    }
                    if sel_item == Some(i) {
                        cv.rrect_stroke(clip, xf.r(*cell), xf.len(12), 1.5 * sc, crate::look::accent() & 0xff_ffff, 200, 200);
                    }
                    let app = &d.apps[*a];
                    let ic = Rect::new(cell.x + 10, cell.y + 7, 34, 34);
                    shell::draw_app_tile(cv, clip, xf, ic, &app.id, &app.name);
                    let tmax = cell.w - 60;
                    let name = shell::ellipsize(&app.name, 13.0, font::W::Semi, tmax);
                    font::draw_text_w(cv, clip, xf, ic.x1() + 12, cell.y + 21, &name, 13.0, col::TEXT, tmax, font::W::Semi);
                    let t = d.start.prefs.recent.iter().find(|(id, _)| *id == app.id).map_or(0, |e| e.1);
                    font::draw_text(cv, clip, xf, ic.x1() + 12, cell.y + 38, &ago(t), 11.5, col::TEXT_DIM, tmax);
                }
                _ => {}
            }
        }
        if pins == 0 {
            font::draw_text(cv, clip, xf, r.x + MARGIN, r.y + GRID_TOP + 30, "Правый клик по приложению закрепит его здесь", 13.0, col::TEXT_DIM, r.w - 2 * MARGIN);
        }
        let ry = r.y + GRID_TOP + PIN_ROWS * CELL_H;
        font::draw_text_w(cv, clip, xf, r.x + MARGIN, ry + 26, "Недавние", 14.0, col::TEXT, 300, font::W::Bold);
        if recents == 0 {
            font::draw_text(cv, clip, xf, r.x + MARGIN, ry + 64, "Здесь появятся приложения, которые вы запускаете", 13.0, col::TEXT_DIM, r.w - 2 * MARGIN);
        }
    }
    // The footer's line.
    cv.fill(clip, xf.r(Rect::new(r.x + 1, r.y1() - FOOT_H, r.w - 2, 1)), argb(26, 255, 255, 255));
}

/// The things of `draw_content`, each on its own (logical): the ones that come in one by one
/// as the menu opens (startanim.rs).
pub fn wave_items(d: &Desk, oi: usize) -> Vec<Rect> {
    let r = rect(d, oi);
    let mut v = vec![Rect::new(r.x + MARGIN, r.y + 80, 300, 24)];
    if searching(d) {
        v.push(Rect::new(r.x1() - MARGIN - 80, r.y + 80, 80, 24));
    }
    let scrolling = searching(d) || d.start.all;
    let area = grid_area(r);
    let its = items(d, oi);
    let mut apps = 0;
    for (it, rr) in &its {
        match it {
            It::App(_) => {
                apps += 1;
                if !scrolling || rr.overlaps(&area) {
                    v.push(*rr);
                }
            }
            It::Recent(_) | It::All | It::Back => v.push(*rr),
            _ => {}
        }
    }
    let hint = Rect::new(r.x + MARGIN, r.y + GRID_TOP + 10, r.w - 2 * MARGIN, 28);
    if scrolling {
        if apps == 0 {
            v.push(hint);
        }
        v.push(Rect::new(r.x1() - 9, area.y, 4, area.h));
    } else {
        if apps == 0 {
            v.push(hint);
        }
        let ry = r.y + GRID_TOP + PIN_ROWS * CELL_H;
        v.push(Rect::new(r.x + MARGIN, ry + 6, 300, 26));
        if !its.iter().any(|(it, _)| matches!(it, It::Recent(_))) {
            v.push(Rect::new(r.x + MARGIN, ry + 44, r.w - 2 * MARGIN, 28));
        }
    }
    v.push(Rect::new(r.x, r.y1() - FOOT_H, r.w, 1));
    v
}


/// ⚙ without an icon theme: a ring with eight teeth.
pub fn draw_gear_glyph(cv: &mut Canvas, clip: Rect, xf: Xf, b: Rect, c: u32) {
    let sc = xf.scale as f32;
    let (cx, cy) = (b.x as f32 + b.w as f32 / 2.0, b.y as f32 + b.h as f32 / 2.0);
    cv.ring(clip, shell::phys(xf, cx, cy), 5.0 * sc, 2.0 * sc, c);
    for k in 0..8 {
        let a = std::f32::consts::TAU * k as f32 / 8.0;
        let (s, co) = a.sin_cos();
        cv.line(clip, shell::phys(xf, cx + 6.0 * co, cy + 6.0 * s), shell::phys(xf, cx + 8.5 * co, cy + 8.5 * s), 2.2 * sc, c);
    }
}

/// ⏻: an open ring with a bar through the gap.
pub fn draw_power_glyph(cv: &mut Canvas, clip: Rect, xf: Xf, b: Rect, c: u32) {
    let sc = xf.scale as f32;
    let (cx, cy) = (b.x as f32 + b.w as f32 / 2.0, b.y as f32 + b.h as f32 / 2.0 + 1.0);
    let rr = 6.5f32;
    let n = 28;
    let mut prev: Option<(f32, f32)> = None;
    for i in 0..=n {
        // From 40° right of the top, round to 40° left of it.
        let a = (-90.0f32 + 40.0 + 280.0 * i as f32 / n as f32).to_radians();
        let p = (cx + rr * a.cos(), cy + rr * a.sin());
        if let Some(q) = prev {
            cv.line(clip, shell::phys(xf, q.0, q.1), shell::phys(xf, p.0, p.1), 1.8 * sc, c);
        }
        prev = Some(p);
    }
    cv.line(clip, shell::phys(xf, cx, cy - 9.0), shell::phys(xf, cx, cy - 2.0), 1.8 * sc, c);
}

fn damage(d: &mut Desk) {
    if let Some(oi) = d.overlay.start_menu {
        let r = rect(d, oi);
        d.damage(r);
    }
}

/// Starts app `idx`, clicked at `cell` (its item in the menu): it comes out of the icon there
/// (life.rs).
pub fn launch(d: &mut Desk, idx: usize, cell: Option<(It, Rect)>) {
    let (cmd, id) = (d.apps[idx].exec.clone(), d.apps[idx].id.clone());
    load_prefs(d);
    let rec = &mut d.start.prefs.recent;
    rec.retain(|(r, _)| *r != id);
    rec.insert(0, (id, now_secs()));
    rec.truncate(REC_MAX * 2);
    save_prefs(d);
    shell::close_menus(d);
    let origin = cell.map(|(it, c)| match it {
        It::Recent(_) => Rect::new(c.x + 10, c.y + 7, 34, 34),
        _ => Rect::new(c.x + (c.w - 40) / 2, c.y + 10, 40, 40),
    });
    match cmd.as_str() {
        "@settings" => shell::open_settings(d),
        "@files" => shell::open_files(d, origin),
        _ => {
            let (icon, wm) = (d.apps[idx].id.clone(), d.apps[idx].wmclass.clone());
            let bin = cmd.split_whitespace().find(|w| !w.contains('=')).unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
            crate::life::launch(d, &cmd, &icon, &[&icon, &wm, &bin], origin);
        }
    }
}

fn toggle_pin(d: &mut Desk, idx: usize) {
    let id = d.apps[idx].id.clone();
    let p = &mut d.start.prefs.pinned;
    if let Some(i) = p.iter().position(|x| *x == id) {
        p.remove(i);
    } else if p.len() < (COLS * PIN_ROWS) as usize {
        p.push(id);
    }
    save_prefs(d);
    damage(d);
}

fn power_action(d: &mut Desk, k: u8) {
    shell::close_menus(d);
    match k {
        0 => d.running = false,
        1 => d.spawn("systemctl reboot"),
        _ => d.spawn("systemctl poweroff"),
    }
}

/// A click while the menu is open on output `oi`; true if it was inside the menu.
pub fn click(d: &mut Desk, oi: usize, x: f64, y: f64, button: u32) -> bool {
    let r = rect(d, oi);
    let hit_cell = item_at(d, oi, x, y).map(|i| items(d, oi)[i]);
    let hit = hit_cell.map(|c| c.0);
    let in_menu = d.start.power && power_menu(r).contains(x, y);
    if !r.contains(x, y) && !in_menu {
        return false;
    }
    let was_power = d.start.power;
    d.start.power = false;
    let right = button == crate::input::BTN_RIGHT;
    match hit {
        Some(It::App(a)) | Some(It::Recent(a)) if right => toggle_pin(d, a),
        Some(It::App(a)) | Some(It::Recent(a)) if !was_power => launch(d, a, hit_cell),
        Some(It::All) => {
            d.start.all = true;
            d.start.scroll = 0;
            d.start_sel = 0;
        }
        Some(It::Back) => {
            d.start.all = false;
            d.start_sel = 0;
        }
        Some(It::Power) => d.start.power = !was_power,
        Some(It::Gear) => {
            shell::open_settings(d);
            return true;
        }
        Some(It::Act(k)) => power_action(d, k),
        _ => {}
    }
    d.start_hover = None;
    damage(d);
    true
}

pub fn hover(d: &mut Desk, x: f64, y: f64) {
    let Some(oi) = d.overlay.start_menu else { return };
    let hit = item_at(d, oi, x, y);
    if hit != d.start_hover {
        d.start_hover = hit;
        d.start.keyboard = false;
        damage(d);
    }
}

fn max_scroll(d: &Desk, oi: usize) -> i32 {
    let area = grid_area(rect(d, oi));
    let n = filtered_apps(d).len() as i32;
    ((n + COLS - 1) / COLS * CELL_H - area.h).max(0)
}

/// Wheel over the menu: scrolls the all-apps grid. True if the menu took it.
pub fn scroll(d: &mut Desk, x: f64, y: f64, value: f64) -> bool {
    let Some(oi) = d.overlay.start_menu else { return false };
    if !rect(d, oi).contains(x, y) {
        return false;
    }
    if searching(d) || d.start.all {
        let s = (d.start.scroll + (value * 4.0).round() as i32).clamp(0, max_scroll(d, oi));
        if s != d.start.scroll {
            d.start.scroll = s;
            let hit = item_at(d, oi, x, y);
            d.start_hover = hit;
            damage(d);
        }
    }
    true
}

/// Keeps the keyboard selection in view on the scrolling page.
fn reveal(d: &mut Desk, oi: usize) {
    if !(searching(d) || d.start.all) {
        return;
    }
    let Some((_, c)) = nav_items(d, oi).get(d.start_sel).copied() else { return };
    let area = grid_area(rect(d, oi));
    if c.y < area.y {
        d.start.scroll -= area.y - c.y;
    } else if c.y1() > area.y1() {
        d.start.scroll += c.y1() - area.y1();
    }
    d.start.scroll = d.start.scroll.clamp(0, max_scroll(d, oi));
}

/// Arrow keys: the nearest item in that direction (by position, so it works across
/// the grid and the two-column recent list alike).
fn step(d: &Desk, oi: usize, dx: i32, dy: i32) -> usize {
    let nav = nav_items(d, oi);
    let Some((_, cur)) = nav.get(d.start_sel) else { return 0 };
    let (cx, cy) = cur.center();
    let mut best: Option<(i64, usize)> = None;
    for (i, (_, r)) in nav.iter().enumerate() {
        let (x, y) = r.center();
        let (ddx, ddy) = ((x - cx) as i64, (y - cy) as i64);
        let ok = if dy != 0 { ddy * dy as i64 > 10 } else { ddx * dx as i64 > 10 && ddy.abs() < 10 };
        if !ok {
            continue;
        }
        // Along the axis counts less than across it.
        let score = if dy != 0 { ddy.abs() * 4 + ddx.abs() } else { ddx.abs() };
        if best.map_or(true, |b| score < b.0) {
            best = Some((score, i));
        }
    }
    best.map_or(d.start_sel, |b| b.1)
}

/// Keyboard while the menu is open (Esc and the rest of the shell keys are handled
/// by the caller).
pub fn key(d: &mut Desk, oi: usize, code: u32) {
    let m = crate::input::text_mods(d);
    use crate::input::key;
    use crate::textedit::Act;
    let before = d.start_field.text();
    let mut moved = true;
    match code {
        key::ENTER => {
            d.seat.swallowed.push(code);
            let nav = nav_items(d, oi);
            let sel = if searching(d) || d.start.keyboard { nav.get(d.start_sel).copied() } else { None };
            match sel {
                Some(c @ (It::App(a), _)) | Some(c @ (It::Recent(a), _)) => launch(d, a, Some(c)),
                _ if !d.start_field.is_empty() => {
                    let q = d.start_field.text();
                    shell::close_menus(d);
                    d.spawn(&q);
                }
                _ => {}
            }
            return;
        }
        key::UP => d.start_sel = step(d, oi, 0, -1),
        key::DOWN if !d.start.keyboard && !searching(d) => d.start_sel = 0,
        key::DOWN => d.start_sel = step(d, oi, 0, 1),
        key::TAB if m.shift => d.start_sel = d.start_sel.saturating_sub(1),
        key::TAB => d.start_sel += 1,
        key::LEFT if d.start_field.is_empty() => d.start_sel = step(d, oi, -1, 0),
        key::RIGHT if d.start_field.is_empty() => d.start_sel = step(d, oi, 1, 0),
        _ => {
            moved = false;
            match d.start_field.key(code, m) {
                Act::Copy(s) | Act::Cut(s) => crate::clipboard::set_text(d, s),
                Act::Paste => crate::clipboard::paste_into_start(d),
                _ => {}
            }
        }
    }
    if moved {
        d.start.keyboard = true;
    }
    if d.start_field.text() != before {
        d.start_sel = 0;
        d.start.scroll = 0;
    }
    let n = nav_items(d, oi).len();
    if n > 0 {
        d.start_sel = d.start_sel.min(n - 1);
    }
    reveal(d, oi);
    damage(d);
}
