//! Copy and paste between X and Wayland programs, both ways: the clipboard (Ctrl+C,
//! CLIPBOARD) and the selection pasted with the middle button (PRIMARY).
//!
//! X → Wayland: XFixes tells us when an X program takes a selection. We ask it which types
//! it has (TARGETS), and the selection here becomes a source with those types. When someone
//! here wants the data (a Wayland program, or the clipboard history), the X program converts
//! it into a property of our selection window, and we copy that into their pipe.
//!
//! Wayland → X: when a Wayland program copies, our selection window becomes the X owner. An
//! X program's SelectionRequest is answered with what the source writes into a pipe.
//!
//! Big data goes in pieces (INCR) both ways. One transfer from X at a time, in order.
use super::{Tag, Xwm, SUB_PIPE};
use crate::desk::{Desk, TOK_XWM};
use crate::state::{DKey, DataSource};
use crate::x11::{self, u32_at, Atom, Xid};
use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// INCR pieces, bytes.
const CHUNK: usize = 64 * 1024;
/// More than this is not copied.
const MAX: usize = 64 << 20;
/// A program that does not answer in this long is given up on.
const TIMEOUT_MS: u32 = 5000;

#[derive(Default)]
pub struct Sels {
    pub win: Xid,
    xfixes_event: Option<u8>,
    names: HashMap<Atom, String>,
    by_name: HashMap<String, Atom>,
    /// 0: CLIPBOARD, 1: PRIMARY.
    one: [One; 2],
    queue: VecDeque<In>,
    active: Option<In>,
    out: Vec<Out>,
    next_id: u64,
}

#[derive(Default)]
struct One {
    /// An X program owns it: our source for it, and (type, X target) pairs.
    src: Option<DKey>,
    from_x: Vec<(String, Atom)>,
    /// We own it in X for a Wayland source: (X target, type) pairs.
    ours: bool,
    to_x: Vec<(Atom, String)>,
    /// Bumped on every change: answers to older questions are dropped.
    gen: u32,
    /// The X owner's targets, waiting for their names.
    targets: Vec<Atom>,
}

/// A transfer from an X program.
struct In {
    sel: usize,
    what: What,
    target: Atom,
    buf: Vec<u8>,
    incr: bool,
    since: u32,
}

enum What {
    Targets(u32),
    Data(OwnedFd),
}

/// A transfer to an X program.
struct Out {
    id: u64,
    requestor: Xid,
    prop: Atom,
    target: Atom,
    ty: Atom,
    selection: Atom,
    time: u32,
    pipe: Option<OwnedFd>,
    buf: Vec<u8>,
    /// INCR: how much is sent.
    sent: Option<usize>,
    since: u32,
}

/// We are the window manager now: our selection window, and XFixes to hear of owners.
pub fn start(x: &mut Xwm) {
    let a = &x.a;
    let c = x.conn.as_mut().unwrap();
    let win = c.gen_id();
    c.create_window(win, x.root, -100, -100, 1, 1, true);
    c.select_input(win, x11::EM_PROPERTY_CHANGE);
    c.query_extension("XFIXES", Tag::XFixes);
    x.sel.win = win;
    let known = [
        (a.utf8_string, "UTF8_STRING"),
        (x11::STRING, "STRING"),
        (a.text, "TEXT"),
        (a.mime_utf8, "text/plain;charset=utf-8"),
        (a.mime_text, "text/plain"),
        (a.targets, "TARGETS"),
        (a.timestamp, "TIMESTAMP"),
    ];
    for (atom, name) in known {
        x.sel.names.insert(atom, name.to_string());
        x.sel.by_name.insert(name.to_string(), atom);
    }
}

/// Xwayland is gone: everything in flight is dropped (readers see the pipe close).
pub fn reset(x: &mut Xwm, d: &mut Desk) {
    for o in &x.sel.out {
        if let Some(p) = &o.pipe {
            d.ep.remove(p.as_raw_fd());
        }
    }
    x.sel = Sels::default();
}

fn index(x: &Xwm, selection: Atom) -> Option<usize> {
    if selection == x.a.clipboard {
        Some(0)
    } else if selection == x11::PRIMARY {
        Some(1)
    } else {
        None
    }
}

