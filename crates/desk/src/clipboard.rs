//! Clipboard history, Win+V (as in Windows 11).
//!
//! Every time an app copies text, the compositor reads it once through a pipe (in the
//! event loop, never blocking) and keeps it. Win+V shows the history on glass; picking
//! an entry makes it the clipboard (a source the compositor itself serves) and pastes it
//! into the focused window with Ctrl+V. Text and pictures (Win+Shift+S, an image copied in
//! an app), kept in memory, not on disk. The panel rises from below as it opens.
//! Sources marked secret by password managers are not recorded.
use crate::desk::{Desk, TOK_CLIP};
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, fade, Canvas, Xf};
use crate::shell::col;
use crate::state::{DKey, DataSource};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// Text types we read and offer, best first.
pub const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];
/// Picture types we read, best first.
pub const IMAGE_MIMES: [&str; 3] = ["image/png", "image/jpeg", "image/jpg"];
const KEEP: usize = 25;
/// At most this many pictures are kept (they are large).
const KEEP_IMAGES: usize = 8;
const MAX_BYTES: usize = 4 << 20;
const MAX_IMAGE_BYTES: usize = 64 << 20;
/// A picture's preview: at most this tall.
const THUMB_H: i32 = 150;
/// How far below its place the panel starts rising from.
const RISE: f32 = 64.0;
const W: i32 = 400;
const MAX_H: i32 = 560;
const PREVIEW_LINES: usize = 3;
const LINE_H: i32 = 19;

pub struct Entry {
    /// The data as offered (text, or the picture's file).
    pub text: Arc<Vec<u8>>,
    pub at: (u32, u32),
    /// A picture: its type and preview.
    pub image: Option<Picture>,
}

pub struct Picture {
    pub mime: String,
    pub w: i32,
    pub h: i32,
    /// Preview pixels, `tw`×`th` (opaque ARGB).
    pub thumb: Vec<u32>,
    pub tw: i32,
    pub th: i32,
}

struct Read {
    id: u64,
    fd: OwnedFd,
    buf: Vec<u8>,
    /// The selection this reads (to know when history[0] is the current one).
    dk: DKey,
    /// Also insert it into the start menu's search field (Ctrl+V there).
    paste: bool,
    /// A picture of this type (else text).
    image: Option<String>,
}

#[derive(Default)]
pub struct Clip {
    pub items: Vec<Entry>,
    reads: Vec<Read>,
    next_id: u64,
    /// Output the panel is open on.
    pub open: Option<usize>,
    pub sel: usize,
    pub hover: Option<usize>,
    top: usize,
    /// The selection that history[0] was read from.
    last_dk: Option<DKey>,
    /// Output the panel is drawn on (also while it sinks away after closing).
    pub shown: Option<usize>,
    /// 0 below its place, 1 in place.
    rise: crate::anim::Spring,
}

// ------------------------------------------------------------------ capture

/// A client made `dk` the selection: read its text into the history.
pub fn capture(d: &mut Desk, dk: DKey) {
    start_read(d, dk, false);
}

fn start_read(d: &mut Desk, dk: DKey, paste: bool) {
    let Some(src) = d.sources.get(dk) else { return };
    if src.own.is_some() || (!paste && src.mimes.iter().any(|m| m == "x-kde-passwordManagerHint")) {
        return;
    }
    let has = |m: &&&str| src.mimes.iter().any(|x| x == **m);
    let (mime, image) = match (TEXT_MIMES.iter().find(has), IMAGE_MIMES.iter().find(has)) {
        (Some(m), _) => (*m, None),
        (None, Some(m)) if !paste => (*m, Some(m.to_string())),
        _ => return,
    };
    let Ok((r, w)) = sys::pipe() else { return };
    crate::server::source_send(d, dk, mime, w);
    let id = d.clip.next_id;
    d.clip.next_id += 1;
    if d.ep.add(r.as_raw_fd(), TOK_CLIP | id, sys::EPOLLIN).is_ok() {
        d.clip.reads.push(Read { id, fd: r, buf: Vec::new(), dk, paste, image });
    }
}

