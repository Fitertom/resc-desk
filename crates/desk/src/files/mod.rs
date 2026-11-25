//! «Проводник» (`resc-desk files [path]`, Win+E): resc-desk's file manager, made to behave
//! like Windows 11 Explorer — tabs, a crumb address bar, the command bar, the
//! navigation pane, per-folder views (details / icons, sort, groups), Windows' natural
//! order and formats, selection by mouse, band and keyboard, type-ahead, inline rename,
//! the copy dialog with its speed graph, name-clash questions, the trash, undo — and
//! 7-Zip for every archive (open as a folder, extract, add, test, checksums).
//! Its own Wayland window (client.rs) like «Параметры».
mod desktop;
mod dnd;
mod draw;
mod input;
#[cfg(test)]
mod tests;
pub mod model;
pub mod open;
pub mod ops;
pub mod trash;
mod ui;

use crate::anim::Spring;
use crate::client::{Ev, Window};
use crate::geom::Rect;
use crate::textedit::{Mods, TextField};
use model::{Entry, Mount};
use ops::{Answer, Op, Undo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ui::{RectDrop, G};

pub const TABS_H: i32 = 40;
pub const TOOL_H: i32 = 48;
pub const CMD_H: i32 = 46;
pub const TOP: i32 = TABS_H + TOOL_H + CMD_H;
pub const STATUS_H: i32 = 30;
pub const NAV_W: i32 = 236;
pub const HEAD_H: i32 = 32;
pub const ROW_H: i32 = 32;
pub const NAV_ROW: i32 = 32;
pub const GROUP_H: i32 = 38;

pub fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ------------------------------------------------------------------ places and items

#[derive(Clone, PartialEq, Debug)]
pub enum Loc {
    Dir(PathBuf),
    /// «Этот компьютер»: the user folders and the disks.
    Pc,
    Trash,
    /// An archive opened as a folder: the file and the path inside ("" or "a/b").
    Arc(PathBuf, String),
    /// Search results under a folder.
    Search(PathBuf, String),
}

impl Loc {
    fn key(&self) -> String {
        match self {
            Loc::Dir(p) => p.to_string_lossy().to_string(),
            Loc::Pc => ":pc".into(),
            Loc::Trash => ":trash".into(),
            Loc::Arc(f, i) => format!(":arc:{}/{i}", f.to_string_lossy()),
            Loc::Search(..) => ":search".into(),
        }
    }

    /// The folder files land in when pasted / dropped here.
    pub fn dir(&self) -> Option<&Path> {
        match self {
            Loc::Dir(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub e: Entry,
    /// What is shown («Документы» for ~/Documents).
    pub disp: String,
    pub kind: String,
    pub icon: String,
    pub mime: &'static str,
    pub orig: Option<PathBuf>,
    pub deleted: i64,
    pub packed: u64,
    /// In an archive: its path inside.
    pub inner: String,
    pub drive: Option<Mount>,
    /// Search results: the folder it is in.
    pub folder: String,
    pub group: (u32, String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Col {
    Name,
    Date,
    Type,
    Size,
    Orig,
    Deleted,
    Packed,
    Folder,
    Perms,
    Owner,
}

impl Col {
    pub fn title(self) -> &'static str {
        match self {
            Col::Name => "Имя",
            Col::Date => "Дата изменения",
            Col::Type => "Тип",
            Col::Size => "Размер",
            Col::Orig => "Исходное расположение",
            Col::Deleted => "Дата удаления",
            Col::Packed => "Сжатый",
            Col::Folder => "Папка",
            Col::Perms => "Права",
            Col::Owner => "Владелец",
        }
    }
    fn width(self) -> i32 {
        match self {
            Col::Name => 300,
            Col::Date | Col::Deleted => 150,
            Col::Type => 170,
            Col::Size | Col::Packed => 100,
            Col::Orig | Col::Folder => 250,
            Col::Perms => 110,
            Col::Owner => 120,
        }
    }
    pub fn right(self) -> bool {
        matches!(self, Col::Size | Col::Packed)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Details,
    /// Icons of this many px (Windows: 16 small … 256 extra large).
    Icons(i32),
    /// «Этот компьютер»: tiles.
    Tiles,
}

pub const ZOOM: [i32; 7] = [32, 48, 64, 96, 128, 192, 256];

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    pub mode: Mode,
    pub sort: Col,
    pub desc: bool,
    pub group: Option<Col>,
}

impl Default for View {
    fn default() -> Self {
        View { mode: Mode::Details, sort: Col::Name, desc: false, group: None }
    }
}

pub struct Tab {
    pub loc: Loc,
    pub back: Vec<Loc>,
    pub fwd: Vec<Loc>,
    pub sel: Vec<String>,
    pub scroll: f32,
}

#[derive(Clone)]
pub struct NavItem {
    pub label: String,
    pub loc: Loc,
    pub icon: String,
    pub glyph: Option<G>,
    pub indent: i32,
    /// A separator line above it.
    pub sep: bool,
    pub eject: bool,
}

// ------------------------------------------------------------------ menus and dialogs

#[derive(Clone, PartialEq, Debug)]
pub enum Act {
    None,
    Open,
    OpenWith(String),
    OpenTab,
    Terminal,
    Cut,
    Copy,
    CopyPath,
    Paste,
    Rename,
    Trash,
    Delete,
    Props,
    NewFolder,
    NewText,
    NewFrom(PathBuf),
    Refresh,
    Undo,
    Redo,
    SelAll,
    SelNone,
    SelInvert,
    Mode(Mode),
    Sort(Col),
    ToggleCol(Col),
    FitCol(usize),
    FitCols,
    ResetCols,
    Desc(bool),
    Group(Option<Col>),
    Hidden,
    Ext,
    ZOpen,
    ZExtract,
    ZHere,
    ZTo,
    ZAdd,
    ZQuick(&'static str),
    ZTest,
    Hash(&'static str),
    Restore,
    EmptyTrash,
    Link,
    NewTab,
    CloseTab(usize),
    CloseOthers(usize),
    DupTab(usize),
    DropCopy,
    DropMove,
    DropLink,
    DropCancel,
    /// «Параметры» at a page.
    Settings(&'static str),
    Eject(PathBuf),
    Nav(Loc),
    NavTab(Loc),
}

#[derive(Clone)]
pub struct MItem {
    pub label: String,
    pub hint: String,
    pub act: Act,
    pub on: bool,
    pub sub: Vec<MItem>,
    pub check: Option<bool>,
    pub glyph: Option<G>,
    pub icon: String,
    pub bold: bool,
}

impl MItem {
    pub fn new(label: &str, act: Act) -> MItem {
        MItem { label: label.into(), hint: String::new(), act, on: true, sub: vec![], check: None, glyph: None, icon: String::new(), bold: false }
    }
    pub fn sep() -> MItem {
        MItem::new("", Act::None)
    }
    pub fn is_sep(&self) -> bool {
        self.label.is_empty()
    }
    fn hint(mut self, h: &str) -> Self {
        self.hint = h.into();
        self
    }
    fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
    fn g(mut self, g: G) -> Self {
        self.glyph = Some(g);
        self
    }
    fn check(mut self, c: bool) -> Self {
        self.check = Some(c);
        self
    }
    fn sub(mut self, s: Vec<MItem>) -> Self {
        self.sub = s;
        self
    }
    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    fn icon(mut self, i: &str) -> Self {
        self.icon = i.into();
        self
    }
}

pub const MENU_ROW: i32 = 32;
pub const MENU_SEP: i32 = 9;

pub struct Menu {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub items: Vec<MItem>,
    pub hot: Option<usize>,
    pub sub: Option<(usize, Box<Menu>)>,
    pub t: Spring,
}

impl Menu {
    fn new(items: Vec<MItem>, x: i32, y: i32, ww: i32, wh: i32) -> Menu {
        let mut w = 200;
        for it in &items {
            w = w.max(ui::tw(&it.label, 13.0) + ui::tw(&it.hint, 12.0) + 96);
        }
        let h = Menu::height(&items);
        let x = x.min(ww - w - 4).max(4);
        let y = if y + h > wh - 4 { (y - h).max(4) } else { y };
        let mut t = Spring::new(0.0, 0.22, 0.8);
        t.to(1.0);
        Menu { x, y, w, items, hot: None, sub: None, t }
    }
    fn height(items: &[MItem]) -> i32 {
        12 + items.iter().map(|i| if i.is_sep() { MENU_SEP } else { MENU_ROW }).sum::<i32>()
    }
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, Menu::height(&self.items))
    }
    pub fn item_rect(&self, i: usize) -> Rect {
        let mut y = self.y + 6;
        for (j, it) in self.items.iter().enumerate() {
            let h = if it.is_sep() { MENU_SEP } else { MENU_ROW };
            if j == i {
                return Rect::new(self.x + 4, y, self.w - 8, h);
            }
            y += h;
        }
        Rect::default()
    }
    fn at(&self, x: f64, y: f64) -> Option<usize> {
        (0..self.items.len()).find(|i| !self.items[*i].is_sep() && self.item_rect(*i).contains(x, y))
    }
}

#[derive(Clone)]
pub enum On {
    Info,
    DeleteForever(Vec<PathBuf>),
    Conflict(u32),
    /// Retry op (7-Zip) with a password.
    ZipPassword(u32),
    /// Open the archive (as a folder) with a password.
    ArcPassword(PathBuf, String),
    Add7z(Vec<PathBuf>, PathBuf),
    Extract(PathBuf),
    /// Rename despite a new extension.
    ExtChange(PathBuf, String),
    /// The name is taken: rename to the offered free one.
    Taken(PathBuf, String),
    MakeExec(PathBuf),
    RunScript(PathBuf),
    EmptyTrash,
    Props(PathBuf),
}

pub struct Field {
    pub label: String,
    pub tf: TextField,
    pub secret: bool,
}

pub struct Dialog {
    pub title: String,
    pub lines: Vec<String>,
    pub fields: Vec<Field>,
    /// (label, options, chosen)
    pub choices: Vec<(String, Vec<String>, usize)>,
    pub check: Option<(String, bool)>,
    pub buttons: Vec<String>,
    pub default: usize,
    pub focus: usize,
    pub on: On,
    pub hot: Option<usize>,
    pub t: Spring,
    pub icon: String,
    pub wide: bool,
}

impl Dialog {
    pub fn new(title: &str, lines: Vec<String>, buttons: &[&str], on: On) -> Dialog {
        let mut t = Spring::new(0.0, 0.26, 0.78);
        t.to(1.0);
        Dialog {
            title: title.into(),
            lines,
            fields: vec![],
            choices: vec![],
            check: None,
            buttons: buttons.iter().map(|b| b.to_string()).collect(),
            default: 0,
            focus: 0,
            on,
            hot: None,
            t,
            icon: String::new(),
            wide: false,
        }
    }
    fn field(mut self, label: &str, text: &str, secret: bool) -> Self {
        let mut tf = TextField::default();
        tf.set(text);
        self.fields.push(Field { label: label.into(), tf, secret });
        self
    }
    fn choice(mut self, label: &str, opts: &[&str], i: usize) -> Self {
        self.choices.push((label.into(), opts.iter().map(|o| o.to_string()).collect(), i));
        self
    }
}

// ------------------------------------------------------------------ pointer state

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    None,
    Tab(usize),
    TabClose(usize),
    NewTab,
    Back,
    Fwd,
    Up,
    Refresh,
    Crumb(usize),
    Addr,
    Search,
    SearchClear,
    Cmd(usize),
    Nav(usize),
    Eject(usize),
    Head(usize),
    HeadEdge(usize),
    Item(usize),
    Empty,
    Scroll,
    ViewDetails,
    ViewIcons,
    OpPause(usize),
    OpCancel(usize),
    OpToggle,
    /// ─ □ ✕ (0, 1, 2) in our title row.
    Caption(usize),
    /// Empty title row: drags the window.
    TitleDrag,
}

/// What the window itself has to do (the compositor moves / sizes it).
#[derive(Clone, PartialEq, Debug)]
pub enum WinReq {
    Move,
    Minimize,
    Maximize,
    /// Hand a drag to the system: types, actions, the picture (px, w, h, hotspot).
    StartDrag { mimes: Vec<&'static str>, actions: u32, icon: Option<(Vec<u32>, i32, i32, i32, i32)> },
    /// What we would take from the drag over us.
    DndAccept { mime: Option<&'static str>, actions: u32, pref: u32 },
}

pub struct Press {
    pub x: f64,
    pub y: f64,
    pub hit: Hit,
    pub button: u32,
    pub dragging: bool,
    /// An unselected item pressed: selection happens on press; a selected one on release.
    pub defer: bool,
    pub scroll0: f32,
    pub col_w0: i32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmd {
    New,
    Cut,
    Copy,
    Paste,
    Rename,
    Delete,
    Sort,
    View,
    Zip,
    More,
    Restore,
    Empty,
}

pub struct App {
    pub w: i32,
    pub h: i32,
    pub tabs: Vec<Tab>,
    pub cur: usize,
    pub items: Vec<Item>,
    pub sel: Vec<bool>,
    pub focus: Option<usize>,
    pub anchor: usize,
    /// Item rects in content space (y 0 = top of the scrolled area).
    pub rects: Vec<Rect>,
    pub heads: Vec<(Rect, String)>,
    pub content_h: i32,
    pub view: View,
    pub views: HashMap<String, View>,
    pub widths: HashMap<Col, i32>,
    /// Column order and visibility per kind of place (see col_kind).
    pub col_sets: HashMap<&'static str, Vec<(Col, bool)>>,
    /// A header being dragged: (shown index, pointer x).
    pub col_drag: Option<(usize, f64)>,
    pub show_hidden: bool,
    pub show_ext: bool,
    pub scroll: Spring,
    pub nav_scroll: Spring,
    pub ptr: Option<(f64, f64)>,
    pub mods: Mods,
    pub press: Option<Press>,
    pub band: Option<(f64, f64, f64, f64)>,
    pub hot: Hit,
    pub fdrop: RectDrop,
    pub hdrop: RectDrop,
    pub ndrop: RectDrop,
    pub tdrop: RectDrop,
    pub addr: Option<TextField>,
    pub search: TextField,
    pub search_on: bool,
    pub rename: Option<(usize, TextField)>,
    pub menu: Option<Menu>,
    pub dialog: Option<Dialog>,
    pub clip: Option<(Vec<PathBuf>, bool)>,
    pub ops: Vec<Op>,
    pub ops_open: bool,
    next_op: u32,
    pub undo: Vec<Undo>,
    pub redo: Vec<Undo>,
    apps: Option<open::Apps>,
    pub nav: Vec<NavItem>,
    pub mounts: Vec<Mount>,
    watch: Watch,
    reload_at: Option<u64>,
    pub toast: Option<(String, u64)>,
    last_click: (u64, Hit),
    typeahead: (String, u64),
    pub have7z: bool,
    pub dirty: bool,
    pub quit: bool,
    pub title: String,
    /// New things made here: they stay at the end until F5 (as in Windows).
    fresh: Vec<String>,
    arcs: HashMap<PathBuf, (Vec<ops::ArcEntry>, Option<String>)>,
    search_rx: Option<std::sync::mpsc::Receiver<Vec<Entry>>>,
    search_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub searching: bool,
    props_rx: Option<std::sync::mpsc::Receiver<(u64, u64)>>,
    hash_rx: Option<std::sync::mpsc::Receiver<Result<Vec<String>, String>>>,
    /// Drag & drop inside the window: the target and the effect.
    pub drag_to: Option<(Loc, Act)>,
    /// A system drag over the window, and ours gone out of it (dnd.rs).
    pub incoming: Option<dnd::Incoming>,
    pub outgoing: Option<dnd::Outgoing>,
    /// A tab dragged over the strip: where it would go.
    pub tab_drop_at: Option<usize>,
    /// Files a drop brought from elsewhere (instead of the selection).
    pub drop_paths: Option<Vec<PathBuf>>,
    /// The desktop's icons rather than a window (desktop.rs).
    pub desk: bool,
    spring_at: Option<(Hit, u64)>,
    pub load_err: Option<String>,
    temp_n: u32,
    then: HashMap<u32, Then>,
    /// The item to focus once a delete has emptied the selection.
    pending_focus: Option<String>,
    /// An undo (false) or redo (true) running: where its own undo record goes.
    undoing: Option<bool>,
    /// Operations waiting for a password.
    retry: HashMap<u32, Op>,
    hash_algo: &'static str,
    /// A slow second click on a selected name: rename it then (unless a double click came).
    pub rename_at: Option<(usize, u64)>,
    pub search_at: Option<u64>,
    pub zoom_acc: f64,
    pub winreq: Vec<WinReq>,
    pub maximized: bool,
}

/// What happens when an operation ends.
enum Then {
    Open(PathBuf),
    /// Move these (extracted into a temp folder) into `dest`.
    Place(Vec<PathBuf>, PathBuf),
    Toast(String),
}

// ------------------------------------------------------------------ inotify

extern "C" {
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, path: *const std::ffi::c_char, mask: u32) -> i32;
    fn inotify_rm_watch(fd: i32, wd: i32) -> i32;
}

struct Watch {
    fd: i32,
    wd: i32,
}

impl Watch {
    fn new() -> Watch {
        // IN_NONBLOCK | IN_CLOEXEC
        Watch { fd: unsafe { inotify_init1(0o4000 | 0o2000000) }, wd: -1 }
    }
    fn set(&mut self, p: Option<&Path>) {
        if self.fd < 0 {
            return;
        }
        if self.wd >= 0 {
            unsafe { inotify_rm_watch(self.fd, self.wd) };
            self.wd = -1;
        }
        if let Some(c) = p.and_then(|p| std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).ok()) {
            // attrib, close_write, moved from/to, create, delete, delete/move self
            self.wd = unsafe { inotify_add_watch(self.fd, c.as_ptr(), 0x4 | 0x8 | 0x40 | 0x80 | 0x100 | 0x200 | 0x400 | 0x800) };
        }
    }
    /// True when something changed.
    fn drain(&self) -> bool {
        if self.fd < 0 {
            return false;
        }
        let mut buf = [0u8; 4096];
        let mut any = false;
        // Non-blocking: read until it would block.
        while unsafe { sys::ffi::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) } > 0 {
            any = true;
        }
        any
    }
}

// ------------------------------------------------------------------ the app

fn user_dir_name(key: &str) -> &'static str {
    match key {
        "DESKTOP" => "Рабочий стол",
        "DOWNLOAD" => "Загрузки",
        "DOCUMENTS" => "Документы",
        "PICTURES" => "Изображения",
        "MUSIC" => "Музыка",
        "VIDEOS" => "Видео",
        "TEMPLATES" => "Шаблоны",
        _ => "Общие",
    }
}

pub fn drive_label(m: &Mount) -> String {
    if m.path == Path::new("/") {
        "Локальный диск (/)".into()
    } else {
        m.label.clone()
    }
}

fn date_bucket(t: i64) -> (u32, &'static str) {
    let now = now_s();
    let off = model::tz_offset(now);
    let day = |x: i64| (x + off).div_euclid(86400);
    let (d, today) = (day(t), day(now));
    // Monday-based weeks: 1970-01-01 was a Thursday.
    let wd = (today + 3).rem_euclid(7);
    let (ny, nm, ..) = model::civil(now + off);
    let (y, m, ..) = model::civil(t + off);
    if d >= today {
        (0, "Сегодня")
    } else if d == today - 1 {
        (1, "Вчера")
    } else if d >= today - wd {
        (2, "Ранее на этой неделе")
    } else if d >= today - wd - 7 {
        (3, "На прошлой неделе")
    } else if y == ny && m == nm {
        (4, "Ранее в этом месяце")
    } else if (y == ny && m + 1 == nm) || (y + 1 == ny && m == 12 && nm == 1) {
        (5, "В прошлом месяце")
    } else if y == ny {
        (6, "Ранее в этом году")
    } else {
        (7, "Давно")
    }
}

impl App {
    fn new(start: Loc, w: i32, h: i32) -> App {
        let mut widths = HashMap::new();
        for c in [Col::Name, Col::Date, Col::Type, Col::Size, Col::Orig, Col::Deleted, Col::Packed, Col::Folder, Col::Perms, Col::Owner] {
            widths.insert(c, c.width());
        }
        let mut a = App {
            w,
            h,
            tabs: vec![Tab { loc: start.clone(), back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 }],
            cur: 0,
            items: vec![],
            sel: vec![],
            focus: None,
            anchor: 0,
            rects: vec![],
            heads: vec![],
            content_h: 0,
            view: View::default(),
            views: HashMap::new(),
            widths,
            col_sets: HashMap::new(),
            col_drag: None,
            show_hidden: false,
            show_ext: true,
            scroll: Spring::new(0.0, 0.22, 1.0),
            nav_scroll: Spring::new(0.0, 0.22, 1.0),
            ptr: None,
            mods: Mods::default(),
            press: None,
            band: None,
            hot: Hit::None,
            fdrop: RectDrop::default(),
            hdrop: RectDrop::default(),
            ndrop: RectDrop::default(),
            tdrop: RectDrop::default(),
            addr: None,
            search: TextField::default(),
            search_on: false,
            rename: None,
            menu: None,
            dialog: None,
            clip: None,
            ops: vec![],
            ops_open: true,
            next_op: 1,
            undo: vec![],
            redo: vec![],
            apps: None,
            nav: vec![],
            mounts: vec![],
            watch: Watch::new(),
            reload_at: None,
            toast: None,
            last_click: (0, Hit::None),
            typeahead: (String::new(), 0),
            have7z: open::have("7z"),
            dirty: true,
            quit: false,
            title: String::new(),
            fresh: vec![],
            arcs: HashMap::new(),
            search_rx: None,
            search_stop: Default::default(),
            searching: false,
            props_rx: None,
            hash_rx: None,
            drag_to: None,
            incoming: None,
            outgoing: None,
            tab_drop_at: None,
            drop_paths: None,
            desk: false,
            spring_at: None,
            load_err: None,
            temp_n: 0,
            then: HashMap::new(),
            pending_focus: None,
            undoing: None,
            retry: HashMap::new(),
            hash_algo: "",
            rename_at: None,
            search_at: None,
            zoom_acc: 0.0,
            winreq: vec![],
            maximized: false,
        };
        a.build_nav();
        a.enter(start, true);
        a
    }

    pub fn loc(&self) -> &Loc {
        &self.tabs[self.cur].loc
    }

    fn apps(&mut self) -> &open::Apps {
        self.apps.get_or_insert_with(open::Apps::load)
    }

    pub fn build_nav(&mut self) {
        let mut v = Vec::new();
        let home = model::home();
        v.push(NavItem { label: "Домашняя папка".into(), loc: Loc::Dir(home.clone()), icon: "user-home;folder-home;folder".into(), glyph: Some(G::Home), indent: 0, sep: false, eject: false });
        let mut first = true;
        for (k, p) in model::user_dirs() {
            if matches!(k, "TEMPLATES" | "PUBLICSHARE") {
                continue;
            }
            let icon = match k {
                "DESKTOP" => "user-desktop;folder",
                "DOWNLOAD" => "folder-download;folder",
                "DOCUMENTS" => "folder-documents;folder",
                "PICTURES" => "folder-pictures;folder",
                "MUSIC" => "folder-music;folder",
                _ => "folder-videos;folder",
            };
            v.push(NavItem { label: user_dir_name(k).into(), loc: Loc::Dir(p), icon: icon.into(), glyph: None, indent: 0, sep: first, eject: false });
            first = false;
        }
        v.push(NavItem { label: "Этот компьютер".into(), loc: Loc::Pc, icon: "computer;".into(), glyph: Some(G::Pc), indent: 0, sep: true, eject: false });
        self.mounts = model::mounts();
        for m in &self.mounts {
            let icon = if m.removable { "drive-removable-media;drive-harddisk" } else { "drive-harddisk;" };
            v.push(NavItem { label: drive_label(m), loc: Loc::Dir(m.path.clone()), icon: icon.into(), glyph: None, indent: 18, sep: false, eject: m.removable });
        }
        v.push(NavItem { label: "Корзина".into(), loc: Loc::Trash, icon: if trash::list().is_empty() { "user-trash;" } else { "user-trash-full;user-trash" }.into(), glyph: Some(G::Trash), indent: 0, sep: true, eject: false });
        self.nav = v;
    }

    /// Display name of a path (user folders in Russian).
    pub fn disp_name(&self, p: &Path) -> String {
        let home = model::home();
        if p == home {
            return std::env::var("USER").unwrap_or_else(|_| "Домашняя папка".into());
        }
        if p.parent() == Some(home.as_path()) {
            for (k, d) in model::user_dirs() {
                if d == p {
                    return user_dir_name(k).into();
                }
            }
        }
        if let Some(m) = self.mounts.iter().find(|m| m.path == p) {
            return drive_label(m);
        }
        p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into())
    }

    pub fn loc_title(&self, l: &Loc) -> String {
        match l {
            Loc::Dir(p) => {
                if *p == model::home() {
                    "Домашняя папка".into()
                } else {
                    self.disp_name(p)
                }
            }
            Loc::Pc => "Этот компьютер".into(),
            Loc::Trash => "Корзина".into(),
            Loc::Arc(f, i) => {
                if i.is_empty() {
                    f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                } else {
                    i.rsplit('/').next().unwrap_or(i).to_string()
                }
            }
            Loc::Search(_, q) => format!("Результаты поиска: {q}"),
        }
    }

    /// The crumbs of the address bar: (label, where).
    pub fn crumbs(&self) -> Vec<(String, Loc)> {
        let home = model::home();
        let mut v = Vec::new();
        let dir_crumbs = |v: &mut Vec<(String, Loc)>, p: &Path| {
            if p.starts_with(&home) {
                v.push(("Домашняя папка".to_string(), Loc::Dir(home.clone())));
                let mut acc = home.clone();
                for c in p.strip_prefix(&home).unwrap().components() {
                    acc.push(c);
                    v.push((self.disp_name(&acc), Loc::Dir(acc.clone())));
                }
            } else {
                v.push(("Этот компьютер".to_string(), Loc::Pc));
                let m = model::mount_of(&self.mounts, p).map(|m| m.path.clone()).unwrap_or_else(|| PathBuf::from("/"));
                v.push((self.disp_name(&m), Loc::Dir(m.clone())));
                let mut acc = m.clone();
                if let Ok(rest) = p.strip_prefix(&m) {
                    for c in rest.components() {
                        acc.push(c);
                        v.push((c.as_os_str().to_string_lossy().to_string(), Loc::Dir(acc.clone())));
                    }
                }
            }
        };
        match self.loc() {
            Loc::Dir(p) => dir_crumbs(&mut v, p),
            Loc::Pc => v.push(("Этот компьютер".into(), Loc::Pc)),
            Loc::Trash => v.push(("Корзина".into(), Loc::Trash)),
            Loc::Search(b, q) => {
                dir_crumbs(&mut v, b);
                v.push((format!("Результаты поиска в «{}»", self.disp_name(b)), Loc::Search(b.clone(), q.clone())));
            }
            Loc::Arc(f, inner) => {
                if let Some(d) = f.parent() {
                    dir_crumbs(&mut v, d);
                }
                v.push((f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), Loc::Arc(f.clone(), String::new())));
                let mut acc = String::new();
                for part in inner.split('/').filter(|s| !s.is_empty()) {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(part);
                    v.push((part.to_string(), Loc::Arc(f.clone(), acc.clone())));
                }
            }
        }
        v
    }

    // -------------------------------------------------------------- navigation

    /// Goes to `l` in the current tab (adding to its history unless `fresh`).
    pub fn navigate(&mut self, l: Loc) {
        if self.desk {
            // From the desktop, places open in a window.
            if *self.loc() == l {
                self.reload(true);
            } else {
                self.open_window(&l);
            }
            return;
        }
        if *self.loc() == l {
            self.reload(true);
            return;
        }
        self.save_tab();
        let t = &mut self.tabs[self.cur];
        let old = std::mem::replace(&mut t.loc, l.clone());
        t.back.push(old);
        t.fwd.clear();
        self.enter(l, true);
    }

    /// Back / forward skip places that are gone since (deleted folders, removed archives).
    fn pop_live(stack: &mut Vec<Loc>, cur: &Loc) -> Option<Loc> {
        while let Some(l) = stack.pop() {
            if l != *cur && loc_exists(&l) {
                return Some(l);
            }
        }
        None
    }

    pub fn go_back(&mut self) {
        self.save_tab();
        let t = &mut self.tabs[self.cur];
        if let Some(l) = App::pop_live(&mut t.back, &t.loc) {
            let cur = std::mem::replace(&mut t.loc, l.clone());
            if t.fwd.last() != Some(&cur) {
                t.fwd.push(cur.clone());
            }
            self.enter(l, false);
            self.select_name_of(&cur);
        }
    }

    pub fn go_fwd(&mut self) {
        self.save_tab();
        let t = &mut self.tabs[self.cur];
        if let Some(l) = App::pop_live(&mut t.fwd, &t.loc) {
            let cur = std::mem::replace(&mut t.loc, l.clone());
            if t.back.last() != Some(&cur) {
                t.back.push(cur);
            }
            self.enter(l, false);
        }
    }

    pub fn parent(&self) -> Option<Loc> {
        match self.loc() {
            Loc::Dir(p) => {
                if self.mounts.iter().any(|m| m.path == *p) || p == Path::new("/") {
                    Some(Loc::Pc)
                } else {
                    p.parent().map(|q| Loc::Dir(q.to_path_buf()))
                }
            }
            Loc::Arc(f, i) => {
                if i.is_empty() {
                    f.parent().map(|q| Loc::Dir(q.to_path_buf()))
                } else {
                    Some(Loc::Arc(f.clone(), i.rsplit_once('/').map_or(String::new(), |x| x.0.to_string())))
                }
            }
            Loc::Search(b, _) => Some(Loc::Dir(b.clone())),
            Loc::Pc | Loc::Trash => None,
        }
    }

    pub fn go_up(&mut self) {
        let from = self.loc().clone();
        if let Some(p) = self.parent() {
            self.navigate(p);
            self.select_name_of(&from);
        }
    }

    /// After going up/back: the folder we came from is selected (as in Windows).
    fn select_name_of(&mut self, from: &Loc) {
        let i = match from {
            Loc::Dir(p) => self.items.iter().position(|it| it.e.path == *p),
            Loc::Arc(f, inner) if inner.is_empty() => self.items.iter().position(|it| it.e.path == *f),
            Loc::Arc(_, inner) => self.items.iter().position(|it| it.inner == *inner),
            _ => None,
        };
        if let Some(i) = i {
            self.select_only(i);
            self.reveal(i);
        }
    }

    fn save_tab(&mut self) {
        let names: Vec<String> = self.items.iter().zip(&self.sel).filter(|(_, s)| **s).map(|(i, _)| i.e.name.clone()).collect();
        let t = &mut self.tabs[self.cur];
        t.sel = names;
        t.scroll = self.scroll.target;
    }

    fn view_for(&self, l: &Loc) -> View {
        if self.desk {
            return self.views.get(&l.key()).copied().unwrap_or(View { mode: Mode::Icons(48), ..Default::default() });
        }
        if let Some(v) = self.views.get(&l.key()) {
            return *v;
        }
        match l {
            Loc::Pc => View { mode: Mode::Tiles, ..View::default() },
            Loc::Trash => View { sort: Col::Deleted, desc: true, ..View::default() },
            Loc::Search(..) => View::default(),
            Loc::Arc(..) => View::default(),
            Loc::Dir(p) => {
                for (k, d) in model::user_dirs() {
                    if d == *p {
                        match k {
                            "DOWNLOAD" => return View { sort: Col::Date, desc: true, group: Some(Col::Date), ..View::default() },
                            "PICTURES" | "VIDEOS" => return View { mode: Mode::Icons(96), ..View::default() },
                            _ => {}
                        }
                    }
                }
                View::default()
            }
        }
    }

    /// Shows `l` (already the tab's location): its view, list, selection.
    fn enter(&mut self, l: Loc, fresh_nav: bool) {
        self.stop_search();
        if self.col_drag.take().is_some() {
            self.press = None;
        }
        // The search box belongs to where it was typed.
        if !matches!(l, Loc::Search(..)) {
            self.search.clear();
            self.search_on = false;
            self.search_at = None;
        }
        self.rename = None;
        self.addr = None;
        self.fresh.clear();
        self.view = self.view_for(&l);
        if let Loc::Dir(p) = &l {
            if !p.is_dir() {
                // Gone since: the nearest folder above (reload does the walk).
                self.tabs[self.cur].loc = l.clone();
                self.items.clear();
                self.sel.clear();
                self.rects.clear();
                self.focus = None;
                self.reload(false);
                return;
            }
        }
        self.watch.set(match &l {
            Loc::Dir(p) => Some(p.as_path()),
            _ => None,
        });
        // What changed in the folder we left doesn't concern the new one.
        self.watch.drain();
        self.reload_at = None;
        self.load();
        // A folder full of pictures opens in icons the first time (like Windows' templates).
        if !self.views.contains_key(&l.key()) && matches!(l, Loc::Dir(_)) && self.view.mode == Mode::Details {
            let files: Vec<&Item> = self.items.iter().filter(|i| !i.e.dir).collect();
            let pics = files.iter().filter(|i| i.mime.starts_with("image/")).count();
            if files.len() >= 5 && pics * 10 >= files.len() * 6 {
                self.view.mode = Mode::Icons(96);
                self.lay();
            }
        }
        let t = &self.tabs[self.cur];
        let (names, sc) = if fresh_nav { (vec![], 0.0) } else { (t.sel.clone(), t.scroll) };
        self.sel = self.items.iter().map(|i| names.contains(&i.e.name)).collect();
        self.focus = self.sel.iter().position(|s| *s);
        self.anchor = self.focus.unwrap_or(0);
        self.scroll.snap(sc);
        self.clamp_scroll();
        self.fdrop = RectDrop::default();
        self.hdrop = RectDrop::default();
        if let Some(f) = self.focus {
            self.fdrop.snap(self.rects[f]);
            self.fdrop.a.snap(1.0);
        }
        self.title = self.loc_title(&l);
        self.dirty = true;
    }

    /// Lists the location again (keeping selection, focus and scroll by name).
    pub fn reload(&mut self, f5: bool) {
        // The folder itself is gone (deleted, moved, unmounted): like Windows, show the
        // nearest one above that still exists.
        if let Loc::Dir(p) = self.loc().clone() {
            if !p.is_dir() {
                let mut up = p.parent();
                while let Some(u) = up.filter(|u| !u.is_dir()) {
                    up = u.parent();
                }
                let to = up.map(|u| Loc::Dir(u.to_path_buf())).unwrap_or(Loc::Pc);
                let t = &mut self.tabs[self.cur];
                t.loc = to.clone();
                while t.back.last() == Some(&to) {
                    t.back.pop();
                }
                while t.fwd.last() == Some(&to) {
                    t.fwd.pop();
                }
                self.enter(to, true);
                return;
            }
        }
        if f5 {
            self.fresh.clear();
            self.arcs.clear();
            self.build_nav();
        }
        let names: Vec<String> = self.items.iter().zip(&self.sel).filter(|(_, s)| **s).map(|(i, _)| i.e.name.clone()).collect();
        let focus = self.focus.and_then(|f| self.items.get(f)).map(|i| i.e.name.clone());
        let ren = self.rename.as_ref().and_then(|(i, _)| self.items.get(*i)).map(|i| i.e.name.clone());
        self.load();
        self.sel = self.items.iter().map(|i| names.contains(&i.e.name)).collect();
        self.focus = focus.and_then(|n| self.items.iter().position(|i| i.e.name == n));
        if let Some(n) = ren {
            match self.items.iter().position(|i| i.e.name == n) {
                Some(i) => {
                    if let Some(r) = self.rename.as_mut() {
                        r.0 = i;
                    }
                }
                None => self.rename = None,
            }
        }
        self.clamp_scroll();
        if let Some(f) = self.focus {
            self.fdrop.snap(self.rects[f]);
        }
        self.dirty = true;
    }

    fn make_item(&self, e: Entry) -> Item {
        let k = e.kind();
        let mut disp = if e.dir { self.disp_name(&e.path) } else { e.name.clone() };
        if !self.show_ext && !e.dir && model::type_for_ext(&e.ext).is_some() {
            disp = disp.chars().take(model::stem_len(&e.name, false)).collect();
        }
        Item { disp, kind: k.name, icon: k.icon, mime: k.mime, orig: None, deleted: 0, packed: 0, inner: String::new(), drive: None, folder: String::new(), group: (0, String::new()), e }
    }

    fn load(&mut self) {
        self.load_err = None;
        let mut items = Vec::new();
        match self.loc().clone() {
            Loc::Dir(p) => match model::list(&p) {
                Ok(v) => {
                    for e in v {
                        if e.hidden && !self.show_hidden {
                            continue;
                        }
                        items.push(self.make_item(e));
                    }
                }
                Err(e) => {
                    self.load_err = Some(if e.raw_os_error() == Some(13) { format!("Нет доступа к «{}»", self.disp_name(&p)) } else { ops::explain(&e) });
                }
            },
            Loc::Pc => {
                for (k, p) in model::user_dirs() {
                    if matches!(k, "TEMPLATES" | "PUBLICSHARE") {
                        continue;
                    }
                    if let Some(e) = model::stat(&p) {
                        let mut it = self.make_item(e);
                        it.group = (0, "Папки".into());
                        items.push(it);
                    }
                }
                for m in self.mounts.clone() {
                    if let Some(e) = model::stat(&m.path) {
                        let mut it = self.make_item(e);
                        it.disp = drive_label(&m);
                        it.kind = if m.removable { "Съёмный диск".into() } else { "Локальный диск".into() };
                        it.icon = if m.removable { "drive-removable-media;drive-harddisk".into() } else { "drive-harddisk;".into() };
                        it.group = (1, "Устройства и диски".into());
                        it.drive = Some(m);
                        items.push(it);
                    }
                }
            }
            Loc::Trash => {
                for t in trash::list() {
                    let name = t.orig.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let Some(mut e) = model::stat(&t.file) else { continue };
                    e.ext = if e.dir { String::new() } else { model::ext_of(&name) };
                    let mut it = self.make_item(e);
                    it.disp = name;
                    let k = it.e.kind();
                    it.kind = k.name;
                    it.icon = k.icon;
                    it.mime = k.mime;
                    it.orig = t.orig.parent().map(Path::to_path_buf);
                    it.deleted = t.deleted;
                    items.push(it);
                }
            }
            Loc::Arc(f, inner) => {
                if !self.arcs.contains_key(&f) {
                    match ops::list_archive(&f, None) {
                        Ok(v) => {
                            self.arcs.insert(f.clone(), (v, None));
                        }
                        Err(e) if e == "password" => {
                            self.ask_arc_password(&f, &inner);
                        }
                        Err(e) => self.load_err = Some(e),
                    }
                }
                if let Some((list, _)) = self.arcs.get(&f) {
                    items = self.arc_items(&f, list, &inner);
                }
            }
            Loc::Search(..) => {
                // Results arrive from the search thread; keep what is there.
                items = std::mem::take(&mut self.items);
            }
        }
        self.items = items;
        self.sort_items();
        self.sel.resize(self.items.len(), false);
        self.lay();
    }

    fn arc_items(&self, f: &Path, list: &[ops::ArcEntry], inner: &str) -> Vec<Item> {
        let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut out: Vec<Item> = Vec::new();
        for a in list {
            let Some(rest) = a.path.strip_prefix(&prefix) else { continue };
            if rest.is_empty() {
                continue;
            }
            let (name, deeper) = match rest.split_once('/') {
                Some((n, _)) => (n.to_string(), true),
                None => (rest.to_string(), false),
            };
            let dir = deeper || a.dir;
            if let Some(&i) = seen.get(&name) {
                if !deeper {
                    // The folder's own entry: its date.
                    out[i].e.mtime = a.mtime;
                }
                continue;
            }
            let ext = if dir { String::new() } else { model::ext_of(&name) };
            let e = Entry {
                name: name.clone(),
                path: f.join(format!("{prefix}{name}")),
                dir,
                link: false,
                broken: false,
                size: if dir { 0 } else { a.size },
                mtime: if deeper { 0 } else { a.mtime },
                ctime: 0,
                mode: if dir { 0o40755 } else { 0o100644 },
                uid: sys::uid(),
                hidden: false,
                ext,
            };
            let mut it = self.make_item(e);
            if dir {
                it.icon = "folder".into();
                it.kind = "Папка с файлами".into();
            }
            it.packed = if deeper { 0 } else { a.packed };
            it.inner = format!("{prefix}{name}");
            seen.insert(name, out.len());
            out.push(it);
        }
        out
    }

    /// Which set of columns a place uses (folders, trash, archives, search results).
    pub fn col_kind(&self) -> &'static str {
        match self.loc() {
            Loc::Trash => "trash",
            Loc::Arc(..) => "arc",
            Loc::Search(..) => "search",
            _ => "dir",
        }
    }

    /// Every column this kind of place can show, in order, and whether it is shown.
    pub fn col_set(&self) -> Vec<(Col, bool)> {
        if let Some(v) = self.col_sets.get(self.col_kind()) {
            return v.clone();
        }
        default_cols(self.col_kind())
    }

    /// The shown columns, in order; «Имя» always first.
    pub fn columns(&self) -> Vec<Col> {
        self.col_set().into_iter().filter(|c| c.1).map(|c| c.0).collect()
    }

    /// Shows or hides a column (not «Имя»).
    pub fn toggle_col(&mut self, c: Col) {
        if c == Col::Name {
            return;
        }
        let mut v = self.col_set();
        if let Some(e) = v.iter_mut().find(|e| e.0 == c) {
            e.1 = !e.1;
        }
        self.col_sets.insert(self.col_kind(), v);
        self.lay();
    }

    /// Moves shown column `from` to position `to` (both among the shown ones; 0 is «Имя»).
    pub fn move_col(&mut self, from: usize, to: usize) {
        let shown = self.columns();
        if from == 0 || to == 0 || from >= shown.len() || to >= shown.len() || from == to {
            return;
        }
        let mut order = shown.clone();
        let c = order.remove(from);
        order.insert(to, c);
        // Hidden columns keep their place after the shown ones.
        let mut v: Vec<(Col, bool)> = order.into_iter().map(|c| (c, true)).collect();
        v.extend(self.col_set().into_iter().filter(|e| !e.1));
        self.col_sets.insert(self.col_kind(), v);
        self.lay();
    }

    pub fn reset_cols(&mut self) {
        self.col_sets.remove(self.col_kind());
        for c in [Col::Name, Col::Date, Col::Type, Col::Size, Col::Orig, Col::Deleted, Col::Packed, Col::Folder, Col::Perms, Col::Owner] {
            self.widths.insert(c, c.width());
        }
        self.lay();
    }

    /// Right click on the header: the columns to show, as Windows has it.
    pub fn head_menu(&self, at: Option<usize>) -> Vec<MItem> {
        let mut v = Vec::new();
        if let Some(i) = at {
            v.push(MItem::new("Подогнать ширину столбца", Act::FitCol(i)));
        }
        v.push(MItem::new("Подогнать ширину всех столбцов", Act::FitCols));
        v.push(MItem::sep());
        for (c, on) in self.col_set() {
            v.push(MItem::new(c.title(), Act::ToggleCol(c)).check(on).on(c != Col::Name));
        }
        v.push(MItem::sep());
        v.push(MItem::new("Столбцы по умолчанию", Act::ResetCols).on(self.col_sets.contains_key(self.col_kind())));
        v
    }

    fn cmp(&self, a: &Item, b: &Item, c: Col) -> std::cmp::Ordering {
        match c {
            Col::Name => model::natural(&a.disp, &b.disp),
            Col::Date => a.e.mtime.cmp(&b.e.mtime),
            Col::Type => model::natural(&a.kind, &b.kind),
            Col::Size => a.e.size.cmp(&b.e.size),
            Col::Orig => a.orig.cmp(&b.orig),
            Col::Deleted => a.deleted.cmp(&b.deleted),
            Col::Packed => a.packed.cmp(&b.packed),
            Col::Folder => model::natural(&a.folder, &b.folder),
            Col::Perms => (a.e.mode & 0o7777).cmp(&(b.e.mode & 0o7777)),
            Col::Owner => model::natural(&user_name(a.e.uid), &user_name(b.e.uid)),
        }
    }

    fn sort_items(&mut self) {
        let v = self.view;
        if matches!(self.loc(), Loc::Pc) {
            self.items.sort_by(|a, b| a.group.0.cmp(&b.group.0));
            self.items.iter_mut().for_each(|_| {});
        } else {
            for it in self.items.iter_mut() {
                it.group = match v.group {
                    Some(Col::Date) => {
                        let (r, s) = date_bucket(it.e.mtime);
                        (r, s.to_string())
                    }
                    Some(Col::Type) => (0, it.kind.clone()),
                    _ => (0, String::new()),
                };
            }
            let mut items = std::mem::take(&mut self.items);
            let fresh = self.fresh.clone();
            items.sort_by(|a, b| {
                let fa = fresh.contains(&a.e.name);
                let fb = fresh.contains(&b.e.name);
                if fa != fb {
                    return fa.cmp(&fb);
                }
                let g = a.group.0.cmp(&b.group.0).then_with(|| if v.group == Some(Col::Type) { model::natural(&a.group.1, &b.group.1) } else { std::cmp::Ordering::Equal });
                if g != std::cmp::Ordering::Equal {
                    return g;
                }
                let folders = b.e.dir.cmp(&a.e.dir);
                let o = if folders != std::cmp::Ordering::Equal { folders } else { self.cmp(a, b, v.sort) };
                let o = if v.desc { o.reverse() } else { o };
                o.then_with(|| model::natural(&a.disp, &b.disp))
            });
            self.items = items;
        }
    }

    /// Re-sorts keeping the selection.
    fn resort(&mut self) {
        let sel: Vec<String> = self.items.iter().zip(&self.sel).filter(|(_, s)| **s).map(|(i, _)| i.e.path.to_string_lossy().to_string()).collect();
        let focus = self.focus.map(|f| self.items[f].e.path.clone());
        self.sort_items();
        self.sel = self.items.iter().map(|i| sel.contains(&i.e.path.to_string_lossy().to_string())).collect();
        self.focus = focus.and_then(|p| self.items.iter().position(|i| i.e.path == p));
        self.lay();
        if let Some(f) = self.focus {
            self.fdrop.go(self.rects[f]);
        }
        self.views.insert(self.loc().key(), self.view);
        self.dirty = true;
    }

    // -------------------------------------------------------------- layout

    /// The scrolled area (below the column header in details).
    pub fn view_rect(&self) -> Rect {
        if self.desk {
            return Rect::new(0, 0, self.w, self.h);
        }
        let top = TOP + if self.view.mode == Mode::Details { HEAD_H } else { 0 };
        Rect::from_edges(NAV_W + 1, top, self.w, self.h - STATUS_H)
    }

    pub fn icon_cell(&self, s: i32) -> (i32, i32) {
        let w = (s + 40).max(88);
        (w, s + 16 + 40)
    }

    /// Sum of the column widths.
    pub fn cols_w(&self) -> i32 {
        self.columns().iter().map(|c| self.widths[c]).sum()
    }

    pub fn lay(&mut self) {
        if self.desk {
            self.lay_desktop();
            return;
        }
        let vr = self.view_rect();
        let cw = vr.w - 14;
        let mut rects = Vec::with_capacity(self.items.len());
        let mut heads = Vec::new();
        let mut y = 6;
        let grouped = self.view.group.is_some() || matches!(self.loc(), Loc::Pc);
        let mut last_group: Option<(u32, String)> = None;
        let mut x = 8;
        let mut row_h = 0;
        for it in &self.items {
            if grouped && last_group.as_ref() != Some(&it.group) {
                if x > 8 {
                    y += row_h;
                }
                x = 8;
                if last_group.is_some() {
                    y += 8;
                }
                heads.push((Rect::new(8, y, cw - 8, GROUP_H - 4), it.group.1.clone()));
                y += GROUP_H;
                last_group = Some(it.group.clone());
            }
            match self.view.mode {
                Mode::Details => {
                    rects.push(Rect::new(6, y, (self.cols_w() + 8).max(cw - 6), ROW_H));
                    y += ROW_H;
                }
                Mode::Icons(s) => {
                    let (w, h) = self.icon_cell(s);
                    if x + w > cw && x > 8 {
                        x = 8;
                        y += h + 4;
                    }
                    rects.push(Rect::new(x, y, w, h));
                    x += w + 4;
                    row_h = h + 4;
                }
                Mode::Tiles => {
                    let (w, h) = (268, if it.drive.is_some() { 78 } else { 58 });
                    if x + w > cw && x > 8 {
                        x = 8;
                        y += row_h;
                    }
                    rects.push(Rect::new(x, y, w, h));
                    x += w + 8;
                    row_h = h + 8;
                }
            }
        }
        if x > 8 {
            y += row_h;
        }
        self.rects = rects;
        self.heads = heads;
        self.content_h = y + 12;
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.view_rect().h).max(0) as f32
    }

    fn clamp_scroll(&mut self) {
        let m = self.max_scroll();
        let t = self.scroll.target.clamp(0.0, m);
        self.scroll.to(t);
        if self.scroll.value > m {
            self.scroll.snap(m);
        }
    }

    /// Scrolls so item `i` is visible.
    pub fn reveal(&mut self, i: usize) {
        let Some(r) = self.rects.get(i) else { return };
        let vh = self.view_rect().h as f32;
        let (top, bot) = (r.y as f32 - 4.0, r.y1() as f32 + 4.0);
        let s = self.scroll.target;
        if top < s {
            self.scroll.to(top.max(0.0));
        } else if bot > s + vh {
            self.scroll.to((bot - vh).min(self.max_scroll()));
        }
    }

    /// Screen rect of item `i` (with the current scroll value).
    pub fn screen(&self, r: Rect) -> Rect {
        let vr = self.view_rect();
        Rect::new(vr.x + r.x, vr.y + r.y - self.scroll.value.round() as i32, r.w, r.h)
    }

    pub fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        let vr = self.view_rect();
        if !vr.contains(x, y) {
            return None;
        }
        let cx = x - vr.x as f64;
        let cy = y - vr.y as f64 + self.scroll.value as f64;
        let i = self.rects.iter().position(|r| r.contains(cx, cy))?;
        // Details: only the name part counts as the item for drags (as in Windows the whole row does).
        Some(i)
    }

    // -------------------------------------------------------------- selection

    pub fn selected(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|i| self.sel[*i]).collect()
    }

    pub fn sel_paths(&self) -> Vec<PathBuf> {
        self.selected().into_iter().map(|i| self.items[i].e.path.clone()).collect()
    }

    pub fn select_only(&mut self, i: usize) {
        self.sel.iter_mut().for_each(|s| *s = false);
        if i < self.sel.len() {
            self.sel[i] = true;
            self.set_focus(i);
            self.anchor = i;
        }
        self.dirty = true;
    }

    pub fn set_focus(&mut self, i: usize) {
        if i >= self.rects.len() {
            return;
        }
        self.focus = Some(i);
        self.fdrop.go(self.rects[i]);
        self.reveal(i);
        self.dirty = true;
    }

    fn select_range(&mut self, a: usize, b: usize, add: bool) {
        if !add {
            self.sel.iter_mut().for_each(|s| *s = false);
        }
        for i in a.min(b)..=a.max(b) {
            if i < self.sel.len() {
                self.sel[i] = true;
            }
        }
    }

    /// The neighbour of `i` in a direction (geometric: works for rows, grids and groups).
    fn neighbor(&self, i: usize, dx: i32, dy: i32) -> usize {
        let n = self.items.len();
        if n == 0 {
            return 0;
        }
        if self.view.mode == Mode::Details || dx != 0 {
            let d = if self.view.mode == Mode::Details { dy } else { dx };
            return (i as i64 + d as i64).clamp(0, n as i64 - 1) as usize;
        }
        let r = self.rects[i];
        let cx = r.x + r.w / 2;
        let mut best: Option<(i32, i32, usize)> = None;
        for (j, q) in self.rects.iter().enumerate() {
            let ok = if dy > 0 { q.y > r.y + 2 } else { q.y1() < r.y1() - 2 };
            if !ok {
                continue;
            }
            let dyv = (q.y - r.y).abs();
            let dxv = (q.x + q.w / 2 - cx).abs();
            if best.map_or(true, |b| dyv < b.0 || (dyv == b.0 && dxv < b.1)) {
                best = Some((dyv, dxv, j));
            }
        }
        best.map_or(i, |b| b.2)
    }

    /// Arrow keys & co. on the item view.
    fn move_key(&mut self, to: usize) {
        let m = self.mods;
        if m.shift {
            self.select_range(self.anchor, to, m.ctrl);
            self.set_focus(to);
        } else if m.ctrl {
            self.set_focus(to);
        } else {
            self.select_only(to);
        }
    }

    pub fn ops_busy(&self) -> bool {
        !self.ops.is_empty()
    }

    pub fn toast(&mut self, s: &str) {
        self.toast = Some((s.to_string(), sys::now_ns()));
        self.dirty = true;
    }

    // -------------------------------------------------------------- opening

    pub fn open_items(&mut self, idx: Vec<usize>) {
        if idx.is_empty() {
            return;
        }
        // Folders: the first one here; several files each with their program.
        if let Some(&i) = idx.iter().find(|i| self.items[**i].e.dir) {
            if idx.len() == 1 || idx.iter().all(|i| self.items[*i].e.dir) {
                let it = self.items[i].clone();
                let l = match self.loc() {
                    Loc::Arc(f, _) => Loc::Arc(f.clone(), it.inner.clone()),
                    Loc::Trash => {
                        self.props_of(i);
                        return;
                    }
                    _ => Loc::Dir(it.e.path.clone()),
                };
                self.navigate(l);
                return;
            }
        }
        for i in idx {
            let it = self.items[i].clone();
            if it.e.dir {
                continue;
            }
            self.open_file(&it);
        }
    }

    fn open_file(&mut self, it: &Item) {
        if let Loc::Arc(f, _) = self.loc().clone() {
            // From an archive: extract that one file to a temp folder, then open it.
            let tmp = self.temp_dir();
            let pw = self.arcs.get(&f).and_then(|a| a.1.clone());
            let id = self.start_op("Извлечение", format!("Извлечение «{}»", it.disp), None, ops::Job::Zip { args: vec!["x".into(), format!("-o{}", tmp.display()), "--".into(), f.to_string_lossy().to_string(), it.inner.clone()], cwd: tmp.clone(), made: None, password: pw });
            self.then.insert(id, Then::Open(tmp.join(&it.inner)));
            return;
        }
        if matches!(self.loc(), Loc::Trash) {
            self.dialog = Some(Dialog::new("Корзина", vec!["Чтобы открыть этот файл, сначала восстановите его из корзины.".into()], &["ОК"], On::Info));
            return;
        }
        let p = it.e.path.clone();
        if ops::is_archive(&it.e.ext) && self.have7z && !matches!(it.e.ext.as_str(), "deb" | "rpm" | "apk" | "jar") {
            self.navigate(Loc::Arc(p, String::new()));
            return;
        }
        if it.e.ext == "desktop" {
            if let Some(h) = std::fs::read_to_string(&p).ok().and_then(|t| {
                let exec = t.lines().find_map(|l| l.strip_prefix("Exec="))?.to_string();
                Some(open::Handler { id: String::new(), name: String::new(), icon: String::new(), exec, terminal: t.contains("Terminal=true"), mimes: vec![] })
            }) {
                open::spawn(&open::command(&h, &[]), None);
            }
            return;
        }
        if it.e.ext == "appimage" && !it.e.exec() {
            let mut d = Dialog::new("Запуск приложения", vec![format!("«{}» не помечен как исполняемый.", it.disp), "Разрешить запуск этого файла как программы?".into()], &["Разрешить и запустить", "Отмена"], On::MakeExec(p));
            d.icon = it.icon.clone();
            self.dialog = Some(d);
            return;
        }
        if it.e.ext == "exe" || it.e.ext == "msi" {
            if open::have("wine") {
                open::spawn(&format!("wine {}", open::quote(&p.to_string_lossy())), p.parent());
            } else {
                self.dialog = Some(Dialog::new("Программа Windows", vec!["Чтобы запускать программы Windows, установите Wine (пакет wine).".into()], &["ОК"], On::Info));
            }
            return;
        }
        if it.e.exec() {
            let elf = std::fs::File::open(&p).ok().and_then(|mut f| {
                use std::io::Read;
                let mut b = [0u8; 4];
                f.read_exact(&mut b).ok().map(|_| b == *b"\x7fELF")
            });
            if elf == Some(true) || it.e.ext == "appimage" {
                open::spawn(&open::quote(&p.to_string_lossy()), p.parent());
                return;
            }
            if it.mime.starts_with("text/") || it.mime == "application/x-shellscript" || it.e.ext.is_empty() {
                self.dialog = Some(Dialog::new("Исполняемый файл", vec![format!("«{}» — исполняемый текстовый файл.", it.disp), "Запустить его или открыть для просмотра?".into()], &["Запустить в терминале", "Запустить", "Открыть", "Отмена"], On::RunScript(p)));
                return;
            }
        }
        let mime = it.mime;
        let h = self.apps().default_for(mime).cloned();
        match h {
            Some(h) => open::spawn(&open::command(&h, &[p.clone()]), p.parent()),
            None => {
                let name = it.kind.clone();
                self.toast(&format!("Нет программы, которая открывает файлы типа «{name}»"));
            }
        }
    }

    fn temp_dir(&mut self) -> PathBuf {
        self.temp_n += 1;
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base).join(format!("resc-files-{}", std::process::id())).join(self.temp_n.to_string());
        let _ = std::fs::create_dir_all(&d);
        d
    }

    // -------------------------------------------------------------- operations

    pub fn start_op(&mut self, verb: &'static str, title: String, dest: Option<PathBuf>, job: ops::Job) -> u32 {
        let id = self.next_op;
        self.next_op += 1;
        self.ops.push(Op::start(id, verb, title, dest, job));
        self.ops_open = true;
        self.dirty = true;
        id
    }

    fn names_of(&self, items: &[PathBuf]) -> String {
        if items.len() == 1 {
            format!("«{}»", items[0].file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
        } else {
            format!("{} {}", items.len(), model::plural(items.len() as u64, ["элемента", "элементов", "элементов"]))
        }
    }

    pub fn copy_to(&mut self, items: Vec<PathBuf>, dest: PathBuf, mv: bool) {
        if items.is_empty() {
            return;
        }
        let from = items[0].parent().map(|p| self.disp_name(p)).unwrap_or_default();
        let title = format!("{} {} из «{}» в «{}»", if mv { "Перемещение" } else { "Копирование" }, self.names_of(&items), from, self.disp_name(&dest));
        let job = if mv { ops::Job::Move { items, dest: dest.clone() } } else { ops::Job::Copy { items, dest: dest.clone() } };
        self.start_op(if mv { "Перемещение" } else { "Копирование" }, title, Some(dest), job);
    }

    pub fn trash_sel(&mut self) {
        let items = self.sel_paths();
        if items.is_empty() {
            return;
        }
        match self.loc() {
            Loc::Trash => {
                let n = items.len();
                let text = if n == 1 { format!("Вы действительно хотите безвозвратно удалить «{}»?", self.items[self.selected()[0]].disp) } else { format!("Вы действительно хотите безвозвратно удалить эти элементы ({n})?") };
                self.dialog = Some(Dialog::new("Удаление", vec![text], &["Да", "Нет"], On::DeleteForever(items)));
            }
            Loc::Dir(_) | Loc::Search(..) => {
                self.focus_after_delete();
                let title = format!("Удаление {}", self.names_of(&items));
                self.start_op("Удаление", title, None, ops::Job::Trash(items));
            }
            _ => {}
        }
    }

    /// After a delete the focus goes to the next item (as in Windows).
    fn focus_after_delete(&mut self) {
        let sel = self.selected();
        let next = (sel.last().copied().unwrap_or(0) + 1..self.items.len()).find(|i| !self.sel[*i]).or_else(|| (0..sel.first().copied().unwrap_or(0)).rev().find(|i| !self.sel[*i]));
        if let Some(n) = next {
            let name = self.items[n].e.name.clone();
            self.tabs[self.cur].sel = vec![name.clone()];
            self.pending_focus = Some(name);
        }
    }

    pub fn delete_sel(&mut self) {
        let items = self.sel_paths();
        if items.is_empty() {
            return;
        }
        let n = items.len();
        let text = if n == 1 {
            let it = &self.items[self.selected()[0]];
            format!("Вы действительно хотите безвозвратно удалить {} «{}»?", if it.e.dir { "папку" } else { "файл" }, it.disp)
        } else {
            format!("Вы действительно хотите безвозвратно удалить эти элементы ({n})?")
        };
        self.dialog = Some(Dialog::new("Удаление", vec![text, "Отменить это действие будет нельзя.".into()], &["Да", "Нет"], On::DeleteForever(items)));
    }

    pub fn paste(&mut self) {
        let Some(dest) = self.loc().dir().map(Path::to_path_buf) else { return };
        let Some((items, cut)) = self.clip.clone() else { return };
        if let Some(src) = items.first().and_then(|p| p.parent()) {
            if let Some((af, ai)) = self.arc_of(src) {
                // Copied out of an archive: extract, then place.
                self.extract_items(&af, &ai, &items, dest);
                return;
            }
        }
        if cut {
            self.clip = None;
        }
        self.copy_to(items, dest, cut);
    }

    /// The archive (and inner folder) a virtual path belongs to, if any.
    fn arc_of(&self, p: &Path) -> Option<(PathBuf, String)> {
        for f in self.arcs.keys() {
            if let Ok(rest) = p.strip_prefix(f) {
                return Some((f.clone(), rest.to_string_lossy().to_string()));
            }
        }
        None
    }

    fn extract_items(&mut self, arc: &Path, inner_dir: &str, items: &[PathBuf], dest: PathBuf) {
        let tmp = self.temp_dir();
        let pw = self.arcs.get(arc).and_then(|a| a.1.clone());
        let mut args = vec!["x".to_string(), format!("-o{}", tmp.display()), "--".into(), arc.to_string_lossy().to_string()];
        let mut names = Vec::new();
        for p in items {
            if let Ok(rest) = p.strip_prefix(arc) {
                let r = rest.to_string_lossy().to_string();
                // A folder: everything under it too.
                args.push(r.clone());
                args.push(format!("{r}/*"));
                names.push(tmp.join(&r));
            }
        }
        let _ = inner_dir;
        let title = format!("Извлечение {} из «{}»", self.names_of(items), arc.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let id = self.start_op("Извлечение", title, Some(dest.clone()), ops::Job::Zip { args, cwd: tmp, made: None, password: pw });
        self.then.insert(id, Then::Place(names, dest));
    }

    pub fn do_undo(&mut self, redo: bool) {
        let u = if redo { self.redo.pop() } else { self.undo.pop() };
        let Some(u) = u else { return };
        self.undoing = Some(redo);
        match u {
            Undo::Moved(pairs) => {
                let t = format!("Отмена перемещения {}", self.names_of(&pairs.iter().map(|p| p.1.clone()).collect::<Vec<_>>()));
                self.start_op("Перемещение", t, None, ops::Job::MoveBack(pairs));
            }
            Undo::Made(items) => {
                let t = format!("Удаление {}", self.names_of(&items));
                self.start_op("Удаление", t, None, ops::Job::Trash(items));
            }
            Undo::Trashed(items) => {
                let t = format!("Восстановление {}", self.names_of(&items));
                self.start_op("Восстановление", t, None, ops::Job::Restore(items));
            }
            Undo::Renamed(from, to) => {
                if std::fs::rename(&to, &from).is_ok() {
                    let rec = Undo::Renamed(to, from);
                    if redo {
                        self.undo.push(rec);
                    } else {
                        self.redo.push(rec);
                    }
                    self.reload(false);
                }
                self.undoing = None;
            }
        }
    }

    // -------------------------------------------------------------- new things

    pub fn new_folder(&mut self) {
        let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
        let p = unique(&dir.join("Новая папка"));
        match std::fs::create_dir(&p) {
            Ok(()) => {
                self.made_here(&p);
            }
            Err(e) => self.error(&format!("Не удалось создать папку: {}", ops::explain(&e))),
        }
    }

    pub fn new_file(&mut self, template: Option<PathBuf>) {
        let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
        let name = match &template {
            Some(t) => t.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            None => "Новый текстовый документ.txt".into(),
        };
        let p = unique(&dir.join(name));
        let r = match &template {
            Some(t) => std::fs::copy(t, &p).map(|_| ()),
            None => std::fs::File::create(&p).map(|_| ()),
        };
        match r {
            Ok(()) => self.made_here(&p),
            Err(e) => self.error(&format!("Не удалось создать файл: {}", ops::explain(&e))),
        }
    }

    /// A new thing appears at the end, selected, its name being edited.
    fn made_here(&mut self, p: &Path) {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.fresh.push(name.clone());
        self.undo.push(Undo::Made(vec![p.to_path_buf()]));
        self.redo.clear();
        self.reload(false);
        if let Some(i) = self.items.iter().position(|i| i.e.name == name) {
            self.select_only(i);
            self.start_rename(i);
        }
    }

    pub fn error(&mut self, s: &str) {
        self.dialog = Some(Dialog::new("Проводник", vec![s.to_string()], &["ОК"], On::Info));
        self.dirty = true;
    }

    // -------------------------------------------------------------- rename

    pub fn start_rename(&mut self, i: usize) {
        if !matches!(self.loc(), Loc::Dir(_) | Loc::Search(..)) || i >= self.items.len() {
            return;
        }
        let it = &self.items[i];
        let mut tf = TextField::default();
        tf.set(&it.e.name);
        // The name without its extension is selected (as in Windows).
        tf.select(0, model::stem_len(&it.e.name, it.e.dir));
        self.rename = Some((i, tf));
        self.reveal(i);
        self.dirty = true;
    }

    /// Ends renaming; `next`: Tab went on to the next item.
    pub fn commit_rename(&mut self, next: Option<i32>) {
        let Some((i, tf)) = self.rename.take() else { return };
        self.dirty = true;
        let Some(it) = self.items.get(i).cloned() else { return };
        let name = tf.text().trim_end_matches([' ', '.']).trim_start().to_string();
        let go_next = |a: &mut App| {
            if let Some(d) = next {
                let j = (i as i32 + d).clamp(0, a.items.len() as i32 - 1) as usize;
                if j != i {
                    a.select_only(j);
                    a.start_rename(j);
                }
            }
        };
        if name == it.e.name || name.is_empty() {
            go_next(self);
            return;
        }
        if name.contains('/') || name == "." || name == ".." || name.contains('\0') {
            self.error("Имя файла не может содержать символ «/» и быть «.» или «..».");
            return;
        }
        let to = it.e.path.with_file_name(&name);
        if !it.e.dir && model::ext_of(&name) != it.e.ext && !it.e.ext.is_empty() {
            self.dialog = Some(Dialog::new("Переименование", vec!["Если изменить расширение имени файла, этот файл может оказаться недоступным.".into(), "Вы действительно хотите изменить его?".into()], &["Да", "Нет"], On::ExtChange(it.e.path.clone(), name)));
            return;
        }
        if std::fs::symlink_metadata(&to).is_ok() && name.to_lowercase() != it.e.name.to_lowercase() {
            let free = unique(&to);
            let fname = free.file_name().unwrap().to_string_lossy().to_string();
            self.dialog = Some(Dialog::new("Переименование", vec![format!("В этом расположении уже есть элемент с именем «{name}»."), format!("Переименовать «{}» в «{fname}»?", it.e.name)], &["Да", "Нет"], On::Taken(it.e.path.clone(), fname)));
            return;
        }
        self.rename_to(&it.e.path, &name);
        go_next(self);
    }

    fn rename_to(&mut self, from: &Path, name: &str) {
        let to = from.with_file_name(name);
        match std::fs::rename(from, &to) {
            Ok(()) => {
                self.undo.push(Undo::Renamed(from.to_path_buf(), to.clone()));
                self.redo.clear();
                let old = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(f) = self.fresh.iter_mut().find(|f| **f == old) {
                    *f = name.to_string();
                }
                self.reload(false);
                if let Some(i) = self.items.iter().position(|it| it.e.path == to) {
                    self.select_only(i);
                }
            }
            Err(e) => self.error(&format!("Не удалось переименовать: {}", ops::explain(&e))),
        }
    }

    // -------------------------------------------------------------- search

    pub fn start_search(&mut self) {
        let q = self.search.text().trim().to_string();
        if q.is_empty() {
            self.exit_search();
            return;
        }
        let base = match self.loc() {
            Loc::Dir(p) => p.clone(),
            Loc::Search(b, _) => b.clone(),
            Loc::Pc => model::home(),
            _ => return,
        };
        self.stop_search();
        let l = Loc::Search(base.clone(), q.clone());
        if !matches!(self.loc(), Loc::Search(..)) {
            self.save_tab();
            let t = &mut self.tabs[self.cur];
            let old = std::mem::replace(&mut t.loc, l.clone());
            t.back.push(old);
            t.fwd.clear();
        } else {
            self.tabs[self.cur].loc = l;
        }
        self.items.clear();
        self.sel.clear();
        self.focus = None;
        self.view = View::default();
        self.scroll.snap(0.0);
        self.lay();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.search_stop = stop.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.search_rx = Some(rx);
        self.searching = true;
        let hidden = self.show_hidden;
        std::thread::spawn(move || search_walk(&base, &q.to_lowercase(), hidden, &stop, &tx));
        self.title = format!("Результаты поиска: {}", self.search.text());
        self.dirty = true;
    }

    fn stop_search(&mut self) {
        self.search_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.search_rx = None;
        self.searching = false;
    }

    pub fn exit_search(&mut self) {
        self.search.clear();
        self.search_on = false;
        if matches!(self.loc(), Loc::Search(..)) {
            self.go_back();
        }
        self.dirty = true;
    }

    // -------------------------------------------------------------- 7-Zip

    pub fn zip_quick(&mut self, ext: &'static str) {
        let items = self.sel_paths();
        let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
        if items.is_empty() {
            return;
        }
        let base = if items.len() == 1 { items[0].file_name().unwrap().to_string_lossy().to_string() } else { self.disp_name(&dir) };
        let base = if items.len() == 1 && !items[0].is_dir() { base.chars().take(model::stem_len(&base, false)).collect() } else { base };
        let out = unique(&dir.join(format!("{base}.{ext}")));
        self.zip_add(items, out, ext, 5, None, None);
    }

    fn zip_add(&mut self, items: Vec<PathBuf>, out: PathBuf, fmt: &str, level: u32, pw: Option<String>, volume: Option<String>) {
        let Some(dir) = out.parent().map(Path::to_path_buf) else { return };
        let t = match fmt {
            "tar.gz" | "tar.xz" | "tar.zst" => None,
            "7z" => Some("7z"),
            "tar" => Some("tar"),
            _ => Some("zip"),
        };
        let mut args = vec!["a".to_string()];
        let names: Vec<String> = items.iter().map(|p| p.strip_prefix(&dir).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| p.to_string_lossy().to_string())).collect();
        let title = format!("Сжатие {} в «{}»", self.names_of(&items), out.file_name().unwrap().to_string_lossy());
        match t {
            Some(t) => {
                args.push(format!("-t{t}"));
                if t != "tar" {
                    args.push(format!("-mx={level}"));
                }
                if let Some(p) = &pw {
                    args.push(format!("-p{p}"));
                    if t == "7z" {
                        args.push("-mhe=on".into());
                    }
                }
                if let Some(v) = volume.filter(|v| !v.is_empty()) {
                    args.push(format!("-v{v}"));
                }
                args.push("--".into());
                args.push(out.to_string_lossy().to_string());
                args.extend(names);
                self.start_op("Сжатие", title, Some(dir.clone()), ops::Job::Zip { args, cwd: dir, made: Some(out), password: None });
            }
            None => {
                // .tar.xz and co: tar into the compressor (7-Zip makes xz/gz; zst through zstd).
                let comp = match fmt {
                    "tar.gz" => "gzip",
                    "tar.xz" => "xz",
                    _ => "zstd",
                };
                let q: Vec<String> = items.iter().map(|p| open::quote(&p.file_name().unwrap().to_string_lossy())).collect();
                let cmd = format!("tar -cf - -- {} | {comp} -{} > {}", q.join(" "), level.clamp(1, 9), open::quote(&out.to_string_lossy()));
                self.start_op("Сжатие", title, Some(dir.clone()), ops::Job::Zip { args: vec!["__sh".into(), cmd], cwd: dir, made: Some(out), password: None });
            }
        }
    }

    pub fn zip_add_dialog(&mut self) {
        let items = self.sel_paths();
        let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
        if items.is_empty() {
            return;
        }
        let base = if items.len() == 1 { items[0].file_name().unwrap().to_string_lossy().to_string() } else { self.disp_name(&dir) };
        let base: String = if items.len() == 1 && !items[0].is_dir() { base.chars().take(model::stem_len(&base, false)).collect() } else { base };
        let mut d = Dialog::new("Добавить к архиву", vec![format!("{} · 7-Zip", self.names_of(&items))], &["ОК", "Отмена"], On::Add7z(items, dir));
        d = d.field("Архив", &base, false);
        d = d.choice("Формат", &["7z", "zip", "tar", "tar.gz", "tar.xz", "tar.zst"], 0);
        d = d.choice("Сжатие", &["Без сжатия", "Скоростное", "Быстрое", "Нормальное", "Максимальное", "Ультра"], 3);
        d = d.field("Пароль", "", true);
        d = d.field("Разбить на тома", "", false);
        d.wide = true;
        if let Some(f) = d.fields.get_mut(1) {
            f.tf.clear();
        }
        if let Some(f) = d.fields.get_mut(2) {
            f.tf.clear();
        }
        self.dialog = Some(d);
    }

    /// Extract the selected archives: `to`: None here, Some(true) each into its own folder.
    pub fn zip_extract(&mut self, own_folder: bool) {
        let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
        for i in self.selected() {
            let it = self.items[i].clone();
            if !ops::is_archive(&it.e.ext) {
                continue;
            }
            let out = if own_folder { unique(&dir.join(stem_of(&it.e.name))) } else { dir.clone() };
            self.extract_to(&it.e.path, out, None);
        }
    }

    fn extract_to(&mut self, arc: &Path, out: PathBuf, pw: Option<String>) {
        let mut args = vec!["x".to_string(), format!("-o{}", out.display())];
        // Into a folder of its own: nothing to clash with. Here: keep both (7-Zip renames).
        args.push("-aou".into());
        args.push("--".into());
        args.push(arc.to_string_lossy().to_string());
        let title = format!("Извлечение «{}» в «{}»", arc.file_name().unwrap().to_string_lossy(), self.disp_name(&out));
        let made = (out.parent().map(Path::to_path_buf) == self.loc().dir().map(Path::to_path_buf)).then(|| out.clone());
        self.start_op("Извлечение", title, Some(out), ops::Job::Zip { args, cwd: arc.parent().unwrap_or(Path::new("/")).to_path_buf(), made, password: pw });
    }

    pub fn zip_extract_dialog(&mut self) {
        let Some(&i) = self.selected().first() else { return };
        let it = self.items[i].clone();
        let Some(dir) = it.e.path.parent() else { return };
        let mut d = Dialog::new("Извлечь", vec![format!("«{}» · 7-Zip", it.disp)], &["ОК", "Отмена"], On::Extract(it.e.path.clone()));
        d = d.field("Распаковать в", &dir.join(stem_of(&it.e.name)).to_string_lossy(), false);
        d = d.field("Пароль", "", true);
        if let Some(f) = d.fields.get_mut(1) {
            f.tf.clear();
        }
        d.wide = true;
        self.dialog = Some(d);
    }

    pub fn zip_test(&mut self) {
        for p in self.sel_paths() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let id = self.start_op("Проверка", format!("Проверка «{name}»"), None, ops::Job::Zip { args: vec!["t".into(), "--".into(), p.to_string_lossy().to_string()], cwd: p.parent().unwrap().to_path_buf(), made: None, password: None });
            self.then.insert(id, Then::Toast(format!("«{name}»: ошибок не найдено")));
        }
    }

    pub fn hash(&mut self, algo: &'static str) {
        let items = self.sel_paths();
        if items.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.hash_rx = Some(rx);
        self.toast(&format!("Вычисляется {algo}…"));
        std::thread::spawn(move || {
            let out = std::process::Command::new("7z").arg("h").arg(format!("-scrc{algo}")).arg("-ba").arg("--").args(&items).stdin(std::process::Stdio::null()).output();
            let r = match out {
                Ok(o) => {
                    let t = String::from_utf8_lossy(&o.stdout).to_string();
                    let lines: Vec<String> = t.lines().filter(|l| l.split_whitespace().next().map_or(false, |h| h.len() >= 8 && h.chars().all(|c| c.is_ascii_hexdigit()))).map(|l| {
                        let mut it = l.split_whitespace();
                        let h = it.next().unwrap_or("").to_string();
                        let _size = it.next();
                        let name: Vec<&str> = it.collect();
                        format!("{}: {h}", name.join(" "))
                    }).collect();
                    if lines.is_empty() { Err("7-Zip не вернул контрольных сумм".to_string()) } else { Ok(lines) }
                }
                Err(e) => Err(if e.kind() == std::io::ErrorKind::NotFound { "7-Zip не установлен (пакет 7zip)".into() } else { e.to_string() }),
            };
            let _ = tx.send(r);
        });
        self.hash_algo = algo;
    }

    fn ask_arc_password(&mut self, f: &Path, inner: &str) {
        let mut d = Dialog::new("Введите пароль", vec![format!("Архив «{}» зашифрован.", f.file_name().unwrap().to_string_lossy())], &["ОК", "Отмена"], On::ArcPassword(f.to_path_buf(), inner.to_string()));
        d = d.field("Пароль", "", true);
        if let Some(fl) = d.fields.get_mut(0) {
            fl.tf.clear();
        }
        self.dialog = Some(d);
    }

    // -------------------------------------------------------------- properties

    pub fn props_of(&mut self, i: usize) {
        let it = self.items[i].clone();
        let mut lines = vec![format!("Тип: {}", it.kind)];
        let loc = match &it.orig {
            Some(o) => format!("Исходное расположение: {}", o.display()),
            None => format!("Расположение: {}", it.e.path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
        };
        lines.push(loc);
        if it.e.dir {
            lines.push("Размер: вычисляется…".into());
            let (tx, rx) = std::sync::mpsc::channel();
            self.props_rx = Some(rx);
            let p = it.e.path.clone();
            std::thread::spawn(move || {
                let r = measure_all(&p);
                let _ = tx.send(r);
            });
        } else {
            lines.push(format!("Размер: {} ({} байт)", model::size_short(it.e.size), it.e.size));
        }
        if it.e.ctime > 0 {
            lines.push(format!("Создан: {}", model::date(it.e.ctime)));
        }
        if it.e.mtime > 0 {
            lines.push(format!("Изменён: {}", model::date(it.e.mtime)));
        }
        if it.deleted > 0 {
            lines.push(format!("Удалён: {}", model::date(it.deleted)));
        }
        if it.e.link {
            if let Ok(t) = std::fs::read_link(&it.e.path) {
                lines.push(format!("Ссылается на: {}", t.display()));
            }
        }
        let m = it.e.mode;
        let rw = |b: u32| match (b & 4 != 0, b & 2 != 0) {
            (true, true) => "чтение и запись",
            (true, false) => "только чтение",
            (false, true) => "только запись",
            _ => "нет доступа",
        };
        lines.push(format!("Владелец ({}): {}", user_name(it.e.uid), rw(m >> 6)));
        lines.push(format!("Остальные: {}", rw(m)));
        let mut d = Dialog::new(&format!("Свойства: {}", it.disp), lines, &["ОК"], On::Props(it.e.path.clone()));
        if !it.e.dir && matches!(self.loc(), Loc::Dir(_) | Loc::Search(..)) {
            d.check = Some(("Разрешить выполнение файла как программы".into(), m & 0o100 != 0));
        }
        d.icon = it.icon.clone();
        d.wide = true;
        self.dialog = Some(d);
    }

    // -------------------------------------------------------------- tabs

    pub fn new_tab(&mut self, l: Loc) {
        if self.desk {
            self.open_window(&l);
            return;
        }
        self.save_tab();
        self.tabs.insert(self.cur + 1, Tab { loc: l.clone(), back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 });
        self.cur += 1;
        self.enter(l, true);
    }

    pub fn switch_tab(&mut self, i: usize) {
        if i == self.cur || i >= self.tabs.len() {
            return;
        }
        self.save_tab();
        self.cur = i;
        let l = self.tabs[i].loc.clone();
        if let Loc::Search(b, _) = &l {
            // Search results are not kept: back to the folder, as «Назад» would.
            let d = Loc::Dir(b.clone());
            let t = &mut self.tabs[i];
            if t.back.last() == Some(&d) {
                t.back.pop();
            }
            t.fwd.clear();
            t.loc = d.clone();
            self.enter(d, false);
            return;
        }
        self.enter(l, false);
    }

    pub fn close_tab(&mut self, i: usize) {
        if self.desk {
            return;
        }
        if self.tabs.len() == 1 {
            self.quit = true;
            return;
        }
        self.save_tab();
        self.tabs.remove(i);
        if self.cur >= self.tabs.len() || i < self.cur {
            self.cur = self.cur.saturating_sub(1).min(self.tabs.len() - 1);
        }
        let l = self.tabs[self.cur].loc.clone();
        self.enter(l, false);
    }

    // -------------------------------------------------------------- per frame

    /// Background work: worker results, directory changes, finished operations.
    pub fn tick(&mut self) {
        if self.watch.drain() {
            self.reload_at = Some(sys::now_ns() + 150_000_000);
        }
        if let Some(t) = self.reload_at {
            if sys::now_ns() >= t {
                self.reload_at = None;
                self.reload(false);
            }
        }
        if let Some((i, at)) = self.rename_at {
            if self.last_click.0 == 0 || self.press.as_ref().map_or(false, |p| p.dragging) {
                // A double click (opened) or a drag came instead.
                self.rename_at = None;
            } else if sys::now_ns() >= at {
                self.rename_at = None;
                if self.selected() == vec![i] {
                    self.start_rename(i);
                }
            }
        }
        if let Some(at) = self.search_at {
            if sys::now_ns() >= at {
                self.search_at = None;
                self.start_search();
            }
        }
        if let Some(name) = self.pending_focus.clone() {
            if let Some(i) = self.items.iter().position(|it| it.e.name == name) {
                if !self.sel.iter().any(|s| *s) {
                    self.select_only(i);
                    self.pending_focus = None;
                }
            }
        }
        // Search results.
        let mut got = Vec::new();
        let mut ended = false;
        if let Some(rx) = &self.search_rx {
            loop {
                match rx.try_recv() {
                    Ok(v) => got.extend(v),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        ended = true;
                        break;
                    }
                }
            }
        }
        if !got.is_empty() {
            let base = match self.loc() {
                Loc::Search(b, _) => b.clone(),
                _ => PathBuf::new(),
            };
            for e in got {
                let mut it = self.make_item(e);
                it.folder = it.e.path.parent().map(|p| p.strip_prefix(&base).map(|r| r.display().to_string()).unwrap_or_default()).unwrap_or_default();
                if it.folder.is_empty() {
                    it.folder = self.disp_name(&base);
                }
                self.items.push(it);
            }
            self.sel.resize(self.items.len(), false);
            self.resort();
        }
        if ended {
            self.search_rx = None;
            self.searching = false;
            self.dirty = true;
        }
        if let Some(rx) = &self.props_rx {
            if let Ok((b, n)) = rx.try_recv() {
                self.props_rx = None;
                if let Some(d) = self.dialog.as_mut() {
                    if let Some(l) = d.lines.iter_mut().find(|l| l.starts_with("Размер:")) {
                        *l = format!("Размер: {} ({} байт) · {} {}", model::size_short(b), b, n, model::plural(n, ["элемент", "элемента", "элементов"]));
                    }
                }
                self.dirty = true;
            }
        }
        if let Some(rx) = &self.hash_rx {
            if let Ok(r) = rx.try_recv() {
                self.hash_rx = None;
                self.toast = None;
                match r {
                    Ok(lines) => {
                        let mut d = Dialog::new(&format!("Контрольная сумма {}", self.hash_algo), lines, &["ОК"], On::Info);
                        d.wide = true;
                        self.dialog = Some(d);
                    }
                    Err(e) => self.error(&e),
                }
                self.dirty = true;
            }
        }
        self.ops_tick();
        // A dialog (a name clash, an error) closes any open menu, as a modal window does.
        if self.dialog.is_some() && self.menu.is_some() {
            self.menu = None;
            self.drag_to = None;
        }
        if let Some((_, t)) = &self.toast {
            if sys::now_ns() - t > 3_500_000_000 {
                self.toast = None;
                self.dirty = true;
            }
        }
        // Spring-loaded folders: hold a drag over a folder and it opens.
        let dragging = self.press.as_ref().map_or(false, |p| p.dragging) || self.incoming.is_some();
        if let Some((hit, at)) = self.spring_at {
            if dragging && !self.desk && sys::now_ns() - at > 1_100_000_000 {
                self.spring_at = None;
                if let Hit::Item(i) = hit {
                    if self.items.get(i).map_or(false, |it| it.e.dir && !self.sel[i]) {
                        let l = Loc::Dir(self.items[i].e.path.clone());
                        self.navigate(l);
                    }
                } else if let Hit::Nav(i) = hit {
                    let l = self.nav[i].loc.clone();
                    self.navigate(l);
                }
            }
        }
    }

    fn ops_tick(&mut self) {
        let mut done = Vec::new();
        for (k, op) in self.ops.iter_mut().enumerate() {
            if op.sample() {
                self.dirty = true;
            }
            let p = op.prog.lock().unwrap();
            if p.ask.is_some() && !matches!(self.dialog.as_ref().map(|d| &d.on), Some(On::Conflict(_))) && self.dialog.is_none() {
                drop(p);
                let c = op.prog.lock().unwrap().ask.clone().unwrap();
                self.dialog = Some(conflict_dialog(op.id, &c));
                self.dirty = true;
                continue;
            }
            if p.finished {
                done.push(k);
            }
        }
        for k in done.into_iter().rev() {
            let op = self.ops.remove(k);
            let mut p = op.prog.lock().unwrap();
            self.dirty = true;
            if p.need_password {
                drop(p);
                let mut d = Dialog::new("Введите пароль", vec![format!("{} — архив зашифрован.", op.title)], &["ОК", "Отмена"], On::ZipPassword(op.id));
                d = d.field("Пароль", "", true);
                if let Some(f) = d.fields.get_mut(0) {
                    f.tf.clear();
                }
                self.dialog = Some(d);
                self.retry.insert(op.id, op);
                continue;
            }
            if let Some(u) = p.undo.take() {
                let empty = match &u {
                    Undo::Moved(v) => v.is_empty(),
                    Undo::Made(v) | Undo::Trashed(v) => v.is_empty(),
                    Undo::Renamed(..) => false,
                };
                if !empty {
                    match self.undoing.take() {
                        Some(false) => self.redo.push(u),
                        Some(true) => self.undo.push(u),
                        None => {
                            self.undo.push(u);
                            self.redo.clear();
                        }
                    }
                }
            }
            self.undoing = None;
            // A parent of where we are may have moved (no inotify event for that).
            if !loc_exists(self.loc()) {
                drop(p);
                self.reload(false);
                p = op.prog.lock().unwrap();
            }
            let no_trash = std::mem::take(&mut p.no_trash);
            let errors = std::mem::take(&mut p.errors);
            let made = std::mem::take(&mut p.made);
            let cancelled = p.cancelled;
            drop(p);
            if !no_trash.is_empty() {
                let mut lines = vec![format!("Эти элементы нельзя поместить в корзину ({}):", no_trash.len())];
                lines.extend(no_trash.iter().take(6).map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string()));
                lines.push("Удалить их безвозвратно?".into());
                self.dialog = Some(Dialog::new("Удаление", lines, &["Да", "Нет"], On::DeleteForever(no_trash)));
            } else if !errors.is_empty() {
                let mut lines = vec![format!("{}: не всё получилось.", op.title)];
                lines.extend(errors.iter().take(8).cloned());
                if errors.len() > 8 {
                    lines.push(format!("…и ещё {}", errors.len() - 8));
                }
                let mut d = Dialog::new("Ошибка", lines, &["ОК"], On::Info);
                d.wide = true;
                self.dialog = Some(d);
            }
            // Select what arrived here.
            if !made.is_empty() && op.dest.as_deref() == self.loc().dir() && errors.is_empty() && !cancelled {
                self.reload(false);
                let names: Vec<String> = made.iter().filter_map(|m| m.file_name()).map(|n| n.to_string_lossy().to_string()).collect();
                self.sel = self.items.iter().map(|i| names.contains(&i.e.name)).collect();
                if let Some(f) = self.sel.iter().position(|s| *s) {
                    self.anchor = f;
                    self.set_focus(f);
                }
            }
            if matches!(self.loc(), Loc::Trash) || op.verb == "Удаление" || op.verb == "Восстановление" {
                self.build_nav();
                if matches!(self.loc(), Loc::Trash) {
                    self.reload(false);
                }
            }
            match self.then.remove(&op.id) {
                Some(Then::Open(p)) if errors.is_empty() && !cancelled => {
                    let e = model::stat(&p);
                    if let Some(e) = e {
                        let it = self.make_item(e);
                        // Not through open_file: that would extract again from the archive.
                        let h = self.apps().default_for(it.mime).cloned();
                        match h {
                            Some(h) => open::spawn(&open::command(&h, &[p.clone()]), p.parent()),
                            None => self.toast(&format!("Нет программы, которая открывает файлы типа «{}»", it.kind)),
                        }
                    }
                }
                Some(Then::Place(items, dest)) if errors.is_empty() && !cancelled => {
                    let items: Vec<PathBuf> = items.into_iter().filter(|p| p.exists()).collect();
                    self.copy_to(items, dest, true);
                }
                Some(Then::Toast(s)) if errors.is_empty() && !cancelled => self.toast(&s),
                _ => {}
            }
        }
    }

    pub fn animating(&self) -> bool {
        self.scroll.moving()
            || self.nav_scroll.moving()
            || [&self.fdrop, &self.hdrop, &self.ndrop, &self.tdrop].iter().any(|d| d.l.moving() || d.t.moving() || d.r.moving() || d.b.moving() || d.a.moving())
            || self.menu.as_ref().map_or(false, |m| m.t.moving() || m.sub.as_ref().map_or(false, |s| s.1.t.moving()))
            || self.dialog.as_ref().map_or(false, |d| d.t.moving())
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let mut m = self.scroll.step(dt) | self.nav_scroll.step(dt);
        for d in [&mut self.fdrop, &mut self.hdrop, &mut self.ndrop, &mut self.tdrop] {
            m |= d.step(dt);
        }
        if let Some(menu) = self.menu.as_mut() {
            m |= menu.t.step(dt);
            if let Some((_, s)) = menu.sub.as_mut() {
                m |= s.t.step(dt);
            }
        }
        if let Some(d) = self.dialog.as_mut() {
            m |= d.t.step(dt);
        }
        m
    }
}

