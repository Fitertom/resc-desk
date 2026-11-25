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
/// Liquify's accent (kit.rs ACCENT, 0xa78bfa).
const ACCENT: [f32; 3] = [167.0 / 255.0, 139.0 / 255.0, 250.0 / 255.0];
const ACCENT_U32: u32 = 0xffa7_8bfa;

const UI_MIN: f64 = 0.75;
const UI_MAX: f64 = 2.5;
const UI_STEP: f64 = 0.25;

const KEY_UP: u32 = 103;
const KEY_DOWN: u32 = 108;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Look,
    Wallpaper,
    Displays,
    Sound,
    About,
}

const PAGES: [(Page, &str, &str); 5] = [
    (Page::Look, "Оформление", "preferences-desktop-theme"),
    (Page::Wallpaper, "Обои", "preferences-desktop-wallpaper;preferences-desktop-background"),
    (Page::Displays, "Мониторы", "preferences-desktop-display"),
    (Page::Sound, "Звук", "preferences-desktop-sound;audio-volume-high;multimedia-volume-control"),
    (Page::About, "О системе", "computer;help-about"),
];

/// Wallpaper tiles: size of a thumbnail.
const TW: usize = 320;
const TH: usize = 180;
const TILE_GAP: i32 = 12;
const TILE_LABEL: i32 = 24;
const DEV_ROW: i32 = 38;

#[derive(Clone, Copy, PartialEq)]
enum Sw {
    Frost,
    Glue,
    OutMute,
    InMute,
}

#[derive(Clone, Copy, PartialEq)]
enum Sl {
    Blur,
    Bezel,
    WinBezel,
    UiScale,
    OutVol,
    InVol,
}

/// A choice of one among a few (life.rs: how programs start and windows close).
#[derive(Clone, Copy, PartialEq)]
enum Ch {
    Launch,
    Close,
}

impl Ch {
    fn options(self) -> &'static [&'static str] {
        match self {
            Ch::Launch => &["Из кнопки на панели задач", "Из значка, по которому кликнули"],
            Ch::Close => &["Втягивается в кнопку, кнопка лопается", "Сжимается в каплю и испаряется", "Втягивается в ✕"],
        }
    }

    fn idx(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Calibrate,
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
    renderer: String,
    session: String,
    outputs: Vec<Out>,
    error: Option<String>,
}

