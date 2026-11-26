//! «Параметры»: resc-desk's settings app, a window of its own like Windows Settings
//! (`resc-desk settings [look|displays|about]`, Win+I, the gear in «Пуск»). Laid out like
//! Liquify's settings tab (`Liquify_last/src/screens/settings.rs`): pages on the left,
//! cards on the right (name, description, and a switch, a slider or a button). The switch
//! and the slider are Liquify's, size for size: the switch's knob is a spring (0.34 s,
//! damping 0.66) that stretches with its speed while the track's color follows it.
//!
//! It reads and changes the compositor through the control socket (ipc.rs `state`,
//! `glass`, `ui-scale`, `glue`, `calibrate`, `save`).
use crate::anim::{Drop, Spring};
use crate::client::{Ev, Window};
use crate::font;
use crate::geom::Rect;
use crate::glass::sd_squircle;
use crate::render::{argb, blend, Canvas, Xf};

const SIDE_W: i32 = 230;
/// `.vsett` side padding, card padding, gap between a card's text and its control.
const PAD: i32 = 24;
const CARD_PAD: i32 = 16;
const GAP: i32 = 16;
const SWITCH_W: f32 = 44.0;
const SWITCH_H: f32 = 26.0;
const KNOB: f32 = 20.0;
const SLIDER_H: i32 = 30;
const SLIDER_KNOB: f32 = 18.0;
const DESC_LINE: i32 = 17; // 13 px × 1.3
const NAV_H: i32 = 40;
const BTN_W: i32 = 132;
const BTN_H: i32 = 32;
const TOP: i32 = 78;
/// The ambient color (look.rs: the wallpaper's, as Liquify takes a cover's), straight RGB.
fn accent_rgb() -> [f32; 3] {
    let c = crate::look::accent();
    [(c >> 16 & 255) as f32 / 255.0, (c >> 8 & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

const UI_MIN: f64 = 0.75;
const UI_MAX: f64 = 2.5;
const UI_STEP: f64 = 0.25;

const KEY_UP: u32 = 103;
const KEY_DOWN: u32 = 108;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Look,
    Glass,
    Wallpaper,
    Displays,
    Sound,
    Bluetooth,
    Mouse,
    About,
}

const PAGES: [(Page, &str, &str); 8] = [
    (Page::Look, "Оформление", "preferences-desktop-theme"),
    (Page::Glass, "Стекло", "preferences-desktop-effects;preferences-system-windows;preferences-desktop-theme"),
    (Page::Wallpaper, "Обои", "preferences-desktop-wallpaper;preferences-desktop-background"),
    (Page::Displays, "Мониторы", "preferences-desktop-display"),
    (Page::Sound, "Звук", "preferences-desktop-sound;audio-volume-high;multimedia-volume-control"),
    (Page::Bluetooth, "Bluetooth", "bluetooth;preferences-system-bluetooth;bluetooth-active;blueman"),
    (Page::Mouse, "Мышь", "input-mouse;preferences-desktop-peripherals;input-mouse-symbolic"),
    (Page::About, "О системе", "computer;help-about"),
];

/// «Мышь»: the actions to pick for a button (label, mousebind.rs action).
const MOUSE_ACTS: [(&str, &str); 27] = [
    ("Как обычно", "default"),
    ("Ничего", "none"),
    ("G-Shift — зажать для второго слоя", "gshift"),
    ("Копировать (Ctrl+C)", "key ctrl+c"),
    ("Вставить (Ctrl+V)", "key ctrl+v"),
    ("Вырезать (Ctrl+X)", "key ctrl+x"),
    ("Отменить (Ctrl+Z)", "key ctrl+z"),
    ("Повторить (Ctrl+Shift+Z)", "key ctrl+shift+z"),
    ("Назад (Alt+←)", "key alt+left"),
    ("Вперёд (Alt+→)", "key alt+right"),
    ("Новая вкладка (Ctrl+T)", "key ctrl+t"),
    ("Закрыть вкладку (Ctrl+W)", "key ctrl+w"),
    ("Следующая вкладка (Ctrl+Tab)", "key ctrl+tab"),
    ("Предыдущая вкладка (Ctrl+Shift+Tab)", "key ctrl+shift+tab"),
    ("«Пуск» (Win)", "key super"),
    ("Представление задач (Win+Tab)", "key super+tab"),
    ("Переключить окно (Alt+Tab)", "key alt+tab"),
    ("Закрыть окно (Alt+F4)", "key alt+f4"),
    ("Снимок области (Win+Shift+S)", "key super+shift+s"),
    ("Буфер обмена (Win+V)", "key super+v"),
    ("Проводник (Win+E)", "key super+e"),
    ("Пауза / воспроизведение", "key playpause"),
    ("Следующий трек", "key nextsong"),
    ("Предыдущий трек", "key previoussong"),
    ("Громче", "key volumeup"),
    ("Тише", "key volumedown"),
    ("Без звука", "key mute"),
];

/// Wallpaper tiles: size of a thumbnail.
const TW: usize = 320;
const TH: usize = 180;
const TILE_GAP: i32 = 12;
const TILE_LABEL: i32 = 24;
const DEV_ROW: i32 = 38;
/// «Удалить» at the right end of a paired Bluetooth device's row.
const BT_FORGET_W: i32 = 84;

/// The monitor layout's height on the «Мониторы» page.
const PLANE_H: i32 = 230;

#[derive(Clone, Copy, PartialEq)]
enum Sw {
    Frost,
    Glue,
    OutMute,
    InMute,
    /// The chosen monitor: the main one, on, VRR, HDR.
    Primary,
    MonOn,
    Vrr,
    Hdr,
    /// The taskbar on every monitor.
    TbAll,
    TbAuto,
    TbDesk,
    /// The Bluetooth adapter's power.
    BtPower,
    /// The live wallpaper «Частицы», and over the picture.
    Live,
    LivePic,
}

#[derive(Clone, Copy, PartialEq)]
enum Sl {
    Blur,
    Bezel,
    WinBezel,
    UiScale,
    OutVol,
    InVol,
    /// The chosen glass place's dim, blur and edge.
    PDim,
    PBlur,
    PBezel,
}

/// A choice of one among a few (life.rs: how programs start and windows close).
#[derive(Clone, Copy, PartialEq)]
enum Ch {
    Launch,
    Close,
    /// Which glass place the sliders below set.
    Place,
    /// Monitors: how several are used (Win+P), which one the cards below set, and its
    /// size, refresh and rotation.
    Project,
    Mon,
    Res,
    Hz,
    Rot,
    /// «Мышь»: the button, its layer (as is, with G-Shift), its action.
    MBtn,
    MLayer,
    MAct,
}

const ALL_CH: [Ch; 11] = [Ch::Launch, Ch::Close, Ch::Place, Ch::Project, Ch::Mon, Ch::Res, Ch::Hz, Ch::Rot, Ch::MBtn, Ch::MLayer, Ch::MAct];

const ROTATIONS: [&str; 4] = ["Без поворота", "90° — портрет", "180° — вверх ногами", "270° — портрет, наоборот"];

impl Ch {
    fn idx(self) -> usize {
        self as usize
    }
}

fn hz_name(mhz: i32) -> String {
    if mhz % 1000 == 0 {
        format!("{} Гц", mhz / 1000)
    } else {
        format!("{:.2} Гц", mhz as f64 / 1000.0)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Calibrate,
    /// «О системе»: check for, install, or start the new version (update.rs).
    Update,
}

/// Where the update is (update.rs runs on a thread of its own).
enum Upd {
    Idle,
    Busy(&'static str),
    UpToDate,
    Newer(crate::update::Release),
    Installed(String),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Ctl {
    None,
    Switch(Sw),
    Slider(Sl),
    Button(Btn, &'static str),
    /// The wallpaper tiles.
    Walls,
    /// The outputs (false) or inputs (true) to choose from.
    Devices(bool),
    /// One of a few named options.
    Choice(Ch),
    /// The monitors on the desk, to drag about.
    Plane,
    /// Bluetooth devices: paired (false) or found nearby (true).
    Bt(bool),
}

// ------------------------------------------------------------------ the compositor's state

struct Out {
    name: String,
    pw: i32,
    ph: i32,
    mhz: i32,
    plane: [f64; 4],
    scale: f64,
}

/// A connected monitor: what it does and can do (`state`: «mon», «mode»).
#[derive(Clone, Default)]
struct Mon {
    name: String,
    on: bool,
    w: i32,
    h: i32,
    mhz: i32,
    rot: u16,
    vrr_cap: bool,
    vrr: bool,
    hdr_cap: bool,
    hdr: bool,
    primary: bool,
    mirror: Option<String>,
    modes: Vec<(i32, i32, i32)>,
}

#[derive(Default)]
struct Model {
    frosted: bool,
    blur: f32,
    max_blur: f32,
    bezel: f32,
    win_bezel: f32,
    max_bezel: f32,
    ui_scale: f64,
    glue: bool,
    /// How programs start and windows close (life.rs).
    anim_launch: usize,
    anim_close: usize,
    /// The current wallpaper (None: the built-in one).
    wallpaper: Option<String>,
    /// Each glass place's dim, blur and edge (look.rs PLACES order).
    places: [[f32; 3]; 5],
    renderer: String,
    session: String,
    outputs: Vec<Out>,
    mons: Vec<Mon>,
    /// Win+P's way (monitors::PROJECTS).
    project: usize,
    taskbar_all: bool,
    taskbar_autohide: bool,
    taskbar_keep_desktop: bool,
    /// The mouse's bindings: button, layer, action (mousebind.rs).
    mbinds: Vec<(String, u8, String)>,
    /// The live wallpaper: on, over the picture.
    live: bool,
    live_pic: bool,
    error: Option<String>,
}

fn fetch() -> Model {
    let mut m = Model { max_blur: 40.0, max_bezel: 48.0, ui_scale: 1.0, project: 2, ..Default::default() };
    let text = match crate::ipc::request("state") {
        Ok(t) => t,
        Err(e) => {
            m.error = Some(e);
            return m;
        }
    };
    for line in text.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        let n: Vec<&str> = v.split_whitespace().collect();
        let f = |i: usize| n.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        match k {
            "glass" => {
                m.frosted = f(0) != 0.0;
                m.blur = f(1) as f32;
            }
            "max_blur" => m.max_blur = f(0) as f32,
            "bezel" => {
                m.bezel = f(0) as f32;
                m.win_bezel = f(1) as f32;
                m.max_bezel = (f(2) as f32).max(1.0);
            }
            "ui_scale" => m.ui_scale = f(0),
            "place" if n.len() >= 4 => {
                if let Some(i) = crate::look::PLACES.iter().position(|p| p.key() == n[0]) {
                    m.places[i] = [f(1) as f32, f(2) as f32, f(3) as f32];
                }
            }
            "anim" => {
                m.anim_launch = f(0) as usize;
                m.anim_close = f(1) as usize;
            }
            "wallpaper" => m.wallpaper = (v != "default" && !v.is_empty()).then(|| v.to_string()),
            "glue" => m.glue = f(0) != 0.0,
            "renderer" => m.renderer = v.to_string(),
            "session" => m.session = v.to_string(),
            "project" => m.project = crate::monitors::PROJECTS.iter().position(|p| p.key() == v).unwrap_or(2),
            "taskbar" => m.taskbar_all = v != "main",
            "taskbar_autohide" => m.taskbar_autohide = v == "on",
            "taskbar_keep_desktop" => m.taskbar_keep_desktop = v == "on",
            "mbind" if n.len() >= 3 => {
                if let Ok(layer) = n[1].parse::<u8>() {
                    m.mbinds.push((n[0].to_string(), layer, n[2..].join(" ")));
                }
            }
            "livewall" => {
                m.live = v != "off";
                m.live_pic = v == "picture";
            }
            "mon" if n.len() >= 12 => m.mons.push(Mon {
                name: n[0].to_string(),
                on: n[1] == "1",
                w: f(2) as i32,
                h: f(3) as i32,
                mhz: f(4) as i32,
                rot: f(5) as u16,
                vrr_cap: n[6] == "1",
                vrr: n[7] == "1",
                hdr_cap: n[8] == "1",
                hdr: n[9] == "1",
                primary: n[10] == "1",
                mirror: (n[11] != "-").then(|| n[11].to_string()),
                modes: Vec::new(),
            }),
            "mode" if n.len() >= 4 => {
                if let Some(mo) = m.mons.iter_mut().find(|x| x.name == n[0]) {
                    mo.modes.push((f(1) as i32, f(2) as i32, f(3) as i32));
                }
            }
            "output" if n.len() >= 9 => m.outputs.push(Out {
                name: n[0].to_string(),
                pw: f(1) as i32,
                ph: f(2) as i32,
                mhz: f(3) as i32,
                plane: [f(4), f(5), f(6), f(7)],
                scale: f(8),
            }),
            _ => {}
        }
    }
    m
}

fn send(cmd: &str) {
    if let Err(e) = crate::ipc::request(cmd) {
        eprintln!("settings: {cmd}: {e}");
    }
}

// ------------------------------------------------------------------ motion

/// Everything that moves, keyed by what it belongs to. `sync_anims` points every spring at
/// what the state says each frame; drawing reads the springs only. A key seen for the first
/// time starts at rest on its target (nothing animates in from zero on opening).
#[derive(Clone, Copy, PartialEq)]
enum AK {
    Switch(Sw),
    Slider(Sl),
    NavHot(usize),
    Btn,
    WallHot(usize),
    WallSel(usize),
    DevHot(bool, usize),
    ChHot(Ch, usize),
}

/// The switch's spring (Liquify: 0.34 s, damping 0.66) and a quick, flat one for hover.
const SPR_SWITCH: (f32, f32) = (0.34, 0.66);
const SPR_HOVER: (f32, f32) = (0.16, 1.0);
const SPR_SEL: (f32, f32) = (0.30, 0.80);
/// A slider's knob: close behind the pointer while dragged, softer when it jumps.
const SPR_DRAG: (f32, f32) = (0.07, 1.0);
const SPR_SLIDE: (f32, f32) = (0.30, 0.86);

#[derive(Default)]
struct Anims(Vec<(AK, Spring)>);

impl Anims {
    fn drive(&mut self, k: AK, target: f32, (response, damping): (f32, f32)) {
        match self.0.iter_mut().find(|(key, _)| *key == k) {
            Some((_, s)) => {
                s.response = response;
                s.damping = damping;
                s.to(target);
            }
            None => self.0.push((k, Spring::new(target, response, damping))),
        }
    }

    fn spring(&self, k: AK) -> Spring {
        self.0.iter().find(|(key, _)| *key == k).map_or(Spring::new(0.0, 0.3, 1.0), |(_, s)| *s)
    }

    fn get(&self, k: AK) -> f32 {
        self.spring(k).value
    }

    fn step(&mut self, dt: f32) -> bool {
        let mut any = false;
        for (_, s) in &mut self.0 {
            any |= s.step(dt);
        }
        any
    }

    fn moving(&self) -> bool {
        self.0.iter().any(|(_, s)| s.moving())
    }
}

fn on(v: bool) -> f32 {
    if v { 1.0 } else { 0.0 }
}

/// A device's row in its list.
fn dev_rect(at: Rect, i: usize) -> Rect {
    Rect::new(at.x - 8, at.y + i as i32 * DEV_ROW, at.w + 16, DEV_ROW - 4)
}

// ------------------------------------------------------------------ the app

struct App {
    m: Model,
    page: Page,
    /// Scroll offset of the cards, a spring (the wheel moves its target).
    scroll: Spring,
    anims: Anims,
    /// The page list's selection, flowing between pages.
    nav: Drop,
    /// The chosen output (0) and input (1) device, flowing between rows.
    devs: [Option<Drop>; 2],
    /// The chosen option of each `Ch`, flowing between rows.
    chs: [Option<Drop>; 11],
    /// The glass place the «Стекло» page sets.
    place: usize,
    /// «Мышь»: the button and the layer the action list sets.
    mbtn: usize,
    mlayer: usize,
    /// The update: its state, and the thread's answer to come.
    upd: Upd,
    upd_rx: Option<std::sync::mpsc::Receiver<Upd>>,
    /// The monitor the «Мониторы» page sets (its name), and a monitor being dragged on
    /// the layout.
    mon: String,
    pdrag: Option<PDrag>,
    /// A page coming in: 0 → 1 (the cards fade in rising a little).
    page_t: Spring,
    /// The slider being dragged and its value (sent on release).
    drag: Option<Sl>,
    drag_val: f64,
    /// Under the pointer: a page (0..) or a button (100).
    hot: Option<usize>,
    ptr: Option<(f64, f64)>,
    w: i32,
    h: i32,
    /// Wallpapers on offer (None: the built-in one) and their thumbnails as they come.
    walls: Vec<(Option<String>, Option<Vec<u32>>)>,
    thumbs: Option<std::sync::mpsc::Receiver<(usize, Vec<u32>)>>,
    /// Sound: our own PulseAudio connection.
    pulse: Option<crate::pulse::Pulse>,
    audio: crate::pulse::Audio,
    pulse_err: Option<String>,
    /// Bluetooth (bt.rs): its thread, started with the page.
    bt: Option<crate::bt::Bt>,
}

/// A monitor dragged on the layout: its output, where it was taken (mm, from its corner),
/// the layout's scale held while dragging, and whether it moved.
#[derive(Clone, Copy)]
struct PDrag {
    i: usize,
    grab: (f64, f64),
    px: PXf,
    moved: bool,
}

/// The layout's millimetres → the card's pixels: `x = bx + mm * k`.
#[derive(Clone, Copy)]
struct PXf {
    k: f64,
    bx: f64,
    by: f64,
}

impl PXf {
    fn r(&self, p: [f64; 4]) -> Rect {
        let x0 = (self.bx + p[0] * self.k).round() as i32;
        let y0 = (self.by + p[1] * self.k).round() as i32;
        let x1 = (self.bx + (p[0] + p[2]) * self.k).round() as i32;
        let y1 = (self.by + (p[1] + p[3]) * self.k).round() as i32;
        Rect::from_edges(x0, y0, x1, y1)
    }

    fn mm(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.bx) / self.k, (y - self.by) / self.k)
    }
}

/// Where a monitor dragged to `p` (mm) snaps: its edges onto the others' (within `tol` mm).
fn snap_plane(p: [f64; 4], others: &[[f64; 4]], tol: f64) -> [f64; 4] {
    let mut best = (tol, 0.0);
    let mut besty = (tol, 0.0);
    for o in others {
        for (mine, theirs) in [(p[0], o[0] + o[2]), (p[0] + p[2], o[0]), (p[0], o[0]), (p[0] + p[2], o[0] + o[2])] {
            let dx = theirs - mine;
            if dx.abs() < best.0 {
                best = (dx.abs(), dx);
            }
        }
        for (mine, theirs) in [(p[1], o[1]), (p[1] + p[3], o[1] + o[3]), (p[1], o[1] + o[3]), (p[1] + p[3], o[1])] {
            let dy = theirs - mine;
            if dy.abs() < besty.0 {
                besty = (dy.abs(), dy);
            }
        }
    }
    [p[0] + best.1, p[1] + besty.1, p[2], p[3]]
}

struct Row {
    name: String,
    /// Right of the name, in the accent (a slider's value).
    value: String,
    desc: Vec<String>,
    ctl: Ctl,
    card: Rect,
    /// The switch, the slider's bar, or the button.
    at: Rect,
}

struct Layout {
    nav: Vec<Rect>,
    rows: Vec<Row>,
    view: Rect,
    /// Content height (for scrolling).
    total: i32,
}

fn read_first(path: &str, key: &str) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let l = t.lines().find(|l| l.starts_with(key))?;
    Some(l.split_once(':')?.1.trim().to_string())
}

impl App {
    fn sound_dev(&self, input: bool) -> Option<crate::pulse::Device> {
        if input { self.audio.source() } else { self.audio.sink() }.cloned()
    }

    fn vol(&self, input: bool) -> f32 {
        let sl = if input { Sl::InVol } else { Sl::OutVol };
        if self.drag == Some(sl) {
            self.drag_val as f32
        } else {
            self.sound_dev(input).map_or(0, |d| d.percent()) as f32
        }
    }

    /// Sound: (re)connects and reads the state.
    fn sound_refresh(&mut self) {
        if self.pulse.is_none() {
            match crate::pulse::Pulse::connect("resc-desk settings").and_then(|mut p| p.subscribe().map(|_| p)) {
                Ok(p) => {
                    self.pulse = Some(p);
                    self.pulse_err = None;
                }
                Err(e) => {
                    self.pulse_err = Some(e.to_string());
                    self.audio = Default::default();
                    return;
                }
            }
        }
        match self.pulse.as_mut().unwrap().state() {
            Ok(a) => self.audio = a,
            Err(e) => {
                self.pulse = None;
                self.pulse_err = Some(e.to_string());
            }
        }
    }

    /// Sound: a change from elsewhere (keys, the tray, other apps). True if redrawn.
    fn sound_poll(&mut self) -> bool {
        let Some(p) = self.pulse.as_mut() else { return false };
        match p.drain() {
            Ok(true) => {
                let before = self.audio.clone();
                self.sound_refresh();
                before != self.audio
            }
            Ok(false) => false,
            Err(_) => {
                self.pulse = None;
                true
            }
        }
    }

    fn sound_op(&mut self, f: impl FnOnce(&mut crate::pulse::Pulse, &crate::pulse::Audio) -> std::io::Result<()>) {
        let a = self.audio.clone();
        if let Some(p) = self.pulse.as_mut() {
            if f(p, &a).is_err() {
                self.pulse = None;
            }
        }
        self.sound_refresh();
    }

    /// Wallpapers: the list, and a thread making the thumbnails.
    fn walls_start(&mut self) {
        if !self.walls.is_empty() {
            return;
        }
        let paths = crate::wallpaper::candidates(60);
        self.walls = std::iter::once(None).chain(paths.iter().cloned().map(Some)).map(|p| (p, None)).collect();
        self.walls[0].1 = Some(crate::wallpaper::builtin(TW, TH));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (i, p) in paths.iter().enumerate() {
                let t = match crate::wallpaper::decode_file(p) {
                    Ok(img) => crate::wallpaper::thumb(&img, TW, TH),
                    Err(_) => vec![0xff20_1c2a; TW * TH],
                };
                if tx.send((i + 1, t)).is_err() {
                    return;
                }
            }
        });
        self.thumbs = Some(rx);
    }

    /// Thumbnails that came in. True if any.
    /// Starts `job` on a thread; its answer becomes the update's state.
    fn upd_start(&mut self, busy: &'static str, job: impl FnOnce() -> Upd + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
        self.upd = Upd::Busy(busy);
        self.upd_rx = Some(rx);
    }

    /// The button: check, install, or restart, depending on where the update is.
    fn upd_press(&mut self) {
        match &self.upd {
            Upd::Busy(_) => {}
            Upd::Newer(r) => {
                let r = r.clone();
                self.upd_start("Скачиваю и проверяю…", move || match crate::update::install(&r) {
                    Ok(_) => Upd::Installed(r.tag.clone()),
                    Err(e) => Upd::Failed(e),
                });
            }
            Upd::Installed(_) => send("restart"),
            _ => self.upd_start("Проверяю…", || match crate::update::check() {
                Ok(Some(r)) => Upd::Newer(r),
                Ok(None) => Upd::UpToDate,
                Err(e) => Upd::Failed(e),
            }),
        }
    }

    fn upd_poll(&mut self) -> bool {
        let Some(rx) = &self.upd_rx else { return false };
        match rx.try_recv() {
            Ok(u) => {
                self.upd = u;
                self.upd_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(_) => {
                self.upd = Upd::Failed("проверка оборвалась".into());
                self.upd_rx = None;
                true
            }
        }
    }

    fn walls_poll(&mut self) -> bool {
        let Some(rx) = &self.thumbs else { return false };
        let mut any = false;
        loop {
            match rx.try_recv() {
                Ok((i, t)) => {
                    if let Some(w) = self.walls.get_mut(i) {
                        w.1 = Some(t);
                        any = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(_) => {
                    self.thumbs = None;
                    break;
                }
            }
        }
        any
    }

    /// The tiles' grid in a card `w` wide: columns, tile width and height.
    fn wall_grid(&self, w: i32) -> (i32, i32, i32) {
        let inner = w - 2 * CARD_PAD;
        let cols = ((inner + TILE_GAP) / (170 + TILE_GAP)).max(2);
        let tw = (inner - (cols - 1) * TILE_GAP) / cols;
        (cols, tw, tw * 9 / 16)
    }

    fn wall_tiles(&self, at: Rect) -> Vec<Rect> {
        let (cols, tw, th) = self.wall_grid(at.w + 2 * CARD_PAD);
        (0..self.walls.len() as i32)
            .map(|i| Rect::new(at.x + (i % cols) * (tw + TILE_GAP), at.y + (i / cols) * (th + TILE_LABEL + TILE_GAP), tw, th))
            .collect()
    }

    /// The option chosen now.
    fn choice(&self, ch: Ch) -> usize {
        let mon = self.sel_mon();
        match ch {
            Ch::Launch => self.m.anim_launch,
            Ch::Close => self.m.anim_close,
            Ch::Place => self.place,
            Ch::Project => self.m.project,
            Ch::Mon => self.m.mons.iter().position(|m| m.name == self.mon).unwrap_or(0),
            Ch::Res => mon.and_then(|m| self.sizes().iter().position(|s| *s == (m.w, m.h))).unwrap_or(0),
            Ch::Hz => mon.and_then(|m| self.rates().iter().position(|r| *r == m.mhz)).unwrap_or(0),
            Ch::Rot => mon.map_or(0, |m| (m.rot / 90) as usize),
            Ch::MBtn => self.mbtn,
            Ch::MLayer => self.mlayer,
            Ch::MAct => {
                let cur = self.mbind(self.mbtn, self.mlayer);
                MOUSE_ACTS.iter().position(|(_, s)| *s == cur).unwrap_or(MOUSE_ACTS.len())
            }
        }
    }

    /// What mouse button `b` does in `layer` («default» when nothing is set).
    fn mbind(&self, b: usize, layer: usize) -> String {
        let name = crate::mousebind::BUTTONS.get(b).map_or("", |x| x.0);
        self.m.mbinds.iter().find(|(n, l, _)| n == name && *l as usize == layer).map_or("default".to_string(), |x| x.2.clone())
    }

    /// An action's label (the preset's, or the action as written).
    fn mact_label(spec: &str) -> String {
        MOUSE_ACTS.iter().find(|(_, s)| *s == spec).map_or_else(|| format!("Своё: {spec}"), |(l, _)| l.to_string())
    }

    /// The options of `ch`.
    fn opts(&self, ch: Ch) -> Vec<String> {
        let st = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        match ch {
            Ch::Launch => st(&["Из кнопки на панели задач", "Из значка, по которому кликнули"]),
            Ch::Close => st(&["Втягивается в кнопку, кнопка лопается", "Сжимается в каплю и испаряется", "Втягивается в ✕"]),
            Ch::Place => st(&["Проводник", "Параметры", "Календарь", "Таймер", "Заголовки остальных окон"]),
            Ch::Project => crate::monitors::PROJECTS.iter().map(|p| p.name().to_string()).collect(),
            Ch::Mon => self
                .m
                .mons
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let state = match (&m.mirror, m.on) {
                        (Some(s), _) => format!(" · копия {s}"),
                        (None, false) => " · выключен".to_string(),
                        _ if m.primary => " · основной".to_string(),
                        _ => String::new(),
                    };
                    format!("{}  {}  ·  {}×{}{state}", i + 1, m.name, m.w, m.h)
                })
                .collect(),
            Ch::Res => self.sizes().iter().map(|(w, h)| format!("{w} × {h}")).collect(),
            Ch::Hz => self.rates().into_iter().map(hz_name).collect(),
            Ch::Rot => st(&ROTATIONS),
            Ch::MBtn => crate::mousebind::BUTTONS
                .iter()
                .enumerate()
                .map(|(i, (_, _, label))| {
                    let (a, b) = (self.mbind(i, 0), self.mbind(i, 1));
                    let mut s = label.to_string();
                    if a != "default" {
                        s.push_str(&format!("  ·  {}", Self::mact_label(&a)));
                    }
                    if b != "default" {
                        s.push_str(&format!("  ·  G-Shift: {}", Self::mact_label(&b)));
                    }
                    s
                })
                .collect(),
            Ch::MLayer => st(&["Обычно", "Пока зажата кнопка G-Shift"]),
            Ch::MAct => {
                let mut v: Vec<String> = MOUSE_ACTS.iter().map(|(l, _)| l.to_string()).collect();
                let cur = self.mbind(self.mbtn, self.mlayer);
                if !MOUSE_ACTS.iter().any(|(_, s)| *s == cur) {
                    v.push(Self::mact_label(&cur));
                }
                v
            }
        }
    }

    /// The monitor the page sets.
    fn sel_mon(&self) -> Option<&Mon> {
        self.m.mons.iter().find(|m| m.name == self.mon).or(self.m.mons.first())
    }

    /// Its sizes, largest first (ten at most, and the one it has).
    fn sizes(&self) -> Vec<(i32, i32)> {
        let Some(m) = self.sel_mon() else { return Vec::new() };
        let mut v: Vec<(i32, i32)> = Vec::new();
        for (w, h, _) in &m.modes {
            if !v.contains(&(*w, *h)) {
                v.push((*w, *h));
            }
        }
        v.sort_by_key(|(w, h)| std::cmp::Reverse((*w as i64 * *h as i64, *w)));
        v.truncate(10);
        if !v.contains(&(m.w, m.h)) {
            v.push((m.w, m.h));
        }
        v
    }

    /// Its refresh rates at its size, highest first.
    fn rates(&self) -> Vec<i32> {
        let Some(m) = self.sel_mon() else { return Vec::new() };
        let mut v: Vec<i32> = m.modes.iter().filter(|x| (x.0, x.1) == (m.w, m.h)).map(|x| x.2).collect();
        if !v.contains(&m.mhz) {
            v.push(m.mhz);
        }
        v.sort_by_key(|r| std::cmp::Reverse(*r));
        v.dedup();
        v
    }

    /// Sends a `monitor NAME ...` setting for the chosen monitor; reads the state again.
    fn mon_cmd(&mut self, what: &str) {
        let Some(name) = self.sel_mon().map(|m| m.name.clone()) else { return };
        send(&format!("monitor {name} {what}"));
        self.refresh();
    }

    /// The layout's scale: every monitor fits the card.
    fn plane_xf(&self, at: Rect) -> PXf {
        if let Some(d) = self.pdrag {
            return d.px;
        }
        let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for o in &self.m.outputs {
            b = (b.0.min(o.plane[0]), b.1.min(o.plane[1]), b.2.max(o.plane[0] + o.plane[2]), b.3.max(o.plane[1] + o.plane[3]));
        }
        if b.0 == f64::MAX {
            return PXf { k: 1.0, bx: at.x as f64, by: at.y as f64 };
        }
        // Room around them for dragging one aside.
        let (bw, bh) = ((b.2 - b.0).max(1.0), (b.3 - b.1).max(1.0));
        let k = ((at.w as f64 - 40.0) / (bw * 1.25)).min((at.h as f64 - 30.0) / (bh * 1.3)).max(0.01);
        PXf { k, bx: at.x as f64 + (at.w as f64 - bw * k) / 2.0 - b.0 * k, by: at.y as f64 + (at.h as f64 - bh * k) / 2.0 - b.1 * k }
    }

    /// The chosen glass place's value `i` (0 dim, 1 blur, 2 edge), the dragged one live.
    fn pval(&self, i: usize) -> f32 {
        let sl = [Sl::PDim, Sl::PBlur, Sl::PBezel][i];
        if self.drag == Some(sl) {
            self.drag_val as f32
        } else {
            self.m.places[self.place][i]
        }
    }

    /// Tells the compositor the chosen place's glass, `i` being `v` now.
    fn send_place(&mut self, i: usize, v: f32) {
        self.m.places[self.place][i] = v;
        let [dim, blur, bezel] = self.m.places[self.place];
        send(&format!("glass-place {} {dim} {blur} {bezel}", crate::look::PLACES[self.place].key()));
    }

    fn devices(&self, input: bool) -> &[crate::pulse::Device] {
        if input { &self.audio.sources } else { &self.audio.sinks }
    }

    /// Bluetooth devices: paired (`others` false) or found nearby.
    fn bt_devs(&self, others: bool) -> Vec<crate::bt::Dev> {
        self.bt.as_ref().map_or(Vec::new(), |b| b.state.devices.iter().filter(|d| d.paired != others).cloned().collect())
    }

    fn ui_scale(&self) -> f64 {
        if self.drag == Some(Sl::UiScale) { self.drag_val } else { self.m.ui_scale }
    }

    fn blur(&self) -> f32 {
        if self.drag == Some(Sl::Blur) { self.drag_val as f32 } else { self.m.blur }
    }

    fn bezel(&self) -> f32 {
        if self.drag == Some(Sl::Bezel) { self.drag_val as f32 } else { self.m.bezel }
    }

    fn win_bezel(&self) -> f32 {
        if self.drag == Some(Sl::WinBezel) { self.drag_val as f32 } else { self.m.win_bezel }
    }

    /// Where a slider's knob belongs, 0..1.
    fn slider_k(&self, sl: Sl) -> f32 {
        match sl {
            Sl::Blur => self.blur() / self.m.max_blur.max(1.0),
            Sl::Bezel => self.bezel() / self.m.max_bezel.max(1.0),
            Sl::WinBezel => self.win_bezel() / self.m.max_bezel.max(1.0),
            Sl::UiScale => ((self.ui_scale() - UI_MIN) / (UI_MAX - UI_MIN)) as f32,
            Sl::OutVol | Sl::InVol => self.vol(sl == Sl::InVol) / crate::pulse::MAX_PERCENT as f32,
            Sl::PDim => self.pval(0) / crate::look::MAX_DIM,
            Sl::PBlur => self.pval(1) / crate::look::MAX_BLUR,
            Sl::PBezel => self.pval(2) / crate::look::MAX_BEZEL,
        }
        .clamp(0.0, 1.0)
    }

    /// Points every spring at what the state says now.
    fn sync_anims(&mut self) {
        self.anims.drive(AK::Switch(Sw::Frost), on(self.m.frosted), SPR_SWITCH);
        self.anims.drive(AK::Switch(Sw::Glue), on(self.m.glue), SPR_SWITCH);
        let mon = self.sel_mon().cloned().unwrap_or_default();
        for (sw, v) in [(Sw::Primary, mon.primary), (Sw::MonOn, mon.on), (Sw::Vrr, mon.vrr), (Sw::Hdr, mon.hdr), (Sw::TbAll, self.m.taskbar_all), (Sw::TbAuto, self.m.taskbar_autohide), (Sw::TbDesk, self.m.taskbar_keep_desktop), (Sw::Live, self.m.live), (Sw::LivePic, self.m.live_pic)] {
            self.anims.drive(AK::Switch(sw), on(v), SPR_SWITCH);
        }
        let bt_on = self.bt.as_ref().map_or(false, |b| b.state.powered);
        self.anims.drive(AK::Switch(Sw::BtPower), on(bt_on), SPR_SWITCH);
        for (sw, input) in [(Sw::OutMute, false), (Sw::InMute, true)] {
            let muted = self.sound_dev(input).map_or(false, |d| d.mute);
            self.anims.drive(AK::Switch(sw), on(muted), SPR_SWITCH);
        }
        for sl in [Sl::Blur, Sl::Bezel, Sl::WinBezel, Sl::UiScale, Sl::OutVol, Sl::InVol, Sl::PDim, Sl::PBlur, Sl::PBezel] {
            let k = self.slider_k(sl);
            self.anims.drive(AK::Slider(sl), k, if self.drag == Some(sl) { SPR_DRAG } else { SPR_SLIDE });
        }
        let pi = PAGES.iter().position(|p| p.0 == self.page).unwrap_or(0);
        for i in 0..PAGES.len() {
            self.anims.drive(AK::NavHot(i), on(self.hot == Some(i) && i != pi), SPR_HOVER);
        }
        if self.nav.left.target != pi as f32 {
            self.nav.go(pi as f32);
        }
        self.anims.drive(AK::Btn, on(self.hot == Some(100)), SPR_HOVER);
        let l = self.layout();
        let under = |r: Rect, ptr: Option<(f64, f64)>| ptr.map_or(false, |(x, y)| r.contains(x, y) && l.view.contains(x, y));
        for ch in ALL_CH {
            let Some(row) = l.rows.iter().find(|r| r.ctl == Ctl::Choice(ch)) else { continue };
            let hots: Vec<bool> = (0..self.opts(ch).len()).map(|i| under(dev_rect(row.at, i), self.ptr)).collect();
            for (i, hot) in hots.into_iter().enumerate() {
                self.anims.drive(AK::ChHot(ch, i), on(hot), SPR_HOVER);
            }
            let cur = self.choice(ch) as f32;
            match self.chs[ch.idx()].as_mut() {
                Some(d) if d.left.target != cur => d.go(cur),
                Some(_) => {}
                None => self.chs[ch.idx()] = Some(Drop::at(cur)),
            }
        }
        match self.page {
            Page::Wallpaper => {
                let Some(row) = l.rows.iter().find(|r| r.ctl == Ctl::Walls) else { return };
                let tiles = self.wall_tiles(row.at);
                let states: Vec<(bool, bool)> = tiles.iter().zip(&self.walls).map(|(t, (p, _))| (under(*t, self.ptr), *p == self.m.wallpaper)).collect();
                for (i, (hot, sel)) in states.into_iter().enumerate() {
                    self.anims.drive(AK::WallHot(i), on(hot), SPR_HOVER);
                    self.anims.drive(AK::WallSel(i), on(sel), SPR_SEL);
                }
            }
            Page::Sound => {
                for input in [false, true] {
                    let Some(row) = l.rows.iter().find(|r| r.ctl == Ctl::Devices(input)) else { continue };
                    let cur = self.sound_dev(input).map(|d| d.name);
                    let devs = self.devices(input);
                    let hots: Vec<bool> = (0..devs.len()).map(|i| under(dev_rect(row.at, i), self.ptr)).collect();
                    let ci = devs.iter().position(|d| Some(&d.name) == cur.as_ref());
                    for (i, hot) in hots.into_iter().enumerate() {
                        self.anims.drive(AK::DevHot(input, i), on(hot), SPR_HOVER);
                    }
                    let slot = &mut self.devs[input as usize];
                    match (ci, slot.as_mut()) {
                        (Some(ci), Some(d)) if d.left.target != ci as f32 => d.go(ci as f32),
                        (Some(ci), None) => *slot = Some(Drop::at(ci as f32)),
                        (None, _) => *slot = None,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Advances everything by `dt`; true while anything moves.
    fn step_anims(&mut self, dt: f32) -> bool {
        let mut any = self.anims.step(dt);
        any |= self.nav.step(dt);
        for d in self.devs.iter_mut().chain(self.chs.iter_mut()).flatten() {
            any |= d.step(dt);
        }
        any |= self.page_t.step(dt);
        any |= self.scroll.step(dt);
        any
    }

    fn animating(&self) -> bool {
        self.anims.moving() || self.nav.moving() || self.devs.iter().chain(self.chs.iter()).flatten().any(|d| d.moving()) || self.page_t.moving() || self.scroll.moving()
    }

    /// (name, value, description, control) of every card on the page.
    fn rows_of(&self) -> Vec<(String, String, String, Ctl)> {
        let m = &self.m;
        let mut v = Vec::new();
        if let Some(e) = &m.error {
            v.push(("Нет связи с resc-desk".into(), String::new(), e.clone(), Ctl::None));
            return v;
        }
        match self.page {
            Page::Look => {
                v.push((
                    "Размытие стекла".into(),
                    String::new(),
                    "Матовое стекло на панели задач, в меню и окнах оболочки. Выключите — останется прозрачное стекло, которое только преломляет край".into(),
                    Ctl::Switch(Sw::Frost),
                ));
                if m.frosted {
                    v.push(("Сила размытия".into(), format!("{:.0}px", self.blur()), String::new(), Ctl::Slider(Sl::Blur)));
                }
                v.push((
                    "Край стекла панелей".into(),
                    format!("{:.0}px", self.bezel()),
                    "Полоса вдоль края панели задач и меню, где стекло скругляется и преломляет то, что под ним".into(),
                    Ctl::Slider(Sl::Bezel),
                ));
                v.push((
                    "Край стекла окон".into(),
                    format!("{:.0}px", self.win_bezel()),
                    "То же у окон. У заголовка обычного окна край не шире половины его высоты".into(),
                    Ctl::Slider(Sl::WinBezel),
                ));
                v.push((
                    "Запуск программы".into(),
                    String::new(),
                    "Откуда вырастает окно новой программы. Пока программа грузится, под иконкой дышит капля стекла".into(),
                    Ctl::Choice(Ch::Launch),
                ));
                v.push(("Закрытие окна".into(), String::new(), "Как уходит закрытое окно".into(), Ctl::Choice(Ch::Close)));
            }
            Page::Glass => {
                v.push((
                    "Цвет акцента".into(),
                    format!("#{:06x}", crate::look::accent() & 0xff_ffff),
                    "Берётся из обоев, как в Liquify из обложки: средний цвет картинки, ярче и насыщеннее. В него окрашены папки, выделение и кнопки".into(),
                    Ctl::None,
                ));
                v.push((
                    "Где".into(),
                    String::new(),
                    "У каждого места со стеклом свои затемнение, размытие и край. Меню, панель задач и «Пуск» — на странице «Оформление»".into(),
                    Ctl::Choice(Ch::Place),
                ));
                v.push((
                    "Затемнение".into(),
                    format!("{:.0} %", self.pval(0) * 100.0),
                    "Насколько тёмное стекло: 0 — прозрачное, 100 — сплошной тёмный фон".into(),
                    Ctl::Slider(Sl::PDim),
                ));
                v.push(("Размытие".into(), format!("{:.0}px", self.pval(1)), "Как сильно размыто то, что под стеклом. 0 — не размыто".into(), Ctl::Slider(Sl::PBlur)));
                v.push((
                    "Край стекла".into(),
                    format!("{:.0}px", self.pval(2)),
                    "Ширина полосы вдоль края, где стекло скругляется и преломляет: чем шире, тем сильнее линза".into(),
                    Ctl::Slider(Sl::PBezel),
                ));
            }
            Page::Displays => {
                v.push((
                    "Расположение".into(),
                    String::new(),
                    "Перетащите мониторы так, как они стоят на столе; края прилипают друг к другу. Точно, по линейке — в «Калибровке» ниже".into(),
                    Ctl::Plane,
                ));
                v.push((
                    "Автоматически скрывать панель задач".into(),
                    String::new(),
                    "Панель уезжает вниз и появляется, когда курсор доходит до нижнего края экрана; развёрнутые окна занимают экран целиком. Над окном на весь экран (игрой) она открывается только клавишей Win".into(),
                    Ctl::Switch(Sw::TbAuto),
                ));
                if m.taskbar_autohide {
                    v.push((
                        "Не скрывать, пока окна не заходят на панель".into(),
                        String::new(),
                        "Панель задач уходит, только когда окно закрывает её место внизу экрана".into(),
                        Ctl::Switch(Sw::TbDesk),
                    ));
                }
                if m.mons.len() > 1 {
                    v.push(("Несколько мониторов".into(), String::new(), "То же, что Win+P".into(), Ctl::Choice(Ch::Project)));
                    v.push((
                        "Панель задач на всех мониторах".into(),
                        String::new(),
                        "Выключите — панель задач, «Пуск» и значки в углу останутся только на основном мониторе, а окна займут другие мониторы целиком".into(),
                        Ctl::Switch(Sw::TbAll),
                    ));
                }
                let sel = self.sel_mon().cloned();
                let out = sel.as_ref().and_then(|s| m.outputs.iter().find(|o| o.name == s.name));
                let desc = match out {
                    Some(o) => format!(
                        "{:.0} × {:.0} мм, {:.1}″ · {:.2} px/мм · масштаб {:.2} · позиция {:.0}, {:.0} мм",
                        o.plane[2],
                        o.plane[3],
                        o.plane[2].hypot(o.plane[3]) / 25.4,
                        o.pw as f64 / o.plane[2].max(1.0),
                        o.scale,
                        o.plane[0],
                        o.plane[1]
                    ),
                    None => "Ничего своего не показывает (выключен или повторяет другой)".into(),
                };
                v.push(("Монитор".into(), String::new(), desc, Ctl::Choice(Ch::Mon)));
                if let Some(s) = sel {
                    if m.mons.len() > 1 {
                        v.push((
                            "Основной монитор".into(),
                            String::new(),
                            "С него начинается курсор, по нему считается масштаб; в режимах Win+P «только основной» и «повторяющийся» показывается он".into(),
                            Ctl::Switch(Sw::Primary),
                        ));
                        let extend = crate::monitors::PROJECTS[m.project] == crate::monitors::Project::Extend;
                        v.push((
                            "Включён".into(),
                            String::new(),
                            if extend { String::new() } else { "Действует в режиме «Расширить»; сейчас мониторы включает Win+P".into() },
                            Ctl::Switch(Sw::MonOn),
                        ));
                    }
                    v.push(("Разрешение".into(), String::new(), String::new(), Ctl::Choice(Ch::Res)));
                    if self.rates().len() > 1 {
                        v.push(("Частота обновления".into(), String::new(), String::new(), Ctl::Choice(Ch::Hz)));
                    }
                    v.push(("Поворот".into(), String::new(), "Как монитор повёрнут на подставке: картинка поворачивается в обратную сторону".into(), Ctl::Choice(Ch::Rot)));
                    if s.vrr_cap {
                        v.push((
                            "Переменная частота (VRR)".into(),
                            String::new(),
                            "FreeSync / Adaptive-Sync: монитор ждёт кадр, а не показывает старый — меньше рывков в играх и видео".into(),
                            Ctl::Switch(Sw::Vrr),
                        ));
                    } else {
                        v.push(("Переменная частота (VRR)".into(), "нет".into(), "Этот монитор или видеокарта её не поддерживает".into(), Ctl::None));
                    }
                    if s.hdr_cap {
                        v.push((
                            "HDR".into(),
                            String::new(),
                            "10 бит на канал, BT.2020 и PQ. Обычные программы остаются такими же яркими (белый — 203 нит)".into(),
                            Ctl::Switch(Sw::Hdr),
                        ));
                    } else {
                        v.push(("HDR".into(), "нет".into(), "Монитор не сообщает о поддержке HDR (PQ) или драйвер не умеет его включать".into(), Ctl::None));
                    }
                }
                v.push((
                    "Масштаб интерфейса".into(),
                    format!("{:.0} %", self.ui_scale() * 100.0),
                    "Размер всего на основном мониторе. Остальные подстраиваются так, чтобы окно везде было одного размера в миллиметрах".into(),
                    Ctl::Slider(Sl::UiScale),
                ));
                v.push((
                    "Склеить мониторы".into(),
                    String::new(),
                    "Убрать зазоры между мониторами по горизонтали: курсор и окна переходят на соседний экран сразу, не «проваливаясь» в рамку".into(),
                    Ctl::Switch(Sw::Glue),
                ));
                v.push((
                    "Калибровка".into(),
                    String::new(),
                    "Линейки и миллиметровая сетка поверх всех мониторов: подогнать размеры по банковской карте и выровнять положение до миллиметра".into(),
                    Ctl::Button(Btn::Calibrate, "Подробно"),
                ));
            }
            Page::Wallpaper => {
                v.push((
                    "Живые обои «Частицы»".into(),
                    String::new(),
                    "2000 частиц тянутся за курсором и оставляют светящиеся следы; щелчок по рабочему столу разбрасывает их. Считается на видеокарте, перерисовывается только там, где есть следы".into(),
                    Ctl::Switch(Sw::Live),
                ));
                if m.live {
                    v.push((
                        "Поверх картинки".into(),
                        String::new(),
                        "Следы гаснут к выбранной картинке, а не к чёрному".into(),
                        Ctl::Switch(Sw::LivePic),
                    ));
                }
                v.push((
                    "Фон рабочего стола".into(),
                    String::new(),
                    "Одна картинка на все мониторы сразу, с учётом их расположения. Картинки берутся из «Изображений» (~/Pictures), /usr/share/backgrounds и /usr/share/wallpapers; JPEG и PNG".into(),
                    Ctl::Walls,
                ));
            }
            Page::Sound => {
                if self.pulse.is_none() {
                    let e = self.pulse_err.clone().unwrap_or_default();
                    v.push(("Нет связи со звуковым сервером".into(), String::new(), format!("Нужен PulseAudio или pipewire-pulse. {e}"), Ctl::None));
                    return v;
                }
                let out = self.sound_dev(false);
                v.push((
                    "Громкость".into(),
                    format!("{:.0} %", self.vol(false)),
                    out.as_ref().map_or("Нет устройства вывода".into(), |d| d.desc.clone()),
                    Ctl::Slider(Sl::OutVol),
                ));
                v.push(("Без звука".into(), String::new(), String::new(), Ctl::Switch(Sw::OutMute)));
                v.push(("Устройство вывода".into(), String::new(), String::new(), Ctl::Devices(false)));
                if !self.audio.sources.is_empty() {
                    let inp = self.sound_dev(true);
                    v.push((
                        "Микрофон".into(),
                        format!("{:.0} %", self.vol(true)),
                        inp.as_ref().map_or(String::new(), |d| d.desc.clone()),
                        Ctl::Slider(Sl::InVol),
                    ));
                    v.push(("Выключить микрофон".into(), String::new(), String::new(), Ctl::Switch(Sw::InMute)));
                    v.push(("Устройство ввода".into(), String::new(), String::new(), Ctl::Devices(true)));
                }
            }
            Page::Bluetooth => {
                let Some(st) = self.bt.as_ref().map(|b| &b.state) else { return v };
                if let Some(m) = &st.missing {
                    v.push(("Bluetooth недоступен".into(), String::new(), m.clone(), Ctl::None));
                    return v;
                }
                let desc = match (&st.error, st.powered) {
                    (Some(e), _) => e.clone(),
                    (None, true) if st.discovering => "Включён · ищу устройства рядом. Наушники переведите в режим сопряжения".into(),
                    (None, true) => "Включён".into(),
                    (None, false) => "Выключен".into(),
                };
                v.push(("Bluetooth".into(), String::new(), desc, Ctl::Switch(Sw::BtPower)));
                if st.powered {
                    if st.devices.iter().any(|d| d.paired) {
                        v.push(("Мои устройства".into(), String::new(), "Нажмите, чтобы подключить или отключить".into(), Ctl::Bt(false)));
                    }
                    let found = st.devices.iter().filter(|d| !d.paired).count();
                    let desc = if found == 0 { "Пока ничего не найдено".to_string() } else { "Нажмите, чтобы сопрячь и подключить".to_string() };
                    v.push(("Другие устройства".into(), String::new(), desc, Ctl::Bt(true)));
                }
            }
            Page::Mouse => {
                v.push((
                    "Кнопки мыши".into(),
                    String::new(),
                    "У каждой кнопки два назначения: обычное и пока зажата кнопка G-Shift (назначьте её любой кнопке). Сочетание клавиш держится, пока держится кнопка. Своё сочетание или команду можно вписать в ~/.config/resc-desk/mouse.conf: bind side 1 key ctrl+shift+t, bind extra 0 cmd firefox".into(),
                    Ctl::None,
                ));
                v.push(("Кнопка".into(), String::new(), String::new(), Ctl::Choice(Ch::MBtn)));
                v.push(("Когда".into(), String::new(), String::new(), Ctl::Choice(Ch::MLayer)));
                v.push(("Что делает".into(), String::new(), String::new(), Ctl::Choice(Ch::MAct)));
            }
            Page::About => {
                v.push((
                    "resc-desk".into(),
                    env!("CARGO_PKG_VERSION").into(),
                    format!("Свой Wayland-композитор и рабочий стол · версия {}", crate::update::pretty(crate::update::BUILD)),
                    Ctl::None,
                ));
                let (desc, label) = match &self.upd {
                    Upd::Idle => (format!("Новые версии приходят из релизов github.com/{}", crate::update::REPO), "Проверить"),
                    Upd::Busy(s) => (s.to_string(), "…"),
                    Upd::UpToDate => ("Установлена последняя версия".into(), "Проверить"),
                    Upd::Newer(r) => {
                        let notes: Vec<&str> = r.notes.lines().map(|l| l.trim().trim_start_matches("- ")).filter(|l| !l.is_empty()).take(4).collect();
                        let mut s = format!("Есть новая версия от {}", crate::update::pretty(&r.tag));
                        if !notes.is_empty() {
                            s.push_str(": ");
                            s.push_str(&notes.join(" · "));
                        }
                        (s, "Обновить")
                    }
                    Upd::Installed(t) => (format!("Версия от {} установлена. Перезапуск закроет все открытые окна", crate::update::pretty(t)), "Перезапустить"),
                    Upd::Failed(e) => (format!("Не получилось: {e}"), "Ещё раз"),
                };
                v.push(("Обновления".into(), String::new(), desc, Ctl::Button(Btn::Update, label)));
                v.push(("Отрисовка".into(), String::new(), m.renderer.clone(), Ctl::None));
                let mut s = m.session.split_whitespace();
                let (wl, x) = (s.next().unwrap_or("?"), s.next().unwrap_or("-"));
                v.push(("Сеанс".into(), String::new(), format!("Wayland: {wl} · X11: {}", if x == "-" { "нет" } else { x }), Ctl::None));
                if let Some(cpu) = read_first("/proc/cpuinfo", "model name") {
                    v.push(("Процессор".into(), String::new(), cpu, Ctl::None));
                }
                if let Some(mem) = read_first("/proc/meminfo", "MemTotal") {
                    let kb: f64 = mem.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
                    v.push(("Память".into(), String::new(), format!("{:.1} ГБ", kb / 1024.0 / 1024.0), Ctl::None));
                }
                if let Ok(k) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
                    v.push(("Ядро Linux".into(), String::new(), k.trim().to_string(), Ctl::None));
                }
            }
        }
        v
    }

    fn layout(&self) -> Layout {
        let nav = (0..PAGES.len()).map(|i| Rect::new(12, TOP + i as i32 * (NAV_H + 4), SIDE_W - 24, NAV_H)).collect();
        let cx = SIDE_W + PAD;
        let cw = (self.w - SIDE_W - 2 * PAD).clamp(200, 760);
        let view = Rect::from_edges(SIDE_W, TOP - 8, self.w, self.h);
        let mut y = TOP - self.scroll.value.round() as i32;
        let y0 = y;
        let mut rows = Vec::new();
        for (name, value, desc, ctl) in self.rows_of() {
            let side_w = match ctl {
                Ctl::Switch(_) => SWITCH_W as i32 + GAP,
                Ctl::Button(..) => BTN_W + GAP,
                _ => 0,
            };
            let text_w = cw - 2 * CARD_PAD - side_w;
            let desc = if desc.is_empty() { Vec::new() } else { crate::clipboard::wrap(&desc, false, 6, |t| font::text_width(t, 13.0) > text_w) };
            let mut h = CARD_PAD + 20 + if desc.is_empty() { 0 } else { 3 + desc.len() as i32 * DESC_LINE } + CARD_PAD;
            if let Ctl::Slider(_) = ctl {
                h += SLIDER_H - 2;
            }
            let body_y = h - CARD_PAD;
            match ctl {
                Ctl::Walls => {
                    let (cols, _, th) = self.wall_grid(cw);
                    let rows = (self.walls.len() as i32 + cols - 1) / cols;
                    h += 6 + rows * (th + TILE_LABEL + TILE_GAP) - TILE_GAP;
                }
                Ctl::Devices(input) => h += self.devices(input).len().max(1) as i32 * DEV_ROW - 6,
                Ctl::Bt(others) => h += (self.bt_devs(others).len() as i32 * DEV_ROW - 6).max(0),
                Ctl::Choice(ch) => h += self.opts(ch).len() as i32 * DEV_ROW - 6,
                Ctl::Plane => h += PLANE_H,
                _ => {}
            }
            let card = Rect::new(cx, y, cw, h);
            let at = match ctl {
                Ctl::Switch(_) => Rect::new(card.x1() - CARD_PAD - SWITCH_W as i32, card.y + (card.h - SWITCH_H as i32) / 2, SWITCH_W as i32, SWITCH_H as i32),
                Ctl::Slider(_) => Rect::new(card.x + CARD_PAD, card.y1() - CARD_PAD - SLIDER_H + 4, card.w - 2 * CARD_PAD, SLIDER_H),
                Ctl::Button(..) => Rect::new(card.x1() - CARD_PAD - BTN_W, card.y + (card.h - BTN_H) / 2, BTN_W, BTN_H),
                Ctl::Walls | Ctl::Devices(_) | Ctl::Choice(_) | Ctl::Plane | Ctl::Bt(_) => Rect::from_edges(card.x + CARD_PAD, card.y + body_y + 6, card.x1() - CARD_PAD, card.y1() - CARD_PAD),
                Ctl::None => Rect::default(),
            };
            rows.push(Row { name, value, desc, ctl, card, at });
            y += h + 10;
        }
        Layout { nav, rows, view, total: y - y0 + 16 }
    }

    fn set_page(&mut self, p: Page) {
        if self.page != p {
            self.page = p;
            self.scroll.snap(0.0);
            self.page_t.snap(0.0);
            self.page_t.to(1.0);
            // The new page's hovers and selections start at rest.
            self.anims.0.retain(|(k, _)| !matches!(k, AK::WallHot(_) | AK::WallSel(_) | AK::DevHot(..)));
            self.devs = [None, None];
        }
        // Looking for devices only while the page is open.
        if let Some(b) = &self.bt {
            b.send(crate::bt::Cmd::Scan(p == Page::Bluetooth));
        }
        match p {
            Page::Bluetooth if self.bt.is_none() => {
                let b = crate::bt::Bt::start();
                b.send(crate::bt::Cmd::Scan(true));
                self.bt = Some(b);
            }
            Page::Sound => self.sound_refresh(),
            Page::Wallpaper => self.walls_start(),
            _ => {}
        }
    }

    fn slider_at(&mut self, sl: Sl, x: f64) {
        let l = self.layout();
        let Some(row) = l.rows.iter().find(|r| r.ctl == Ctl::Slider(sl)) else { return };
        let k = ((x - row.at.x as f64) / row.at.w.max(1) as f64).clamp(0.0, 1.0);
        self.drag_val = match sl {
            Sl::Blur => {
                let step = crate::glass::BLUR_STEP as f64;
                (k * self.m.max_blur as f64 / step).round() * step
            }
            Sl::Bezel | Sl::WinBezel => {
                let step = crate::glass::BEZEL_STEP as f64;
                (k * self.m.max_bezel as f64 / step).round() * step
            }
            Sl::UiScale => ((UI_MIN + k * (UI_MAX - UI_MIN)) / UI_STEP).round() * UI_STEP,
            Sl::OutVol | Sl::InVol => (k * crate::pulse::MAX_PERCENT as f64).round(),
            Sl::PDim => (k * 20.0).round() / 20.0,
            Sl::PBlur => {
                let step = crate::glass::BLUR_STEP as f64;
                (k * crate::look::MAX_BLUR as f64 / step).round() * step
            }
            Sl::PBezel => {
                let step = crate::glass::BEZEL_STEP as f64;
                (k * crate::look::MAX_BEZEL as f64 / step).round() * step
            }
        };
        // Glass follows the slider at once.
        if let Some(i) = [Sl::PDim, Sl::PBlur, Sl::PBezel].iter().position(|s| *s == sl) {
            let v = self.drag_val as f32;
            if (self.m.places[self.place][i] - v).abs() > 1e-4 {
                self.send_place(i, v);
            }
        }
        // Volume follows the slider at once.
        if matches!(sl, Sl::OutVol | Sl::InVol) {
            let input = sl == Sl::InVol;
            let v = self.drag_val as u32;
            if let Some(dev) = self.sound_dev(input).filter(|d| d.percent() != v) {
                self.sound_op(|p, _| p.set_volume(&dev, input, v));
            }
        }
    }

    fn press(&mut self, x: f64, y: f64) {
        let l = self.layout();
        if let Some(i) = l.nav.iter().position(|r| r.contains(x, y)) {
            self.set_page(PAGES[i].0);
            return;
        }
        if !l.view.contains(x, y) {
            return;
        }
        let Some(row) = l.rows.iter().find(|r| r.card.contains(x, y)) else { return };
        match row.ctl {
            Ctl::Switch(Sw::Frost) => {
                let on = !self.m.frosted;
                self.m.frosted = on;
                send(&format!("glass {} {}", if on { "on" } else { "off" }, self.m.blur));
            }
            Ctl::Switch(Sw::Glue) => {
                let on = !self.m.glue;
                self.m.glue = on;
                send(&format!("glue {}", if on { "on" } else { "off" }));
                send("save");
                self.refresh();
            }
            Ctl::Slider(sl) if row.at.inflate(6).contains(x, y) => {
                self.drag = Some(sl);
                self.slider_at(sl, x);
            }
            Ctl::Button(Btn::Calibrate, _) if row.at.contains(x, y) => send("calibrate"),
            Ctl::Button(Btn::Update, _) if row.at.contains(x, y) => self.upd_press(),
            Ctl::Switch(sw @ (Sw::OutMute | Sw::InMute)) => {
                let input = sw == Sw::InMute;
                if let Some(dev) = self.sound_dev(input) {
                    self.sound_op(|p, _| p.set_mute(&dev, input, !dev.mute));
                }
            }
            Ctl::Choice(ch) => {
                let i = ((y - row.at.y as f64) / DEV_ROW as f64).floor();
                if i >= 0.0 && (i as usize) < self.opts(ch).len() {
                    let i = i as usize;
                    match ch {
                        Ch::Launch => self.m.anim_launch = i,
                        Ch::Close => self.m.anim_close = i,
                        Ch::Place => self.place = i,
                        Ch::MBtn => self.mbtn = i,
                        Ch::MLayer => self.mlayer = i,
                        Ch::MAct => {
                            if let (Some((_, spec)), Some(b)) = (MOUSE_ACTS.get(i), crate::mousebind::BUTTONS.get(self.mbtn)) {
                                send(&format!("mouse-bind {} {} {spec}", b.0, self.mlayer));
                                self.refresh();
                            }
                        }
                        Ch::Project => {
                            self.m.project = i;
                            send(&format!("project {}", crate::monitors::PROJECTS[i].key()));
                            self.refresh();
                        }
                        Ch::Mon => {
                            if let Some(m) = self.m.mons.get(i) {
                                self.mon = m.name.clone();
                            }
                        }
                        Ch::Res => {
                            let (w, h) = self.sizes()[i];
                            let Some(m) = self.sel_mon() else { return };
                            // The same refresh if it has it at that size, else the highest.
                            let rates: Vec<i32> = m.modes.iter().filter(|x| (x.0, x.1) == (w, h)).map(|x| x.2).collect();
                            let r = if rates.contains(&m.mhz) { m.mhz } else { rates.iter().copied().max().unwrap_or(m.mhz) };
                            self.mon_cmd(&format!("mode {w}x{h}@{r}"));
                        }
                        Ch::Hz => {
                            let r = self.rates()[i];
                            let Some((w, h)) = self.sel_mon().map(|m| (m.w, m.h)) else { return };
                            self.mon_cmd(&format!("mode {w}x{h}@{r}"));
                        }
                        Ch::Rot => self.mon_cmd(&format!("rot {}", i * 90)),
                    }
                    if matches!(ch, Ch::Launch | Ch::Close) {
                        send(&format!("anim {} {i}", if ch == Ch::Launch { "launch" } else { "close" }));
                    }
                }
            }
            Ctl::Switch(Sw::Live) => {
                self.m.live = !self.m.live;
                send(&format!("livewall {}", if !self.m.live { "off" } else if self.m.live_pic { "picture" } else { "on" }));
                self.refresh();
            }
            Ctl::Switch(Sw::LivePic) => {
                self.m.live_pic = !self.m.live_pic;
                send(&format!("livewall {}", if self.m.live_pic { "picture" } else { "on" }));
                self.refresh();
            }
            Ctl::Switch(Sw::TbDesk) => {
                self.m.taskbar_keep_desktop = !self.m.taskbar_keep_desktop;
                send(&format!("taskbar-desktop {}", if self.m.taskbar_keep_desktop { "on" } else { "off" }));
                self.refresh();
            }
            Ctl::Switch(Sw::TbAuto) => {
                self.m.taskbar_autohide = !self.m.taskbar_autohide;
                send(&format!("taskbar-autohide {}", if self.m.taskbar_autohide { "on" } else { "off" }));
                self.refresh();
            }
            Ctl::Switch(Sw::TbAll) => {
                self.m.taskbar_all = !self.m.taskbar_all;
                send(&format!("taskbar {}", if self.m.taskbar_all { "all" } else { "main" }));
                self.refresh();
            }
            Ctl::Switch(sw @ (Sw::Primary | Sw::MonOn | Sw::Vrr | Sw::Hdr)) => {
                let Some(m) = self.sel_mon().cloned() else { return };
                match sw {
                    Sw::Primary if !m.primary => {
                        send(&format!("primary {}", m.name));
                        self.refresh();
                    }
                    Sw::MonOn => self.mon_cmd(if m.on && m.mirror.is_none() { "off" } else { "on" }),
                    Sw::Vrr => self.mon_cmd(if m.vrr { "vrr off" } else { "vrr on" }),
                    Sw::Hdr => self.mon_cmd(if m.hdr { "hdr off" } else { "hdr on" }),
                    _ => {}
                }
            }
            Ctl::Plane => {
                let px = self.plane_xf(row.at);
                let hit = self.m.outputs.iter().rposition(|o| px.r(o.plane).contains(x, y));
                if let Some(i) = hit {
                    let o = &self.m.outputs[i];
                    let (mx, my) = px.mm(x, y);
                    self.mon = o.name.clone();
                    self.pdrag = Some(PDrag { i, grab: (mx - o.plane[0], my - o.plane[1]), px, moved: false });
                }
            }
            Ctl::Switch(Sw::BtPower) => {
                if let Some(b) = &self.bt {
                    b.send(crate::bt::Cmd::Power(!b.state.powered));
                }
            }
            Ctl::Bt(others) => {
                let i = ((y - row.at.y as f64) / DEV_ROW as f64).floor();
                let dev = if i >= 0.0 { self.bt_devs(others).get(i as usize).cloned() } else { None };
                if let (Some(d), Some(b)) = (dev, &self.bt) {
                    use crate::bt::Cmd;
                    let r = dev_rect(row.at, i as usize);
                    let busy = b.state.busy.is_some();
                    if !others && x >= (r.x1() - BT_FORGET_W) as f64 {
                        b.send(Cmd::Forget(d.path));
                    } else if busy {
                        // One at a time.
                    } else if others {
                        b.send(Cmd::Pair(d.path));
                    } else if d.connected {
                        b.send(Cmd::Disconnect(d.path));
                    } else {
                        b.send(Cmd::Connect(d.path));
                    }
                }
            }
            Ctl::Devices(input) => {
                let i = ((y - row.at.y as f64) / DEV_ROW as f64).floor();
                if i >= 0.0 {
                    if let Some(dev) = self.devices(input).get(i as usize).cloned() {
                        self.sound_op(|p, _| p.set_default(&dev.name, input));
                    }
                }
            }
            Ctl::Walls => {
                let tiles = self.wall_tiles(row.at);
                if let Some(i) = tiles.iter().position(|t| t.inflate(4).contains(x, y)) {
                    let p = self.walls[i].0.clone();
                    match crate::ipc::request(&format!("wallpaper {}", p.as_deref().unwrap_or("default"))) {
                        Ok(r) if r.contains("err ") => eprintln!("settings: {r}"),
                        Err(e) => eprintln!("settings: {e}"),
                        _ => {}
                    }
                    self.m.wallpaper = p;
                }
            }
            _ => {}
        }
    }

    fn release(&mut self) {
        if let Some(d) = self.pdrag.take() {
            if d.moved {
                if let Some(o) = self.m.outputs.get(d.i) {
                    let p = o.plane;
                    send(&format!("set-plane {} {:.1} {:.1} {:.2} {:.2}", o.name, p[0], p[1], p[2], p[3]));
                    send("save");
                }
                self.refresh();
            }
            return;
        }
        match self.drag.take() {
            Some(Sl::Blur) => {
                if (self.drag_val as f32 - self.m.blur).abs() > 0.01 {
                    self.m.blur = self.drag_val as f32;
                    send(&format!("glass {} {}", if self.m.frosted { "on" } else { "off" }, self.m.blur));
                }
            }
            Some(sl @ (Sl::Bezel | Sl::WinBezel)) => {
                let v = self.drag_val as f32;
                let cur = if sl == Sl::Bezel { &mut self.m.bezel } else { &mut self.m.win_bezel };
                if (v - *cur).abs() > 0.01 {
                    *cur = v;
                    send(&format!("bezel {} {}", self.m.bezel, self.m.win_bezel));
                }
            }
            Some(Sl::OutVol | Sl::InVol) => self.sound_refresh(),
            Some(Sl::PDim | Sl::PBlur | Sl::PBezel) => {}
            Some(Sl::UiScale) => {
                if (self.drag_val - self.m.ui_scale).abs() > 1e-9 {
                    self.m.ui_scale = self.drag_val;
                    send(&format!("ui-scale {}", self.drag_val));
                    send("save");
                    self.refresh();
                }
            }
            None => {}
        }
    }

    fn motion(&mut self, x: f64, y: f64) {
        self.ptr = Some((x, y));
        if let Some(d) = self.pdrag.as_mut() {
            let (mx, my) = d.px.mm(x, y);
            let i = d.i;
            let tol = 16.0 / d.px.k;
            d.moved = true;
            let Some(p) = self.m.outputs.get(i).map(|o| o.plane) else { return };
            let others: Vec<[f64; 4]> = self.m.outputs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.plane).collect();
            let want = [mx - d.grab.0, my - d.grab.1, p[2], p[3]];
            self.m.outputs[i].plane = snap_plane(want, &others, tol);
            return;
        }
        if let Some(sl) = self.drag {
            self.slider_at(sl, x);
            return;
        }
        let l = self.layout();
        self.hot = l.nav.iter().position(|r| r.contains(x, y)).or_else(|| {
            l.rows.iter().any(|r| matches!(r.ctl, Ctl::Button(..)) && r.at.contains(x, y) && l.view.contains(x, y)).then_some(100)
        });
    }

    fn wheel(&mut self, v: f64) {
        let l = self.layout();
        let max = (l.total - l.view.h).max(0) as f32;
        let t = (self.scroll.target + (v * 4.0) as f32).clamp(0.0, max);
        self.scroll.to(t.round());
    }

    fn refresh(&mut self) {
        self.m = fetch();
    }
}

/// `resc-desk settings [look|displays|about]`.
pub fn run(args: &[String]) -> i32 {
    let page = match args.first().map(String::as_str) {
        Some("displays") | Some("monitors") => Page::Displays,
        Some("about") => Page::About,
        Some("sound") => Page::Sound,
        Some("bluetooth") | Some("bt") => Page::Bluetooth,
        Some("wallpaper") | Some("wallpapers") => Page::Wallpaper,
        Some("glass") => Page::Glass,
        Some("mouse") => Page::Mouse,
        _ => Page::Look,
    };
    crate::font::init();
    let mut win = match Window::open("Параметры", "resc.settings", 900, 620, (640, 420)) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("settings: {e}");
            return 1;
        }
    };
    let mut app = App {
        m: Model::default(),
        page,
        scroll: Spring::new(0.0, 0.22, 1.0),
        anims: Anims::default(),
        nav: Drop::at(PAGES.iter().position(|p| p.0 == page).unwrap_or(0) as f32),
        devs: [None, None],
        chs: [None; 11],
        place: 0,
        mbtn: 0,
        mlayer: 0,
        mon: String::new(),
        pdrag: None,
        upd: Upd::Idle,
        upd_rx: None,
        page_t: Spring::new(1.0, 0.32, 1.0),
        drag: None,
        drag_val: 0.0,
        hot: None,
        ptr: None,
        w: win.w,
        h: win.h,
        walls: Vec::new(),
        thumbs: None,
        pulse: None,
        audio: Default::default(),
        pulse_err: None,
        bt: None,
    };
    app.refresh();
    let first = app.page;
    app.page = Page::About;
    app.set_page(first);
    let mut dirty = true;
    let mut last = sys::now_ns();
    let mut last_state = last;
    app.sync_anims();
    loop {
        let animating = app.animating();
        // Thumbnails coming in, or sound changing elsewhere: look again soon.
        let wait = if animating {
            16
        } else if app.thumbs.is_some() && app.page == Page::Wallpaper {
            50
        } else if app.upd_rx.is_some() {
            200
        } else if app.page == Page::Sound || app.page == Page::Bluetooth {
            300
        } else {
            1000
        };
        let evs = match win.poll(wait) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("settings: {e}");
                return 1;
            }
        };
        for ev in evs {
            dirty = true;
            match ev {
                Ev::Resized => {
                    app.w = win.w;
                    app.h = win.h;
                    app.wheel(0.0);
                }
                Ev::Close => return 0,
                Ev::Motion(x, y) => {
                    if app.ptr.is_none() {
                        app.refresh();
                    }
                    app.motion(x, y);
                }
                Ev::Leave => {
                    app.ptr = None;
                    app.hot = None;
                }
                Ev::Button(0x110, true) => {
                    if let Some((x, y)) = app.ptr {
                        app.press(x, y);
                    }
                }
                Ev::Button(0x110, false) => app.release(),
                Ev::Button(..) => {}
                Ev::Wheel(v) => app.wheel(v),
                Ev::Key(k, true) => {
                    let i = PAGES.iter().position(|p| p.0 == app.page).unwrap_or(0);
                    match k {
                        KEY_UP if i > 0 => app.set_page(PAGES[i - 1].0),
                        KEY_DOWN if i + 1 < PAGES.len() => app.set_page(PAGES[i + 1].0),
                        _ => {}
                    }
                }
                Ev::Key(..) | Ev::Mods(_) => {}
                _ => {}
            }
        }
        if app.upd_poll() {
            dirty = true;
        }
        if app.walls_poll() && app.page == Page::Wallpaper {
            dirty = true;
        }
        if crate::look::poll() {
            crate::icons::reset();
            dirty = true;
        }
        if app.page == Page::Sound && app.sound_poll() {
            dirty = true;
        }
        if app.bt.as_mut().map_or(false, |b| b.poll()) && app.page == Page::Bluetooth {
            dirty = true;
        }
        // Monitors change elsewhere too (Win+P, plugged in): read them again now and then.
        if app.page == Page::Displays && app.pdrag.is_none() && app.drag.is_none() && sys::now_ns() - last_state > 1_000_000_000 {
            last_state = sys::now_ns();
            let (outs, mons, project) = (app.m.outputs.len(), app.m.mons.len(), app.m.project);
            let before: Vec<_> = app.m.mons.iter().map(|m| (m.name.clone(), m.on, m.w, m.h, m.mhz, m.rot, m.primary, m.mirror.clone())).collect();
            let planes: Vec<[f64; 4]> = app.m.outputs.iter().map(|o| o.plane).collect();
            app.refresh();
            let after: Vec<_> = app.m.mons.iter().map(|m| (m.name.clone(), m.on, m.w, m.h, m.mhz, m.rot, m.primary, m.mirror.clone())).collect();
            let planes2: Vec<[f64; 4]> = app.m.outputs.iter().map(|o| o.plane).collect();
            if outs != app.m.outputs.len() || mons != app.m.mons.len() || project != app.m.project || before != after || planes != planes2 {
                dirty = true;
            }
        }
        let now = sys::now_ns();
        let dt = ((now - last) as f64 / 1e9).min(0.05) as f32;
        last = now;
        app.sync_anims();
        if app.step_anims(dt) {
            dirty = true;
        }
        if dirty && win.ready() {
            win.draw(|cv| draw(&app, cv));
            dirty = false;
        }
    }
}