/// Whether a place can still be shown.
fn loc_exists(l: &Loc) -> bool {
    match l {
        Loc::Dir(p) | Loc::Search(p, _) => p.is_dir(),
        Loc::Arc(f, _) => f.is_file(),
        Loc::Pc | Loc::Trash => true,
    }
}

fn stem_of(name: &str) -> String {
    name.chars().take(model::stem_len(name, false)).collect()
}

/// A free name like Windows': «Новая папка», «Новая папка (2)», …
pub fn unique(p: &Path) -> PathBuf {
    if std::fs::symlink_metadata(p).is_err() {
        return p.to_path_buf();
    }
    ops::free_name(p, false)
}

pub fn user_name(uid: u32) -> String {
    thread_local! {
        static USERS: std::cell::RefCell<HashMap<u32, String>> = std::cell::RefCell::new(HashMap::new());
    }
    if let Some(n) = USERS.with(|u| u.borrow().get(&uid).cloned()) {
        return n;
    }
    let t = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let n = t.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2] == uid.to_string()).then(|| f[0].to_string())
    }).unwrap_or_else(|| uid.to_string());
    USERS.with(|u| u.borrow_mut().insert(uid, n.clone()));
    n
}

/// rwxr-xr-x
pub fn perm_str(mode: u32) -> String {
    let mut s = String::new();
    for sh in [6, 3, 0] {
        let b = (mode >> sh) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// The columns a kind of place starts with (shown ones first).
fn default_cols(kind: &str) -> Vec<(Col, bool)> {
    let (on, off): (&[Col], &[Col]) = match kind {
        "trash" => (&[Col::Name, Col::Orig, Col::Deleted, Col::Size, Col::Type], &[Col::Date]),
        "arc" => (&[Col::Name, Col::Size, Col::Packed, Col::Date, Col::Type], &[]),
        "search" => (&[Col::Name, Col::Date, Col::Type, Col::Size, Col::Folder], &[Col::Perms, Col::Owner]),
        _ => (&[Col::Name, Col::Date, Col::Type, Col::Size], &[Col::Perms, Col::Owner]),
    };
    on.iter().map(|c| (*c, true)).chain(off.iter().map(|c| (*c, false))).collect()
}

fn measure_all(p: &Path) -> (u64, u64) {
    let mut t = (0u64, 0u64);
    let Ok(rd) = std::fs::read_dir(p) else { return t };
    for e in rd.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if m.is_dir() {
            let (b, n) = measure_all(&e.path());
            t.0 += b;
            t.1 += n + 1;
        } else {
            t.0 += m.len();
            t.1 += 1;
        }
    }
    t
}

fn search_walk(base: &Path, q: &str, hidden: bool, stop: &std::sync::atomic::AtomicBool, tx: &std::sync::mpsc::Sender<Vec<Entry>>) {
    let mut stack = vec![base.to_path_buf()];
    let mut batch = Vec::new();
    let mut last = sys::now_ns();
    let mut found = 0;
    while let Some(d) = stack.pop() {
        if stop.load(std::sync::atomic::Ordering::Relaxed) || found > 5000 {
            return;
        }
        let Ok(list) = model::list(&d) else { continue };
        for e in list {
            if e.hidden && !hidden {
                continue;
            }
            if e.dir && !e.link {
                stack.push(e.path.clone());
            }
            if e.name.to_lowercase().contains(q) {
                batch.push(e);
                found += 1;
            }
        }
        if !batch.is_empty() && sys::now_ns() - last > 120_000_000 {
            if tx.send(std::mem::take(&mut batch)).is_err() {
                return;
            }
            last = sys::now_ns();
        }
    }
    let _ = tx.send(batch);
}

fn conflict_dialog(id: u32, c: &ops::Conflict) -> Dialog {
    let name = c.dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let info = |p: &Path| {
        model::stat(p).map(|e| if e.dir { format!("папка, изменена {}", model::date(e.mtime)) } else { format!("{}, изменён {}", model::size_short(e.size), model::date(e.mtime)) }).unwrap_or_default()
    };
    let lines = vec![
        format!("В папке назначения уже есть {} с именем «{name}».", if c.dir { "папка" } else { "файл" }),
        format!("Новый: {}", info(&c.src)),
        format!("Имеющийся: {}", info(&c.dst)),
    ];
    let mut d = Dialog::new("Замена или пропуск файлов", lines, &["Заменить", "Пропустить", "Оставить оба", "Отмена"], On::Conflict(id));
    d.check = Some(("Сделать это для всех конфликтов".into(), false));
    d.wide = true;
    d
}

// ------------------------------------------------------------------ entry point

fn parse_loc(a: &str) -> Loc {
    match a {
        "trash:" | "trash:///" => Loc::Trash,
        "computer:" | "computer:///" => Loc::Pc,
        _ => {
            let a = a.strip_prefix("file://").unwrap_or(a);
            let p = PathBuf::from(a.replace("~", &model::home().to_string_lossy()));
            if p.is_dir() {
                Loc::Dir(p)
            } else if p.is_file() {
                if ops::is_archive(&model::ext_of(&a)) {
                    Loc::Arc(p, String::new())
                } else {
                    Loc::Dir(p.parent().map(Path::to_path_buf).unwrap_or_else(model::home))
                }
            } else {
                Loc::Dir(model::home())
            }
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    crate::font::init();
    // --tab FILE: a tab torn off another window (its place and history).
    let torn = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).and_then(|f| {
        let t = std::fs::read_to_string(f).ok();
        let _ = std::fs::remove_file(f);
        t.and_then(|t| dnd::tab_parse(&t))
    });
    let tab_file = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned();
    let start = args.iter().find(|a| !a.starts_with('-') && Some(*a) != tab_file.as_ref()).map(|a| parse_loc(a)).unwrap_or_else(|| Loc::Dir(model::home()));
    let start = torn.as_ref().map(|t| t.loc.clone()).unwrap_or(start);
    let desk = args.iter().any(|a| a == "--desktop");
    let start = if desk { Loc::Dir(desktop::desktop_dir()) } else { start };
    let (title0, app_id) = if desk { ("Рабочий стол", "resc.desktop") } else { ("Проводник", "resc.files") };
    let mut win = match Window::open(title0, app_id, 1180, 720, if desk { (1, 1) } else { (640, 420) }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("files: {e}");
            return 1;
        }
    };
    let mut app = App::new(start, win.w, win.h);
    if desk {
        app.desk = true;
        app.view = app.view_for(&app.loc().clone());
        app.lay();
    }
    if let Some(t) = torn {
        app.tabs[0].back = t.back;
        app.tabs[0].fwd = t.fwd;
    }
    if app.watch.fd >= 0 {
        win.watch_fd(app.watch.fd, true);
    }
    let mut last = sys::now_ns();
    let mut title = String::new();
    loop {
        let wait = if app.animating() {
            16
        } else if app.ops_busy() || app.searching || app.rename_at.is_some() || app.search_at.is_some() || app.reload_at.is_some() || app.props_rx.is_some() || app.hash_rx.is_some() || app.toast.is_some() || app.press.as_ref().map_or(false, |p| p.dragging) || app.incoming.is_some() {
            100
        } else {
            // Now and then: the look (the ambient color with the wallpaper) may change.
            1000
        };
        if crate::look::poll() {
            crate::icons::reset();
            app.dirty = true;
        }
        let evs = match win.poll(wait) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("files: {e}");
                return 1;
            }
        };
        for ev in evs {
            app.dirty = true;
            match ev {
                Ev::Resized => {
                    app.w = win.w;
                    app.h = win.h;
                    app.lay();
                    app.clamp_scroll();
                    if let Some(f) = app.focus {
                        app.fdrop.snap(app.rects[f]);
                    }
                }
                Ev::Close => app.quit = !app.desk,
                Ev::Motion(x, y) => app.motion(x, y),
                Ev::Leave => app.leave(),
                Ev::Button(b, down) => app.button(b, down),
                Ev::Wheel(v) => app.wheel(v),
                Ev::Mods(m) => app.mods = m,
                Ev::Key(k, down) => {
                    if down {
                        app.key(k);
                    }
                }
                Ev::DndEnter { x, y, mimes, actions: _ } => {
                    let mut read = |m: &str| win.dnd_receive(m);
                    app.dnd_enter(x, y, &mimes, &mut read);
                }
                Ev::DndMotion(x, y) => app.dnd_motion(x, y),
                Ev::DndLeave => app.dnd_leave(),
                Ev::DndDrop => {
                    let a = win.dnd_action();
                    app.dnd_drop(a);
                    win.dnd_finish();
                }
                Ev::DragSend(mime, fd) => {
                    use std::io::Write;
                    let data = app.drag_data(&mime);
                    let mut f = std::fs::File::from(fd);
                    let _ = f.write_all(&data);
                }
                Ev::DragAction(_) => {}
                Ev::DragEnd(done) => app.drag_end(done),
                Ev::OverlayDone => {
                    app.menu = None;
                }
            }
        }
        app.maximized = win.maximized;
        for r in std::mem::take(&mut app.winreq) {
            match r {
                WinReq::Move => win.begin_move(),
                WinReq::Minimize => win.minimize(),
                WinReq::Maximize => win.toggle_maximize(),
                WinReq::StartDrag { mimes, actions, icon } => {
                    let icon = icon.as_ref().map(|(px, w, h, hx, hy)| (px.as_slice(), *w, *h, *hx, *hy));
                    win.start_drag(&mimes, actions, icon);
                }
                WinReq::DndAccept { mime, actions, pref } => win.dnd_accept(mime, actions, pref),
            }
        }
        if app.quit {
            return 0;
        }
        app.tick();
        let t = if app.desk {
            "Рабочий стол".to_string()
        } else if app.title.is_empty() {
            "Проводник".to_string()
        } else {
            format!("{} — Проводник", app.title)
        };
        if t != title {
            win.set_title(&t);
            title = t;
        }
        let now = sys::now_ns();
        let dt = ((now - last) as f64 / 1e9).min(0.05) as f32;
        last = now;
        if app.step(dt) {
            app.dirty = true;
        }
        // The desktop's menus and dialogs: over every window, in a popup of their own.
        if app.desk {
            let need = app.menu.is_some() || app.dialog.is_some();
            if need && !win.overlay_shown() {
                win.overlay_show(app.menu.is_some());
                app.dirty = true;
            } else if !need && win.overlay_shown() {
                win.overlay_hide();
            }
        }
        if app.dirty {
            let mut done = true;
            if win.ready() {
                win.draw(|cv| draw::draw(&app, cv));
            } else {
                done = false;
            }
            if app.desk && win.overlay_shown() {
                let mut input = Vec::new();
                if let Some(m) = &app.menu {
                    input.push(m.rect());
                    if let Some((_, s)) = &m.sub {
                        input.push(s.rect());
                    }
                }
                if let Some(d) = &app.dialog {
                    input.push(draw::dialog_rect(d, app.w, app.h));
                }
                done &= win.overlay_draw(&input, |cv| draw::overlay(&app, cv));
            }
            if done {
                app.dirty = false;
            }
        }
    }
}