/// The pipe of capture `tok` has data (or hung up).
pub fn readable(d: &mut Desk, tok: u64) {
    let id = tok & !crate::desk::TOK_KIND_MASK;
    let Some(i) = d.clip.reads.iter().position(|r| r.id == id) else { return };
    let mut chunk = [0u8; 16384];
    let done = loop {
        let n = unsafe { sys::ffi::read(d.clip.reads[i].fd.as_raw_fd(), chunk.as_mut_ptr() as *mut _, chunk.len()) };
        if n > 0 {
            let r = &mut d.clip.reads[i];
            r.buf.extend_from_slice(&chunk[..n as usize]);
            if r.buf.len() > if r.image.is_some() { MAX_IMAGE_BYTES } else { MAX_BYTES } {
                r.buf.clear();
                break true; // too big to keep: drop it
            }
            continue;
        }
        if n == 0 {
            break true;
        }
        let e = std::io::Error::last_os_error();
        break match e.kind() {
            std::io::ErrorKind::WouldBlock => false,
            std::io::ErrorKind::Interrupted => continue,
            _ => true,
        };
    };
    if done {
        let r = d.clip.reads.swap_remove(i);
        d.ep.remove(r.fd.as_raw_fd());
        if r.paste {
            if d.overlay.start_menu.is_some() {
                d.start_field.insert(&search_text(&r.buf));
                d.start_sel = 0;
                crate::shell::damage_start(d);
            }
            return;
        }
        if let Some(mime) = r.image {
            let img = if mime == "image/png" { crate::wallpaper::decode_png(&r.buf) } else { crate::jpeg::decode(&r.buf) };
            if let Some(img) = img {
                let p = picture(mime, &img.px, img.w as i32, img.h as i32);
                if push_entry(d, r.buf, Some(p)) {
                    d.clip.last_dk = Some(r.dk);
                }
            }
            return;
        }
        let secret = d.sources.get(r.dk).is_some_and(|s| s.mimes.iter().any(|m| m == "x-kde-passwordManagerHint"));
        if !secret && push(d, r.buf) {
            d.clip.last_dk = Some(r.dk);
        }
    }
}

/// Copy/Cut in one of the shell's own text fields: the text becomes the clipboard.
pub fn set_text(d: &mut Desk, s: String) {
    if s.trim().is_empty() {
        return;
    }
    push(d, s.into_bytes());
    make_selection(d, 0);
}

/// Ctrl+V in the start menu's search: the clipboard, now or once it is read.
pub fn paste_into_start(d: &mut Desk) {
    let Some(k) = d.selection else { return };
    // Text only (a screenshot's PNG is not a search), one line of it.
    let Some(src) = d.sources.get(k) else { return };
    if !TEXT_MIMES.iter().any(|m| src.mimes.iter().any(|x| x == *m)) {
        return;
    }
    let own = src.own.clone();
    let text = match own {
        Some(t) => Some(t),
        None if d.clip.last_dk == Some(k) => d.clip.items.first().map(|e| e.text.clone()),
        None => None,
    };
    match text {
        Some(t) => {
            d.start_field.insert(&search_text(&t));
            d.start_sel = 0;
        }
        None => start_read(d, k, true),
    }
}

/// What of pasted bytes goes into the search field: the first line, at most 256 characters.
fn search_text(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().filter(|c| !c.is_control()).take(256).collect()
}

/// A picture's history entry: its preview fits the panel's cell.
fn picture(mime: String, px: &[u32], w: i32, h: i32) -> Picture {
    let max_w = W - 32 - 28;
    let s = (max_w as f64 / w as f64).min(THUMB_H as f64 / h as f64).min(1.0);
    let (tw, th) = (((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1));
    let img = crate::wallpaper::Image { w: w as usize, h: h as usize, px: px.to_vec() };
    let thumb = crate::wallpaper::thumb(&img, tw as usize, th as usize);
    Picture { mime, w, h, thumb, tw, th }
}

/// Win+Shift+S took a picture and made it the selection `dk`: into the history.
pub fn snipped(d: &mut Desk, dk: DKey, png: Arc<Vec<u8>>, px: &[u32], w: i32, h: i32) {
    let p = picture("image/png".into(), px, w, h);
    let bytes = Arc::try_unwrap(png).unwrap_or_else(|a| (*a).clone());
    if push_entry(d, bytes, Some(p)) {
        d.clip.last_dk = Some(dk);
    }
}

/// Adds text to the history (or moves an equal entry to the top). False if nothing kept.
fn push(d: &mut Desk, bytes: Vec<u8>) -> bool {
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return false;
    }
    push_entry(d, bytes, None)
}

