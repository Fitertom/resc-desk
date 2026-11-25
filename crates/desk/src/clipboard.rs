//! Clipboard history, Win+V (as in Windows 11).
//!
//! Every time an app copies text, the compositor reads it once through a pipe (in the
//! event loop, never blocking) and keeps it. Win+V shows the history on glass; picking
//! an entry makes it the clipboard (a source the compositor itself serves) and pastes it
//! into the focused window with Ctrl+V. Text only for now; kept in memory, not on disk.
//! Sources marked secret by password managers are not recorded.
use crate::desk::{Desk, TOK_CLIP};
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::shell::col;
use crate::state::{DKey, DataSource};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// Text types we read and offer, best first.
pub const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];
const KEEP: usize = 25;
const MAX_BYTES: usize = 4 << 20;
const W: i32 = 400;
const MAX_H: i32 = 560;
const PREVIEW_LINES: usize = 3;
const LINE_H: i32 = 19;

pub struct Entry {
    pub text: Arc<Vec<u8>>,
    pub at: (u32, u32),
}

struct Read {
    id: u64,
    fd: OwnedFd,
    buf: Vec<u8>,
    /// The selection this reads (to know when history[0] is the current one).
    dk: DKey,
    /// Also insert it into the start menu's search field (Ctrl+V there).
    paste: bool,
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
    let Some(mime) = TEXT_MIMES.iter().find(|m| src.mimes.iter().any(|x| x == *m)) else { return };
    let Ok((r, w)) = sys::pipe() else { return };
    crate::server::source_send(d, dk, mime, w);
    let id = d.clip.next_id;
    d.clip.next_id += 1;
    if d.ep.add(r.as_raw_fd(), TOK_CLIP | id, sys::EPOLLIN).is_ok() {
        d.clip.reads.push(Read { id, fd: r, buf: Vec::new(), dk, paste });
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
            if r.buf.len() > MAX_BYTES {
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
                d.start_field.insert(&String::from_utf8_lossy(&r.buf));
                d.start_sel = 0;
                crate::shell::damage_start(d);
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
    let own = d.sources.get(k).and_then(|s| s.own.clone());
    let text = match own {
        Some(t) => Some(t),
        None if d.clip.last_dk == Some(k) => d.clip.items.first().map(|e| e.text.clone()),
        None => None,
    };
    match text {
        Some(t) => {
            d.start_field.insert(&String::from_utf8_lossy(&t));
            d.start_sel = 0;
        }
        None => start_read(d, k, true),
    }
}

/// Adds to the history (or moves an equal entry to the top). False if nothing kept.
fn push(d: &mut Desk, bytes: Vec<u8>) -> bool {
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return false;
    }
    let (hh, mm, ..) = crate::clock::local_now();
    let items = &mut d.clip.items;
    items.retain(|e| e.text.as_slice() != bytes.as_slice());
    items.insert(0, Entry { text: Arc::new(bytes), at: (hh, mm) });
    items.truncate(KEEP);
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
    let mimes = TEXT_MIMES.iter().map(|s| s.to_string()).collect();
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
    if let Some(oi) = d.clip.open.take() {
        let r = panel(d, oi).0;
        d.damage(r.inflate(crate::shell::GLASS_REACH));
        return;
    }
    crate::shell::close_menus(d);
    d.overlay.switcher = None;
    let oi = crate::shell::bar_output(d, d.output_at(d.seat.x, d.seat.y).unwrap_or(0));
    d.clip.open = Some(oi);
    d.clip.sel = 0;
    d.clip.top = 0;
    d.clip.hover = None;
    let r = panel(d, oi).0;
    d.damage(r.inflate(crate::shell::GLASS_REACH));
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
    let wa = crate::wm::workarea(d, oi);
    let inner = W - 32;
    let mut cells = Vec::new();
    let mut y = 64;
    for (i, e) in d.clip.items.iter().enumerate().skip(d.clip.top) {
        let lines = preview(&e.text, inner - 28);
        let h = 14 + lines.len() as i32 * LINE_H + 22 + 10;
        if y + h > MAX_H - 16 && !cells.is_empty() {
            break;
        }
        cells.push((i, Rect::new(16, y, inner, h), lines));
        y += h + 8;
    }
    let h = if cells.is_empty() { 150 } else { (y + 8).min(MAX_H) };
    let p = Rect::new(wa.x1() - W - 12, wa.y1() - h - 8, W, h);
    let cells = cells.into_iter().map(|(i, r, l)| (i, r.translate(p.x, p.y), l)).collect();
    let clear = Rect::new(p.x1() - 16 - 110, p.y + 18, 110, 30);
    (p, clear, cells)
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let (p, clear, cells) = panel(d, oi);
    crate::shell::draw_glass(cv, clip, xf, p, RADIUS);
    font::draw_text_w(cv, clip, xf, p.x + 20, p.y + 39, "Буфер обмена", 16.0, col::TEXT, 200, font::W::Bold);
    if d.clip.items.is_empty() {
        font::draw_text_w(cv, clip, xf, p.x + 20, p.y + 84, "Здесь появится то, что вы скопируете", 13.0, col::TEXT, W - 40, font::W::Semi);
        font::draw_text(cv, clip, xf, p.x + 20, p.y + 106, "Ctrl+C в любом приложении, потом Win+V", 12.5, col::TEXT_DIM, W - 40);
        return;
    }
    cv.rrect(clip, xf.r(clear), xf.len(10), argb(18, 255, 255, 255));
    let t = "Очистить всё";
    let tw = font::text_width_w(t, 12.5, font::W::Semi);
    font::draw_text_w(cv, clip, xf, clear.x + (clear.w - tw) / 2, clear.y + 20, t, 12.5, col::TEXT, clear.w, font::W::Semi);
    for (i, r, lines) in cells {
        if !xf.r(r).overlaps(&clip) {
            continue;
        }
        let sel = d.clip.sel == i;
        let hot = d.clip.hover == Some(i);
        let fill = if sel { argb(34, 255, 255, 255) } else if hot { argb(26, 255, 255, 255) } else { argb(14, 255, 255, 255) };
        cv.rrect(clip, xf.r(r), xf.len(12), fill);
        if sel {
            cv.rrect_stroke(clip, xf.r(r), xf.len(12), 1.5 * xf.scale as f32, crate::look::accent() & 0xff_ffff, 220, 220);
        }
        for (n, l) in lines.iter().enumerate() {
            font::draw_text(cv, clip, xf, r.x + 14, r.y + 14 + 13 + n as i32 * LINE_H, l, 13.0, col::TEXT, r.w - 28);
        }
        let e = &d.clip.items[i];
        let chars = String::from_utf8_lossy(&e.text).chars().count();
        let meta = format!("{}  ·  {} {}", fmt_time(e.at), chars, plural(chars, "символ", "символа", "символов"));
        font::draw_text(cv, clip, xf, r.x + 14, r.y1() - 12, &meta, 11.5, col::TEXT_DIM, r.w - 28);
        if i == 0 {
            let s = "сейчас в буфере";
            let sw = font::text_width_w(s, 11.5, font::W::Semi);
            font::draw_text_w(cv, clip, xf, r.x1() - 14 - sw, r.y1() - 12, s, 11.5, col::accent(), 160, font::W::Semi);
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
    (d.clip.open == Some(oi)).then(|| panel(d, oi).0)
}

fn redraw(d: &mut Desk) {
    if let Some(oi) = d.clip.open {
        let r = panel(d, oi).0;
        d.damage(r.inflate(crate::shell::GLASS_REACH));
    }
}

/// Picks entry `i`: it becomes the clipboard and goes into the focused window.
fn choose(d: &mut Desk, i: usize) {
    if i >= d.clip.items.len() {
        return;
    }
    let before = panel(d, d.clip.open.unwrap_or(0)).0;
    d.clip.open = None;
    d.damage(before.inflate(crate::shell::GLASS_REACH));
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