impl App {
    /// The menu of «Создать».
    pub fn new_menu(&self) -> Vec<MItem> {
        let mut v = vec![MItem::new("Папку", Act::NewFolder).hint("Ctrl+Shift+N").icon("folder"), MItem::sep(), MItem::new("Текстовый документ", Act::NewText).icon("text-plain;text-x-generic")];
        let tpl = model::user_dirs().into_iter().find(|(k, _)| *k == "TEMPLATES").map(|x| x.1);
        if let Some(t) = tpl {
            if let Ok(list) = model::list(&t) {
                for e in list.into_iter().filter(|e| !e.dir && !e.hidden).take(12) {
                    let k = e.kind();
                    let label = stem_of(&e.name);
                    v.push(MItem::new(&label, Act::NewFrom(e.path.clone())).icon(&k.icon));
                }
            }
        }
        v
    }

    pub fn sort_menu(&self) -> Vec<MItem> {
        let mut v: Vec<MItem> = self.columns().into_iter().map(|c| MItem::new(c.title(), Act::Sort(c)).check(self.view.sort == c)).collect();
        v.push(MItem::sep());
        v.push(MItem::new("По возрастанию", Act::Desc(false)).check(!self.view.desc));
        v.push(MItem::new("По убыванию", Act::Desc(true)).check(self.view.desc));
        v.push(MItem::sep());
        v.push(MItem::new("Группировка", Act::None).sub(vec![
            MItem::new("Нет", Act::Group(None)).check(self.view.group.is_none()),
            MItem::new("Дата изменения", Act::Group(Some(Col::Date))).check(self.view.group == Some(Col::Date)),
            MItem::new("Тип", Act::Group(Some(Col::Type))).check(self.view.group == Some(Col::Type)),
        ]));
        v
    }