fn push_entry(d: &mut Desk, bytes: Vec<u8>, image: Option<Picture>) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let old = d.clip.shown.map(|oi| panel(d, oi).0);
    let (hh, mm, ..) = crate::clock::local_now();
    let items = &mut d.clip.items;
    items.retain(|e| e.text.as_slice() != bytes.as_slice());
    items.insert(0, Entry { text: Arc::new(bytes), at: (hh, mm), image });
    items.truncate(KEEP);
    // The oldest pictures go first.
    while items.iter().filter(|e| e.image.is_some()).count() > KEEP_IMAGES {
        if let Some(i) = items.iter().rposition(|e| e.image.is_some()) {
            items.remove(i);
        }
    }
    if let Some(r) = old {
        d.damage(r.inflate(crate::shell::GLASS_REACH));
    }
    if let Some(oi) = d.clip.open {
        let r = panel(d, oi).0;
        d.damage(r.inflate(crate::shell::GLASS_REACH));
    }
    true
}

/// History entry `i` becomes the clipboard; the compositor serves it from now on.
fn make_selection(d: &mut Desk, i: usize) {
    let e = d.clip.items.remove(i);
    let data = e.text.clone();
    d.clip.items.insert(0, e);
    let mimes = match &d.clip.items[0].image {
        Some(p) => vec![p.mime.clone()],
        None => TEXT_MIMES.iter().map(|s| s.to_string()).collect(),
    };
    let dk = d.sources.insert(DataSource { client: Default::default(), obj: 0, mimes, primary: false, own: Some(data), x11: false, actions: 0 });
    crate::server::set_selection(d, Some(dk));
    d.clip.last_dk = Some(dk);
}

/// A client asked for our own selection: write it from a thread (the reader may be slow).
pub fn serve(data: Arc<Vec<u8>>, fd: OwnedFd) {
    std::thread::spawn(move || {
        let mut rest: &[u8] = &data;
        while !rest.is_empty() {
            let n = unsafe { sys::ffi::write(fd.as_raw_fd(), rest.as_ptr() as *const _, rest.len()) };
            if n > 0 {
                rest = &rest[n as usize..];
                continue;
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::Interrupted => {}
                std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(2)),
                _ => return,
            }
        }
    });
}

// ------------------------------------------------------------------ panel

pub fn toggle(d: &mut Desk) {
    if d.clip.open.take().is_some() {
        // Sinks away (tick).
        d.clip.rise.to(0.0);
        crate::shell::taskbar_poke(d);
        return;
    }
    crate::shell::close_menus(d);
    d.overlay.switcher = None;
    let oi = crate::shell::bar_output(d, d.output_at(d.seat.x, d.seat.y).unwrap_or(0));
    if let Some(was) = d.clip.shown.filter(|&o| o != oi) {
        let r = panel(d, was).0;
        d.damage(r.inflate(crate::shell::GLASS_REACH));
        d.clip.rise.snap(0.0);
    }
    if d.clip.shown.is_none() {
        d.clip.rise.snap(0.0);
    }
    d.clip.open = Some(oi);
    d.clip.shown = Some(oi);
    d.clip.sel = 0;
    d.clip.top = 0;
    d.clip.hover = None;
    d.clip.rise.response = 0.32;
    d.clip.rise.damping = 0.86;
    d.clip.rise.to(1.0);
    let r = panel(d, oi).0;
    d.damage(r.inflate(crate::shell::GLASS_REACH));
    crate::shell::taskbar_poke(d);
}

/// The panel rising or sinking. True while it moves.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let Some(oi) = d.clip.shown else { return false };
    if !d.clip.rise.moving() {
        if d.clip.open.is_none() {
            d.clip.shown = None;
        }
        return false;
    }
    let before = panel(d, oi).0;
    let m = d.clip.rise.step(dt);
    let after = panel(d, oi).0;
    d.damage(before.union(&after).inflate(crate::shell::GLASS_REACH));
    if !m && d.clip.open.is_none() {
        d.clip.shown = None;
    }
    true
}

/// How far it has risen, 0..1 (for fading what is on it).
fn shown_alpha(d: &Desk) -> u32 {
    (d.clip.rise.value.clamp(0.0, 1.0) * 255.0).round() as u32
}