// ------------------------------------------------------------------ drawing

/// Straight-alpha color → premultiplied f32 RGBA.
fn rgba(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    [r * a, g * a, b * a, a]
}

fn white(a: f32) -> [f32; 4] {
    rgba(1.0, 1.0, 1.0, a)
}

fn pack(c: [f32; 4], k: f32) -> u32 {
    let q = |v: f32| ((v * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(c[3]) << 24) | (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])
}

/// erf (Abramowitz–Stegun 7.1.26), for box shadows.
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let y = 1.0 - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_7) * t - 0.284_496_74) * t + 0.254_829_6) * t * (-x * x).exp();
    y.copysign(x)
}

/// A box in physical px (floats), Liquify's continuous corners.
#[derive(Clone, Copy)]
struct FBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
}

impl FBox {
    fn sd(&self, px: f32, py: f32) -> f32 {
        let hb = (self.w * 0.5, self.h * 0.5);
        sd_squircle((px - self.x - hb.0, py - self.y - hb.1), hb, self.r)
    }
    fn bounds(&self, grow: f32) -> Rect {
        Rect::from_edges((self.x - grow).floor() as i32, (self.y - grow).floor() as i32, (self.x + self.w + grow).ceil() as i32, (self.y + self.h + grow).ceil() as i32)
    }
}