    pub fn view_menu(&self) -> Vec<MItem> {
        let m = self.view.mode;
        vec![
            MItem::new("Огромные значки", Act::Mode(Mode::Icons(256))).check(m == Mode::Icons(256)),
            MItem::new("Крупные значки", Act::Mode(Mode::Icons(96))).check(m == Mode::Icons(96)),
            MItem::new("Обычные значки", Act::Mode(Mode::Icons(48))).check(m == Mode::Icons(48)),
            MItem::new("Мелкие значки", Act::Mode(Mode::Icons(32))).check(m == Mode::Icons(32)),
            MItem::new("Таблица", Act::Mode(Mode::Details)).hint("Ctrl+Shift+6").check(m == Mode::Details),
            MItem::sep(),
            MItem::new("Показать", Act::None).sub(vec![
                MItem::new("Расширения имён файлов", Act::Ext).check(self.show_ext),
                MItem::new("Скрытые элементы", Act::Hidden).hint("Ctrl+H").check(self.show_hidden),
            ]),
        ]
    }

    pub fn zip_menu(&self) -> Vec<MItem> {
        let sel = self.selected();
        let here = self.loc().dir().is_some();
        let arcs: Vec<&Item> = sel.iter().map(|i| &self.items[*i]).filter(|i| !i.e.dir && ops::is_archive(&i.e.ext)).collect();
        let on = self.have7z;
        let mut v = Vec::new();
        if !arcs.is_empty() && here {
            let one = arcs.len() == 1;
            v.push(MItem::new("Открыть архив", Act::ZOpen).on(on && one));
            v.push(MItem::new("Извлечь файлы…", Act::ZExtract).on(on && one));
            v.push(MItem::new("Извлечь здесь", Act::ZHere).on(on));
            let to = if one { format!("Извлечь в «{}/»", stem_of(&arcs[0].e.name)) } else { "Извлечь каждый архив в отдельную папку".into() };
            v.push(MItem::new(&to, Act::ZTo).on(on));
            v.push(MItem::new("Проверить архив", Act::ZTest).on(on));
            v.push(MItem::sep());
        }
        if !sel.is_empty() && here {
            let base = if sel.len() == 1 {
                let it = &self.items[sel[0]];
                if it.e.dir { it.e.name.clone() } else { stem_of(&it.e.name) }
            } else {
                self.loc().dir().map(|d| self.disp_name(d)).unwrap_or_default()
            };
            v.push(MItem::new("Добавить к архиву…", Act::ZAdd).on(on));
            v.push(MItem::new(&format!("Сжать в «{base}.7z»"), Act::ZQuick("7z")).on(on));
            v.push(MItem::new(&format!("Сжать в «{base}.zip»"), Act::ZQuick("zip")).on(on));
            v.push(MItem::sep());
            if sel.iter().all(|i| !self.items[*i].e.dir) {
                v.push(MItem::new("Контрольная сумма", Act::None).on(on).sub(vec![
                    MItem::new("CRC32", Act::Hash("CRC32")),
                    MItem::new("CRC64", Act::Hash("CRC64")),
                    MItem::new("SHA-1", Act::Hash("SHA1")),
                    MItem::new("SHA-256", Act::Hash("SHA256")),
                ]));
            }
        }
        while v.last().map_or(false, |i| i.is_sep()) {
            v.pop();
        }
        if !on && !v.is_empty() {
            v.insert(0, MItem::new("Установите пакет 7zip", Act::None).on(false));
            v.insert(1, MItem::sep());
        }
        v
    }