fn atom_of(x: &Xwm, i: usize) -> Atom {
    if i == 0 { x.a.clipboard } else { x11::PRIMARY }
}

fn current(d: &Desk, i: usize) -> Option<DKey> {
    if i == 0 { d.selection } else { d.primary }
}

// ------------------------------------------------------------------ replies and events

pub fn reply(x: &mut Xwm, d: &mut Desk, tag: Tag, r: &[u8]) {
    match tag {
        Tag::XFixes => {
            if r[8] == 0 {
                eprintln!("xwayland: no XFIXES: copy and paste between X and Wayland programs is off");
                return;
            }
            let (major, event) = (r[9], r[10]);
            let (win, clip) = (x.sel.win, x.a.clipboard);
            let c = x.conn.as_mut().unwrap();
            c.ext_reply(major, 0, &[5, 0, 0, 0, 0, 0, 0, 0], Tag::Ignore);
            for s in [clip, x11::PRIMARY] {
                // SelectSelectionInput: owner set, owner window destroyed, owner client gone.
                let mut b = Vec::with_capacity(12);
                for v in [win, s, 7] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                c.ext(major, 2, &b);
            }
            x.sel.xfixes_event = Some(event);
            // Copied here before Xwayland started: X programs can paste it too.
            wayland_changed(x, d, 0);
            wayland_changed(x, d, 1);
        }
        Tag::SelProp => data(x, d, r),
        Tag::AtomName(atom) => {
            let n = x11::u16_at(r, 8) as usize;
            let name = String::from_utf8_lossy(&r[32..(32 + n).min(r.len())]).into_owned();
            x.sel.by_name.insert(name.clone(), atom);
            x.sel.names.insert(atom, name);
        }
        Tag::MimeAtom(name) => {
            let atom = u32_at(r, 8);
            x.sel.names.insert(atom, name.clone());
            x.sel.by_name.insert(name, atom);
        }
        Tag::TargetsReady(i, gen) => targets_ready(x, d, i, gen),
        Tag::Own(i, gen) => own(x, d, i, gen),
        _ => {}
    }
}