fn close(d: &mut Desk) {
    if d.clip.open.is_some() {
        toggle(d);
    }
}

/// Preview of an entry: at most PREVIEW_LINES lines wrapped to `max_w`, "…" when cut.
fn preview(text: &[u8], max_w: i32) -> Vec<String> {
    let s = String::from_utf8_lossy(&text[..text.len().min(4096)]);
    wrap(&s, text.len() > 4096, PREVIEW_LINES, |t| font::text_width(t, 13.0) > max_w)
}

/// Word wrap: lines break at the last space that fits; a word wider than the
/// whole line (a link, a path) breaks between characters. At most `max_lines`
/// lines; the last one ends in "…" when text was left out.
pub(crate) fn wrap(s: &str, mut cut: bool, max_lines: usize, too_wide: impl Fn(&str) -> bool) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    'outer: for (pi, para) in s.split('\n').enumerate() {
        if pi > 0 {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max_lines {
                cut = true;
                break;
            }
        }
        for ch in para.chars() {
            let ch = if ch == '\t' { ' ' } else { ch };
            if ch.is_control() {
                continue;
            }
            cur.push(ch);
            if !too_wide(&cur) {
                continue;
            }
            // Overflow. A space itself never starts a new line: drop it.
            if ch == ' ' {
                cur.pop();
                lines.push(std::mem::take(&mut cur));
            } else {
                match cur.trim_end().rfind(' ') {
                    // Break after the last space; the partial word moves down.
                    Some(sp) if cur[..sp].trim_end().len() > 0 => {
                        let rest = cur[sp + 1..].to_string();
                        cur.truncate(sp);
                        let line = cur.trim_end().to_string();
                        lines.push(line);
                        cur = rest;
                    }
                    // One long word (a link, a path): break after the last "/" or "-".
                    _ if cur[..cur.len() - ch.len_utf8()].rfind(['/', '-']).is_some_and(|p| p > 0) => {
                        let p = cur[..cur.len() - ch.len_utf8()].rfind(['/', '-']).unwrap() + 1;
                        let rest = cur[p..].to_string();
                        cur.truncate(p);
                        lines.push(std::mem::replace(&mut cur, rest));
                    }
                    // No break point at all: break between characters.
                    _ => {
                        cur.pop();
                        lines.push(std::mem::replace(&mut cur, ch.to_string()));
                    }
                }
            }
            if lines.len() == max_lines {
                cut = true;
                break 'outer;
            }
        }
    }
    if lines.len() < max_lines && !cur.is_empty() {
        lines.push(cur);
    } else if !cur.trim().is_empty() {
        cut = true;
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    if cut {
        if let Some(l) = lines.last_mut() {
            while too_wide(&format!("{l}…")) && l.pop().is_some() {}
            let t = l.trim_end().len();
            l.truncate(t);
            l.push('…');
        }
    }
    lines
}