    /// Right click on items.
    pub fn item_menu(&mut self) -> Vec<MItem> {
        let sel = self.selected();
        let Some(&first) = sel.first() else { return vec![] };
        let it = self.items[first].clone();
        let mut v = Vec::new();
        if matches!(self.loc(), Loc::Trash) {
            v.push(MItem::new("Восстановить", Act::Restore).bold());
            v.push(MItem::sep());
            v.push(MItem::new("Вырезать", Act::Cut).hint("Ctrl+X").g(G::Cut));
            v.push(MItem::new("Удалить безвозвратно", Act::Trash).hint("Del").g(G::Delete));
            v.push(MItem::sep());
            v.push(MItem::new("Свойства", Act::Props).hint("Alt+Enter"));
            return v;
        }
        let in_arc = matches!(self.loc(), Loc::Arc(..));
        v.push(MItem::new("Открыть", Act::Open).hint("Enter").bold());
        if it.e.dir && !in_arc && !self.desk {
            v.push(MItem::new("Открыть в новой вкладке", Act::OpenTab));
            v.push(MItem::new("Открыть в терминале", Act::Terminal));
        }
        if !it.e.dir && !in_arc && sel.len() == 1 {
            let mime = it.mime;
            let apps: Vec<(String, String, String)> = self.apps().for_mime(mime).into_iter().take(10).map(|h| (h.name.clone(), h.id.clone(), h.icon.clone())).collect();
            if !apps.is_empty() {
                let sub = apps.into_iter().map(|(n, id, ic)| MItem::new(&n, Act::OpenWith(id)).icon(&ic)).collect();
                v.push(MItem::new("Открыть с помощью", Act::None).sub(sub));
            }
        }
        v.push(MItem::sep());
        if !in_arc {
            let z = self.zip_menu();
            if !z.is_empty() {
                v.push(MItem::new("7-Zip", Act::None).g(G::Archive).sub(z));
                v.push(MItem::sep());
            }
        }
        v.push(MItem::new("Вырезать", Act::Cut).hint("Ctrl+X").g(G::Cut).on(!in_arc));
        v.push(MItem::new("Копировать", Act::Copy).hint("Ctrl+C").g(G::Copy));
        v.push(MItem::new("Копировать как путь", Act::CopyPath).hint("Ctrl+Shift+C").on(!in_arc));
        if !in_arc {
            v.push(MItem::sep());
            v.push(MItem::new("Создать ярлык", Act::Link));
            v.push(MItem::new("Переименовать", Act::Rename).hint("F2").g(G::Rename).on(sel.len() == 1));
            v.push(MItem::new("Удалить", Act::Trash).hint("Del").g(G::Delete));
        }
        v.push(MItem::sep());
        v.push(MItem::new("Свойства", Act::Props).hint("Alt+Enter").on(sel.len() == 1));
        v
    }

