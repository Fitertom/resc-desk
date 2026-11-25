//! Where things are (shared with draw.rs) and what the mouse and keys do — Windows
//! Explorer's rules: click / Ctrl / Shift / band selection, slow second click renames,
//! drag moves on the same disk and copies across (Shift / Ctrl / Alt change it, the
//! right button asks), held drags open folders, Alt+arrows walk the history.
use super::*;
use crate::textedit::Act as TAct;

pub mod k {
    pub const ESC: u32 = 1;
    pub const N6: u32 = 7;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const W: u32 = 17;
    pub const E: u32 = 18;
    pub const T: u32 = 20;
    pub const Y: u32 = 21;
    pub const ENTER: u32 = 28;
    pub const A: u32 = 30;
    pub const D: u32 = 32;
    pub const F: u32 = 33;
    pub const H: u32 = 35;
    pub const L: u32 = 38;
    pub const Z: u32 = 44;
    pub const X: u32 = 45;
    pub const C: u32 = 46;
    pub const V: u32 = 47;
    pub const N: u32 = 49;
    pub const SPACE: u32 = 57;
    pub const F2: u32 = 60;
    pub const F3: u32 = 61;
    pub const F4: u32 = 62;
    pub const F5: u32 = 63;
    pub const F6: u32 = 64;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const PGUP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const PGDN: u32 = 109;
    pub const DELETE: u32 = 111;
    pub const MENU: u32 = 127;
}

pub const CAP_W: i32 = 46;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

pub struct CmdBtn {
    pub cmd: Cmd,
    pub r: Rect,
    pub label: &'static str,
    pub g: G,
    pub on: bool,
    pub drop: bool,
}

impl App {
    // -------------------------------------------------------------- geometry

    pub fn tab_rects(&self) -> (Vec<Rect>, Rect) {
        let n = self.tabs.len() as i32;
        let w = ((self.w - 60 - 3 * CAP_W - 40) / n.max(1)).clamp(80, 230);
        let v: Vec<Rect> = (0..n).map(|i| Rect::new(8 + i * (w + 2), 6, w, TABS_H - 6)).collect();
        let x = v.last().map_or(8, |r| r.x1() + 4);
        (v, Rect::new(x, 9, 30, 28))
    }

    /// ─ □ ✕ at the right end of the tab row (Windows 11 Explorer).
    pub fn caption_rects(&self) -> [Rect; 3] {
        let x = self.w - 3 * CAP_W;
        [0, 1, 2].map(|i| Rect::new(x + i * CAP_W, 0, CAP_W, TABS_H - 2))
    }

    pub fn tool_btn(&self, i: i32) -> Rect {
        Rect::new(8 + i * 34, TABS_H + 8, 32, 32)
    }

    pub fn addr_rect(&self) -> Rect {
        Rect::from_edges(150, TABS_H + 8, self.w - 268, TABS_H + 40)
    }

    pub fn search_rect(&self) -> Rect {
        Rect::from_edges(self.w - 258, TABS_H + 8, self.w - 8, TABS_H + 40)
    }

    /// The crumbs as laid out: (rect, index), skipping leading ones that don't fit.
    pub fn crumb_rects(&self) -> (Vec<(Rect, usize)>, bool) {
        let cs = self.crumbs();
        let ar = self.addr_rect();
        let ws: Vec<i32> = cs.iter().map(|c| ui::tw(&c.0, 13.0) + 16).collect();
        let sep = 18;
        let avail = ar.w - 40;
        let mut first = 0;
        while first + 1 < cs.len() && ws[first..].iter().sum::<i32>() + sep * (cs.len() - first) as i32 > avail {
            first += 1;
        }
        let mut x = ar.x + 34 + if first > 0 { 22 } else { 0 };
        let mut v = Vec::new();
        for i in first..cs.len() {
            let w = ws[i].min(ar.x1() - 6 - x);
            v.push((Rect::new(x, ar.y + 3, w.max(0), ar.h - 6), i));
            x += ws[i] + sep;
        }
        (v, first > 0)
    }

