//! X programs (Steam, games, older apps) as windows of this desktop: Xwayland draws them as
//! Wayland surfaces, and this is the X window manager that tells it where they go.
//!
//! Xwayland starts only when the first X program connects: we hold the display's sockets
//! (/tmp/.X11-unix/Xn and its abstract twin) and hand them to Xwayland then. Xwayland joins
//! as a Wayland client over a socket pair, and gives us a second pair end to manage it as
//! an X client (the X protocol itself is x11.rs).
//!
//! Each X window that gets shown gets a wl_surface from Xwayland, which names it to us in a
//! WL_SURFACE_ID message. Normal windows then become ordinary windows here (Window.x11),
//! with our frame, focus, snapping and taskbar button; menus and tooltips (override-redirect)
//! sit on top of their program's window where X puts them.
//!
//! Nothing here waits for Xwayland (it may be waiting for us): requests are queued, and
//! `sync` brings X up to date with our windows once per loop. Copy and paste between X and
//! Wayland programs is sel.rs.
mod sel;

use crate::desk::{Desk, TOK_KIND_MASK, TOK_XWM};
use crate::state::*;
use crate::x11::{self, u16_at, u32_at, Atom, Conn, Msg, Xid};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

const SUB_WM: u64 = 0;
const SUB_LISTEN: u64 = 1;
/// + transfer id: pipes of selection transfers (sel.rs).
const SUB_PIPE: u64 = 0x100;

macro_rules! atoms {
    ($($f:ident = $n:literal,)*) => {
        #[derive(Default)]
        struct Atoms { $($f: Atom,)* }
        const ATOM_NAMES: &[&str] = &[$($n,)*];
        impl Atoms {
            fn set(&mut self, i: usize, v: Atom) {
                let all: [&mut Atom; ATOM_NAMES.len()] = [$(&mut self.$f,)*];
                if let Some(a) = all.into_iter().nth(i) {
                    *a = v;
                }
            }
        }
    };
}

atoms! {
    wm_protocols = "WM_PROTOCOLS",
    wm_delete_window = "WM_DELETE_WINDOW",
    wm_take_focus = "WM_TAKE_FOCUS",
    wm_state = "WM_STATE",
    wm_change_state = "WM_CHANGE_STATE",
    wm_s0 = "WM_S0",
    wl_surface_id = "WL_SURFACE_ID",
    utf8_string = "UTF8_STRING",
    net_wm_name = "_NET_WM_NAME",
    net_supported = "_NET_SUPPORTED",
    net_supporting_wm_check = "_NET_SUPPORTING_WM_CHECK",
    net_active_window = "_NET_ACTIVE_WINDOW",
    net_close_window = "_NET_CLOSE_WINDOW",
    net_client_list = "_NET_CLIENT_LIST",
    net_wm_state = "_NET_WM_STATE",
    state_fullscreen = "_NET_WM_STATE_FULLSCREEN",
    state_max_vert = "_NET_WM_STATE_MAXIMIZED_VERT",
    state_max_horz = "_NET_WM_STATE_MAXIMIZED_HORZ",
    state_hidden = "_NET_WM_STATE_HIDDEN",
    state_focused = "_NET_WM_STATE_FOCUSED",
    net_wm_moveresize = "_NET_WM_MOVERESIZE",
    net_wm_window_type = "_NET_WM_WINDOW_TYPE",
    type_normal = "_NET_WM_WINDOW_TYPE_NORMAL",
    type_dialog = "_NET_WM_WINDOW_TYPE_DIALOG",
    type_utility = "_NET_WM_WINDOW_TYPE_UTILITY",
    type_splash = "_NET_WM_WINDOW_TYPE_SPLASH",
    type_dock = "_NET_WM_WINDOW_TYPE_DOCK",
    type_menu = "_NET_WM_WINDOW_TYPE_MENU",
    type_dropdown = "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
    type_popup = "_NET_WM_WINDOW_TYPE_POPUP_MENU",
    type_tooltip = "_NET_WM_WINDOW_TYPE_TOOLTIP",
    type_notification = "_NET_WM_WINDOW_TYPE_NOTIFICATION",
    type_combo = "_NET_WM_WINDOW_TYPE_COMBO",
    type_dnd = "_NET_WM_WINDOW_TYPE_DND",
    motif_wm_hints = "_MOTIF_WM_HINTS",
    net_wm_cm_s0 = "_NET_WM_CM_S0",
    clipboard = "CLIPBOARD",
    targets = "TARGETS",
    timestamp = "TIMESTAMP",
    incr = "INCR",
    text = "TEXT",
    wl_selection = "_WL_SELECTION",
    mime_utf8 = "text/plain;charset=utf-8",
    mime_text = "text/plain",
}

/// What a reply answers.
enum Tag {
    Atom(usize),
    Composite,
    Ignore,
    Tree,
    Attrs(Xid),
    Geometry(Xid),
    Prop(Xid, Atom),
    /// Marker after a window's properties: they have all been read, map it.
    Map(Xid),
    // Selections (sel.rs).
    XFixes,
    SelProp,
    AtomName(Atom),
    MimeAtom(String),
    TargetsReady(usize, u32),
    Own(usize, u32),
    // RandR: the main monitor for X programs.
    Randr,
    RandrRes,
    RandrOut(u32),
}

/// An X window at the top level (a child of the root).
struct XWin {
    or: bool,
    /// X geometry (root coords = our global logical coords).
    geo: (i32, i32, i32, i32),
    title: String,
    net_title: bool,
    class: String,
    transient: Xid,
    protocols: Vec<Atom>,
    decorated: bool,
    types: Vec<Atom>,
    /// Asked for before being shown (_NET_WM_STATE), or while not shown.
    want_full: bool,
    want_max: bool,
    min: (i32, i32),
    max: (i32, i32),
    /// WM_HINTS input: takes focus from SetInputFocus (else only WM_TAKE_FOCUS).
    input: bool,
    surf: Option<SKey>,
    wk: Option<WKey>,
    /// What X was last told (sync).
    sent_pos: Option<(i32, i32)>,
    sent_size: (i32, i32),
    sent_state: Option<Vec<Atom>>,
}

impl XWin {
    fn new(or: bool, geo: (i32, i32, i32, i32)) -> XWin {
        XWin {
            or,
            geo,
            title: String::new(),
            net_title: false,
            class: String::new(),
            transient: 0,
            protocols: vec![],
            decorated: true,
            types: vec![],
            want_full: false,
            want_max: false,
            min: (0, 0),
            max: (0, 0),
            input: true,
            surf: None,
            wk: None,
            sent_pos: None,
            sent_size: (geo.2, geo.3),
            sent_state: None,
        }
    }
}