fn fetch() -> Model {
    let mut m = Model { max_blur: 40.0, max_bezel: 48.0, ui_scale: 1.0, ..Default::default() };
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
            "anim" => {
                m.anim_launch = f(0) as usize;
                m.anim_close = f(1) as usize;
            }
            "wallpaper" => m.wallpaper = (v != "default" && !v.is_empty()).then(|| v.to_string()),
            "glue" => m.glue = f(0) != 0.0,
            "renderer" => m.renderer = v.to_string(),
            "session" => m.session = v.to_string(),
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
    chs: [Option<Drop>; 2],
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
        match ch {
            Ch::Launch => self.m.anim_launch,
            Ch::Close => self.m.anim_close,
        }
    }

    fn devices(&self, input: bool) -> &[crate::pulse::Device] {
        if input { &self.audio.sources } else { &self.audio.sinks }
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
        }
        .clamp(0.0, 1.0)
    }

    /// Points every spring at what the state says now.
    fn sync_anims(&mut self) {
        self.anims.drive(AK::Switch(Sw::Frost), on(self.m.frosted), SPR_SWITCH);
        self.anims.drive(AK::Switch(Sw::Glue), on(self.m.glue), SPR_SWITCH);
        for (sw, input) in [(Sw::OutMute, false), (Sw::InMute, true)] {
            let muted = self.sound_dev(input).map_or(false, |d| d.mute);
            self.anims.drive(AK::Switch(sw), on(muted), SPR_SWITCH);
        }
        for sl in [Sl::Blur, Sl::Bezel, Sl::WinBezel, Sl::UiScale, Sl::OutVol, Sl::InVol] {
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
        for ch in [Ch::Launch, Ch::Close] {
            let Some(row) = l.rows.iter().find(|r| r.ctl == Ctl::Choice(ch)) else { continue };
            let hots: Vec<bool> = (0..ch.options().len()).map(|i| under(dev_rect(row.at, i), self.ptr)).collect();
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
            Page::Displays => {
                for o in &m.outputs {
                    let hz = if o.mhz > 0 { format!(" · {:.0} Гц", o.mhz as f64 / 1000.0) } else { String::new() };
                    v.push((
                        format!("{}  {}×{}{}", o.name, o.pw, o.ph, hz),
                        String::new(),
                        format!(
                            "{:.0} × {:.0} мм, {:.1}″ · {:.2} px/мм · масштаб {:.2} · позиция {:.0}, {:.0} мм",
                            o.plane[2],
                            o.plane[3],
                            o.plane[2].hypot(o.plane[3]) / 25.4,
                            o.pw as f64 / o.plane[2].max(1.0),
                            o.scale,
                            o.plane[0],
                            o.plane[1]
                        ),
                        Ctl::None,
                    ));
                }
                v.push((
                    "Масштаб интерфейса".into(),
                    format!("{:.0} %", self.ui_scale() * 100.0),
                    "Размер всего на первом мониторе. Остальные мониторы подстраиваются так, чтобы окно везде было одного размера в миллиметрах".into(),
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
                    "Линейки и миллиметровая сетка поверх всех мониторов: подогнать размеры по банковской карте и выровнять положение".into(),
                    Ctl::Button(Btn::Calibrate, "Открыть"),
                ));
            }
            Page::Wallpaper => {
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
            Page::About => {
                v.push(("resc-desk".into(), env!("CARGO_PKG_VERSION").into(), "Свой Wayland-композитор и рабочий стол".into(), Ctl::None));
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
                Ctl::Choice(ch) => h += ch.options().len() as i32 * DEV_ROW - 6,
                _ => {}
            }
            let card = Rect::new(cx, y, cw, h);
            let at = match ctl {
                Ctl::Switch(_) => Rect::new(card.x1() - CARD_PAD - SWITCH_W as i32, card.y + (card.h - SWITCH_H as i32) / 2, SWITCH_W as i32, SWITCH_H as i32),
                Ctl::Slider(_) => Rect::new(card.x + CARD_PAD, card.y1() - CARD_PAD - SLIDER_H + 4, card.w - 2 * CARD_PAD, SLIDER_H),
                Ctl::Button(..) => Rect::new(card.x1() - CARD_PAD - BTN_W, card.y + (card.h - BTN_H) / 2, BTN_W, BTN_H),
                Ctl::Walls | Ctl::Devices(_) | Ctl::Choice(_) => Rect::from_edges(card.x + CARD_PAD, card.y + body_y + 6, card.x1() - CARD_PAD, card.y1() - CARD_PAD),
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
        match p {
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
        };
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
            Ctl::Switch(sw @ (Sw::OutMute | Sw::InMute)) => {
                let input = sw == Sw::InMute;
                if let Some(dev) = self.sound_dev(input) {
                    self.sound_op(|p, _| p.set_mute(&dev, input, !dev.mute));
                }
            }
            Ctl::Choice(ch) => {
                let i = ((y - row.at.y as f64) / DEV_ROW as f64).floor();
                if i >= 0.0 && (i as usize) < ch.options().len() {
                    let i = i as usize;
                    match ch {
                        Ch::Launch => self.m.anim_launch = i,
                        Ch::Close => self.m.anim_close = i,
                    }
                    send(&format!("anim {} {i}", if ch == Ch::Launch { "launch" } else { "close" }));
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
        Some("wallpaper") | Some("wallpapers") => Page::Wallpaper,
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
        chs: [None, None],
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
    };
    app.refresh();
    let first = app.page;
    app.page = Page::About;
    app.set_page(first);
    let mut dirty = true;
    let mut last = sys::now_ns();
    app.sync_anims();
    loop {
        let animating = app.animating();
        // Thumbnails coming in, or sound changing elsewhere: look again soon.
        let wait = if animating {
            16
        } else if app.thumbs.is_some() && app.page == Page::Wallpaper {
            50
        } else if app.page == Page::Sound {
            300
        } else {
            -1
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
        if app.walls_poll() && app.page == Page::Wallpaper {
            dirty = true;
        }
        if app.page == Page::Sound && app.sound_poll() {
            dirty = true;
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
    let accent = rgba(ACCENT[0], ACCENT[1], ACCENT[2], 1.0);
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
        let accent = rgba(ACCENT[0], ACCENT[1], ACCENT[2], 1.0);
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
            let a = 0.30 + 0.08 * t + g;
            let r = 14.0 * 0.30 + g * 167.0;
            let gg = 12.0 * 0.30 + g * 139.0;
            let b = 20.0 * 0.30 + g * 250.0;
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
            cv.rrect(clip, xf.r(sel), xf.len(10), argb(34, 167, 139, 250));
            let bar = (sel.h - 22).max(4);
            cv.rrect(clip, xf.r(Rect::new(sel.x, sel.y + (sel.h - bar) / 2, 3, bar)), xf.len(1), ACCENT_U32);
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
            font::draw_text_w(cv, vclip, xf, c.x1() - CARD_PAD - vw, c.y + CARD_PAD + 15, &row.value, 14.0, ACCENT_U32, vw + 4, font::W::Bold);
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
                let opts = ch.options();
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
                        cv.rrect(vclip, xf.r(sel), xf.len(10), argb(30, 167, 139, 250));
                    }
                }
                for (i, name) in opts.iter().enumerate() {
                    let r = dev_rect(row.at, i);
                    let c = drop.map_or(0.0, |d| d.cover(i));
                    let (cx, cy) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
                    cv.ring(vclip, crate::shell::phys(xf, cx, cy), 7.0, 1.5, lerp_argb(text_color(0.5), ACCENT_U32, c));
                    let dot = (7.0 * c).round() as i32;
                    if dot >= 1 {
                        cv.rrect(vclip, xf.r(Rect::new(cx as i32 - dot / 2, cy as i32 - dot / 2, dot, dot)), xf.len((dot + 1) / 2), ACCENT_U32);
                    }
                    let name = crate::shell::ellipsize(name, 13.5, font::W::Semi, r.w - 60);
                    font::draw_text_w(cv, vclip, xf, r.x + 38, r.y + 23, &name, 13.5, text_color(0.8 + 0.2 * c), r.w - 60, font::W::Semi);
                }
            }
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
                        cv.rrect(vclip, xf.r(sel), xf.len(10), argb(30, 167, 139, 250));
                    }
                }
                for (i, dev) in devs.iter().enumerate() {
                    let r = dev_rect(row.at, i);
                    let c = drop.map_or(0.0, |d| d.cover(i));
                    let (cx, cy) = (r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0);
                    cv.ring(vclip, crate::shell::phys(xf, cx, cy), 7.0, 1.5, lerp_argb(text_color(0.5), ACCENT_U32, c));
                    let dot = (7.0 * c).round() as i32;
                    if dot >= 1 {
                        cv.rrect(vclip, xf.r(Rect::new(cx as i32 - dot / 2, cy as i32 - dot / 2, dot, dot)), xf.len((dot + 1) / 2), ACCENT_U32);
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
                        cv.rrect_stroke(vclip, xf.r(t.inflate(grow)), xf.len(8 + grow), 1.0 + 2.0 * sel.min(1.0), 0xa78bfa, a(sel), a(sel));
                    }
                    let name = match path {
                        None => "Liquify (встроенные)".to_string(),
                        Some(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
                    };
                    let name = crate::shell::ellipsize(&name, 12.0, font::W::Text, t.w);
                    font::draw_text(cv, vclip, xf, t.x, t.y1() + 17, &name, 12.0, text_color(0.6 + 0.4 * sel.clamp(0.0, 1.0)), t.w);
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