/// kit.rs BoxStyle as the UI raster draws it: drop shadow (dy, CSS blur, color), fill,
/// CSS `inset 1px 1px 0 white(highlight)`, hairline border.
struct Look {
    fill: [f32; 4],
    shadow: Option<(f32, f32, [f32; 4])>,
    highlight: f32,
    border: Option<(f32, [f32; 4])>,
}

fn draw_box(cv: &mut Canvas, clip: Rect, b: FBox, look: &Look, s: f32) {
    let grow = look.shadow.map_or(1.0, |(dy, blur, _)| dy.abs() + blur * 1.5 + 1.0);
    let area = b.bounds(grow).intersect(&clip).intersect(&cv.bounds());
    for y in area.y..area.y1() {
        let row = y as usize * cv.stride;
        for x in area.x..area.x1() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let sd = b.sd(px, py);
            let dst = &mut cv.px[row + x as usize];
            if let Some((dy, blur, c)) = look.shadow {
                // CSS blur radius B: a gaussian of sigma B / 2.
                let sigma = (blur * 0.5).max(0.5);
                let ss = b.sd(px, py - dy);
                let a = 0.5 * (1.0 - erf(ss / (sigma * std::f32::consts::SQRT_2)));
                let cover = (0.5 - sd).clamp(0.0, 1.0);
                let k = a * (1.0 - cover);
                if k > 0.002 {
                    *dst = blend(*dst, pack(c, k));
                }
            }
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            *dst = blend(*dst, pack(look.fill, cov));
            if look.highlight > 0.0 {
                let sd2 = b.sd(px - s, py - s);
                let band = 1.0 - (0.5 - sd2).clamp(0.0, 1.0);
                let k = look.highlight * band * cov;
                if k > 0.0 {
                    *dst = blend(*dst, pack(white(1.0), k));
                }
            }
            if let Some((bw, c)) = look.border {
                let inner = (0.5 - (sd + bw)).clamp(0.0, 1.0);
                let k = cov * (1.0 - inner);
                if k > 0.0 {
                    *dst = blend(*dst, pack(c, k));
                }
            }
        }
    }
}