/// Panel, "clear all" button, and the visible entries (index, rect, preview).
fn panel(d: &Desk, oi: usize) -> (Rect, Rect, Vec<(usize, Rect, Vec<String>)>) {
    let wa = crate::wm::panel_area(d, oi);
    let inner = W - 32;
    let mut cells = Vec::new();
    let mut y = 64;
    for (i, e) in d.clip.items.iter().enumerate().skip(d.clip.top) {
        let (lines, body) = match &e.image {
            Some(p) => (Vec::new(), p.th),
            None => {
                let l = preview(&e.text, inner - 28);
                let n = l.len() as i32;
                (l, n * LINE_H)
            }
        };
        let h = 14 + body + 22 + 10;
        if y + h > MAX_H - 16 && !cells.is_empty() {
            break;
        }
        cells.push((i, Rect::new(16, y, inner, h), lines));
        y += h + 8;
    }
    let h = if cells.is_empty() { 150 } else { (y + 8).min(MAX_H) };
    let drop = ((1.0 - d.clip.rise.value.min(1.0)) * RISE).round() as i32;
    let p = Rect::new(wa.x1() - W - 12, wa.y1() - h - 8 + drop, W, h);
    let cells = cells.into_iter().map(|(i, r, l)| (i, r.translate(p.x, p.y), l)).collect();
    let clear = Rect::new(p.x1() - 16 - 110, p.y + 18, 110, 30);
    (p, clear, cells)
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let (p, clear, cells) = panel(d, oi);
    let a = shown_alpha(d);
    let f = |c: u32| fade(c, a);
    crate::shell::draw_glass(cv, clip, xf, p, RADIUS);
    font::draw_text_w(cv, clip, xf, p.x + 20, p.y + 39, "Буфер обмена", 16.0, f(col::TEXT), 200, font::W::Bold);
    if d.clip.items.is_empty() {
        font::draw_text_w(cv, clip, xf, p.x + 20, p.y + 84, "Здесь появится то, что вы скопируете", 13.0, f(col::TEXT), W - 40, font::W::Semi);
        font::draw_text(cv, clip, xf, p.x + 20, p.y + 106, "Ctrl+C или Win+Shift+S, потом Win+V", 12.5, f(col::TEXT_DIM), W - 40);
        return;
    }
    cv.rrect(clip, xf.r(clear), xf.len(10), f(argb(18, 255, 255, 255)));
    let t = "Очистить всё";
    let tw = font::text_width_w(t, 12.5, font::W::Semi);
    font::draw_text_w(cv, clip, xf, clear.x + (clear.w - tw) / 2, clear.y + 20, t, 12.5, f(col::TEXT), clear.w, font::W::Semi);
    for (i, r, lines) in cells {
        if !xf.r(r).overlaps(&clip) {
            continue;
        }
        let sel = d.clip.sel == i;
        let hot = d.clip.hover == Some(i);
        let fill = if sel { argb(34, 255, 255, 255) } else if hot { argb(26, 255, 255, 255) } else { argb(14, 255, 255, 255) };
        cv.rrect(clip, xf.r(r), xf.len(12), f(fill));
        if sel {
            let s = 220 * a / 255;
            cv.rrect_stroke(clip, xf.r(r), xf.len(12), 1.5 * xf.scale as f32, crate::look::accent() & 0xff_ffff, s, s);
        }
        let e = &d.clip.items[i];
        let meta = match &e.image {
            Some(p) => {
                let dst = Rect::new(r.x + 14, r.y + 14, p.tw, p.th);
                cv.blit(clip, xf.r(dst), &p.thumb, p.tw as usize, Rect::new(0, 0, p.tw, p.th), a == 255, a);
                format!("{}  ·  картинка {}×{}", fmt_time(e.at), p.w, p.h)
            }
            None => {
                for (n, l) in lines.iter().enumerate() {
                    font::draw_text(cv, clip, xf, r.x + 14, r.y + 14 + 13 + n as i32 * LINE_H, l, 13.0, f(col::TEXT), r.w - 28);
                }
                let chars = String::from_utf8_lossy(&e.text).chars().count();
                format!("{}  ·  {} {}", fmt_time(e.at), chars, plural(chars, "символ", "символа", "символов"))
            }
        };
        font::draw_text(cv, clip, xf, r.x + 14, r.y1() - 12, &meta, 11.5, f(col::TEXT_DIM), r.w - 28);
        if i == 0 {
            let s = "сейчас в буфере";
            let sw = font::text_width_w(s, 11.5, font::W::Semi);
            font::draw_text_w(cv, clip, xf, r.x1() - 14 - sw, r.y1() - 12, s, 11.5, f(col::accent()), 160, font::W::Semi);
        }
    }
}

fn fmt_time(at: (u32, u32)) -> String {
    format!("{:02}:{:02}", at.0, at.1)
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    }
}

/// Corner radius of the panel's glass.
pub const RADIUS: i32 = 20;

pub fn glass_rect(d: &Desk, oi: usize) -> Option<Rect> {
    (d.clip.shown == Some(oi)).then(|| panel(d, oi).0)
}

fn redraw(d: &mut Desk) {
    if let Some(oi) = d.clip.shown {
        let r = panel(d, oi).0;
        d.damage(r.inflate(crate::shell::GLASS_REACH));
    }
}

/// Picks entry `i`: it becomes the clipboard and goes into the focused window.
fn choose(d: &mut Desk, i: usize) {
    if i >= d.clip.items.len() {
        return;
    }
    if d.clip.open.take().is_some() {
        d.clip.rise.to(0.0);
        crate::shell::taskbar_poke(d);
    }
    make_selection(d, i);
    crate::input::paste_into_focus(d);
}