/// Selection events; false if `code` is not one of ours.
pub fn event(x: &mut Xwm, d: &mut Desk, code: u8, e: &[u8; 32]) -> bool {
    if Some(code) == x.sel.xfixes_event {
        owner_changed(x, d, u32_at(e, 12), u32_at(e, 8));
        return true;
    }
    match code {
        x11::SELECTION_NOTIFY if u32_at(e, 8) == x.sel.win => {
            if x.sel.active.is_some() {
                if u32_at(e, 20) == 0 {
                    finish(x, d, false);
                } else {
                    let (win, prop) = (x.sel.win, x.a.wl_selection);
                    x.conn.as_mut().unwrap().get_property_ex(win, prop, true, Tag::SelProp);
                }
            }
            true
        }
        x11::SELECTION_REQUEST => {
            request(x, d, e);
            true
        }
        x11::PROPERTY_NOTIFY => {
            let (w, atom, deleted) = (u32_at(e, 4), u32_at(e, 8), e[16] == 1);
            // The next INCR piece from an X program is in.
            if w == x.sel.win {
                if atom == x.a.wl_selection && !deleted && x.sel.active.as_ref().is_some_and(|t| t.incr) {
                    let win = x.sel.win;
                    x.conn.as_mut().unwrap().get_property_ex(win, atom, true, Tag::SelProp);
                }
                return true;
            }
            // An X program took the last INCR piece we gave: the next one.
            if deleted {
                if let Some(i) = x.sel.out.iter().position(|o| o.requestor == w && o.prop == atom && o.sent.is_some()) {
                    next_piece(x, d, i);
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

// ------------------------------------------------------------------ X → Wayland

fn owner_changed(x: &mut Xwm, d: &mut Desk, selection: Atom, owner: Xid) {
    let Some(i) = index(x, selection) else { return };
    if owner == x.sel.win {
        return; // that is us, for a Wayland source
    }
    x.sel.one[i].ours = false;
    x.sel.one[i].gen += 1;
    // What the previous X owner had is gone here too.
    if let Some(dk) = x.sel.one[i].src.take() {
        if current(d, i) == Some(dk) {
            if i == 0 {
                crate::server::set_selection(d, None);
            } else {
                crate::server::set_primary(d, None);
            }
        } else {
            d.sources.remove(dk);
        }
    }
    if owner == 0 {
        return;
    }
    let gen = x.sel.one[i].gen;
    let target = x.a.targets;
    x.sel.queue.push_back(In { sel: i, what: What::Targets(gen), target, buf: vec![], incr: false, since: 0 });
    pump(x, d);
}

/// Starts the next transfer from X if none is running.
fn pump(x: &mut Xwm, d: &mut Desk) {
    if x.sel.active.is_some() {
        return;
    }
    let Some(mut t) = x.sel.queue.pop_front() else { return };
    t.since = d.now_ms();
    let (win, prop, sel) = (x.sel.win, x.a.wl_selection, atom_of(x, t.sel));
    x.conn.as_mut().unwrap().convert_selection(win, sel, t.target, prop);
    x.sel.active = Some(t);
}

/// The X program put (a piece of) the data into our property.
fn data(x: &mut Xwm, d: &mut Desk, r: &[u8]) {
    let incr_atom = x.a.incr;
    let Some(t) = x.sel.active.as_mut() else { return };
    let (ty, _, v) = x11::property_value(r);
    if !t.incr && ty == incr_atom {
        // Pieces follow, each announced by a PropertyNotify (we just deleted the property).
        t.incr = true;
        return;
    }
    if t.buf.len() + v.len() > MAX {
        finish(x, d, false);
        return;
    }
    t.buf.extend_from_slice(v);
    if !t.incr || v.is_empty() {
        finish(x, d, true);
    }
}

fn finish(x: &mut Xwm, d: &mut Desk, ok: bool) {
    let Some(t) = x.sel.active.take() else { return };
    match t.what {
        What::Targets(gen) if ok => {
            let targets = x11::values32(&t.buf);
            let c = x.conn.as_mut().unwrap();
            for &a in &targets {
                if !x.sel.names.contains_key(&a) {
                    c.get_atom_name(a, Tag::AtomName(a));
                }
            }
            c.sync(Tag::TargetsReady(t.sel, gen));
            x.sel.one[t.sel].targets = targets;
        }
        What::Targets(_) => {}
        What::Data(fd) => {
            if ok {
                crate::clipboard::serve(Arc::new(t.buf), fd);
            }
            // Not ok: the pipe closes with nothing in it.
        }
    }
    pump(x, d);
}

/// The X owner's types are known: the selection here becomes a source with them.
fn targets_ready(x: &mut Xwm, d: &mut Desk, i: usize, gen: u32) {
    if x.sel.one[i].gen != gen {
        return;
    }
    let a = &x.a;
    let mut types: Vec<(String, Atom)> = Vec::new();
    let mut add = |m: &str, t: Atom| {
        if !types.iter().any(|(n, _)| n == m) {
            types.push((m.to_string(), t));
        }
    };
    let targets = std::mem::take(&mut x.sel.one[i].targets);
    // Text first, the best kind first.
    if targets.contains(&a.utf8_string) {
        add("text/plain;charset=utf-8", a.utf8_string);
        add("UTF8_STRING", a.utf8_string);
    }
    for &t in &targets {
        if t == x11::STRING {
            add("STRING", t);
            add("text/plain", t);
        } else if t == a.text {
            add("TEXT", t);
        } else if let Some(n) = x.sel.names.get(&t).filter(|n| n.contains('/')) {
            add(n, t);
        }
    }
    if types.is_empty() {
        return;
    }
    let mimes = types.iter().map(|(m, _)| m.clone()).collect();
    let dk = d.sources.insert(DataSource { client: x.client.unwrap_or_default(), obj: 0, mimes, primary: i == 1, own: None, x11: true, actions: 0 });
    x.sel.one[i].src = Some(dk);
    x.sel.one[i].from_x = types;
    if i == 0 {
        crate::server::set_selection(d, Some(dk));
    } else {
        crate::server::set_primary(d, Some(dk));
    }
}

/// Once per loop: requests from here for X's data, and transfers that hang.
pub fn tick(x: &mut Xwm, d: &mut Desk) {
    for (dk, mime, fd) in std::mem::take(&mut d.x_receives) {
        let Some(i) = (0..2).find(|&i| x.sel.one[i].src == Some(dk)) else { continue };
        let Some(&(_, target)) = x.sel.one[i].from_x.iter().find(|(m, _)| *m == mime) else { continue };
        x.sel.queue.push_back(In { sel: i, what: What::Data(fd), target, buf: vec![], incr: false, since: 0 });
    }
    let now = d.now_ms();
    if x.sel.active.as_ref().is_some_and(|t| now.wrapping_sub(t.since) > TIMEOUT_MS) {
        finish(x, d, false);
    }
    pump(x, d);
    let old: Vec<u64> = x.sel.out.iter().filter(|o| now.wrapping_sub(o.since) > 2 * TIMEOUT_MS).map(|o| o.id).collect();
    for id in old {
        drop_out(x, d, id);
    }
}

// ------------------------------------------------------------------ Wayland → X

/// The selection here changed: X programs get it too (unless it came from X).
pub fn wayland_changed(x: &mut Xwm, d: &mut Desk, i: usize) {
    x.sel.one[i].gen += 1;
    let gen = x.sel.one[i].gen;
    let sel = atom_of(x, i);
    let src = current(d, i).and_then(|k| d.sources.get(k));
    let c = x.conn.as_mut().unwrap();
    match src {
        Some(s) if s.x11 => {}
        Some(s) => {
            // Atoms for its types first, then take the selection.
            for m in &s.mimes {
                if !x.sel.by_name.contains_key(m) {
                    c.intern_atom(m, Tag::MimeAtom(m.clone()));
                }
            }
            c.sync(Tag::Own(i, gen));
        }
        None => {
            if x.sel.one[i].ours {
                x.sel.one[i].ours = false;
                c.set_selection_owner(0, sel);
            }
        }
    }
}

fn own(x: &mut Xwm, d: &mut Desk, i: usize, gen: u32) {
    if x.sel.one[i].gen != gen {
        return;
    }
    let Some(src) = current(d, i).and_then(|k| d.sources.get(k)) else { return };
    let a = &x.a;
    let mut to_x: Vec<(Atom, String)> = Vec::new();
    if let Some(best) = crate::clipboard::TEXT_MIMES.iter().find(|m| src.mimes.iter().any(|s| s == *m)) {
        for t in [a.utf8_string, x11::STRING, a.text] {
            to_x.push((t, best.to_string()));
        }
    }
    for m in &src.mimes {
        if let Some(&t) = x.sel.by_name.get(m) {
            if !to_x.iter().any(|(a, _)| *a == t) {
                to_x.push((t, m.clone()));
            }
        }
    }
    let (win, sel) = (x.sel.win, atom_of(x, i));
    x.sel.one[i].to_x = to_x;
    x.sel.one[i].ours = true;
    x.conn.as_mut().unwrap().set_selection_owner(win, sel);
}

/// An X program wants to paste what a Wayland program copied.
fn request(x: &mut Xwm, d: &mut Desk, e: &[u8; 32]) {
    let (time, requestor, selection, target) = (u32_at(e, 4), u32_at(e, 12), u32_at(e, 16), u32_at(e, 20));
    // Old programs leave the property out: the target's name is used then.
    let prop = match u32_at(e, 24) {
        0 => target,
        p => p,
    };
    let a = &x.a;
    let (targets, timestamp, utf8, text) = (a.targets, a.timestamp, a.utf8_string, a.text);
    let i = index(x, selection).filter(|&i| x.sel.one[i].ours);
    let c = x.conn.as_mut().unwrap();
    let Some(i) = i else {
        c.selection_notify(requestor, selection, target, 0, time);
        return;
    };
    if target == targets {
        let mut list = vec![targets, timestamp];
        list.extend(x.sel.one[i].to_x.iter().map(|(t, _)| *t));
        c.change_property32(requestor, prop, x11::ATOM, &list);
        c.selection_notify(requestor, selection, target, prop, time);
        return;
    }
    if target == timestamp {
        c.change_property32(requestor, prop, x11::CARDINAL, &[0]);
        c.selection_notify(requestor, selection, target, prop, time);
        return;
    }
    let mime = x.sel.one[i].to_x.iter().find(|(t, _)| *t == target).map(|(_, m)| m.clone());
    let (Some(mime), Some(dk)) = (mime, current(d, i)) else {
        c.selection_notify(requestor, selection, target, 0, time);
        return;
    };
    let Ok((r, w)) = sys::pipe() else {
        c.selection_notify(requestor, selection, target, 0, time);
        return;
    };
    crate::server::source_send(d, dk, &mime, w);
    let id = x.sel.next_id;
    x.sel.next_id += 1;
    if d.ep.add(r.as_raw_fd(), TOK_XWM | (SUB_PIPE + id), sys::EPOLLIN).is_err() {
        x.conn.as_mut().unwrap().selection_notify(requestor, selection, target, 0, time);
        return;
    }
    let ty = if target == text { utf8 } else { target };
    let since = d.now_ms();
    x.sel.out.push(Out { id, requestor, prop, target, ty, selection, time, pipe: Some(r), buf: vec![], sent: None, since });
}

/// The source wrote into the pipe of transfer `id`.
pub fn pipe_readable(x: &mut Xwm, d: &mut Desk, id: u64) {
    let Some(i) = x.sel.out.iter().position(|o| o.id == id) else { return };
    let mut chunk = [0u8; 16384];
    let done = loop {
        let Some(p) = x.sel.out[i].pipe.as_ref() else { return };
        let n = unsafe { sys::ffi::read(p.as_raw_fd(), chunk.as_mut_ptr() as *mut _, chunk.len()) };
        if n > 0 {
            let o = &mut x.sel.out[i];
            o.buf.extend_from_slice(&chunk[..n as usize]);
            if o.buf.len() > MAX {
                break false;
            }
            continue;
        }
        if n == 0 {
            break true;
        }
        match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::WouldBlock => return,
            std::io::ErrorKind::Interrupted => {}
            _ => break false,
        }
    };
    if let Some(p) = x.sel.out[i].pipe.take() {
        d.ep.remove(p.as_raw_fd());
    }
    if !done {
        let o = &x.sel.out[i];
        let (r, s, t, tm) = (o.requestor, o.selection, o.target, o.time);
        x.conn.as_mut().unwrap().selection_notify(r, s, t, 0, tm);
        drop_out(x, d, id);
        return;
    }
    let limit = x.conn.as_ref().unwrap().setup.max_request.saturating_sub(64).min(256 * 1024);
    let watch = if x.wins.contains_key(&x.sel.out[i].requestor) { x11::EM_PROPERTY_CHANGE | x11::EM_FOCUS_CHANGE } else { x11::EM_PROPERTY_CHANGE };
    let incr = x.a.incr;
    let c = x.conn.as_mut().unwrap();
    let o = &mut x.sel.out[i];
    if o.buf.len() <= limit {
        c.change_property(o.requestor, o.prop, o.ty, 8, &o.buf);
        c.selection_notify(o.requestor, o.selection, o.target, o.prop, o.time);
        drop_out(x, d, id);
    } else {
        // Too big for one request: in pieces, each when the last one was taken.
        c.select_input(o.requestor, watch);
        c.change_property32(o.requestor, o.prop, incr, &[o.buf.len() as u32]);
        c.selection_notify(o.requestor, o.selection, o.target, o.prop, o.time);
        o.sent = Some(0);
        o.since = d.now_ms();
    }
}

fn next_piece(x: &mut Xwm, d: &mut Desk, i: usize) {
    let o = &mut x.sel.out[i];
    let pos = o.sent.unwrap_or(0);
    let end = (pos + CHUNK).min(o.buf.len());
    // The empty piece after the last one says "that was all".
    x.conn.as_mut().unwrap().change_property(o.requestor, o.prop, o.ty, 8, &o.buf[pos..end]);
    o.since = d.now_ms();
    if pos == o.buf.len() {
        let id = o.id;
        drop_out(x, d, id);
    } else {
        o.sent = Some(end);
    }
}

fn drop_out(x: &mut Xwm, d: &mut Desk, id: u64) {
    if let Some(i) = x.sel.out.iter().position(|o| o.id == id) {
        let o = x.sel.out.swap_remove(i);
        if let Some(p) = &o.pipe {
            d.ep.remove(p.as_raw_fd());
        }
    }
}