/// kit.rs `glass_deep(14)` over the panel's glass: tint rgba(18, 18, 22, 0.55), hairline
/// border white(0.1), inset highlight 0.12.
fn card(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect) {
    let p = xf.r(r);
    let s = xf.scale as f32;
    let look = Look {
        fill: white(0.05),
        shadow: None,
        highlight: 0.08,
        border: Some((s, white(0.09))),
    };
    draw_box(cv, clip, FBox { x: p.x as f32, y: p.y as f32, w: p.w as f32, h: p.h as f32, r: 14.0 * s }, &look, s);
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// settings.rs `switch(t, v)`: at `t` (0 off .. 1 on) moving at `v` per second, its top-left
/// at logical (`x`, `y`).
pub(crate) fn switch(cv: &mut Canvas, clip: Rect, xf: Xf, x: f32, y: f32, t: f32, v: f32) {
    let s = xf.scale as f32;
    let (ox, oy) = (xf.fx(x as f64) as f32, xf.fy(y as f64) as f32);
    let accent = { let a = accent_rgb(); rgba(a[0], a[1], a[2], 1.0) };
    let track = Look { fill: mix(white(0.16), accent, t), shadow: None, highlight: 0.0, border: None };
    draw_box(cv, clip, FBox { x: ox, y: oy, w: SWITCH_W * s, h: SWITCH_H * s, r: 13.0 * s }, &track, s);
    let stretch = (v.abs() * 1.6).min(9.0);
    let (kw, kh) = (KNOB + stretch, KNOB - stretch * 0.3);
    let cx = 3.0 + KNOB * 0.5 + t * (SWITCH_W - 6.0 - KNOB);
    let kx = (cx - kw * 0.5).clamp(3.0 - stretch * 0.25, SWITCH_W - 3.0 - kw + stretch * 0.25);
    let knob = Look { fill: white(1.0), shadow: Some((1.0 * s, 4.0 * s, rgba(0.0, 0.0, 0.0, 0.25))), highlight: 0.0, border: None };
    let b = FBox { x: ox + kx * s, y: oy + (SWITCH_H - kh) * 0.5 * s, w: kw * s, h: kh * s, r: kh * 0.5 * s };
    draw_box(cv, clip, b, &knob, s);
}

/// settings.rs `slider_card`'s bar: track white(0.12), the filled part in the accent, an
/// 18 px knob with its shadow.
pub(crate) fn slider(cv: &mut Canvas, clip: Rect, xf: Xf, bar: Rect, k: f32) {
    let s = xf.scale as f32;
    let k = k.clamp(0.0, 1.0);
    let (ox, oy) = (xf.fx(bar.x as f64) as f32, xf.fy(bar.y as f64) as f32);
    let iw = bar.w as f32;
    let ty = (SLIDER_H as f32 - 4.0) * 0.5;
    let flat = |fill| Look { fill, shadow: None, highlight: 0.0, border: None };
    draw_box(cv, clip, FBox { x: ox, y: oy + ty * s, w: iw * s, h: 4.0 * s, r: 2.0 * s }, &flat(white(0.12)), s);
    if k > 0.0 {
        let accent = { let a = accent_rgb(); rgba(a[0], a[1], a[2], 1.0) };
        draw_box(cv, clip, FBox { x: ox, y: oy + ty * s, w: iw * k * s, h: 4.0 * s, r: 2.0 * s }, &flat(accent), s);
    }
    let kx = (iw * k - SLIDER_KNOB * 0.5).clamp(0.0, iw - SLIDER_KNOB);
    let knob = Look { fill: white(1.0), shadow: Some((2.0 * s, 8.0 * s, rgba(0.0, 0.0, 0.0, 0.35))), highlight: 0.0, border: None };
    let b = FBox { x: ox + kx * s, y: oy + (SLIDER_H as f32 - SLIDER_KNOB) * 0.5 * s, w: SLIDER_KNOB * s, h: SLIDER_KNOB * s, r: SLIDER_KNOB * 0.5 * s };
    draw_box(cv, clip, b, &knob, s);
}

fn text_color(a: f32) -> u32 {
    crate::render::argb((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32, 255, 255, 255)
}

/// Between two straight-alpha ARGB colours.
fn lerp_argb(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xff, (b >> s) & 0xff);
        ((x as f32 + (y as f32 - x as f32) * t).round() as u32) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// A small pill button (Liquify `.btn`): white(0.08) fill, hairline, white(0.16) on hover.
pub(crate) fn button_pub(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, label: &str, hot: bool) {
    button(cv, clip, xf, r, label, on(hot))
}

/// `hot`: 0 .. 1, the hover fading in.
fn button(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, label: &str, hot: f32) {
    let p = xf.r(r);
    let s = xf.scale as f32;
    let look = Look { fill: white(0.08 + 0.08 * hot.clamp(0.0, 1.0)), shadow: None, highlight: 0.0, border: Some((s, white(0.12))) };
    draw_box(cv, clip, FBox { x: p.x as f32, y: p.y as f32, w: p.w as f32, h: p.h as f32, r: r.h as f32 * 0.5 * s }, &look, s);
    let tw = font::text_width_w(label, 13.0, font::W::Semi);
    font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + 21, label, 13.0, text_color(1.0), r.w, font::W::Semi);
}

/// The window's background: see-through, so the compositor's glass under the window shows
/// (a light violet-black veil with the accent glowing from the top left). Without glass the
/// compositor puts its solid frame colour under it.
fn background(cv: &mut Canvas, w: i32, h: i32) {
    let ac = accent_rgb();
    let (gx, gy, gr) = (w as f32 * 0.15, -(h as f32) * 0.1, (w.max(h) as f32) * 0.9);
    for y in 0..h {
        let row = y as usize * cv.stride;
        let t = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let dx = x as f32 - gx;
            let dy = y as f32 - gy;
            let g = (1.0 - (dx * dx + dy * dy).sqrt() / gr).max(0.0);
            let g = g * g * 0.16;
            // Premultiplied: a dark veil of alpha `a` plus the accent glow.
            // (The window's darkness is its glass's: «Стекло» → «Параметры».)
            let a = 0.10 + 0.06 * t + g;
            let r = 14.0 * 0.10 + g * ac[0] * 255.0;
            let gg = 12.0 * 0.10 + g * ac[1] * 255.0;
            let b = 20.0 * 0.10 + g * ac[2] * 255.0;
            let q = |v: f32| (v.round() as u32).min(255);
            cv.px[row + x as usize] = q(a * 255.0) << 24 | q(r) << 16 | q(gg) << 8 | q(b);
        }
    }
    // The page list sits on a slightly lighter strip.
    for y in 0..h {
        let row = y as usize * cv.stride;
        for x in 0..SIDE_W.min(w) {
            let p = &mut cv.px[row + x as usize];
            *p = blend(*p, argb(10, 255, 255, 255));
        }
    }
}