/// Keyboard while the panel is open. Returns true if consumed.
pub fn key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    if d.clip.open.is_none() {
        return false;
    }
    if !pressed {
        return true;
    }
    use crate::input::key;
    let n = d.clip.items.len();
    match code {
        key::ESC => close(d),
        key::ENTER => {
            d.seat.swallowed.push(code);
            let i = d.clip.sel;
            choose(d, i);
            return true;
        }
        key::UP => d.clip.sel = d.clip.sel.saturating_sub(1),
        key::DOWN => d.clip.sel = (d.clip.sel + 1).min(n.saturating_sub(1)),
        key::DELETE => {
            if d.clip.sel < n {
                let old = panel(d, d.clip.open.unwrap_or(0)).0;
                d.clip.items.remove(d.clip.sel);
                d.clip.sel = d.clip.sel.min(d.clip.items.len().saturating_sub(1));
                d.damage(old.inflate(crate::shell::GLASS_REACH));
            }
        }
        _ => {}
    }
    // Keep the selection on screen.
    if d.clip.sel < d.clip.top {
        d.clip.top = d.clip.sel;
    }
    if let Some(oi) = d.clip.open {
        while d.clip.top < d.clip.sel && !panel(d, oi).2.iter().any(|(i, ..)| *i == d.clip.sel) {
            d.clip.top += 1;
        }
    }
    redraw(d);
    true
}

/// Clicks while the panel is open. Returns true if consumed.
pub fn click(d: &mut Desk, x: f64, y: f64) -> bool {
    let Some(oi) = d.clip.open else { return false };
    let (p, clear, cells) = panel(d, oi);
    if !p.contains(x, y) {
        close(d);
        return true;
    }
    if !d.clip.items.is_empty() && clear.contains(x, y) {
        d.clip.items.clear();
        d.damage(p.inflate(crate::shell::GLASS_REACH));
        return true;
    }
    if let Some((i, ..)) = cells.iter().find(|(_, r, _)| r.contains(x, y)) {
        choose(d, *i);
    }
    true
}

pub fn hover(d: &mut Desk, x: f64, y: f64) {
    let Some(oi) = d.clip.open else { return };
    let h = panel(d, oi).2.iter().find(|(_, r, _)| r.contains(x, y)).map(|(i, ..)| *i);
    if h != d.clip.hover {
        d.clip.hover = h;
        redraw(d);
    }
}

pub fn scroll(d: &mut Desk, dir: f64) -> bool {
    let Some(oi) = d.clip.open else { return false };
    if !panel(d, oi).0.contains(d.seat.x, d.seat.y) {
        return false;
    }
    let n = d.clip.items.len();
    d.clip.top = if dir > 0.0 { (d.clip.top + 1).min(n.saturating_sub(1)) } else { d.clip.top.saturating_sub(1) };
    redraw(d);
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn wraps_at_spaces() {
        use super::wrap;
        // Monospace stand-in: at most 10 characters per line.
        let w = |t: &str| t.chars().count() > 10;
        assert_eq!(wrap("привет мир как дела", false, 4, w), ["привет мир", "как дела"]);
        assert_eq!(wrap("один два три четыре", false, 4, w), ["один два", "три четыре"]);
        // A long link breaks after "/" (or "-"), a word with neither between characters.
        assert_eq!(wrap("https://example.org/x", false, 4, w), ["https://", "example.or", "g/x"]);
        assert_eq!(wrap("abcdefghijklm", false, 4, w), ["abcdefghij", "klm"]);
        // Line breaks are kept; the overflow gets "…".
        assert_eq!(wrap("a\nb\nc\nd\ne", false, 3, w), ["a", "b", "c…"]);
        assert_eq!(wrap("aaa bbb ccc ddd eee fff", false, 2, w), ["aaa bbb", "ccc ddd…"]);
    }

    #[test]
    fn russian_plurals() {
        use super::plural;
        let p = |n| plural(n, "символ", "символа", "символов");
        assert_eq!(p(1), "символ");
        assert_eq!(p(3), "символа");
        assert_eq!(p(11), "символов");
        assert_eq!(p(21), "символ");
        assert_eq!(p(112), "символов");
    }
}