    /// Right click on empty space.
    pub fn bg_menu(&self) -> Vec<MItem> {
        if self.desk {
            return self.desk_menu();
        }
        let mut v = Vec::new();
        if matches!(self.loc(), Loc::Trash) {
            v.push(MItem::new("Очистить корзину", Act::EmptyTrash).on(!self.items.is_empty()));
            v.push(MItem::sep());
        }
        v.push(MItem::new("Вид", Act::None).g(G::View).sub(self.view_menu()));
        v.push(MItem::new("Сортировка", Act::None).g(G::Sort).sub(self.sort_menu()));
        v.push(MItem::sep());
        v.push(MItem::new("Обновить", Act::Refresh).hint("F5").g(G::Refresh));
        if self.loc().dir().is_some() {
            v.push(MItem::sep());
            v.push(MItem::new("Вставить", Act::Paste).hint("Ctrl+V").g(G::Paste).on(self.clip.is_some()));
            v.push(MItem::new("Отменить", Act::Undo).hint("Ctrl+Z").on(!self.undo.is_empty()));
            v.push(MItem::new("Вернуть", Act::Redo).hint("Ctrl+Y").on(!self.redo.is_empty()));
            v.push(MItem::sep());
            v.push(MItem::new("Создать", Act::None).g(G::Plus).sub(self.new_menu()));
            v.push(MItem::sep());
            v.push(MItem::new("Открыть в терминале", Act::Terminal));
        }
        v.push(MItem::new("Выделить всё", Act::SelAll).hint("Ctrl+A"));
        v
    }

