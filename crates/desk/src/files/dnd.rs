//! Drag and drop with other windows and programs (the system drag, wl_data_device):
//! files leave «Проводник» as text/uri-list, files come in from anywhere, and tabs move
//! between «Проводник» windows: dragged out they become a window of their own, dropped on
//! another window's tab strip they become one of its tabs.
use super::*;

pub const TAB_MIME: &str = "application/x-resc-files-tab";
pub const URI_MIME: &str = "text/uri-list";
pub const COPY: u32 = 1;
pub const MOVE: u32 = 2;
pub const ASK: u32 = 4;

/// A drag over our window from the system drag.
pub struct Incoming {
    pub paths: Vec<PathBuf>,
    pub tab: Option<Tab>,
    /// Our own drag come back.
    pub own: bool,
    /// What we told the compositor last (mime, actions, preferred).
    pub told: Option<(Option<&'static str>, u32, u32)>,
}

/// Our drag that went out of the window.
pub struct Outgoing {
    pub paths: Vec<PathBuf>,
    /// A tab being dragged (its index).
    pub tab: Option<usize>,
    /// Back over our own window now.
    pub over_self: bool,
    /// Where in the window the tab was grabbed (a torn-off window opens with it there).
    pub grab: (i32, i32),
}

/// file:// URIs (or plain paths) → paths that exist.
pub fn parse_uris(data: &[u8]) -> Vec<PathBuf> {
    let t = String::from_utf8_lossy(data);
    t.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let p = if let Some(r) = l.strip_prefix("file://") {
                // file://host/path: the host part is empty or ours.
                let r = &r[r.find('/')?..];
                unescape(r)
            } else if l.starts_with('/') {
                l.to_string()
            } else {
                return None;
            };
            let p = PathBuf::from(p);
            std::fs::symlink_metadata(&p).is_ok().then_some(p)
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// A place as one line: dir:/path, pc:, trash:, arc:/file\x1finner, search:/base\x1fquery.
pub fn loc_str(l: &Loc) -> String {
    match l {
        Loc::Dir(p) => format!("dir:{}", p.display()),
        Loc::Pc => "pc:".into(),
        Loc::Trash => "trash:".into(),
        Loc::Arc(f, i) => format!("arc:{}\x1f{i}", f.display()),
        Loc::Search(b, q) => format!("search:{}\x1f{q}", b.display()),
    }
}

pub fn loc_parse(s: &str) -> Option<Loc> {
    let (k, v) = s.split_once(':')?;
    Some(match k {
        "dir" => Loc::Dir(v.into()),
        "pc" => Loc::Pc,
        "trash" => Loc::Trash,
        "arc" => {
            let (f, i) = v.split_once('\x1f')?;
            Loc::Arc(f.into(), i.into())
        }
        "search" => {
            let (b, q) = v.split_once('\x1f')?;
            Loc::Search(b.into(), q.into())
        }
        _ => return None,
    })
}

/// A tab with its history, as text (what a tab drag carries).
pub fn tab_str(t: &Tab) -> String {
    let mut s = format!("loc\t{}\n", loc_str(&t.loc));
    for l in &t.back {
        s += &format!("back\t{}\n", loc_str(l));
    }
    for l in &t.fwd {
        s += &format!("fwd\t{}\n", loc_str(l));
    }
    s
}

pub fn tab_parse(s: &str) -> Option<Tab> {
    let mut t = Tab { loc: Loc::Pc, back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 };
    let mut any = false;
    for line in s.lines() {
        let Some((k, v)) = line.split_once('\t') else { continue };
        let Some(l) = loc_parse(v) else { continue };
        match k {
            "loc" => {
                t.loc = l;
                any = true;
            }
            "back" => t.back.push(l),
            "fwd" => t.fwd.push(l),
            _ => {}
        }
    }
    any.then_some(t)
}

/// A request to the compositor (written down instead in tests).
fn ctl(cmd: &str) {
    if open::dry() {
        open::DRY_LOG.lock().unwrap_or_else(|e| e.into_inner()).push(format!("ctl {cmd}"));
        return;
    }
    let _ = crate::ipc::request(cmd);
}

impl App {
    fn outside(&self, x: f64, y: f64) -> bool {
        x < 0.0 || y < 0.0 || x >= self.w as f64 || y >= self.h as f64
    }

    /// Dragged items reached the window's edge: the system drag carries them on.
    pub fn maybe_drag_out(&mut self, x: f64, y: f64, right: bool) -> bool {
        if self.outgoing.is_some() || !self.outside(x, y) || matches!(self.loc(), Loc::Arc(..) | Loc::Trash | Loc::Pc) {
            return false;
        }
        let paths = self.sel_paths();
        if paths.is_empty() {
            return false;
        }
        self.drag_to = None;
        self.hdrop.hide();
        self.ndrop.hide();
        self.spring_at = None;
        let icon = draw::drag_icon(self);
        self.outgoing = Some(Outgoing { paths, tab: None, over_self: false, grab: (0, 0) });
        let actions = COPY | MOVE | if right { ASK } else { 0 };
        self.winreq.push(WinReq::StartDrag { mimes: vec![URI_MIME, "text/plain;charset=utf-8", "text/plain"], actions, icon });
        true
    }

    /// A tab pulled off the strip: the system drag carries it (to another window, or out
    /// into a window of its own).
    pub fn drag_out_tab(&mut self, i: usize, gx: i32, gy: i32) {
        if self.outgoing.is_some() || i >= self.tabs.len() {
            return;
        }
        self.save_tab();
        let icon = draw::tab_icon(self, i);
        self.outgoing = Some(Outgoing { paths: vec![], tab: Some(i), over_self: false, grab: (gx, gy) });
        self.winreq.push(WinReq::StartDrag { mimes: vec![TAB_MIME], actions: MOVE, icon });
    }

    /// What our drag hands over as `mime`.
    pub fn drag_data(&self, mime: &str) -> Vec<u8> {
        let Some(o) = &self.outgoing else { return vec![] };
        if mime == TAB_MIME {
            return o.tab.and_then(|i| self.tabs.get(i)).map(|t| tab_str(t).into_bytes()).unwrap_or_default();
        }
        if mime == URI_MIME {
            return o.paths.iter().map(|p| open::uri_of(p) + "\r\n").collect::<String>().into_bytes();
        }
        o.paths.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>().join("\n").into_bytes()
    }

    /// Our drag is over: taken (`done`) or not.
    pub fn drag_end(&mut self, done: bool) {
        let Some(o) = self.outgoing.take() else { return };
        self.press = None;
        self.drag_to = None;
        self.spring_at = None;
        self.hdrop.hide();
        self.ndrop.hide();
        self.incoming = None;
        let Some(i) = o.tab else { return };
        if i >= self.tabs.len() {
            return;
        }
        if done {
            // Another window took the tab.
            self.close_tab(i);
        } else if !o.over_self {
            if self.tabs.len() > 1 {
                // Let go outside any tab strip: a window of its own, where it was dropped.
                self.tear_off(i, o.grab);
                self.close_tab(i);
            } else {
                // The only tab: the window itself goes there.
                ctl(&format!("move-active {} {}", o.grab.0, o.grab.1));
            }
        }
    }

    fn tear_off(&mut self, i: usize, grab: (i32, i32)) {
        let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let f = PathBuf::from(dir).join(format!("resc-files-tab-{}-{}", std::process::id(), sys::now_ns()));
        if std::fs::write(&f, tab_str(&self.tabs[i])).is_err() {
            return;
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "resc-desk".into());
        ctl(&format!("place-next resc.files {} {}", grab.0, grab.1));
        open::spawn(&format!("{} files --tab {}", open::quote(&exe), open::quote(&f.to_string_lossy())), None);
    }

    /// A system drag came over the window. `read` fetches its data as a type.
    pub fn dnd_enter(&mut self, x: f64, y: f64, mimes: &[String], read: &mut dyn FnMut(&str) -> Option<Vec<u8>>) {
        let has = |m: &str| mimes.iter().any(|x| x == m);
        self.incoming = if let Some(o) = self.outgoing.as_mut() {
            o.over_self = true;
            Some(Incoming { paths: o.paths.clone(), tab: None, own: true, told: None })
        } else if has(TAB_MIME) {
            let t = read(TAB_MIME).and_then(|d| tab_parse(&String::from_utf8_lossy(&d)));
            t.map(|t| Incoming { paths: vec![], tab: Some(t), own: false, told: None })
        } else if has(URI_MIME) {
            let paths = read(URI_MIME).map(|d| parse_uris(&d)).unwrap_or_default();
            (!paths.is_empty()).then(|| Incoming { paths, tab: None, own: false, told: None })
        } else {
            None
        };
        self.dnd_motion(x, y);
    }

    pub fn dnd_motion(&mut self, x: f64, y: f64) {
        self.ptr = Some((x, y));
        let h = self.hit(x, y);
        self.hot = h;
        self.dirty = true;
        let Some(inc) = &self.incoming else {
            self.tell(None, 0, 0);
            return;
        };
        let own_tab = self.outgoing.as_ref().map_or(false, |o| o.tab.is_some());
        if inc.tab.is_some() || own_tab {
            // A tab: only onto the tab strip (not its own window's).
            let ok = !own_tab && (y as i32) < TABS_H;
            self.tab_drop_at = if ok { Some(self.tab_slot(x)) } else { None };
            self.tell(if ok { Some(TAB_MIME) } else { None }, MOVE, MOVE);
            return;
        }
        self.drag_target(x, y);
        match self.drag_to.clone() {
            Some((to, eff)) => {
                let pref = match eff {
                    Act::DropCopy => COPY,
                    _ => MOVE,
                };
                let actions = if to == Loc::Trash { MOVE } else { COPY | MOVE | ASK };
                let _ = to;
                self.tell(Some(URI_MIME), actions, pref);
            }
            None => self.tell(None, 0, 0),
        }
    }

    fn tell(&mut self, mime: Option<&'static str>, actions: u32, pref: u32) {
        let now = Some((mime, actions, pref));
        if let Some(inc) = self.incoming.as_mut() {
            if inc.told == now {
                return;
            }
            inc.told = now;
        }
        self.winreq.push(WinReq::DndAccept { mime, actions, pref });
    }

    /// Where a tab dropped at `x` goes among the tabs.
    fn tab_slot(&self, x: f64) -> usize {
        let (tabs, _) = self.tab_rects();
        tabs.iter().position(|r| x < (r.x + r.w / 2) as f64).unwrap_or(tabs.len())
    }

    pub fn dnd_leave(&mut self) {
        self.incoming = None;
        self.tab_drop_at = None;
        if let Some(o) = self.outgoing.as_mut() {
            o.over_self = false;
        }
        self.drag_to = None;
        self.spring_at = None;
        self.hdrop.hide();
        self.ndrop.hide();
        self.hot = Hit::None;
        self.dirty = true;
    }

    /// Dropped here with `action` (what the compositor settled on).
    pub fn dnd_drop(&mut self, action: u32) {
        let Some(inc) = self.incoming.take() else { return };
        let slot = self.tab_drop_at.take();
        if let Some(t) = inc.tab {
            if let Some(at) = slot {
                self.save_tab();
                let at = at.min(self.tabs.len());
                let l = t.loc.clone();
                self.tabs.insert(at, t);
                self.cur = at;
                self.enter(l, true);
            }
            return;
        }
        let Some((to, eff)) = self.drag_to.take() else { return };
        self.hdrop.hide();
        self.ndrop.hide();
        self.spring_at = None;
        if inc.own {
            // Our own items came back: as a drag inside the window.
            if action == ASK {
                self.drag_to = Some((to, eff));
                let (x, y) = self.ptr.unwrap_or_default();
                self.open_menu(self.drop_menu(), x as i32, y as i32);
            } else {
                let eff = if action == COPY && eff != Act::Trash { Act::DropCopy } else { eff };
                self.drop_on(to, eff);
            }
            return;
        }
        let paths = inc.paths;
        if to == Loc::Trash {
            let title = format!("Удаление {}", self.names_of(&paths));
            self.start_op("Удаление", title, None, ops::Job::Trash(paths));
            return;
        }
        self.drop_paths = Some(paths);
        self.drag_to = Some((to, eff.clone()));
        match action {
            ASK => {
                let (x, y) = self.ptr.unwrap_or_default();
                self.open_menu(self.drop_menu(), x as i32, y as i32);
            }
            COPY => self.act(Act::DropCopy),
            _ => self.act(Act::DropMove),
        }
    }

    /// The menu a right drag ends with.
    pub fn drop_menu(&self) -> Vec<MItem> {
        let arc = matches!(self.loc(), Loc::Arc(..)) && self.outgoing.is_none() && self.incoming.is_none();
        vec![
            MItem::new("Копировать сюда", Act::DropCopy).bold(),
            MItem::new("Переместить сюда", Act::DropMove).on(!arc),
            MItem::new("Создать ярлыки", Act::DropLink).on(!arc),
            MItem::sep(),
            MItem::new("Отмена", Act::DropCancel),
        ]
    }
}