fn draw(app: &App, cv: &mut Canvas) {
    let xf = Xf::new(0, 0, 1.0);
    let clip = cv.bounds();
    background(cv, app.w, app.h);
    let l = app.layout();
    font::draw_text_w(cv, clip, xf, 24, 50, "Параметры", 22.0, text_color(1.0), SIDE_W - 30, font::W::Bold);
    for (i, r) in l.nav.iter().enumerate() {
        let hot = app.anims.get(AK::NavHot(i));
        if hot > 0.01 {
            cv.rrect(clip, xf.r(*r), xf.len(10), argb((16.0 * hot).round() as u32, 255, 255, 255));
        }
    }
    // The selection: a drop flowing from page to page, the accent bar inside it.
    if let Some(n0) = l.nav.first() {
        let step = (NAV_H + 4) as f32;
        let y0 = n0.y as f32 + app.nav.left.value * step;
        let y1 = n0.y as f32 + app.nav.right.value * step - 4.0;
        let sel = Rect::from_edges(n0.x, y0.round() as i32, n0.x1(), y1.round() as i32);
        if sel.h > 0 {
            cv.rrect(clip, xf.r(sel), xf.len(10), crate::look::accent_a(34));
            let bar = (sel.h - 22).max(4);
            cv.rrect(clip, xf.r(Rect::new(sel.x, sel.y + (sel.h - bar) / 2, 3, bar)), xf.len(1), crate::look::accent());
        }
    }
    for (i, r) in l.nav.iter().enumerate() {
        let (_, name, icon) = PAGES[i];
        let cover = app.nav.cover(i);
        let ic = Rect::new(r.x + 12, r.y + 10, 20, 20);
        crate::shell::draw_app_tile(cv, clip, xf, ic, icon, name);
        font::draw_text_w(cv, clip, xf, ic.x1() + 12, r.y + 25, name, 13.5, text_color(0.8 + 0.2 * cover), r.w - 50, font::W::Semi);
    }

    // A page coming in: drawn aside, then faded in rising a little.
    let t = app.page_t.value.clamp(0.0, 1.0);
    if t >= 0.999 {
        draw_page(app, cv, &l);
        return;
    }
    let (w, h) = (cv.w, cv.h);
    let mut buf = vec![0u32; (w * h).max(0) as usize];
    {
        let mut off = Canvas { px: &mut buf, w, h, stride: w as usize };
        draw_page(app, &mut off, &l);
    }
    let e = 1.0 - t;
    let dy = (e * e * 24.0).round() as i32;
    let alpha = (t * 255.0).round() as u32;
    let area = Rect::from_edges(SIDE_W, 0, w, h);
    cv.blit(area, Rect::new(0, dy, w, h), &buf, w as usize, Rect::new(0, 0, w, h), false, alpha);
}