pub struct Xwm {
    pub display: u32,
    bin: String,
    lock_path: String,
    sock_path: Option<String>,
    listeners: Vec<UnixListener>,
    listening: bool,
    child: Option<std::process::Child>,
    /// Xwayland as our Wayland client.
    pub client: Option<CKey>,
    conn: Option<Conn<Tag>>,
    want_out: bool,
    setup_done: bool,
    atoms_left: usize,
    composite_done: bool,
    ready: bool,
    a: Atoms,
    root: Xid,
    id_mask: u32,
    check: Xid,
    wins: HashMap<Xid, XWin>,
    /// WL_SURFACE_ID that came before its wl_surface: surface id → X window.
    pending: HashMap<u32, Xid>,
    focus: Xid,
    client_list: Vec<Xid>,
    sel: sel::Sels,
    /// RandR's major opcode (0: not there or not asked yet).
    randr: u8,
    /// The main monitor X has been told, with the monitor count then: told again on change.
    randr_told: Option<(String, usize)>,
    debug: bool,
}

// ------------------------------------------------------------------ setup

/// Picks a free display number and takes its sockets. Off with RESC_XWAYLAND=0.
pub fn init(d: &mut Desk) {
    if std::env::var("RESC_XWAYLAND").map_or(false, |v| v == "0") {
        return;
    }
    let Some(bin) = find_bin("Xwayland") else {
        eprintln!("xwayland: not installed (package xwayland): X programs (Steam, games) will not start");
        return;
    };
    for n in 1..64 {
        let Some((lock_path, sock_path, listeners)) = claim_display(n) else { continue };
        let mut x = Xwm {
            display: n,
            bin,
            lock_path,
            sock_path,
            listeners,
            listening: false,
            child: None,
            client: None,
            conn: None,
            want_out: false,
            setup_done: false,
            atoms_left: 0,
            composite_done: false,
            ready: false,
            a: Atoms::default(),
            root: 0,
            id_mask: 0,
            check: 0,
            wins: HashMap::new(),
            pending: HashMap::new(),
            focus: 0,
            client_list: vec![],
            sel: Default::default(),
            randr: 0,
            randr_told: None,
            debug: std::env::var("RESC_XWM_DEBUG").map_or(false, |v| !v.is_empty()),
        };
        x.listen(d);
        d.xwm = Some(Box::new(x));
        return;
    }
    eprintln!("xwayland: no free X display number");
}

fn find_bin(name: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    path.split(':').map(|dir| format!("{dir}/{name}")).find(|p| std::path::Path::new(p).is_file())
}

/// The lock file /tmp/.Xn-lock (with our pid, as X servers write it) and the sockets of
/// display n: the abstract one always, the path one when /tmp/.X11-unix lets us.
fn claim_display(n: u32) -> Option<(String, Option<String>, Vec<UnixListener>)> {
    use std::io::Write;
    use std::os::linux::net::SocketAddrExt;
    let lock = format!("/tmp/.X{n}-lock");
    let path = format!("/tmp/.X11-unix/X{n}");
    for attempt in 0..2 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut f) => {
                let _ = write!(f, "{:>10}\n", std::process::id());
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                // Left by a server that is gone? Then the number is free again.
                let pid = std::fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<u32>().ok());
                if let Some(p) = pid {
                    if std::path::Path::new(&format!("/proc/{p}")).exists() {
                        return None;
                    }
                }
                let _ = std::fs::remove_file(&lock);
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => return None,
        }
    }
    let give_up = || {
        let _ = std::fs::remove_file(&lock);
        None
    };
    // A socket with no lock: another kind of X server (WSLg's :0).
    if std::path::Path::new(&path).exists() {
        return give_up();
    }
    let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(path.as_bytes()) else { return give_up() };
    let Ok(abs) = UnixListener::bind_addr(&addr) else { return give_up() };
    let mut ls = vec![abs];
    if !std::path::Path::new("/tmp/.X11-unix").exists() && std::fs::create_dir("/tmp/.X11-unix").is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions("/tmp/.X11-unix", std::fs::Permissions::from_mode(0o1777));
    }
    let sock = match UnixListener::bind(&path) {
        Ok(l) => {
            ls.push(l);
            Some(path)
        }
        // Read-only (WSLg mounts it): programs find the abstract socket first anyway.
        Err(_) => None,
    };
    Some((lock, sock, ls))
}