    pub fn cmd_btns(&self) -> Vec<CmdBtn> {
        let y = TABS_H + TOOL_H + 4;
        let mut v = Vec::new();
        let mut x = 10;
        let has_sel = self.sel.iter().any(|s| *s);
        let here = self.loc().dir().is_some();
        let trash = matches!(self.loc(), Loc::Trash);
        let in_arc = matches!(self.loc(), Loc::Arc(..));
        let mut push = |cmd: Cmd, label: &'static str, g: G, on: bool, drop: bool, x: &mut i32| {
            let w = if label.is_empty() { 34 } else { ui::tw(label, 13.0) + 48 + if drop { 16 } else { 0 } };
            v.push(CmdBtn { cmd, r: Rect::new(*x, y, w, 32), label, g, on, drop });
            *x += w + 2;
        };
        if trash {
            push(Cmd::Empty, "Очистить корзину", G::Delete, !self.items.is_empty(), false, &mut x);
            push(Cmd::Restore, "Восстановить", G::Back, has_sel, false, &mut x);
            x += 12;
            push(Cmd::Delete, "", G::Delete, has_sel, false, &mut x);
        } else {
            push(Cmd::New, "Создать", G::Plus, here, true, &mut x);
            x += 12;
            push(Cmd::Cut, "", G::Cut, has_sel && !in_arc && self.loc().dir().is_some(), false, &mut x);
            push(Cmd::Copy, "", G::Copy, has_sel, false, &mut x);
            push(Cmd::Paste, "", G::Paste, here && self.clip.is_some(), false, &mut x);
            push(Cmd::Rename, "", G::Rename, has_sel && here, false, &mut x);
            push(Cmd::Delete, "", G::Delete, has_sel && !in_arc && !matches!(self.loc(), Loc::Pc), false, &mut x);
        }
        x += 12;
        push(Cmd::Sort, "Сортировать", G::Sort, !matches!(self.loc(), Loc::Pc), true, &mut x);
        push(Cmd::View, "Просмотреть", G::View, !matches!(self.loc(), Loc::Pc), true, &mut x);
        if !trash && !in_arc && here {
            x += 12;
            push(Cmd::Zip, "7-Zip", G::Archive, has_sel, true, &mut x);
        }
        let more_x = self.w - 44;
        v.push(CmdBtn { cmd: Cmd::More, r: Rect::new(more_x.max(x), y, 34, 32), label: "", g: G::More, on: true, drop: false });
        v
    }

    pub fn nav_rects(&self) -> Vec<Rect> {
        let mut y = TOP + 8 - self.nav_scroll.value.round() as i32;
        let mut v = Vec::new();
        for n in &self.nav {
            if n.sep {
                y += 13;
            }
            v.push(Rect::new(6, y, NAV_W - 12, NAV_ROW));
            y += NAV_ROW + 2;
        }
        v
    }

    pub fn nav_h(&self) -> i32 {
        self.nav.iter().map(|n| NAV_ROW + 2 + if n.sep { 13 } else { 0 }).sum::<i32>() + 20
    }

    pub fn head_cols(&self) -> Vec<(Col, Rect)> {
        let vr = self.view_rect();
        let mut x = vr.x + 6;
        let mut v = Vec::new();
        for c in self.columns() {
            let w = self.widths[&c];
            v.push((c, Rect::new(x, TOP, w, HEAD_H)));
            x += w;
        }
        v
    }

    pub fn status_btns(&self) -> (Rect, Rect) {
        let y = self.h - STATUS_H + 3;
        (Rect::new(self.w - 72, y, 30, 24), Rect::new(self.w - 38, y, 30, 24))
    }

    pub fn scrollbar(&self) -> Option<(Rect, Rect)> {
        let vr = self.view_rect();
        let max = self.max_scroll();
        if max <= 0.0 {
            return None;
        }
        let track = Rect::new(vr.x1() - 12, vr.y + 4, 10, vr.h - 8);
        let th = ((vr.h as f32 / self.content_h as f32) * track.h as f32).max(30.0) as i32;
        let ty = track.y + ((self.scroll.value / max) * (track.h - th) as f32).round() as i32;
        Some((track, Rect::new(track.x + 2, ty, 6, th)))
    }

    /// The operations panel and its rows.
    pub fn ops_rects(&self) -> (Rect, Vec<Rect>) {
        let n = self.ops.len() as i32;
        let w = 400;
        let row = 150;
        let h = if self.ops_open { 44 + n.min(3) * row } else { 44 };
        let r = Rect::new(self.w - w - 16, self.h - STATUS_H - h - 12, w, h);
        let rows = (0..n.min(3)).map(|i| Rect::new(r.x + 12, r.y + 40 + i * row, w - 24, row - 8)).collect();
        (r, rows)
    }

    pub fn hit(&self, x: f64, y: f64) -> Hit {
        let (xi, yi) = (x as i32, y as i32);
        if self.desk && self.ops.is_empty() {
            return match self.item_at(x, y) {
                Some(i) => Hit::Item(i),
                None if self.view_rect().contains(x, y) => Hit::Empty,
                None => Hit::None,
            };
        }
        if !self.ops.is_empty() {
            let (r, rows) = self.ops_rects();
            if r.contains(x, y) {
                if Rect::new(r.x, r.y, r.w, 40).contains(x, y) {
                    return Hit::OpToggle;
                }
                for (i, rr) in rows.iter().enumerate() {
                    if Rect::new(rr.x1() - 64, rr.y, 30, 28).contains(x, y) {
                        return Hit::OpPause(i);
                    }
                    if Rect::new(rr.x1() - 30, rr.y, 30, 28).contains(x, y) {
                        return Hit::OpCancel(i);
                    }
                }
                return Hit::None;
            }
        }
        if yi < TABS_H {
            for (i, r) in self.caption_rects().iter().enumerate() {
                if r.contains(x, y) {
                    return Hit::Caption(i);
                }
            }
            let (tabs, plus) = self.tab_rects();
            for (i, r) in tabs.iter().enumerate() {
                if r.contains(x, y) {
                    if Rect::new(r.x1() - 30, r.y + 5, 24, 24).contains(x, y) && (self.tabs.len() > 1 || true) {
                        return Hit::TabClose(i);
                    }
                    return Hit::Tab(i);
                }
            }
            if plus.contains(x, y) {
                return Hit::NewTab;
            }
            return Hit::TitleDrag;
        }
        if yi < TABS_H + TOOL_H {
            for (i, h) in [Hit::Back, Hit::Fwd, Hit::Up, Hit::Refresh].into_iter().enumerate() {
                if self.tool_btn(i as i32).contains(x, y) {
                    return h;
                }
            }
            if self.addr_rect().contains(x, y) {
                if self.addr.is_none() {
                    for (r, i) in self.crumb_rects().0 {
                        if r.contains(x, y) {
                            return Hit::Crumb(i);
                        }
                    }
                }
                return Hit::Addr;
            }
            let sr = self.search_rect();
            if sr.contains(x, y) {
                if !self.search.is_empty() && x > (sr.x1() - 30) as f64 {
                    return Hit::SearchClear;
                }
                return Hit::Search;
            }
            return Hit::None;
        }
        if yi < TOP {
            for (i, b) in self.cmd_btns().iter().enumerate() {
                if b.r.contains(x, y) {
                    return Hit::Cmd(i);
                }
            }
            return Hit::None;
        }
        if yi >= self.h - STATUS_H {
            let (a, b) = self.status_btns();
            if a.contains(x, y) {
                return Hit::ViewDetails;
            }
            if b.contains(x, y) {
                return Hit::ViewIcons;
            }
            return Hit::None;
        }
        if xi < NAV_W {
            for (i, r) in self.nav_rects().iter().enumerate() {
                if r.contains(x, y) {
                    if self.nav[i].eject && Rect::new(r.x1() - 30, r.y + 4, 24, 24).contains(x, y) {
                        return Hit::Eject(i);
                    }
                    return Hit::Nav(i);
                }
            }
            return Hit::None;
        }
        if let Some((track, _)) = self.scrollbar() {
            if track.inflate(2).contains(x, y) {
                return Hit::Scroll;
            }
        }
        if self.view.mode == Mode::Details && yi < TOP + HEAD_H {
            for (i, (_, r)) in self.head_cols().iter().enumerate() {
                if (x - r.x1() as f64).abs() <= 4.0 {
                    return Hit::HeadEdge(i);
                }
                if r.contains(x, y) {
                    return Hit::Head(i);
                }
            }
            return Hit::None;
        }
        match self.item_at(x, y) {
            Some(i) => {
                // Details: like Windows, only the name cell counts as the item for a
                // click elsewhere in the row too — keep the whole row (simpler and kind).
                Hit::Item(i)
            }
            None => Hit::Empty,
        }
    }

    // -------------------------------------------------------------- pointer

    pub fn leave(&mut self) {
        if self.press.is_none() {
            self.ptr = None;
            self.hot = Hit::None;
            self.hdrop.hide();
        }
    }

    pub fn motion(&mut self, x: f64, y: f64) {
        self.ptr = Some((x, y));
        // Menus first.
        if let Some(m) = self.menu.as_mut() {
            if let Some((_, s)) = m.sub.as_mut() {
                if s.rect().contains(x, y) {
                    s.hot = s.at(x, y);
                    return;
                }
            }
            let hot = m.at(x, y);
            if hot.is_some() || m.rect().contains(x, y) {
                m.hot = hot;
                match hot {
                    Some(i) if !m.items[i].sub.is_empty() => self.open_sub(i),
                    Some(_) => {
                        if let Some(m) = self.menu.as_mut() {
                            m.sub = None;
                        }
                    }
                    None => {}
                }
            }
            return;
        }
        if let Some(d) = self.dialog.as_mut() {
            let r = draw::dialog_rect(d, self.w, self.h);
            d.hot = draw::dialog_buttons(d, r).iter().position(|b| b.contains(x, y));
            return;
        }
        let Some(p0) = self.press.as_ref() else {
            let h = self.hit(x, y);
            if h != self.hot {
                self.hot = h;
            }
            match h {
                Hit::Item(i) => self.hdrop.go(self.rects[i]),
                _ => self.hdrop.hide(),
            }
            match h {
                Hit::Nav(i) => {
                    let r = self.nav_rects()[i].translate(0, self.nav_scroll.value.round() as i32);
                    self.ndrop.go(r);
                }
                _ => self.ndrop.hide(),
            }
            return;
        };
        let (px, py, phit, mut pdrag, scroll0, col_w0, p_button) = (p0.x, p0.y, p0.hit, p0.dragging, p0.scroll0, p0.col_w0, p0.button);
        let (dx, dy) = (x - px, y - py);
        let far = dx * dx + dy * dy > 25.0;
        match phit {
            Hit::Item(_) if far || pdrag => {
                if !pdrag && !matches!(self.loc(), Loc::Pc) {
                    pdrag = true;
                    if let Some(p) = self.press.as_mut() {
                        p.dragging = true;
                        p.defer = false;
                    }
                }
                if pdrag && !self.maybe_drag_out(x, y, p_button == BTN_RIGHT) && self.outgoing.is_none() {
                    self.drag_target(x, y);
                }
            }
            Hit::Empty => {
                let (x0, y0) = (px, py + scroll0 as f64);
                let vr = self.view_rect();
                let sy = self.scroll.value as f64;
                self.band = Some((x0, y0, x, y + sy));
                // Items under the band (content coordinates).
                let (bx0, bx1) = (x0.min(x) - vr.x as f64, x0.max(x) - vr.x as f64);
                let (by0, by1) = (y0.min(y + sy) - vr.y as f64, y0.max(y + sy) - vr.y as f64);
                let ctrl = self.mods.ctrl;
                let base: Vec<bool> = self.tabs[self.cur].sel.iter().map(|_| false).collect();
                let _ = base;
                for (i, r) in self.rects.iter().enumerate() {
                    let inside = (r.x as f64) < bx1 && (r.x1() as f64) > bx0 && (r.y as f64) < by1 && (r.y1() as f64) > by0;
                    if ctrl {
                        if inside {
                            self.sel[i] = true;
                        }
                    } else {
                        self.sel[i] = inside;
                    }
                }
                // Scroll when the band reaches the edges.
                if y < vr.y as f64 + 10.0 {
                    self.scroll.to((self.scroll.target - 20.0).max(0.0));
                } else if y > vr.y1() as f64 - 10.0 {
                    let m = self.max_scroll();
                    self.scroll.to((self.scroll.target + 20.0).min(m));
                }
            }
            Hit::Tab(i) if p_button == BTN_LEFT && self.outgoing.is_none() && (y < -12.0 || y > (TABS_H + 28) as f64 || x < -12.0 || x > self.w as f64 + 12.0) => {
                // Pulled off the strip: to another window, or a window of its own.
                let grab = if self.tabs.len() > 1 { (90, 20) } else { (px as i32, py as i32) };
                if let Some(p) = self.press.as_mut() {
                    p.dragging = true;
                }
                self.drag_out_tab(i, grab.0, grab.1);
            }
            Hit::Head(i) if i > 0 && (far || pdrag) => {
                // Dragging a header moves the column («Имя» stays first).
                if let Some(p) = self.press.as_mut() {
                    p.dragging = true;
                }
                self.col_drag = Some((i, x));
            }
            Hit::HeadEdge(i) => {
                if let Some((c, _)) = self.head_cols().get(i) {
                    let w = (col_w0 + dx as i32).max(50);
                    self.widths.insert(*c, w);
                    self.lay();
                }
            }
            Hit::Scroll => {
                let vr = self.view_rect();
                let k = self.content_h as f32 / vr.h.max(1) as f32;
                let t = (scroll0 + dy as f32 * k).clamp(0.0, self.max_scroll());
                self.scroll.snap(t);
            }
            _ => {}
        }
    }

    /// While dragging items: what is under the pointer and what a drop would do.
    pub fn drag_target(&mut self, x: f64, y: f64) {
        let h = self.hit(x, y);
        let target = match h {
            Hit::Item(i) if self.items[i].e.dir && (!self.sel[i] || self.incoming.as_ref().map_or(false, |i| !i.own)) && !matches!(self.loc(), Loc::Arc(..) | Loc::Trash) && !self.drag_paths().contains(&self.items[i].e.path) => Some(Loc::Dir(self.items[i].e.path.clone())),
            Hit::Nav(i) => Some(self.nav[i].loc.clone()).filter(|l| matches!(l, Loc::Dir(_) | Loc::Trash)),
            Hit::Tab(i) => Some(self.tabs[i].loc.clone()).filter(|l| matches!(l, Loc::Dir(_))),
            // Into this folder: only things from elsewhere (not its own items).
            Hit::Empty | Hit::Item(_) if self.incoming.as_ref().map_or(false, |i| !i.own) && !matches!(self.loc(), Loc::Arc(..) | Loc::Search(..)) => {
                let here = self.loc().dir().map(Path::to_path_buf);
                let paths = self.drag_paths();
                here.filter(|d| !paths.iter().all(|p| p.parent() == Some(d.as_path()))).map(Loc::Dir)
            }
            _ => None,
        };
        let spring = matches!(h, Hit::Item(_) | Hit::Nav(_)) && target.is_some();
        match (self.spring_at, spring) {
            (Some((sh, _)), true) if sh == h => {}
            (_, true) => self.spring_at = Some((h, sys::now_ns())),
            _ => self.spring_at = None,
        }
        if let Hit::Tab(i) = h {
            // Hovering a tab while dragging switches to it (as in Windows 11).
            if i != self.cur {
                self.switch_tab(i);
            }
        }
        self.drag_to = target.map(|t| {
            let first = self.drag_paths().first().cloned().unwrap_or_default();
            let eff = self.drag_effect(&first, &t);
            (t, eff)
        });
        match h {
            Hit::Item(i) if self.drag_to.is_some() => self.hdrop.go(self.rects[i]),
            _ => self.hdrop.hide(),
        }
        match h {
            Hit::Nav(i) if self.drag_to.is_some() => {
                let r = self.nav_rects()[i].translate(0, self.nav_scroll.value.round() as i32);
                self.ndrop.go(r);
            }
            _ => self.ndrop.hide(),
        }
    }

    /// What is being dragged: files from elsewhere, or the selection.
    pub fn drag_paths(&self) -> Vec<PathBuf> {
        match &self.incoming {
            Some(i) if !i.own => i.paths.clone(),
            _ => self.sel_paths(),
        }
    }

    /// Windows' rule: same disk moves, another disk copies; Shift moves, Ctrl copies,
    /// Alt makes links; the trash takes things in.
    fn drag_effect(&self, src: &Path, to: &Loc) -> Act {
        if *to == Loc::Trash {
            return Act::Trash;
        }
        if matches!(self.loc(), Loc::Arc(..)) {
            return Act::DropCopy;
        }
        let m = self.mods;
        if m.alt {
            return Act::DropLink;
        }
        if m.ctrl {
            return Act::DropCopy;
        }
        if m.shift {
            return Act::DropMove;
        }
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        match to {
            Loc::Dir(d) if dev(src).is_some() && dev(src) == dev(d) => Act::DropMove,
            _ => Act::DropCopy,
        }
    }

    pub fn button(&mut self, b: u32, down: bool) {
        let Some((x, y)) = self.ptr else { return };
        if !down {
            self.release(b, x, y);
            return;
        }
        // Menus: a click inside picks, outside closes; other buttons (back / forward)
        // only close it.
        if self.menu.is_some() && b != BTN_LEFT && b != BTN_RIGHT {
            self.menu = None;
            self.drag_to = None;
            self.dirty = true;
            return;
        }
        if let Some(m) = self.menu.as_ref() {
            let in_sub = m.sub.as_ref().map(|(_, s)| (s.rect().contains(x, y), s.at(x, y))).unwrap_or((false, None));
            if in_sub.0 {
                if let Some(i) = in_sub.1 {
                    let it = m.sub.as_ref().unwrap().1.items[i].clone();
                    if it.on && it.sub.is_empty() {
                        self.act(it.act);
                    }
                }
                return;
            }
            if m.rect().contains(x, y) {
                if let Some(i) = m.at(x, y) {
                    let it = m.items[i].clone();
                    if it.on && it.sub.is_empty() {
                        self.act(it.act);
                    } else if !it.sub.is_empty() {
                        self.open_sub(i);
                    }
                }
                return;
            }
            self.menu = None;
            self.drag_to = None;
            self.dirty = true;
            if b != BTN_RIGHT {
                return;
            }
        }
        if self.dialog.is_some() {
            // A dialog takes only the left button (back/forward must not press its buttons).
            if b == BTN_LEFT {
                self.dialog_click(x, y);
            }
            return;
        }
        if b == BTN_SIDE {
            self.go_back();
            return;
        }
        if b == BTN_EXTRA {
            self.go_fwd();
            return;
        }
        let h = self.hit(x, y);
        // A click anywhere else ends renaming / the address editing.
        if self.rename.is_some() && !matches!(h, Hit::Item(i) if self.rename.as_ref().map_or(false, |r| r.0 == i)) {
            self.commit_rename(None);
        }
        if self.addr.is_some() && h != Hit::Addr {
            self.addr = None;
        }
        if h != Hit::Search && self.search_on && self.search.is_empty() {
            self.search_on = false;
        }
        let now = sys::now_ns();
        let double = self.last_click.1 == h && now - self.last_click.0 < 450_000_000 && b == BTN_LEFT;
        self.last_click = if double { (0, Hit::None) } else { (now, h) };
        if b == BTN_MIDDLE {
            match h {
                Hit::Item(i) if self.items[i].e.dir && matches!(self.loc(), Loc::Dir(_) | Loc::Pc | Loc::Search(..)) => {
                    let l = Loc::Dir(self.items[i].e.path.clone());
                    self.save_tab();
                    self.tabs.push(Tab { loc: l, back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 });
                }
                Hit::Nav(i) => {
                    let l = self.nav[i].loc.clone();
                    self.new_tab(l);
                }
                Hit::Tab(i) | Hit::TabClose(i) => self.close_tab(i),
                _ => {}
            }
            return;
        }
        if b == BTN_RIGHT {
            self.right_click(h, x, y);
            return;
        }
        if b != BTN_LEFT {
            return;
        }
        let mut press = Press { x, y, hit: h, button: b, dragging: false, defer: false, scroll0: self.scroll.value, col_w0: 0 };
        match h {
            Hit::Item(i) => {
                if double {
                    if self.sel[i] {
                        self.press = None;
                        self.open_items(self.selected());
                        return;
                    }
                }
                let m = self.mods;
                if m.ctrl {
                    self.sel[i] = !self.sel[i];
                    self.set_focus(i);
                    self.anchor = i;
                } else if m.shift {
                    self.select_range(self.anchor, i, false);
                    self.set_focus(i);
                } else if !self.sel[i] {
                    self.select_only(i);
                } else {
                    press.defer = true;
                }
            }
            Hit::Empty => {
                if !self.mods.ctrl {
                    self.sel.iter_mut().for_each(|s| *s = false);
                }
                self.band = None;
                if double && matches!(self.loc(), Loc::Dir(_)) {
                    // Windows doesn't; GTK managers go up. Leave it alone.
                }
            }
            Hit::HeadEdge(i) => {
                if double {
                    self.autofit(i);
                    return;
                }
                press.col_w0 = self.head_cols()[i].1.w;
            }
            Hit::Scroll => {
                if let Some((track, thumb)) = self.scrollbar() {
                    if !thumb.contains(x, y) {
                        // Click on the track: jump there.
                        let k = ((y as i32 - track.y - thumb.h / 2) as f32 / (track.h - thumb.h).max(1) as f32).clamp(0.0, 1.0);
                        self.scroll.to(k * self.max_scroll());
                        press.scroll0 = self.scroll.target;
                    }
                }
            }
            Hit::Addr => {
                if self.addr.is_none() {
                    let mut tf = TextField::default();
                    tf.set(&self.addr_text());
                    self.addr = Some(tf);
                }
            }
            Hit::Search => {
                self.search_on = true;
            }
            Hit::SearchClear => self.exit_search(),
            Hit::Back => self.go_back(),
            Hit::Fwd => self.go_fwd(),
            Hit::Up => self.go_up(),
            Hit::Refresh => self.reload(true),
            Hit::Tab(i) => self.switch_tab(i),
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.act(Act::NewTab),
            Hit::ViewDetails => self.set_mode(Mode::Details),
            Hit::ViewIcons => self.set_mode(Mode::Icons(96)),
            Hit::Eject(i) => {
                if let Loc::Dir(p) = self.nav[i].loc.clone() {
                    self.act(Act::Eject(p));
                }
            }
            Hit::OpToggle => self.ops_open = !self.ops_open,
            Hit::Caption(0) => self.winreq.push(WinReq::Minimize),
            Hit::Caption(1) => self.winreq.push(WinReq::Maximize),
            Hit::Caption(_) => self.quit = true,
            Hit::TitleDrag => {
                if double {
                    self.winreq.push(WinReq::Maximize);
                } else {
                    // The compositor takes the pointer from here.
                    self.winreq.push(WinReq::Move);
                    return;
                }
            }
            Hit::OpPause(i) => {
                if let Some(op) = self.ops.get(i) {
                    let p = !op.pause.load(std::sync::atomic::Ordering::Relaxed);
                    op.pause.store(p, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Hit::OpCancel(i) => {
                if let Some(op) = self.ops.get(i) {
                    op.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    op.pause.store(false, std::sync::atomic::Ordering::Relaxed);
                    op.reply(Answer::Cancel, true);
                }
            }
            Hit::Cmd(i) => {
                let btns = self.cmd_btns();
                let bt = &btns[i];
                if bt.on {
                    let (mx, my) = (bt.r.x, bt.r.y1() + 4);
                    match bt.cmd {
                        Cmd::New => {
                            let m = self.new_menu();
                            self.open_menu(m, mx, my);
                        }
                        Cmd::Sort => {
                            let m = self.sort_menu();
                            self.open_menu(m, mx, my);
                        }
                        Cmd::View => {
                            let m = self.view_menu();
                            self.open_menu(m, mx, my);
                        }
                        Cmd::Zip => {
                            let m = self.zip_menu();
                            self.open_menu(m, mx, my);
                        }
                        Cmd::More => {
                            let m = self.more_menu();
                            self.open_menu(m, bt.r.x1() - 220, my);
                        }
                        Cmd::Cut => self.act(Act::Cut),
                        Cmd::Copy => self.act(Act::Copy),
                        Cmd::Paste => self.act(Act::Paste),
                        Cmd::Rename => self.act(Act::Rename),
                        Cmd::Delete => self.act(Act::Trash),
                        Cmd::Restore => self.act(Act::Restore),
                        Cmd::Empty => self.act(Act::EmptyTrash),
                    }
                }
            }
            _ => {}
        }
        self.press = Some(press);
        self.dirty = true;
    }

    fn right_click(&mut self, h: Hit, x: f64, y: f64) {
        let (xi, yi) = (x as i32, y as i32);
        match h {
            Hit::Item(i) => {
                if !self.sel[i] {
                    self.select_only(i);
                } else {
                    self.set_focus(i);
                }
                // Pressed for a right drag; the menu comes on release without a drag.
                self.press = Some(Press { x, y, hit: h, button: BTN_RIGHT, dragging: false, defer: false, scroll0: self.scroll.value, col_w0: 0 });
                return;
            }
            Hit::Empty => {
                self.sel.iter_mut().for_each(|s| *s = false);
                let m = self.bg_menu();
                self.open_menu(m, xi, yi);
            }
            Hit::Head(i) | Hit::HeadEdge(i) => {
                let at = if matches!(h, Hit::Head(_)) { Some(i) } else { None };
                let m = self.head_menu(at);
                self.open_menu(m, xi, yi);
            }
            Hit::Tab(i) | Hit::TabClose(i) => {
                let m = vec![
                    MItem::new("Новая вкладка", Act::NewTab).hint("Ctrl+T"),
                    MItem::new("Дублировать вкладку", Act::DupTab(i)),
                    MItem::sep(),
                    MItem::new("Закрыть вкладку", Act::CloseTab(i)).hint("Ctrl+W"),
                    MItem::new("Закрыть другие вкладки", Act::CloseOthers(i)).on(self.tabs.len() > 1),
                ];
                self.open_menu(m, xi, yi);
            }
            Hit::Nav(i) => {
                let n = self.nav[i].clone();
                let mut m = vec![MItem::new("Открыть", Act::Nav(n.loc.clone())).bold(), MItem::new("Открыть в новой вкладке", Act::NavTab(n.loc.clone()))];
                if n.loc == Loc::Trash {
                    m.push(MItem::sep());
                    m.push(MItem::new("Очистить корзину", Act::EmptyTrash).on(!trash::list().is_empty()));
                }
                if n.eject {
                    if let Loc::Dir(p) = &n.loc {
                        m.push(MItem::sep());
                        m.push(MItem::new("Извлечь", Act::Eject(p.clone())).g(G::Eject));
                    }
                }
                self.open_menu(m, xi, yi);
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn release(&mut self, b: u32, x: f64, y: f64) {
        let Some(p) = self.press.take() else { return };
        if b != p.button {
            self.press = Some(p);
            return;
        }
        self.band = None;
        self.spring_at = None;
        self.dirty = true;
        let h = self.hit(x, y);
        if let Some((from, _)) = self.col_drag.take() {
            let to = self.col_drop_at(from, x);
            self.move_col(from, to);
            return;
        }
        if p.dragging {
            // Desktop icons dropped on the wallpaper (not into a folder): they move there.
            if self.desk && self.drag_to.is_none() && self.outgoing.is_none() && p.button == BTN_LEFT && matches!(p.hit, Hit::Item(_)) && self.view_rect().contains(x, y) {
                self.desk_move((p.x, p.y), (x, y));
                return;
            }
            let Some((to, eff)) = self.drag_to.clone() else { return };
            if p.button == BTN_RIGHT {
                self.drop_paths = None;
                let m = self.drop_menu();
                if to != Loc::Trash {
                    self.open_menu(m, x as i32, y as i32);
                    return;
                }
            }
            self.drop_on(to, eff);
            return;
        }
        if p.button == BTN_RIGHT {
            if let Hit::Item(_) = p.hit {
                let m = self.item_menu();
                self.open_menu(m, x as i32, y as i32);
            }
            return;
        }
        match (p.hit, h) {
            (Hit::Item(i), Hit::Item(j)) if i == j && p.defer => {
                let was_only = self.selected() == vec![i] && self.focus == Some(i);
                self.select_only(i);
                // A second slow click on the selected item's name renames it.
                if was_only && !self.mods.ctrl && !self.mods.shift && self.on_name(i, x, y) {
                    self.rename_at = Some((i, sys::now_ns() + 500_000_000));
                }
            }
            (Hit::Head(i), Hit::Head(j)) if i == j => {
                if let Some((c, _)) = self.head_cols().get(i) {
                    let c = *c;
                    self.act(Act::Sort(c));
                }
            }
            (Hit::Nav(i), Hit::Nav(j)) if i == j => {
                let l = self.nav[i].loc.clone();
                self.navigate(l);
            }
            (Hit::Crumb(i), Hit::Crumb(j)) if i == j => {
                if let Some((_, l)) = self.crumbs().get(i).cloned() {
                    self.navigate(l);
                }
            }
            _ => {}
        }
    }

    /// Whether (x, y) is on item `i`'s name (the slow-click rename spot).
    fn on_name(&self, i: usize, x: f64, _y: f64) -> bool {
        let r = self.screen(self.rects[i]);
        match self.view.mode {
            Mode::Details => {
                let tw = ui::tw(&self.items[i].disp, 13.0);
                x >= (r.x + 34) as f64 && x <= (r.x + 40 + tw.min(self.widths[&Col::Name] - 40)) as f64
            }
            _ => true,
        }
    }

    pub fn drop_on(&mut self, to: Loc, eff: Act) {
        self.drop_paths = None;
        self.drag_to = Some((to.clone(), eff.clone()));
        match eff {
            Act::Trash => {
                self.drag_to = None;
                self.trash_sel();
            }
            Act::DropCopy if matches!(self.loc(), Loc::Arc(..)) => {
                self.drag_to = None;
                if let (Loc::Arc(f, inner), Loc::Dir(d)) = (self.loc().clone(), to) {
                    let items = self.sel_paths();
                    self.extract_items(&f, &inner, &items, d);
                }
            }
            e => self.act(e),
        }
        self.hdrop.hide();
        self.ndrop.hide();
    }

    /// Where header `from`, dragged to `x`, lands (a shown index, never 0 = «Имя»).
    pub fn col_drop_at(&self, from: usize, x: f64) -> usize {
        let cols = self.head_cols();
        let mut to = cols.len().saturating_sub(1);
        for (j, (_, r)) in cols.iter().enumerate() {
            if x < (r.x + r.w / 2) as f64 {
                to = j;
                break;
            }
        }
        // Past its own slot the others shift left by one.
        if to > from && x < (cols[cols.len() - 1].1.x + cols[cols.len() - 1].1.w / 2) as f64 {
            to -= 1;
        }
        to.max(1)
    }

    pub fn autofit(&mut self, i: usize) {
        let Some((c, _)) = self.head_cols().get(i).copied() else { return };
        let mut w = ui::tw(c.title(), 13.0) + 30;
        for it in &self.items {
            let t = draw::cell_text(self, it, c);
            w = w.max(ui::tw(&t, 13.0) + if c == Col::Name { 52 } else { 24 });
        }
        self.widths.insert(c, w.min(700));
        self.lay();
    }

    pub fn wheel(&mut self, v: f64) {
        let Some((x, y)) = self.ptr else { return };
        if self.menu.is_some() || self.dialog.is_some() {
            return;
        }
        if (x as i32) < NAV_W && (y as i32) > TOP {
            let max = (self.nav_h() - (self.h - TOP - STATUS_H)).max(0) as f32;
            let t = (self.nav_scroll.target + v as f32 * 4.0).clamp(0.0, max);
            self.nav_scroll.to(t);
            return;
        }
        if (y as i32) < TOP {
            // Over the tabs: switch them (as Chrome / Windows 11 do not; keep still).
            return;
        }
        if self.mods.ctrl {
            self.zoom_acc += v;
            if self.zoom_acc.abs() >= 8.0 {
                self.zoom(self.zoom_acc < 0.0);
                self.zoom_acc = 0.0;
            }
            return;
        }
        let t = (self.scroll.target + v as f32 * 7.0).clamp(0.0, self.max_scroll());
        self.scroll.to(t);
        if let Some(i) = self.item_at(x, y) {
            self.hdrop.go(self.rects[i]);
        }
    }

    // -------------------------------------------------------------- dialogs

    fn dialog_click(&mut self, x: f64, y: f64) {
        let Some(d) = self.dialog.as_mut() else { return };
        let r = draw::dialog_rect(d, self.w, self.h);
        if let Some(i) = draw::dialog_buttons(d, r).iter().position(|b| b.contains(x, y)) {
            self.dialog_done(i);
            return;
        }
        let lay = draw::dialog_layout(d, r);
        for (i, fr) in lay.fields.iter().enumerate() {
            if fr.contains(x, y) {
                d.focus = i;
                self.dirty = true;
                return;
            }
        }
        for (ci, opts) in lay.choices.iter().enumerate() {
            for (oi, orc) in opts.iter().enumerate() {
                if orc.contains(x, y) {
                    d.choices[ci].2 = oi;
                    self.dirty = true;
                    return;
                }
            }
        }
        if let (Some(cr), Some(c)) = (lay.check, d.check.as_mut()) {
            if cr.contains(x, y) {
                c.1 = !c.1;
                self.dirty = true;
            }
        }
    }

    fn dialog_key(&mut self, code: u32) {
        let m = self.mods;
        let Some(d) = self.dialog.as_mut() else { return };
        self.dirty = true;
        match code {
            k::ESC => {
                let cancel = match d.on {
                    On::Conflict(_) => 3,
                    _ => d.buttons.len() - 1,
                };
                self.dialog_done(cancel);
            }
            k::ENTER => {
                let def = d.default;
                self.dialog_done(def);
            }
            k::TAB => {
                if !d.fields.is_empty() {
                    let n = d.fields.len();
                    d.focus = if m.shift { (d.focus + n - 1) % n } else { (d.focus + 1) % n };
                }
            }
            k::LEFT | k::RIGHT if d.fields.is_empty() && !d.buttons.is_empty() => {
                let n = d.buttons.len();
                d.default = if code == k::LEFT { (d.default + n - 1) % n } else { (d.default + 1) % n };
            }
            k::SPACE if d.fields.is_empty() => {
                if let Some(c) = d.check.as_mut() {
                    c.1 = !c.1;
                }
            }
            _ => {
                if let Some(f) = d.fields.get_mut(d.focus) {
                    match f.tf.key(code, m) {
                        TAct::Paste => {
                            if let Some(t) = paste_text() {
                                f.tf.insert(&t);
                            }
                        }
                        TAct::Copy(s) | TAct::Cut(s) if !f.secret => copy_out(&s),
                        _ => {}
                    }
                }
            }
        }
    }

    fn menu_key(&mut self, code: u32) {
        let Some(m) = self.menu.as_mut() else { return };
        self.dirty = true;
        let in_sub = m.sub.is_some() && m.sub.as_ref().unwrap().1.hot.is_some();
        let target: &mut Menu = if in_sub { &mut m.sub.as_mut().unwrap().1 } else { m };
        let n = target.items.len();
        let step = |from: Option<usize>, d: i32, items: &[MItem]| -> Option<usize> {
            let mut i = from.map_or(if d > 0 { -1 } else { n as i32 }, |i| i as i32);
            for _ in 0..n {
                i = (i + d).rem_euclid(n as i32);
                if !items[i as usize].is_sep() && items[i as usize].on {
                    return Some(i as usize);
                }
            }
            from
        };
        match code {
            k::ESC => {
                if in_sub {
                    m.sub = None;
                } else {
                    self.menu = None;
                }
            }
            k::UP => target.hot = step(target.hot, -1, &target.items),
            k::DOWN => target.hot = step(target.hot, 1, &target.items),
            k::LEFT if in_sub => m.sub = None,
            k::RIGHT | k::ENTER | k::SPACE => {
                let Some(i) = target.hot else { return };
                let it = target.items[i].clone();
                if !it.sub.is_empty() {
                    if !in_sub {
                        self.open_sub(i);
                        if let Some(m) = self.menu.as_mut() {
                            if let Some((_, s)) = m.sub.as_mut() {
                                s.hot = step(None, 1, &s.items);
                            }
                        }
                    }
                } else if code != k::RIGHT && it.on {
                    self.act(it.act);
                }
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------- keys

    pub fn key(&mut self, code: u32) {
        self.dirty = true;
        if self.dialog.is_some() {
            self.dialog_key(code);
            return;
        }
        if self.menu.is_some() {
            // Keys a menu doesn't use close it and do nothing else (Alt+Left must not go back
            // from under an open menu).
            let m = self.mods;
            if m.alt || m.ctrl || !matches!(code, k::UP | k::DOWN | k::LEFT | k::RIGHT | k::ENTER | k::SPACE | k::ESC) {
                self.menu = None;
                self.drag_to = None;
            } else {
                self.menu_key(code);
            }
            return;
        }
        let m = self.mods;
        if let Some((_, tf)) = self.rename.as_mut() {
            match code {
                k::ESC => self.rename = None,
                k::ENTER => self.commit_rename(None),
                k::TAB => self.commit_rename(Some(if m.shift { -1 } else { 1 })),
                _ => match tf.key(code, m) {
                    TAct::Paste => {
                        if let Some(t) = paste_text() {
                            tf.insert(&t);
                        }
                    }
                    TAct::Copy(s) | TAct::Cut(s) => copy_out(&s),
                    _ => {}
                },
            }
            return;
        }
        if let Some(tf) = self.addr.as_mut() {
            match code {
                k::ESC => self.addr = None,
                k::ENTER => {
                    let t = tf.text();
                    self.addr = None;
                    self.addr_go(&t);
                }
                _ => match tf.key(code, m) {
                    TAct::Paste => {
                        if let Some(t) = paste_text() {
                            tf.insert(&t);
                        }
                    }
                    TAct::Copy(s) | TAct::Cut(s) => copy_out(&s),
                    _ => {}
                },
            }
            return;
        }
        if self.search_on {
            match code {
                k::ESC => self.exit_search(),
                k::ENTER => self.start_search(),
                k::DOWN | k::TAB => self.search_on = false,
                _ => match self.search.key(code, m) {
                    TAct::Changed => self.search_at = Some(sys::now_ns() + 350_000_000),
                    TAct::Paste => {
                        if let Some(t) = paste_text() {
                            self.search.insert(&t);
                            self.search_at = Some(sys::now_ns() + 350_000_000);
                        }
                    }
                    TAct::Copy(s) | TAct::Cut(s) => copy_out(&s),
                    _ => {}
                },
            }
            return;
        }
        let n = self.items.len();
        let f = self.focus.unwrap_or(0);
        match code {
            // History and places.
            k::LEFT if m.alt => self.go_back(),
            k::RIGHT if m.alt => self.go_fwd(),
            k::UP if m.alt => self.go_up(),
            k::BACKSPACE => self.go_back(),
            k::L | k::D if (code == k::L && m.ctrl) || (code == k::D && m.alt) => self.edit_addr(),
            k::F4 | k::F6 => self.edit_addr(),
            k::E | k::F if m.ctrl => self.search_on = true,
            k::F3 => self.search_on = true,
            k::F5 => self.reload(true),
            k::T if m.ctrl => self.act(Act::NewTab),
            k::W if m.ctrl => self.close_tab(self.cur),
            k::TAB if m.ctrl => {
                let n = self.tabs.len();
                let i = if m.shift { (self.cur + n - 1) % n } else { (self.cur + 1) % n };
                self.switch_tab(i);
            }
            c @ 2..=10 if m.ctrl => {
                // Ctrl+1..9: tabs (9 = the last).
                let i = if c == 10 { self.tabs.len() - 1 } else { (c - 2) as usize };
                self.switch_tab(i.min(self.tabs.len() - 1));
            }
            k::N6 if m.ctrl && m.shift => self.set_mode(Mode::Details),
            k::N if m.ctrl && m.shift => self.act(Act::NewFolder),
            k::H if m.ctrl => self.act(Act::Hidden),
            // Editing.
            k::A if m.ctrl => self.act(Act::SelAll),
            k::C if m.ctrl && m.shift => self.act(Act::CopyPath),
            k::C if m.ctrl => self.act(Act::Copy),
            k::X if m.ctrl => self.act(Act::Cut),
            k::V if m.ctrl => self.act(Act::Paste),
            k::Z if m.ctrl => self.act(Act::Undo),
            k::Y if m.ctrl => self.act(Act::Redo),
            k::F2 => self.act(Act::Rename),
            k::DELETE if m.shift => self.act(Act::Delete),
            k::DELETE => self.act(Act::Trash),
            k::ENTER if m.alt => self.act(Act::Props),
            k::ENTER => self.act(Act::Open),
            k::MENU => {
                let (x, y) = self.focus.map(|f| {
                    let r = self.screen(self.rects[f]);
                    (r.x + 40, r.y1())
                }).unwrap_or((NAV_W + 40, TOP + 40));
                let menu = if self.selected().is_empty() { self.bg_menu() } else { self.item_menu() };
                self.open_menu(menu, x, y);
            }
            k::ESC => {
                // Esc drops a header being dragged where it was.
                if self.col_drag.take().is_some() {
                    self.press = None;
                    return;
                }
                self.clip_cut_clear();
                self.sel.iter_mut().for_each(|s| *s = false);
            }
            // Moving.
            k::UP | k::DOWN | k::LEFT | k::RIGHT | k::HOME | k::END | k::PGUP | k::PGDN if n > 0 => {
                let to = if self.focus.is_none() {
                    0
                } else {
                    match code {
                        k::UP => self.neighbor(f, 0, -1),
                        k::DOWN => self.neighbor(f, 0, 1),
                        k::LEFT if self.view.mode != Mode::Details => self.neighbor(f, -1, 0),
                        k::RIGHT if self.view.mode != Mode::Details => self.neighbor(f, 1, 0),
                        k::LEFT | k::RIGHT => f,
                        k::HOME => 0,
                        k::END => n - 1,
                        _ => {
                            // A page: the item about one view height away.
                            let vh = self.view_rect().h;
                            let r = self.rects[f];
                            let ty = if code == k::PGUP { r.y - vh + r.h } else { r.y + vh - r.h };
                            let mut best = f;
                            for (i, q) in self.rects.iter().enumerate() {
                                let ok = if code == k::PGUP { q.y >= ty && q.y < r.y } else { q.y <= ty && q.y > r.y };
                                if ok && (q.x - r.x).abs() < r.w && (code == k::PGDN || best == f || q.y < self.rects[best].y) {
                                    best = i;
                                    if code == k::PGUP {
                                        break;
                                    }
                                }
                            }
                            if best == f {
                                if code == k::PGUP { 0 } else { n - 1 }
                            } else {
                                best
                            }
                        }
                    }
                };
                self.move_key(to);
            }
            k::SPACE if m.ctrl => {
                if let Some(f) = self.focus {
                    self.sel[f] = !self.sel[f];
                }
            }
            _ if !m.ctrl && !m.alt => {
                // Type-ahead: jump to the item that begins with what was typed.
                if let Some(c) = crate::textedit::keycode_char(code, m.shift, m.caps, m.group) {
                    let now = sys::now_ns();
                    if now - self.typeahead.1 > 1_000_000_000 {
                        self.typeahead.0.clear();
                    }
                    self.typeahead.1 = now;
                    self.typeahead.0.extend(c.to_lowercase());
                    let q = self.typeahead.0.clone();
                    // The same letter again: the next item with it.
                    let same = q.chars().count() > 1 && q.chars().all(|x| Some(x) == q.chars().next());
                    let single = q.chars().count() == 1;
                    let (q, start) = if same { (q.chars().next().unwrap().to_string(), f + 1) } else { (q, if single { f + 1 } else { f }) };
                    let hit = (0..n).map(|k| (start + k) % n).find(|i| self.items[*i].disp.to_lowercase().starts_with(&q));
                    if let Some(i) = hit {
                        self.select_only(i);
                    }
                }
            }
            _ => {}
        }
    }

    fn clip_cut_clear(&mut self) {
        if self.clip.as_ref().map_or(false, |c| c.1) {
            self.clip = None;
        }
    }

    pub fn edit_addr(&mut self) {
        let mut tf = TextField::default();
        tf.set(&self.addr_text());
        self.addr = Some(tf);
        self.search_on = false;
    }

    pub fn addr_text(&self) -> String {
        match self.loc() {
            Loc::Dir(p) => p.display().to_string(),
            Loc::Pc => "Этот компьютер".into(),
            Loc::Trash => "Корзина".into(),
            Loc::Arc(f, i) => if i.is_empty() { f.display().to_string() } else { format!("{}/{i}", f.display()) },
            Loc::Search(b, _) => b.display().to_string(),
        }
    }

    /// What was typed into the address bar: a folder, a file, a place, or a program.
    pub fn addr_go(&mut self, t: &str) {
        let t = t.trim();
        let low = t.to_lowercase();
        if low == "этот компьютер" || low == "computer:" || low == "computer:///" {
            self.navigate(Loc::Pc);
            return;
        }
        if low == "корзина" || low == "trash:" || low == "trash:///" {
            self.navigate(Loc::Trash);
            return;
        }
        let home = model::home();
        let mut s = t.strip_prefix("file://").unwrap_or(t).to_string();
        if s == "~" || s.starts_with("~/") {
            s = format!("{}{}", home.display(), &s[1..]);
        }
        let p = if s.starts_with('/') {
            PathBuf::from(&s)
        } else {
            match self.loc().dir() {
                Some(d) => d.join(&s),
                None => home.join(&s),
            }
        };
        if p.is_dir() {
            self.navigate(Loc::Dir(p));
            return;
        }
        if p.is_file() {
            if let Some(e) = model::stat(&p) {
                let it = self.make_item(e);
                if ops::is_archive(&it.e.ext) {
                    self.navigate(Loc::Arc(p, String::new()));
                } else {
                    if let Some(d) = p.parent() {
                        self.navigate(Loc::Dir(d.to_path_buf()));
                    }
                    if let Some(i) = self.items.iter().position(|x| x.e.path == p) {
                        self.select_only(i);
                        self.open_items(vec![i]);
                    }
                }
            }
            return;
        }
        // A program name, as Windows runs «cmd» from there.
        let prog = t.split_whitespace().next().unwrap_or("");
        if !prog.is_empty() && !prog.contains('/') && open::have(prog) {
            open::spawn(t, self.loc().dir());
            return;
        }
        self.error(&format!("Не удаётся найти «{t}». Проверьте правильность написания и повторите попытку."));
    }
}

/// The clipboard's text (wl-paste when installed).
pub fn paste_text() -> Option<String> {
    if open::dry() {
        return open::DRY_LOG.lock().unwrap().iter().rev().find_map(|l| l.strip_prefix("clip ").map(str::to_string));
    }
    let o = std::process::Command::new("wl-paste").arg("-n").arg("-t").arg("text/plain;charset=utf-8").stdin(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
}

pub fn copy_out(s: &str) {
    if open::dry() {
        open::DRY_LOG.lock().unwrap().push(format!("clip {s}"));
        return;
    }
    if let Ok(mut c) = std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
        use std::io::Write;
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(s.as_bytes());
        }
        let _ = c.wait();
    }
}