/// The page's title, then the cards (scrolling, clipped to the view), then the scroll bar.
fn draw_page(app: &App, cv: &mut Canvas, l: &Layout) {
    let xf = Xf::new(0, 0, 1.0);
    let clip = cv.bounds();
    let title = PAGES.iter().find(|p| p.0 == app.page).map_or("", |p| p.1);
    font::draw_text_w(cv, clip, xf, SIDE_W + PAD, 50, title, 26.0, text_color(1.0), app.w - SIDE_W - 2 * PAD, font::W::Bold);
    let vclip = clip.intersect(&l.view);
    for row in &l.rows {
        if !row.card.inflate(2).overlaps(&vclip) {
            continue;
        }
        let c = row.card;
        card(cv, vclip, xf, c);
        let tx = c.x + CARD_PAD;
        let side_w = match row.ctl {
            Ctl::Switch(_) => SWITCH_W as i32 + GAP,
            Ctl::Button(..) => BTN_W + GAP,
            _ => 0,
        };
        let text_w = c.w - 2 * CARD_PAD - side_w;
        font::draw_text_w(cv, vclip, xf, tx, c.y + CARD_PAD + 15, &row.name, 15.0, text_color(1.0), text_w, font::W::Semi);
        if !row.value.is_empty() {
            let vw = font::text_width_w(&row.value, 14.0, font::W::Bold);
            font::draw_text_w(cv, vclip, xf, c.x1() - CARD_PAD - vw, c.y + CARD_PAD + 15, &row.value, 14.0, crate::look::accent(), vw + 4, font::W::Bold);
        }
        for (i, line) in row.desc.iter().enumerate() {
            font::draw_text(cv, vclip, xf, tx, c.y + CARD_PAD + 20 + 3 + 13 + i as i32 * DESC_LINE, line, 13.0, text_color(0.55), text_w);
        }
        match row.ctl {
            Ctl::Switch(sw) => {
                let sp = app.anims.spring(AK::Switch(sw));
                switch(cv, vclip, xf, row.at.x as f32, row.at.y as f32, sp.value, sp.velocity);
            }
            Ctl::Choice(ch) => {
                let opts = app.opts(ch);
                for i in 0..opts.len() {
                    let hot = app.anims.get(AK::ChHot(ch, i));
                    if hot > 0.01 {
                        cv.rrect(vclip, xf.r(dev_rect(row.at, i)), xf.len(10), argb((14.0 * hot).round() as u32, 255, 255, 255));
                    }
                }
                let drop = app.chs[ch.idx()];
                if let Some(d) = drop {
                    let r0 = dev_rect(row.at, 0);
                    let y0 = row.at.y as f32 + d.left.value * DEV_ROW as f32;
                    let y1 = row.at.y as f32 + d.right.value * DEV_ROW as f32 - 4.0;
                    let sel = Rect::from_edges(r0.x, y0.round() as i32, r0.x1(), y1.round() as i32);
                    if sel.h > 0 {
                        cv.rrect(vclip, xf.r(sel), xf.len(10), crate::look::accent_a(30));
                    }
                }
                for (i, name) in opts.iter().enumerate() {
                    let r = dev_rect(row.at, i);
                    let c = drop.map_or(0.0, |d| d.cover(i));
                    let (cx, cy) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
                    cv.ring(vclip, crate::shell::phys(xf, cx, cy), 7.0, 1.5, lerp_argb(text_color(0.5), crate::look::accent(), c));
                    let dot = (7.0 * c).round() as i32;
                    if dot >= 1 {
                        cv.rrect(vclip, xf.r(Rect::new(cx as i32 - dot / 2, cy as i32 - dot / 2, dot, dot)), xf.len((dot + 1) / 2), crate::look::accent());
                    }
                    let name = crate::shell::ellipsize(name, 13.5, font::W::Semi, r.w - 60);
                    font::draw_text_w(cv, vclip, xf, r.x + 38, r.y + 23, &name, 13.5, text_color(0.8 + 0.2 * c), r.w - 60, font::W::Semi);
                }
            }
            Ctl::Bt(others) => draw_bt(app, cv, vclip, xf, row.at, others),
            Ctl::Devices(input) => {
                let devs = app.devices(input);
                for i in 0..devs.len() {
                    let hot = app.anims.get(AK::DevHot(input, i));
                    if hot > 0.01 {
                        cv.rrect(vclip, xf.r(dev_rect(row.at, i)), xf.len(10), argb((14.0 * hot).round() as u32, 255, 255, 255));
                    }
                }
                let drop = app.devs[input as usize];
                if let Some(d) = drop {
                    let r0 = dev_rect(row.at, 0);
                    let y0 = row.at.y as f32 + d.left.value * DEV_ROW as f32;
                    let y1 = row.at.y as f32 + d.right.value * DEV_ROW as f32 - 4.0;
                    let sel = Rect::from_edges(r0.x, y0.round() as i32, r0.x1(), y1.round() as i32);
                    if sel.h > 0 {
                        cv.rrect(vclip, xf.r(sel), xf.len(10), crate::look::accent_a(30));
                    }
                }
                for (i, dev) in devs.iter().enumerate() {
                    let r = dev_rect(row.at, i);
                    let c = drop.map_or(0.0, |d| d.cover(i));
                    let (cx, cy) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
                    cv.ring(vclip, crate::shell::phys(xf, cx, cy), 7.0, 1.5, lerp_argb(text_color(0.5), crate::look::accent(), c));
                    let dot = (7.0 * c).round() as i32;
                    if dot >= 1 {
                        cv.rrect(vclip, xf.r(Rect::new(cx as i32 - dot / 2, cy as i32 - dot / 2, dot, dot)), xf.len((dot + 1) / 2), crate::look::accent());
                    }
                    let name = crate::shell::ellipsize(&dev.desc, 13.5, font::W::Semi, r.w - 60);
                    font::draw_text_w(cv, vclip, xf, r.x + 38, r.y + 23, &name, 13.5, text_color(0.8 + 0.2 * c), r.w - 60, font::W::Semi);
                }
            }
            Ctl::Walls => {
                let tiles = app.wall_tiles(row.at);
                for (i, (t, (path, thumb))) in tiles.iter().zip(&app.walls).enumerate() {
                    if !t.inflate(4).overlaps(&vclip) {
                        continue;
                    }
                    let sel = app.anims.get(AK::WallSel(i));
                    let hot = app.anims.get(AK::WallHot(i));
                    match thumb {
                        Some(px) => cv.blit(vclip, xf.r(*t), px, TW, Rect::new(0, 0, TW as i32, TH as i32), true, 255),
                        None => cv.rrect(vclip, xf.r(*t), xf.len(8), argb(24, 255, 255, 255)),
                    }
                    // Round the corners off with the card's colour is not possible on a
                    // see-through window: rings mark them instead. Hover and the choice fade
                    // in and out; the chosen one's ring grows out a little.
                    let a = |k: f32| (k.clamp(0.0, 1.0) * 255.0).round() as u32;
                    cv.rrect_stroke(vclip, xf.r(*t), xf.len(8), 1.0, 0xffffff, 36, 36);
                    if hot > 0.01 {
                        let k = a(hot * 0.55 * (1.0 - sel));
                        cv.rrect_stroke(vclip, xf.r(*t), xf.len(8), 2.0, 0xffffff, k, k);
                    }
                    if sel > 0.01 {
                        let grow = (2.0 * sel).round() as i32;
                        cv.rrect_stroke(vclip, xf.r(t.inflate(grow)), xf.len(8 + grow), 1.0 + 2.0 * sel.min(1.0), crate::look::accent() & 0xff_ffff, a(sel), a(sel));
                    }
                    let name = match path {
                        None => "Liquify (встроенные)".to_string(),
                        Some(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
                    };
                    let name = crate::shell::ellipsize(&name, 12.0, font::W::Text, t.w);
                    font::draw_text(cv, vclip, xf, t.x, t.y1() + 17, &name, 12.0, text_color(0.6 + 0.4 * sel.clamp(0.0, 1.0)), t.w);
                }
            }
            Ctl::Plane => {
                let px = app.plane_xf(row.at);
                cv.rrect(vclip, xf.r(row.at), xf.len(12), argb(40, 0, 0, 0));
                let sel = app.sel_mon().map(|m| m.name.clone());
                for (i, o) in app.m.outputs.iter().enumerate() {
                    let r = px.r(o.plane).inflate(-2);
                    let chosen = sel.as_deref() == Some(o.name.as_str());
                    let dragged = app.pdrag.map_or(false, |d| d.i == i);
                    let fill = if chosen { crate::look::accent_a(if dragged { 110 } else { 80 }) } else { argb(36, 255, 255, 255) };
                    cv.rrect(vclip, xf.r(r), xf.len(8), fill);
                    let (sc, a) = if chosen { (crate::look::accent_light(0.35) & 0xff_ffff, 255) } else { (0xffffff, 70) };
                    cv.rrect_stroke(vclip, xf.r(r), xf.len(8), if chosen { 2.0 } else { 1.0 }, sc, a, a);
                    let n = app.m.mons.iter().position(|m| m.name == o.name).map_or(i + 1, |k| k + 1).to_string();
                    let size = (r.h as f32 * 0.32).clamp(14.0, 34.0);
                    let nw = font::text_width_w(&n, size, font::W::Bold);
                    font::draw_text_w(cv, vclip, xf, r.x + (r.w - nw) / 2, r.y + r.h / 2 + (size * 0.25) as i32, &n, size, text_color(0.95), r.w, font::W::Bold);
                    if r.h > 60 && r.w > 70 {
                        let label = crate::shell::ellipsize(&format!("{}  {}×{}", o.name, o.pw, o.ph), 11.5, font::W::Text, r.w - 12);
                        let lw = font::text_width(&label, 11.5);
                        font::draw_text(cv, vclip, xf, r.x + (r.w - lw) / 2, r.y1() - 10, &label, 11.5, text_color(0.7), r.w - 12);
                    }
                    if app.m.mons.iter().any(|m| m.name == o.name && m.primary) && app.m.mons.len() > 1 {
                        font::draw_text_w(cv, vclip, xf, r.x + 8, r.y + 17, "основной", 11.0, text_color(0.85), r.w - 16, font::W::Semi);
                    }
                }
            }
            Ctl::Slider(sl) => slider(cv, vclip, xf, row.at, app.anims.get(AK::Slider(sl))),
            Ctl::Button(_, label) => button(cv, vclip, xf, row.at, label, app.anims.get(AK::Btn)),
            Ctl::None => {}
        }
    }
    // Scroll bar.
    let v = l.view;
    if l.total > v.h {
        let th = (v.h * v.h / l.total).max(24);
        let ty = v.y + (v.h - th) * app.scroll.value.round() as i32 / (l.total - v.h).max(1);
        cv.rrect(clip, Rect::new(app.w - 8, ty, 4, th), 2, argb(70, 255, 255, 255));
    }
}

/// A Bluetooth list: each device with its state on the right; paired ones also «Удалить».
fn draw_bt(app: &App, cv: &mut Canvas, vclip: Rect, xf: Xf, at: Rect, others: bool) {
    let Some(st) = app.bt.as_ref().map(|b| &b.state) else { return };
    for (i, d) in app.bt_devs(others).iter().enumerate() {
        let r = dev_rect(at, i);
        let hot = app.ptr.map_or(false, |(x, y)| r.contains(x, y));
        if hot {
            cv.rrect(vclip, xf.r(r), xf.len(10), argb(14, 255, 255, 255));
        }
        let busy = st.busy.as_ref().filter(|(p, _)| *p == d.path).map(|(_, w)| *w);
        let (cx, cy) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
        let lit = d.connected || busy.is_some();
        cv.ring(vclip, crate::shell::phys(xf, cx, cy), 7.0, 1.5, if lit { crate::look::accent() } else { text_color(0.5) });
        if d.connected {
            cv.rrect(vclip, xf.r(Rect::new(cx as i32 - 3, cy as i32 - 3, 7, 7)), xf.len(4), crate::look::accent());
        }
        let status = match busy {
            Some(w) => w.to_string(),
            None if d.connected => match d.battery {
                Some(b) => format!("Подключено · {b} %"),
                None => "Подключено".into(),
            },
            None if d.paired => "Не подключено".into(),
            None => d.kind().into(),
        };
        let right = if others { 12 } else { BT_FORGET_W };
        let sw = font::text_width(&status, 12.5);
        let name_w = (r.w - 38 - right - sw - 16).max(40);
        // Two devices with one name (earbuds, a second unit): the address's end tells them apart.
        let twin = st.devices.iter().filter(|o| o.name == d.name).count() > 1;
        let label = match (d.name.is_empty(), twin) {
            (true, _) => d.addr.clone(),
            (false, true) => format!("{} · {}", d.name, d.addr.get(12..).unwrap_or(&d.addr)),
            (false, false) => d.name.clone(),
        };
        let name = crate::shell::ellipsize(&label, 13.5, font::W::Semi, name_w);
        font::draw_text_w(cv, vclip, xf, r.x + 38, r.y + 23, &name, 13.5, text_color(0.9), name_w, font::W::Semi);
        let sc = if lit { crate::look::accent() } else { text_color(0.55) };
        font::draw_text(cv, vclip, xf, r.x1() - right - sw, r.y + 23, &status, 12.5, sc, sw + 4);
        if !others {
            let fx = r.x1() - BT_FORGET_W + 14;
            let fhot = app.ptr.map_or(false, |(x, y)| r.contains(x, y) && x >= (r.x1() - BT_FORGET_W) as f64);
            font::draw_text(cv, vclip, xf, fx, r.y + 23, "Удалить", 12.5, text_color(if fhot { 0.95 } else { 0.4 }), BT_FORGET_W);
        }
    }
}