impl Drop for Xwm {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.lock_path);
        if let Some(p) = &self.sock_path {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Xwm {
    fn listen(&mut self, d: &mut Desk) {
        for (i, l) in self.listeners.iter().enumerate() {
            let _ = d.ep.add(l.as_raw_fd(), TOK_XWM | (SUB_LISTEN + i as u64), sys::EPOLLIN);
        }
        self.listening = true;
    }

    fn unlisten(&mut self, d: &mut Desk) {
        for l in &self.listeners {
            d.ep.remove(l.as_raw_fd());
        }
        self.listening = false;
    }

    /// Starts Xwayland on our display sockets (an X program is knocking).
    fn launch(&mut self, d: &mut Desk) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        let (wl_ours, wl_theirs) = UnixStream::pair()?;
        let (wm_ours, wm_theirs) = UnixStream::pair()?;
        let mut keep: Vec<RawFd> = self.listeners.iter().map(|l| l.as_raw_fd()).collect();
        keep.push(wl_theirs.as_raw_fd());
        keep.push(wm_theirs.as_raw_fd());
        let mut cmd = std::process::Command::new(&self.bin);
        cmd.arg(format!(":{}", self.display)).arg("-rootless").arg("-core");
        for l in &self.listeners {
            cmd.arg("-listenfd").arg(l.as_raw_fd().to_string());
        }
        cmd.arg("-wm").arg(wm_theirs.as_raw_fd().to_string());
        cmd.env("WAYLAND_SOCKET", wl_theirs.as_raw_fd().to_string());
        cmd.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        cmd.stdin(std::process::Stdio::null());
        // The three sockets are the child's to keep (ours are all close-on-exec).
        unsafe {
            cmd.pre_exec(move || {
                for &f in &keep {
                    sys::set_cloexec(f, false);
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop(wl_theirs);
        drop(wm_theirs);
        self.child = Some(child);
        self.client = Some(d.add_client(wl_ours));
        let mut conn = Conn::new(wm_ours.into())?;
        conn.flush()?;
        d.ep.add(conn.fd(), TOK_XWM | SUB_WM, sys::EPOLLIN)?;
        self.conn = Some(conn);
        self.unlisten(d);
        eprintln!("xwayland: started on DISPLAY=:{}", self.display);
        Ok(())
    }

    /// Xwayland quit or broke: forget its windows, and wait for the next X program.
    fn gone(&mut self, d: &mut Desk, why: &str) {
        if self.conn.is_none() && self.client.is_none() && self.child.is_none() {
            return;
        }
        eprintln!("xwayland: stopped ({why}); it starts again with the next X program");
        let all: Vec<Xid> = self.wins.keys().copied().collect();
        for w in all {
            self.unassociate(d, w);
        }
        self.wins.clear();
        self.pending.clear();
        sel::reset(self, d);
        if let Some(c) = self.conn.take() {
            d.ep.remove(c.fd());
        }
        if let Some(ck) = self.client.take() {
            if let Some(c) = d.clients.get_mut(ck) {
                c.kill("xwayland stopped".into());
            }
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.setup_done = false;
        self.composite_done = false;
        self.ready = false;
        self.randr = 0;
        self.randr_told = None;
        self.want_out = false;
        self.focus = 0;
        self.client_list.clear();
        self.listen(d);
    }

    fn start(&mut self) {
        let c = self.conn.as_mut().unwrap();
        self.root = c.setup.root;
        self.id_mask = c.setup.id_mask;
        for (i, n) in ATOM_NAMES.iter().enumerate() {
            c.intern_atom(n, Tag::Atom(i));
        }
        self.atoms_left = ATOM_NAMES.len();
        c.query_extension("Composite", Tag::Composite);
    }

    /// Atoms and Composite are in: become the window manager.
    fn become_wm(&mut self) {
        let a = &self.a;
        let root = self.root;
        let c = self.conn.as_mut().unwrap();
        c.select_input(root, x11::EM_SUBSTRUCTURE_REDIRECT | x11::EM_SUBSTRUCTURE_NOTIFY | x11::EM_PROPERTY_CHANGE);
        let check = c.gen_id();
        self.check = check;
        c.create_window(check, root, -100, -100, 1, 1, true);
        c.change_property32(root, a.net_supporting_wm_check, x11::WINDOW, &[check]);
        c.change_property32(check, a.net_supporting_wm_check, x11::WINDOW, &[check]);
        c.change_property(check, a.net_wm_name, a.utf8_string, 8, b"resc-desk");
        let supported = [
            a.net_supporting_wm_check,
            a.net_active_window,
            a.net_close_window,
            a.net_client_list,
            a.net_wm_state,
            a.state_fullscreen,
            a.state_max_vert,
            a.state_max_horz,
            a.state_hidden,
            a.state_focused,
            a.net_wm_moveresize,
            a.net_wm_window_type,
            a.net_wm_name,
        ];
        c.change_property32(root, a.net_supported, x11::ATOM, &supported);
        c.change_property32(root, a.net_active_window, x11::WINDOW, &[0]);
        c.set_selection_owner(check, a.wm_s0);
        c.set_selection_owner(check, a.net_wm_cm_s0);
        // Windows made before we were listening (the program that started Xwayland).
        c.query_tree(root, Tag::Tree);
        c.query_extension("RANDR", Tag::Randr);
        sel::start(self);
        self.ready = true;
    }

    /// X's main monitor follows ours (RandR SetOutputPrimary): without one, X programs take
    /// the first monitor in the list, which is not the main one.
    fn tell_primary(&mut self, d: &Desk) {
        if self.randr == 0 || d.outputs.is_empty() {
            return;
        }
        let main = d.outputs[crate::monitors::primary_index(d).min(d.outputs.len() - 1)].name.clone();
        let want = (main, d.outputs.len());
        if self.randr_told.as_ref() == Some(&want) {
            return;
        }
        self.randr_told = Some(want);
        let (major, root) = (self.randr, self.root);
        self.conn().ext_reply(major, 25, &root.to_le_bytes(), Tag::RandrRes);
    }

    fn maybe_ready(&mut self) {
        if !self.ready && self.atoms_left == 0 && self.composite_done {
            self.become_wm();
        }
    }

    fn conn(&mut self) -> &mut Conn<Tag> {
        self.conn.as_mut().expect("xwm connection")
    }

    // -------------------------------------------------------------- reading

    fn readable(&mut self, d: &mut Desk, ev: u32) {
        let Some(conn) = self.conn.as_mut() else { return };
        if !self.setup_done {
            match conn.try_setup() {
                Ok(false) => {
                    if ev & (sys::EPOLLHUP | sys::EPOLLERR) != 0 {
                        self.gone(d, "closed before the X setup");
                    }
                    return;
                }
                Ok(true) => {
                    self.setup_done = true;
                    self.start();
                }
                Err(e) => {
                    self.gone(d, &e.to_string());
                    return;
                }
            }
        }
        let msgs = match self.conn.as_mut().unwrap().read() {
            Ok(m) => m,
            Err(e) => {
                self.gone(d, &e.to_string());
                return;
            }
        };
        for m in msgs {
            if self.conn.is_none() {
                break;
            }
            self.handle(d, m);
        }
    }

    fn handle(&mut self, d: &mut Desk, m: Msg<Tag>) {
        match m {
            Msg::Reply(Tag::Atom(i), r) => {
                self.a.set(i, u32_at(&r, 8));
                self.atoms_left -= 1;
                self.maybe_ready();
            }
            Msg::Reply(Tag::Composite, r) => {
                if r[8] != 0 {
                    let major = r[9];
                    let root = self.root;
                    let c = self.conn();
                    // QueryVersion first (0.4), then redirect the top-level windows: Xwayland
                    // hands their contents to us as surfaces.
                    c.ext_reply(major, 0, &[0, 0, 0, 0, 4, 0, 0, 0], Tag::Ignore);
                    let mut b = root.to_le_bytes().to_vec();
                    b.extend_from_slice(&[1, 0, 0, 0]);
                    c.ext(major, 2, &b);
                }
                self.composite_done = true;
                self.maybe_ready();
            }
            Msg::Reply(Tag::Ignore, _) => {}
            Msg::Reply(Tag::Randr, r) => {
                if r[8] != 0 {
                    self.randr = r[9];
                    // QueryVersion 1.6 before the newer requests.
                    self.conn().ext_reply(r[9], 0, &[1, 0, 0, 0, 6, 0, 0, 0], Tag::Ignore);
                }
            }
            Msg::Reply(Tag::RandrRes, r) => {
                // GetScreenResourcesCurrent: crtcs, then outputs; ask each output its name.
                let (nc, no) = (u16_at(&r, 16) as usize, u16_at(&r, 18) as usize);
                let ts = u32_at(&r, 12);
                let major = self.randr;
                for i in 0..no {
                    let at = 32 + (nc + i) * 4;
                    if at + 4 > r.len() {
                        break;
                    }
                    let out = u32_at(&r, at);
                    let mut b = out.to_le_bytes().to_vec();
                    b.extend_from_slice(&ts.to_le_bytes());
                    self.conn().ext_reply(major, 9, &b, Tag::RandrOut(out));
                }
            }
            Msg::Reply(Tag::RandrOut(out), r) => {
                // GetOutputInfo: the name after the crtcs, modes and clones lists.
                let (nc, nm, ncl, nl) = (u16_at(&r, 26) as usize, u16_at(&r, 28) as usize, u16_at(&r, 32) as usize, u16_at(&r, 34) as usize);
                let at = 36 + (nc + nm + ncl) * 4;
                let name = r.get(at..at + nl).map(|n| String::from_utf8_lossy(n).to_string());
                if name.is_some() && name.as_deref() == self.randr_told.as_ref().map(|t| t.0.as_str()) {
                    let mut b = self.root.to_le_bytes().to_vec();
                    b.extend_from_slice(&out.to_le_bytes());
                    let major = self.randr;
                    // SetOutputPrimary: X programs (Steam and the like) open there.
                    self.conn().ext(major, 30, &b);
                }
            }
            Msg::Reply(Tag::Tree, r) => {
                let n = u16_at(&r, 16) as usize;
                let kids: Vec<Xid> = (0..n).map(|i| u32_at(&r, 32 + i * 4)).filter(|w| *w != self.check).collect();
                for w in kids {
                    if self.wins.contains_key(&w) {
                        continue;
                    }
                    self.wins.insert(w, XWin::new(false, (0, 0, 1, 1)));
                    self.watch(w);
                    let c = self.conn();
                    c.get_geometry(w, Tag::Geometry(w));
                    c.get_window_attributes(w, Tag::Attrs(w));
                }
            }
            Msg::Reply(Tag::Geometry(w), r) => {
                if let Some(xw) = self.wins.get_mut(&w) {
                    xw.geo = (x11::i16_at(&r, 12) as i32, x11::i16_at(&r, 14) as i32, u16_at(&r, 16) as i32, u16_at(&r, 18) as i32);
                    xw.sent_size = (xw.geo.2, xw.geo.3);
                }
            }
            Msg::Reply(Tag::Attrs(w), r) => {
                let (viewable, or) = (r[26] == 2, r[27] != 0);
                if let Some(xw) = self.wins.get_mut(&w) {
                    xw.or = or;
                    if viewable && !or {
                        let a = self.a.wm_state;
                        self.conn().change_property32(w, a, a, &[1, 0]);
                    }
                }
            }
            Msg::Reply(Tag::Prop(w, atom), r) => self.property(d, w, atom, &r),
            Msg::Reply(t @ (Tag::XFixes | Tag::SelProp | Tag::AtomName(_) | Tag::MimeAtom(_) | Tag::TargetsReady(..) | Tag::Own(..)), r) => {
                sel::reply(self, d, t, &r)
            }
            Msg::Reply(Tag::Map(w), _) => {
                if self.wins.contains_key(&w) {
                    let a = self.a.wm_state;
                    let c = self.conn();
                    c.change_property32(w, a, a, &[1, 0]);
                    c.map_window(w);
                }
            }
            Msg::Error(tag, code, major, minor) => {
                // Mostly requests about windows that were destroyed meanwhile.
                if self.debug {
                    let what = match tag {
                        Some(Tag::Prop(w, a)) => format!("property {a} of {w:#x}"),
                        Some(_) => "a request with a reply".into(),
                        None => "a request".into(),
                    };
                    eprintln!("xwm: X error {code} for {what} (opcode {major}.{minor})");
                }
            }
            Msg::Event(code, e) => self.event(d, code, &e),
        }
    }

    /// Reads everything we want to know about a top-level window, and hears of changes.
    fn watch(&mut self, w: Xid) {
        let a = &self.a;
        let props = [
            x11::WM_NAME,
            a.net_wm_name,
            x11::WM_CLASS,
            x11::WM_TRANSIENT_FOR,
            a.wm_protocols,
            x11::WM_HINTS,
            x11::WM_NORMAL_HINTS,
            a.motif_wm_hints,
            a.net_wm_window_type,
            a.net_wm_state,
        ];
        let c = self.conn.as_mut().unwrap();
        c.select_input(w, x11::EM_PROPERTY_CHANGE | x11::EM_FOCUS_CHANGE);
        for p in props {
            c.get_property(w, p, Tag::Prop(w, p));
        }
    }

    fn event(&mut self, d: &mut Desk, code: u8, e: &[u8; 32]) {
        if sel::event(self, d, code, e) {
            return;
        }
        if self.debug {
            eprintln!("xwm: event {code} window {:#x}", u32_at(e, if code == x11::PROPERTY_NOTIFY || code == x11::CLIENT_MESSAGE { 4 } else { 8 }));
        }
        match code {
            x11::CREATE_NOTIFY => {
                let (parent, w) = (u32_at(e, 4), u32_at(e, 8));
                if parent != self.root || w == self.check {
                    return;
                }
                let geo = (x11::i16_at(e, 12) as i32, x11::i16_at(e, 14) as i32, u16_at(e, 16) as i32, u16_at(e, 18) as i32);
                self.wins.insert(w, XWin::new(e[22] != 0, geo));
                self.watch(w);
            }
            x11::DESTROY_NOTIFY => {
                let w = u32_at(e, 8);
                self.unassociate(d, w);
                self.wins.remove(&w);
                self.pending.retain(|_, v| *v != w);
            }
            x11::UNMAP_NOTIFY => {
                let w = u32_at(e, 8);
                let Some(xw) = self.wins.get(&w) else { return };
                let or = xw.or;
                self.unassociate(d, w);
                if !or {
                    let a = self.a.wm_state;
                    self.conn().change_property32(w, a, a, &[0, 0]);
                }
            }
            x11::MAP_REQUEST => {
                // Map once the properties asked for at creation are in (replies come in order).
                let w = u32_at(e, 8);
                if self.wins.contains_key(&w) {
                    self.conn().sync(Tag::Map(w));
                } else {
                    self.conn().map_window(w);
                }
            }
            x11::CONFIGURE_NOTIFY => {
                let w = u32_at(e, 8);
                let geo = (x11::i16_at(e, 16) as i32, x11::i16_at(e, 18) as i32, u16_at(e, 20) as i32, u16_at(e, 22) as i32);
                let Some(xw) = self.wins.get_mut(&w) else { return };
                xw.geo = geo;
                xw.or = e[26] != 0;
                // A menu or tooltip that moved.
                if let Some(sk) = xw.surf.filter(|k| d.surfaces.get(*k).map_or(false, |s| s.x_pos.is_some())) {
                    d.damage_surface_tree(sk);
                    d.surfaces[sk].x_pos = Some((geo.0, geo.1));
                    d.damage_surface_tree(sk);
                }
            }
            x11::CONFIGURE_REQUEST => self.configure_request(d, e),
            x11::PROPERTY_NOTIFY => {
                let (w, atom) = (u32_at(e, 4), u32_at(e, 8));
                if self.wins.contains_key(&w) && self.interesting(atom) {
                    self.conn().get_property(w, atom, Tag::Prop(w, atom));
                }
            }
            x11::CLIENT_MESSAGE => self.client_message(d, e),
            _ => {}
        }
    }

    fn interesting(&self, atom: Atom) -> bool {
        let a = &self.a;
        [
            x11::WM_NAME,
            a.net_wm_name,
            x11::WM_CLASS,
            x11::WM_TRANSIENT_FOR,
            a.wm_protocols,
            x11::WM_HINTS,
            x11::WM_NORMAL_HINTS,
            a.motif_wm_hints,
            a.net_wm_window_type,
        ]
        .contains(&atom)
    }

    fn property(&mut self, d: &mut Desk, w: Xid, atom: Atom, r: &[u8]) {
        let a = &self.a;
        let Some(xw) = self.wins.get_mut(&w) else { return };
        let (_, _, v) = x11::property_value(r);
        let nums = x11::values32(v);
        if atom == x11::WM_NAME {
            if !xw.net_title {
                xw.title = v.iter().map(|&b| b as char).collect();
            }
        } else if atom == a.net_wm_name {
            if !v.is_empty() {
                xw.title = String::from_utf8_lossy(v).into_owned();
                xw.net_title = true;
            }
        } else if atom == x11::WM_CLASS {
            // "instance\0class\0": the class names the program (desktop files use it).
            let parts: Vec<&[u8]> = v.split(|b| *b == 0).filter(|p| !p.is_empty()).collect();
            if let Some(p) = parts.get(1).or(parts.first()) {
                xw.class = String::from_utf8_lossy(p).into_owned();
            }
        } else if atom == x11::WM_TRANSIENT_FOR {
            xw.transient = nums.first().copied().unwrap_or(0);
        } else if atom == a.wm_protocols {
            xw.protocols = nums;
        } else if atom == x11::WM_HINTS {
            if nums.len() >= 2 && nums[0] & 1 != 0 {
                xw.input = nums[1] != 0;
            }
        } else if atom == x11::WM_NORMAL_HINTS {
            // flags, x, y, w, h, min w/h (PMinSize 16), max w/h (PMaxSize 32), ...
            if nums.len() >= 9 {
                if nums[0] & 16 != 0 {
                    xw.min = (nums[5] as i32, nums[6] as i32);
                }
                if nums[0] & 32 != 0 {
                    xw.max = (nums[7] as i32, nums[8] as i32);
                }
            }
        } else if atom == a.motif_wm_hints {
            // flags, functions, decorations, ...: MWM_HINTS_DECORATIONS = 2.
            xw.decorated = !(nums.len() >= 3 && nums[0] & 2 != 0 && nums[2] == 0);
        } else if atom == a.net_wm_window_type {
            xw.types = nums;
        } else if atom == a.net_wm_state {
            xw.want_full = nums.contains(&a.state_fullscreen);
            xw.want_max = nums.contains(&a.state_max_vert) && nums.contains(&a.state_max_horz);
            return;
        }
        // Shown already: the window here follows.
        let Some(wk) = xw.wk.filter(|k| d.windows.contains(*k)) else { return };
        let ssd = self.decorated(w);
        let parent = self.wins.get(&self.wins[&w].transient).and_then(|t| t.wk).filter(|k| d.windows.contains(*k));
        let xw = &self.wins[&w];
        let old_frame = crate::wm::frame_rect(d, wk);
        let win = &mut d.windows[wk];
        let changed_title = win.title != xw.title;
        win.title = xw.title.clone();
        win.app_id = xw.class.clone();
        win.min_size = xw.min;
        win.max_size = xw.max;
        win.parent = parent;
        let ssd_changed = win.ssd != ssd;
        win.ssd = ssd;
        if ssd_changed {
            if let Some(r) = old_frame {
                d.damage(r.inflate(crate::shell::SHADOW));
            }
            crate::wm::damage_window(d, wk);
        } else if changed_title {
            if let Some(f) = crate::wm::frame_rect(d, wk) {
                d.damage(crate::geom::Rect::new(f.x, f.y, f.w, crate::shell::TITLE_H + crate::shell::BORDER));
            }
        }
        crate::shell::damage_taskbars(d);
    }

    /// Gets our frame: unless the program draws its own (Motif hints) or it is not a window
    /// of its own kind (splash, dock, ...).
    fn decorated(&self, w: Xid) -> bool {
        let a = &self.a;
        let Some(xw) = self.wins.get(&w) else { return false };
        let bare = [a.type_splash, a.type_dock, a.type_notification, a.type_dnd];
        xw.decorated && !xw.types.iter().any(|t| bare.contains(t))
    }

    /// A menu, tooltip or the like: sits over its program's window, no frame, no focus.
    fn popup_like(&self, w: Xid) -> bool {
        let a = &self.a;
        let Some(xw) = self.wins.get(&w) else { return false };
        let kinds = [a.type_menu, a.type_dropdown, a.type_popup, a.type_tooltip, a.type_combo, a.type_dnd];
        xw.or || xw.types.iter().any(|t| kinds.contains(t))
    }

    fn configure_request(&mut self, d: &mut Desk, e: &[u8; 32]) {
        let w = u32_at(e, 8);
        let mask = u16_at(e, 26);
        let (rx, ry) = (x11::i16_at(e, 16) as i32, x11::i16_at(e, 18) as i32);
        let (rw, rh) = (u16_at(e, 20) as i32, u16_at(e, 22) as i32);
        let Some(xw) = self.wins.get_mut(&w) else {
            // Not ours to manage: as asked.
            let vals: Vec<u32> = [(x11::CW_X, rx as u32), (x11::CW_Y, ry as u32), (x11::CW_WIDTH, rw as u32), (x11::CW_HEIGHT, rh as u32)]
                .iter()
                .filter(|(b, _)| mask & b != 0)
                .map(|(_, v)| *v)
                .collect();
            self.conn().configure_window(w, mask & 15, &vals);
            return;
        };
        let wk = xw.wk.filter(|k| d.windows.contains(*k));
        let Some(wk) = wk else {
            // Not shown yet: it may have the size and place it wants (placement comes at map).
            let mut g = xw.geo;
            if mask & x11::CW_X != 0 {
                g.0 = rx;
            }
            if mask & x11::CW_Y != 0 {
                g.1 = ry;
            }
            if mask & x11::CW_WIDTH != 0 {
                g.2 = rw.max(1);
            }
            if mask & x11::CW_HEIGHT != 0 {
                g.3 = rh.max(1);
            }
            xw.geo = g;
            xw.sent_size = (g.2, g.3);
            let vals = [g.0 as u32, g.1 as u32, g.2 as u32, g.3 as u32, 0];
            self.conn().configure_window(w, x11::CW_X | x11::CW_Y | x11::CW_WIDTH | x11::CW_HEIGHT | x11::CW_BORDER, &vals);
            return;
        };
        // Shown: a floating window may change its size and move itself; a maximized, snapped
        // or fullscreen one stays as it is. Either way it hears where it is now.
        if d.windows[wk].tile == Tile::Floating {
            let (cw, ch) = xw.sent_size;
            let size = (if mask & x11::CW_WIDTH != 0 { rw.max(1) } else { cw }, if mask & x11::CW_HEIGHT != 0 { rh.max(1) } else { ch });
            if size != (cw, ch) {
                d.windows[wk].last_cfg = (size.0, size.1, vec![]);
            }
            if mask & (x11::CW_X | x11::CW_Y) != 0 {
                let win = &d.windows[wk];
                let to = (if mask & x11::CW_X != 0 { rx } else { win.x }, if mask & x11::CW_Y != 0 { ry } else { win.y });
                if to != (win.x, win.y) {
                    crate::wm::damage_window(d, wk);
                    let win = &mut d.windows[wk];
                    (win.x, win.y) = to;
                    crate::wm::damage_window(d, wk);
                    crate::wm::update_window_outputs(d, wk);
                }
            }
        }
        let (gx, gy) = xw.sent_pos.unwrap_or((xw.geo.0, xw.geo.1));
        let (gw, gh) = xw.sent_size;
        self.conn().configure_notify(w, gx, gy, gw as u32, gh as u32);
    }

    fn client_message(&mut self, d: &mut Desk, e: &[u8; 32]) {
        let (w, ty) = (u32_at(e, 4), u32_at(e, 8));
        let v: [u32; 5] = std::array::from_fn(|i| u32_at(e, 12 + i * 4));
        if ty == self.a.wl_surface_id {
            self.surface_id(d, w, v[0]);
            return;
        }
        let a = &self.a;
        let Some(xw) = self.wins.get_mut(&w) else { return };
        let wk = xw.wk.filter(|k| d.windows.contains(*k));
        if ty == a.net_wm_state {
            // action (0 remove, 1 add, 2 toggle), two properties.
            let has = |p: Atom| v[1] == p || v[2] == p;
            let pick = |now: bool| match v[0] {
                0 => false,
                1 => true,
                _ => !now,
            };
            if has(a.state_fullscreen) {
                let now = wk.map_or(xw.want_full, |k| d.windows[k].tile == Tile::Fullscreen);
                let on = pick(now);
                match wk {
                    Some(k) => crate::wm::fullscreen(d, k, on, None),
                    None => xw.want_full = on,
                }
            }
            if has(a.state_max_vert) || has(a.state_max_horz) {
                let now = wk.map_or(xw.want_max, |k| d.windows[k].tile == Tile::Maximized);
                let on = pick(now);
                match wk {
                    Some(k) if on && !now => crate::wm::maximize(d, k),
                    Some(k) if !on && now => crate::wm::restore(d, k),
                    Some(_) => {}
                    None => xw.want_max = on,
                }
            }
        } else if ty == a.net_active_window {
            if let Some(k) = wk {
                crate::wm::unminimize(d, k);
            }
        } else if ty == a.net_wm_moveresize {
            // Direction: 0..7 a corner or side clockwise from top-left, 8 move.
            const EDGES: [u32; 8] = [5, 1, 9, 8, 10, 2, 6, 4];
            if let Some(k) = wk {
                match v[2] {
                    8 => crate::input::begin_move(d, k),
                    i @ 0..=7 => crate::input::begin_resize(d, k, EDGES[i as usize]),
                    _ => {}
                }
            }
        } else if ty == a.wm_change_state {
            if v[0] == 3 {
                if let Some(k) = wk {
                    crate::wm::minimize(d, k);
                }
            }
        } else if ty == a.net_close_window {
            if let Some(k) = wk {
                crate::wm::close(d, k);
            }
        }
    }

    // -------------------------------------------------------------- surfaces

    /// WL_SURFACE_ID: Xwayland's surface `sid` shows the X window `w`.
    fn surface_id(&mut self, d: &mut Desk, w: Xid, sid: u32) {
        let Some(ck) = self.client else { return };
        if !self.wins.contains_key(&w) {
            return;
        }
        let found = d
            .surfaces
            .iter()
            .find(|(_, s)| s.client == ck && s.id == sid && s.xwin == 0 && s.role == Role::None && !s.destroyed)
            .map(|(k, _)| k);
        match found {
            Some(sk) => self.associate(d, w, sk),
            None => {
                self.pending.insert(sid, w);
            }
        }
    }

    fn associate(&mut self, d: &mut Desk, w: Xid, sk: SKey) {
        self.pending.retain(|_, v| *v != w);
        if self.wins.get(&w).map_or(true, |x| x.surf.is_some()) {
            return;
        }
        d.surfaces[sk].xwin = w;
        self.wins.get_mut(&w).unwrap().surf = Some(sk);
        if self.popup_like(w) {
            if let Some(parent) = self.pick_parent(d, w) {
                let xw = &self.wins[&w];
                let s = &mut d.surfaces[sk];
                s.role = Role::Popup;
                s.popup = Some(PopupInfo {
                    parent,
                    x: 0,
                    y: 0,
                    w: xw.geo.2,
                    h: xw.geo.3,
                    obj: 0,
                    grab: false,
                    positioner: Default::default(),
                });
                s.x_pos = Some((xw.geo.0, xw.geo.1));
                s.mapped = s.buffer.is_some();
                d.surfaces[parent].popups.push(sk);
                d.damage_surface_tree(sk);
                if let Some(wk) = d.window_of(sk) {
                    crate::wm::update_window_outputs(d, wk);
                }
                return;
            }
            // No window of its program to sit on: shown as a window of its own.
        }
        let ck = d.surfaces[sk].client;
        crate::wm::create_window(d, ck, sk, 0);
        let Role::Toplevel(wk) = d.surfaces[sk].role else { return };
        let ssd = self.decorated(w) && !self.wins[&w].or;
        let parent = self.wins.get(&self.wins[&w].transient).and_then(|t| t.wk).filter(|k| d.windows.contains(*k));
        let xw = self.wins.get_mut(&w).unwrap();
        xw.wk = Some(wk);
        xw.sent_pos = None;
        xw.sent_state = None;
        let win = &mut d.windows[wk];
        win.x11 = w;
        win.title = xw.title.clone();
        win.app_id = xw.class.clone();
        win.min_size = xw.min;
        win.max_size = xw.max;
        win.parent = parent;
        win.ssd = ssd;
        // No xdg handshake: it is shown as soon as it has a picture.
        let s = &mut d.surfaces[sk];
        s.xdg_initial_sent = true;
        s.xdg_configured = true;
        if s.buffer.is_some() {
            crate::wm::toplevel_commit(d, wk, None);
            if d.surfaces[sk].mapped {
                self.after_map(d, wk);
            }
        }
        self.update_client_list(d);
    }

    /// The window a menu of `w` belongs to: its transient-for, else the last focused window
    /// of the same program, else the focused X window.
    fn pick_parent(&self, d: &Desk, w: Xid) -> Option<SKey> {
        let top = |sk: SKey| d.window_of(sk).map(|wk| d.windows[wk].surf);
        if let Some(t) = self.wins.get(&self.wins[&w].transient).and_then(|t| t.surf) {
            if let Some(s) = top(t) {
                return Some(s);
            }
        }
        let owner = w & !self.id_mask;
        let mru_x = || d.mru.iter().filter_map(|k| d.windows.get(*k)).filter(|win| win.x11 != 0);
        mru_x()
            .find(|win| win.x11 & !self.id_mask == owner)
            .or_else(|| mru_x().next())
            .map(|win| win.surf)
            .filter(|sk| d.surfaces.get(*sk).map_or(false, |s| s.mapped))
    }

    /// The X window `w` is not shown any more (unmapped, destroyed, or Xwayland gone).
    fn unassociate(&mut self, d: &mut Desk, w: Xid) {
        let Some(xw) = self.wins.get_mut(&w) else { return };
        let (sk, wk) = (xw.surf.take(), xw.wk.take());
        if let Some(wk) = wk.filter(|k| d.windows.contains(*k)) {
            let surf = d.windows[wk].surf;
            crate::wm::destroy_window(d, wk);
            if let Some(s) = d.surfaces.get_mut(surf) {
                s.role = Role::None;
                s.mapped = false;
            }
        }
        if let Some(sk) = sk.filter(|k| d.surfaces.contains(*k)) {
            if d.surfaces[sk].role == Role::Popup {
                crate::server::unmap_popup(d, sk);
                let s = &mut d.surfaces[sk];
                s.role = Role::None;
                s.popup = None;
                s.x_pos = None;
            }
        }
        if self.focus == w {
            self.focus = 0;
        }
        if wk.is_some() {
            self.update_client_list(d);
        }
    }

    /// Shown for the first time: what it asked for, or what it is (a borderless window the
    /// size of a monitor is a game in fullscreen).
    fn after_map(&mut self, d: &mut Desk, wk: WKey) {
        let w = d.windows[wk].x11;
        let Some(xw) = self.wins.get(&w) else { return };
        if xw.want_full {
            crate::wm::fullscreen(d, wk, true, None);
        } else if xw.want_max {
            crate::wm::maximize(d, wk);
        } else if !d.windows[wk].ssd {
            let g = d.surfaces[d.windows[wk].surf].geom();
            let at = d.output_at(xw.geo.0 as f64 + 1.0, xw.geo.1 as f64 + 1.0);
            let fits = |oi: usize| d.outputs.get(oi).map_or(false, |o| g.w >= o.lw() && g.h >= o.lh());
            let oi = at.filter(|&oi| fits(oi)).or_else(|| (0..d.outputs.len()).find(|&oi| fits(oi)));
            if let Some(oi) = oi {
                crate::wm::fullscreen(d, wk, true, Some(oi));
            }
        }
    }

    fn update_client_list(&mut self, d: &Desk) {
        let list: Vec<Xid> = d.stack.iter().filter_map(|k| d.windows.get(*k)).map(|w| w.x11).filter(|x| *x != 0).collect();
        if list != self.client_list && self.ready {
            let (root, a) = (self.root, self.a.net_client_list);
            self.conn().change_property32(root, a, x11::WINDOW, &list);
            self.client_list = list;
        }
    }

    // -------------------------------------------------------------- our side → X

    /// Tells X where its windows are now, how big, in what state, and which has focus.
    fn sync_windows(&mut self, d: &mut Desk) {
        let a = &self.a;
        let mut reqs: Vec<(Xid, Option<(i32, i32, i32, i32)>, Option<Vec<Atom>>)> = Vec::new();
        for (&xid, xw) in self.wins.iter_mut() {
            let Some(wk) = xw.wk.filter(|k| d.windows.contains(*k)) else { continue };
            if !d.surfaces.get(d.windows[wk].surf).map_or(false, |s| s.mapped) {
                continue;
            }
            // A move decided together with a resize (maximize, snap): X resizes at once,
            // so the place goes along at once too.
            if let Some((_, px, py)) = d.windows[wk].pending_pos.take() {
                crate::wm::damage_window(d, wk);
                let win = &mut d.windows[wk];
                (win.x, win.y) = (px, py);
                crate::wm::damage_window(d, wk);
                crate::wm::update_window_outputs(d, wk);
            }
            let win = &d.windows[wk];
            let size = if win.last_cfg.0 > 0 && win.last_cfg.1 > 0 { (win.last_cfg.0, win.last_cfg.1) } else { xw.sent_size };
            let pos = (win.x, win.y);
            let geo = if xw.sent_pos != Some(pos) || xw.sent_size != size {
                xw.sent_pos = Some(pos);
                xw.sent_size = size;
                Some((pos.0, pos.1, size.0, size.1))
            } else {
                None
            };
            let mut st = Vec::new();
            match win.tile {
                Tile::Fullscreen => st.push(a.state_fullscreen),
                Tile::Maximized => st.extend([a.state_max_vert, a.state_max_horz]),
                _ => {}
            }
            if win.minimized {
                st.push(a.state_hidden);
            }
            if win.activated {
                st.push(a.state_focused);
            }
            let state = if xw.sent_state.as_ref() != Some(&st) {
                xw.sent_state = Some(st.clone());
                Some(st)
            } else {
                None
            };
            if geo.is_some() || state.is_some() {
                reqs.push((xid, geo, state));
            }
        }
        let net_wm_state = self.a.net_wm_state;
        let c = self.conn.as_mut().unwrap();
        for (xid, geo, state) in reqs {
            if let Some((x, y, w, h)) = geo {
                let m = x11::CW_X | x11::CW_Y | x11::CW_WIDTH | x11::CW_HEIGHT | x11::CW_BORDER;
                c.configure_window(xid, m, &[x as u32, y as u32, w.max(1) as u32, h.max(1) as u32, 0]);
                c.configure_notify(xid, x, y, w.max(1) as u32, h.max(1) as u32);
            }
            if let Some(st) = state {
                c.change_property32(xid, net_wm_state, x11::ATOM, &st);
            }
        }
        // Keyboard focus: X needs to be told too (Xwayland only sees which surface has it).
        let want = crate::wm::focused(d).and_then(|k| d.windows.get(k)).map_or(0, |w| w.x11);
        if want != self.focus {
            self.focus = want;
            let (root, active, take, protocols) = (self.root, self.a.net_active_window, self.a.wm_take_focus, self.a.wm_protocols);
            let (input, takes) = self.wins.get(&want).map_or((true, false), |x| (x.input, x.protocols.contains(&take)));
            let c = self.conn.as_mut().unwrap();
            if want != 0 {
                if input {
                    c.set_input_focus(want);
                }
                if takes {
                    c.client_message(want, protocols, [take, 0, 0, 0, 0]);
                }
                c.configure_window(want, x11::CW_STACK_MODE, &[0]);
            } else {
                c.set_input_focus(0);
            }
            c.change_property32(root, active, x11::WINDOW, &[want]);
        }
        self.update_client_list(d);
    }

    fn flush(&mut self, d: &mut Desk) {
        let Some(c) = self.conn.as_mut() else { return };
        if let Err(e) = c.flush() {
            self.gone(d, &e.to_string());
            return;
        }
        let want = c.pending_out() > 0;
        if want != self.want_out {
            self.want_out = want;
            let ev = if want { sys::EPOLLIN | sys::EPOLLOUT } else { sys::EPOLLIN };
            let _ = d.ep.modify(c.fd(), TOK_XWM | SUB_WM, ev);
        }
    }
}

// ------------------------------------------------------------------ entry points

pub fn dispatch(d: &mut Desk, tok: u64, ev: u32) {
    let sub = tok & !TOK_KIND_MASK;
    if sub >= SUB_PIPE {
        let Some(mut x) = d.xwm.take() else { return };
        sel::pipe_readable(&mut x, d, sub - SUB_PIPE);
        x.flush(d);
        d.xwm = Some(x);
        return;
    }
    if sub >= SUB_LISTEN {
        let Some(mut x) = d.xwm.take() else { return };
        if x.listening {
            if let Err(e) = x.launch(d) {
                eprintln!("xwayland: cannot start {} ({e})", x.bin);
                // Turn the waiting programs away rather than leave them hanging.
                for l in &x.listeners {
                    let _ = l.set_nonblocking(true);
                    while l.accept().is_ok() {}
                }
            }
        }
        d.xwm = Some(x);
        return;
    }
    // Xwayland's Wayland requests first: the surfaces its X messages name should exist.
    if let Some(ck) = d.xwm.as_ref().and_then(|x| x.client) {
        if d.clients.contains(ck) {
            crate::server::client_readable(d, ck);
        }
    }
    let Some(mut x) = d.xwm.take() else { return };
    if ev & sys::EPOLLIN != 0 || ev & (sys::EPOLLHUP | sys::EPOLLERR) != 0 {
        x.readable(d, ev);
    }
    x.flush(d);
    d.xwm = Some(x);
}

/// Once per loop: X learns what changed on our side.
pub fn sync(d: &mut Desk) {
    let Some(mut x) = d.xwm.take() else { return };
    if x.ready && x.conn.is_some() {
        x.tell_primary(d);
        x.sync_windows(d);
        sel::tick(&mut x, d);
    } else {
        // Nobody to ask: those waiting for X's data get an empty pipe.
        d.x_receives.clear();
    }
    x.flush(d);
    d.xwm = Some(x);
}

/// The clipboard (or, `primary`, the middle-button selection) changed here.
pub fn selection_changed(d: &mut Desk, primary: bool) {
    // None while xwm itself sets it (for what an X program copied).
    let Some(mut x) = d.xwm.take() else { return };
    if x.ready && x.conn.is_some() {
        sel::wayland_changed(&mut x, d, primary as usize);
    }
    d.xwm = Some(x);
}

/// A new surface of Xwayland's: maybe the one an X window already named.
pub fn surface_created(d: &mut Desk, sk: SKey) {
    let Some(mut x) = d.xwm.take() else { return };
    let sid = d.surfaces[sk].id;
    if let Some(w) = x.pending.remove(&sid) {
        x.associate(d, w, sk);
    }
    d.xwm = Some(x);
}

/// An X window was shown (wm::map_window).
pub fn window_mapped(d: &mut Desk, wk: WKey) {
    // None while xwm itself is mapping it (`associate` does this part then).
    let Some(mut x) = d.xwm.take() else { return };
    x.after_map(d, wk);
    d.xwm = Some(x);
}

/// Close button, Alt+F4: politely when the program understands WM_DELETE_WINDOW.
pub fn close(d: &mut Desk, w: Xid) {
    let Some(x) = d.xwm.as_mut() else { return };
    let (proto, del) = (x.a.wm_protocols, x.a.wm_delete_window);
    let polite = x.wins.get(&w).map_or(false, |xw| xw.protocols.contains(&del));
    let Some(c) = x.conn.as_mut() else { return };
    if polite {
        c.client_message(w, proto, [del, 0, 0, 0, 0]);
    } else {
        c.kill_client(w);
    }
}

pub fn xwayland_gone(d: &mut Desk, why: &str) {
    let Some(mut x) = d.xwm.take() else { return };
    x.client = None;
    x.gone(d, why);
    d.xwm = Some(x);
}