    pub fn more_menu(&self) -> Vec<MItem> {
        let mut v = vec![
            MItem::new("Отменить", Act::Undo).hint("Ctrl+Z").on(!self.undo.is_empty()),
            MItem::new("Вернуть", Act::Redo).hint("Ctrl+Y").on(!self.redo.is_empty()),
            MItem::sep(),
            MItem::new("Выделить всё", Act::SelAll).hint("Ctrl+A"),
            MItem::new("Снять выделение", Act::SelNone),
            MItem::new("Обратить выделение", Act::SelInvert),
            MItem::sep(),
            MItem::new("Копировать как путь", Act::CopyPath).hint("Ctrl+Shift+C").on(!self.selected().is_empty()),
            MItem::new("Свойства", Act::Props).hint("Alt+Enter").on(self.selected().len() == 1),
        ];
        if self.loc().dir().is_some() {
            v.push(MItem::new("Открыть в терминале", Act::Terminal));
        }
        v
    }

    pub fn open_menu(&mut self, items: Vec<MItem>, x: i32, y: i32) {
        if items.is_empty() {
            return;
        }
        self.menu = Some(Menu::new(items, x, y, self.w, self.h));
        self.dirty = true;
    }

    pub fn open_sub(&mut self, i: usize) {
        let (w, h) = (self.w, self.h);
        let Some(m) = self.menu.as_mut() else { return };
        if m.sub.as_ref().map_or(false, |s| s.0 == i) {
            return;
        }
        let r = m.item_rect(i);
        let items = m.items[i].sub.clone();
        if items.is_empty() || !m.items[i].on {
            m.sub = None;
            return;
        }
        let mut s = Menu::new(items, r.x1() + 2, r.y - 6, w, h);
        if s.x < r.x1() - 10 {
            // No room on the right: to the left of the menu.
            s.x = (m.x - s.w + 4).max(4);
        }
        m.sub = Some((i, Box::new(s)));
    }

    // -------------------------------------------------------------- acts

    pub fn act(&mut self, a: Act) {
        self.menu = None;
        self.dirty = true;
        let sel = self.selected();
        match a {
            Act::None => {}
            Act::Open => self.open_items(sel),
            Act::OpenWith(id) => {
                let h = self.apps().all.get(&id).cloned();
                if let Some(h) = h {
                    let files = self.sel_paths();
                    open::spawn(&open::command(&h, &files), files.first().and_then(|f| f.parent()));
                }
            }
            Act::OpenTab => {
                for i in sel {
                    if self.items[i].e.dir {
                        let l = Loc::Dir(self.items[i].e.path.clone());
                        self.save_tab();
                        self.tabs.push(Tab { loc: l, back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 });
                    }
                }
            }
            Act::Terminal => {
                let dir = match sel.first().map(|i| &self.items[*i]) {
                    Some(it) if it.e.dir => Some(it.e.path.clone()),
                    _ => self.loc().dir().map(Path::to_path_buf),
                };
                if let Some(d) = dir {
                    let term = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
                    open::spawn(&term, Some(&d));
                }
            }
            Act::Cut | Act::Copy => {
                let p = self.sel_paths();
                if !p.is_empty() {
                    let cut = a == Act::Cut && !matches!(self.loc(), Loc::Arc(..));
                    self.clip = Some((p, cut));
                }
            }
            Act::CopyPath => {
                let p: Vec<String> = self.sel_paths().iter().map(|p| format!("\"{}\"", p.display())).collect();
                if !p.is_empty() {
                    self.copy_text(&p.join("\n"));
                }
            }
            Act::Paste => self.paste(),
            Act::Rename => {
                if let Some(f) = self.focus.filter(|f| self.sel[*f]).or(sel.first().copied()) {
                    self.start_rename(f);
                }
            }
            Act::Trash => self.trash_sel(),
            Act::Delete => self.delete_sel(),
            Act::Props => {
                if let Some(&i) = sel.first() {
                    self.props_of(i);
                } else if let Loc::Dir(p) = self.loc().clone() {
                    if let Some(e) = model::stat(&p) {
                        let it = self.make_item(e);
                        self.items.push(it);
                        let k = self.items.len() - 1;
                        self.props_of(k);
                        self.items.pop();
                    }
                }
            }
            Act::NewFolder => self.new_folder(),
            Act::NewText => self.new_file(None),
            Act::NewFrom(p) => self.new_file(Some(p)),
            Act::Refresh => self.reload(true),
            Act::Undo => self.do_undo(false),
            Act::Redo => self.do_undo(true),
            Act::SelAll => {
                self.sel.iter_mut().for_each(|s| *s = true);
            }
            Act::SelNone => {
                self.sel.iter_mut().for_each(|s| *s = false);
            }
            Act::SelInvert => {
                self.sel.iter_mut().for_each(|s| *s = !*s);
            }
            Act::Mode(m) => self.set_mode(m),
            Act::ToggleCol(c) => self.toggle_col(c),
            Act::FitCol(i) => self.autofit(i),
            Act::FitCols => {
                for i in 0..self.columns().len() {
                    self.autofit(i);
                }
            }
            Act::ResetCols => self.reset_cols(),
            Act::Sort(c) => {
                if self.view.sort == c {
                    self.view.desc = !self.view.desc;
                } else {
                    self.view.sort = c;
                    self.view.desc = matches!(c, Col::Date | Col::Deleted | Col::Size);
                }
                self.resort();
            }
            Act::Desc(d) => {
                self.view.desc = d;
                self.resort();
            }
            Act::Group(g) => {
                self.view.group = g;
                self.resort();
            }
            Act::Hidden => {
                self.show_hidden = !self.show_hidden;
                self.reload(false);
            }
            Act::Ext => {
                self.show_ext = !self.show_ext;
                self.reload(false);
            }
            Act::ZOpen => {
                if let Some(&i) = sel.first() {
                    let l = Loc::Arc(self.items[i].e.path.clone(), String::new());
                    self.navigate(l);
                }
            }
            Act::ZExtract => self.zip_extract_dialog(),
            Act::ZHere => self.zip_extract(false),
            Act::ZTo => self.zip_extract(true),
            Act::ZAdd => self.zip_add_dialog(),
            Act::ZQuick(f) => self.zip_quick(f),
            Act::ZTest => self.zip_test(),
            Act::Hash(h) => self.hash(h),
            Act::Restore => {
                let items = self.sel_paths();
                if !items.is_empty() {
                    let t = format!("Восстановление {}", self.names_of(&items));
                    self.start_op("Восстановление", t, None, ops::Job::Restore(items));
                }
            }
            Act::EmptyTrash => {
                let n = self.items.len();
                self.dialog = Some(Dialog::new("Очистка корзины", vec![format!("Вы действительно хотите безвозвратно удалить все элементы корзины ({n})?")], &["Да", "Нет"], On::EmptyTrash));
            }
            Act::Link => {
                let Some(dir) = self.loc().dir().map(Path::to_path_buf) else { return };
                for p in self.sel_paths() {
                    let name = p.file_name().unwrap().to_string_lossy().to_string();
                    let l = unique(&dir.join(format!("{name} — ярлык")));
                    if let Err(e) = std::os::unix::fs::symlink(&p, &l) {
                        self.error(&format!("Не удалось создать ярлык: {}", ops::explain(&e)));
                        break;
                    }
                    self.fresh.push(l.file_name().unwrap().to_string_lossy().to_string());
                }
                self.reload(false);
            }
            Act::NewTab => self.new_tab(Loc::Dir(model::home())),
            Act::CloseTab(i) => self.close_tab(i),
            Act::CloseOthers(i) => {
                self.save_tab();
                let t = self.tabs.remove(i);
                self.tabs = vec![t];
                self.cur = 0;
                let l = self.tabs[0].loc.clone();
                self.enter(l, false);
            }
            Act::DupTab(i) => {
                let l = self.tabs[i].loc.clone();
                self.new_tab(l);
            }
            Act::Settings(page) => {
                let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "resc-desk".into());
                open::spawn(&format!("{} settings {page}", open::quote(&exe)), None);
            }
            Act::DropCancel => {
                self.drop_paths = None;
                self.drag_to = None;
            }
            Act::DropCopy | Act::DropMove | Act::DropLink => {
                let from = self.drop_paths.take();
                if let Some((Loc::Dir(dest), _)) = self.drag_to.take() {
                    let items = from.unwrap_or_else(|| self.sel_paths());
                    match a {
                        Act::DropLink => {
                            for p in items {
                                let name = p.file_name().unwrap().to_string_lossy().to_string();
                                let _ = std::os::unix::fs::symlink(&p, unique(&dest.join(format!("{name} — ярлык"))));
                            }
                        }
                        _ => self.copy_to(items, dest, a == Act::DropMove),
                    }
                }
            }
            Act::Eject(p) => {
                let dev = self.mounts.iter().find(|m| m.path == p).map(|m| m.dev.clone()).unwrap_or_default();
                open::spawn(&format!("udisksctl unmount -b {0} && udisksctl power-off -b {0}", open::quote(&dev)), None);
                self.toast("Извлечение устройства…");
            }
            Act::Nav(l) => self.navigate(l),
            Act::NavTab(l) => self.new_tab(l),
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.view.mode = m;
        self.views.insert(self.loc().key(), self.view);
        self.lay();
        self.clamp_scroll();
        if let Some(f) = self.focus {
            self.fdrop.snap(self.rects[f]);
            self.reveal(f);
        }
        self.hdrop.hide();
        self.dirty = true;
    }

    /// Ctrl+wheel: through the icon sizes into the table (Windows' order).
    pub fn zoom(&mut self, up: bool) {
        let m = match (self.view.mode, up) {
            (Mode::Tiles, _) => return,
            (Mode::Details, true) => Mode::Icons(ZOOM[0]),
            (Mode::Details, false) => return,
            (Mode::Icons(s), true) => Mode::Icons(ZOOM.iter().copied().find(|z| *z > s).unwrap_or(256)),
            (Mode::Icons(s), false) => match ZOOM.iter().rev().copied().find(|z| *z < s) {
                Some(z) => Mode::Icons(z),
                None => Mode::Details,
            },
        };
        self.set_mode(m);
    }

    /// Text to the clipboard: through wl-copy when there is one (our client has no
    /// data device yet).
    pub fn copy_text(&mut self, s: &str) {
        if open::dry() {
            input::copy_out(s);
            self.toast("Путь скопирован");
            return;
        }
        if open::have("wl-copy") {
            if let Ok(mut c) = std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
                use std::io::Write;
                if let Some(mut i) = c.stdin.take() {
                    let _ = i.write_all(s.as_bytes());
                }
                let _ = c.wait();
                self.toast("Путь скопирован");
                return;
            }
        }
        self.toast("Нет буфера обмена (установите wl-clipboard)");
    }

    // -------------------------------------------------------------- dialogs

    pub fn dialog_done(&mut self, btn: usize) {
        let Some(mut d) = self.dialog.take() else { return };
        self.dirty = true;
        let ok = btn == 0;
        match d.on.clone() {
            On::Info | On::Props(_) if d.check.is_none() => {}
            On::Props(p) => {
                if let Some((_, on)) = d.check {
                    if let Ok(m) = std::fs::metadata(&p) {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = m.permissions().mode();
                        let x = if on { mode | ((mode & 0o444) >> 2) } else { mode & !0o111 };
                        if x != mode {
                            if let Err(e) = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(x)) {
                                self.error(&format!("Не удалось изменить права: {}", ops::explain(&e)));
                            }
                            self.reload(false);
                        }
                    }
                }
            }
            On::Info => {}
            On::DeleteForever(items) => {
                if ok {
                    let t = format!("Удаление {}", self.names_of(&items));
                    self.focus_after_delete();
                    self.start_op("Удаление", t, None, ops::Job::Delete(items));
                }
            }
            On::EmptyTrash => {
                if ok {
                    let items: Vec<PathBuf> = trash::list().into_iter().map(|t| t.file).collect();
                    let t = format!("Очистка корзины ({})", items.len());
                    self.start_op("Удаление", t, None, ops::Job::Delete(items));
                }
            }
            On::Conflict(id) => {
                let all = d.check.as_ref().map_or(false, |c| c.1);
                let a = [Answer::Replace, Answer::Skip, Answer::Both, Answer::Cancel][btn.min(3)];
                if let Some(op) = self.ops.iter().find(|o| o.id == id) {
                    op.reply(a, all);
                }
            }
            On::ZipPassword(id) => {
                let op = self.retry.remove(&id);
                if let (true, Some(op)) = (ok, op) {
                    let pw = d.fields.first().map(|f| f.tf.text()).unwrap_or_default();
                    if let Some((args, cwd, made)) = op.zip.clone() {
                        let new = self.start_op(op.verb, op.title.clone(), op.dest.clone(), ops::Job::Zip { args, cwd, made, password: Some(pw.clone()) });
                        if let Some(t) = self.then.remove(&id) {
                            self.then.insert(new, t);
                        }
                    }
                }
            }
            On::ArcPassword(f, inner) => {
                if ok {
                    let pw = d.fields.first().map(|f| f.tf.text()).unwrap_or_default();
                    match ops::list_archive(&f, Some(&pw)) {
                        Ok(v) => {
                            self.arcs.insert(f.clone(), (v, Some(pw)));
                            self.reload(false);
                        }
                        Err(e) if e == "password" => {
                            self.ask_arc_password(&f, &inner);
                            if let Some(d) = self.dialog.as_mut() {
                                d.lines.push("Неверный пароль.".into());
                            }
                        }
                        Err(e) => self.error(&e),
                    }
                } else {
                    self.go_back();
                }
            }
            On::Add7z(items, dir) => {
                if ok {
                    let name = d.fields[0].tf.text();
                    let fmt = d.choices[0].1[d.choices[0].2].clone();
                    let level = [0, 1, 3, 5, 7, 9][d.choices[1].2.min(5)];
                    let pw = Some(d.fields[1].tf.text()).filter(|p| !p.is_empty());
                    let vol = Some(d.fields[2].tf.text()).filter(|p| !p.is_empty());
                    let name = if name.to_lowercase().ends_with(&format!(".{fmt}")) { name } else { format!("{name}.{fmt}") };
                    let out = if name.starts_with('/') { PathBuf::from(name) } else { dir.join(name) };
                    let out = unique(&out);
                    let fmt_s: &str = &fmt;
                    self.zip_add(items, out, fmt_s, level, pw, vol);
                }
            }
            On::Extract(arc) => {
                if ok {
                    let to = d.fields[0].tf.text();
                    let pw = Some(d.fields[1].tf.text()).filter(|p| !p.is_empty());
                    let to = PathBuf::from(to.replace('~', &model::home().to_string_lossy()));
                    self.extract_to(&arc, to, pw);
                }
            }
            On::ExtChange(p, name) => {
                if ok {
                    let to = p.with_file_name(&name);
                    if std::fs::symlink_metadata(&to).is_ok() {
                        self.error(&format!("В этом расположении уже есть элемент с именем «{name}»."));
                    } else {
                        self.rename_to(&p, &name);
                    }
                }
            }
            On::Taken(p, name) => {
                if ok {
                    self.rename_to(&p, &name);
                }
            }
            On::MakeExec(p) => {
                if ok {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(m) = std::fs::metadata(&p) {
                        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m.permissions().mode() | 0o111));
                    }
                    open::spawn(&open::quote(&p.to_string_lossy()), p.parent());
                    self.reload(false);
                }
            }
            On::RunScript(p) => {
                let q = open::quote(&p.to_string_lossy());
                match btn {
                    0 => {
                        let term = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
                        open::spawn(&format!("{term} -e {q}"), p.parent());
                    }
                    1 => open::spawn(&q, p.parent()),
                    2 => {
                        let h = self.apps().default_for("text/plain").cloned();
                        match h {
                            Some(h) => open::spawn(&open::command(&h, &[p.clone()]), p.parent()),
                            None => self.toast("Нет текстового редактора"),
                        }
                    }
                    _ => {}
                }
            }
        }
        let _ = &mut d;
    }
}
