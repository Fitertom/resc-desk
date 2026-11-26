//! The mouse's buttons bound to other things («Параметры» → «Мышь», as Logitech's G Hub
//! does without it): each button has an action as it is and one while a G-Shift button is
//! held (a second layer). Actions: a key combination (held as long as the button is), a
//! command, nothing, G-Shift itself, the button as it was, an autoclicker (clicks of a
//! mouse button so many times a second, while held or switched on and off by a press), or a
//! macro (keys, clicks and pauses in a row: once, or over and over while held).
//!
//! The side buttons, the wheel's click and tilt, and the wheel itself (only with G-Shift)
//! are bound here (evdev.rs asks before passing them on). Bindings live in
//! ~/.config/resc-desk/mouse.conf: `bind NAME LAYER ACTION` (LAYER 0: as is, 1: with
//! G-Shift; ACTION: default, none, gshift, `key ctrl+c`, `cmd COMMAND`, `click left 15`,
//! `click left 15 toggle`, `macro ctrl+c, 50, ctrl+v`, `macro-loop click left, 100`).
use crate::desk::Desk;
use std::collections::HashMap;

/// Bindable buttons: config name, evdev code (tilt and wheel: none), label.
pub const BUTTONS: [(&str, u16, &str); 13] = [
    ("side", 0x113, "Боковая «назад» (G4)"),
    ("extra", 0x114, "Боковая «вперёд» (G5)"),
    ("middle", 0x112, "Нажатие колеса"),
    ("tilt_left", 0, "Наклон колеса влево"),
    ("tilt_right", 0, "Наклон колеса вправо"),
    ("wheel_up", 0, "Колесо вверх (с G-Shift)"),
    ("wheel_down", 0, "Колесо вниз (с G-Shift)"),
    ("right", 0x111, "Правая кнопка"),
    ("left", 0x110, "Левая кнопка (только с G-Shift)"),
    ("forward", 0x115, "Доп. кнопка 6"),
    ("back", 0x116, "Доп. кнопка 7"),
    ("task", 0x117, "Доп. кнопка 8"),
    ("btn9", 0x118, "Доп. кнопка 9"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Default,
    None,
    GShift,
    Keys(Vec<u32>),
    Cmd(String),
    /// Autoclicker: `btn` (evdev) `cps` times a second; while held, or on/off by a press.
    Click { btn: u16, cps: u32, toggle: bool },
    /// Steps in a row; `repeat`: over and over while held.
    Macro { steps: Vec<Step>, repeat: bool },
}

/// A macro's step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A key combination pressed and let go.
    Tap(Vec<u32>),
    /// A click of a mouse button (evdev).
    Click(u16),
    /// A pause, ms.
    Wait(u32),
}

const MOUSE_BTNS: [(&str, u16); 5] = [("left", 0x110), ("right", 0x111), ("middle", 0x112), ("side", 0x113), ("extra", 0x114)];

fn btn_code(n: &str) -> Option<u16> {
    MOUSE_BTNS.iter().find(|(k, _)| *k == n.trim()).map(|(_, c)| *c)
}

fn btn_name(c: u16) -> &'static str {
    MOUSE_BTNS.iter().find(|(_, k)| *k == c).map_or("left", |(n, _)| n)
}

fn steps(s: &str) -> Option<Vec<Step>> {
    let mut v = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let num = part.strip_suffix("ms").unwrap_or(part).trim();
        if let Ok(ms) = num.parse::<u32>() {
            v.push(Step::Wait(ms.min(60_000)));
        } else if let Some(b) = part.strip_prefix("click") {
            v.push(Step::Click(btn_code(if b.trim().is_empty() { "left" } else { b })?));
        } else {
            v.push(Step::Tap(keys(part)?));
        }
    }
    (!v.is_empty()).then_some(v)
}

fn steps_spec(v: &[Step]) -> String {
    v.iter()
        .map(|st| match st {
            Step::Tap(k) => k.iter().map(|c| key_name(*c)).collect::<Vec<_>>().join("+"),
            Step::Click(b) => format!("click {}", btn_name(*b)),
            Step::Wait(ms) => ms.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl Act {
    pub fn parse(s: &str) -> Act {
        let s = s.trim();
        match s.split_once(' ').map_or((s, ""), |(a, b)| (a, b.trim())) {
            ("none", _) => Act::None,
            ("gshift", _) => Act::GShift,
            ("key", k) => keys(k).map_or(Act::Default, Act::Keys),
            ("cmd", c) if !c.is_empty() => Act::Cmd(c.to_string()),
            ("click", rest) => {
                let w: Vec<&str> = rest.split_whitespace().collect();
                let btn = w.first().and_then(|b| btn_code(b)).unwrap_or(0x110);
                let cps = w.get(1).and_then(|n| n.parse::<u32>().ok()).unwrap_or(10).clamp(1, 100);
                Act::Click { btn, cps, toggle: w.contains(&"toggle") }
            }
            ("macro", st) => steps(st).map_or(Act::Default, |steps| Act::Macro { steps, repeat: false }),
            ("macro-loop", st) => steps(st).map_or(Act::Default, |steps| Act::Macro { steps, repeat: true }),
            _ => Act::Default,
        }
    }

    pub fn spec(&self) -> String {
        match self {
            Act::Default => "default".into(),
            Act::None => "none".into(),
            Act::GShift => "gshift".into(),
            Act::Keys(k) => format!("key {}", k.iter().map(|c| key_name(*c)).collect::<Vec<_>>().join("+")),
            Act::Cmd(c) => format!("cmd {c}"),
            Act::Click { btn, cps, toggle } => format!("click {} {cps}{}", btn_name(*btn), if *toggle { " toggle" } else { "" }),
            Act::Macro { steps, repeat } => format!("{} {}", if *repeat { "macro-loop" } else { "macro" }, steps_spec(steps)),
        }
    }
}

/// Key names (as in `key ctrl+shift+t`) and their evdev codes.
const KEYS: [(&str, u32); 64] = [
    ("ctrl", 29), ("shift", 42), ("alt", 56), ("super", 125), ("altgr", 100),
    ("esc", 1), ("tab", 15), ("enter", 28), ("space", 57), ("backspace", 14), ("delete", 111), ("insert", 110),
    ("home", 102), ("end", 107), ("pageup", 104), ("pagedown", 109),
    ("up", 103), ("down", 108), ("left", 105), ("right", 106),
    ("f1", 59), ("f2", 60), ("f3", 61), ("f4", 62), ("f5", 63), ("f6", 64), ("f7", 65), ("f8", 66), ("f9", 67), ("f10", 68), ("f11", 87), ("f12", 88),
    ("f13", 183), ("f14", 184), ("f15", 185), ("f16", 186), ("f17", 187), ("f18", 188), ("f19", 189), ("f20", 190), ("f21", 191), ("f22", 192), ("f23", 193), ("f24", 194),
    ("playpause", 164), ("nextsong", 163), ("previoussong", 165), ("stopcd", 166),
    ("volumeup", 115), ("volumedown", 114), ("mute", 113),
    ("print", 99), ("minus", 12), ("equal", 13), ("grave", 41),
    ("comma", 51), ("dot", 52), ("slash", 53), ("semicolon", 39), ("apostrophe", 40),
    ("leftbrace", 26), ("rightbrace", 27), ("backslash", 43), ("capslock", 58),
];

/// Letters and digits on the QWERTY rows (evdev codes).
const LETTERS: &str = "qwertyuiop";
const LETTERS2: &str = "asdfghjkl";
const LETTERS3: &str = "zxcvbnm";

fn key_code(name: &str) -> Option<u32> {
    let n = name.trim().to_lowercase();
    if let Some((_, c)) = KEYS.iter().find(|(k, _)| *k == n) {
        return Some(*c);
    }
    let ch = n.chars().next()?;
    if n.chars().count() == 1 {
        if let Some(i) = LETTERS.find(ch) {
            return Some(16 + i as u32);
        }
        if let Some(i) = LETTERS2.find(ch) {
            return Some(30 + i as u32);
        }
        if let Some(i) = LETTERS3.find(ch) {
            return Some(44 + i as u32);
        }
        if ch == '0' {
            return Some(11);
        }
        if let Some(dgt) = ch.to_digit(10) {
            return Some(1 + dgt);
        }
    }
    None
}

fn key_name(code: u32) -> String {
    if let Some((n, _)) = KEYS.iter().find(|(_, c)| *c == code) {
        return n.to_string();
    }
    let pick = |s: &str, base: u32| s.chars().nth((code - base) as usize).map(|c| c.to_string());
    match code {
        16..=25 => pick(LETTERS, 16),
        30..=38 => pick(LETTERS2, 30),
        44..=50 => pick(LETTERS3, 44),
        2..=10 => Some((code - 1).to_string()),
        11 => Some("0".into()),
        _ => None,
    }
    .unwrap_or_else(|| format!("k{code}"))
}

fn keys(s: &str) -> Option<Vec<u32>> {
    let v: Option<Vec<u32>> = s.split('+').filter(|p| !p.trim().is_empty()).map(key_code).collect();
    v.filter(|v| !v.is_empty())
}

#[derive(Default)]
pub struct Binds {
    pub map: HashMap<(String, u8), Act>,
    /// G-Shift buttons held now.
    gshift: u32,
    /// What each button held now does (its release does the same, layer or not).
    held: HashMap<u16, Act>,
    /// Autoclickers and macros running.
    jobs: Vec<Job>,
}

/// One low-level event of a running autoclicker or macro.
#[derive(Clone, Copy, Debug)]
enum Ev {
    Key(u32, bool),
    Btn(u16, bool),
    Wait(u32),
}

struct Job {
    /// The button that started it (a tilt or the wheel: 0).
    owner: u16,
    evs: Vec<Ev>,
    at: usize,
    /// Over and over (an autoclicker, a looping macro) until stopped.
    repeat: bool,
    /// When the next event is due (ms, Desk::now_ms).
    next: u32,
    /// Keys and buttons it holds down now (let go when it stops).
    down: Vec<Ev>,
}

/// How long a key or a button of a macro is held, and the pause after it (ms): games read
/// what is down at their frame, a press and release in the same instant may not count.
const HOLD_MS: u32 = 12;

fn expand(steps: &[Step]) -> Vec<Ev> {
    let mut v = Vec::new();
    for st in steps {
        match st {
            Step::Tap(k) => {
                v.extend(k.iter().map(|c| Ev::Key(*c, true)));
                v.push(Ev::Wait(HOLD_MS));
                v.extend(k.iter().rev().map(|c| Ev::Key(*c, false)));
                v.push(Ev::Wait(HOLD_MS));
            }
            Step::Click(b) => v.extend([Ev::Btn(*b, true), Ev::Wait(HOLD_MS), Ev::Btn(*b, false), Ev::Wait(HOLD_MS)]),
            Step::Wait(ms) => v.push(Ev::Wait(*ms)),
        }
    }
    v
}

fn start(d: &mut Desk, owner: u16, evs: Vec<Ev>, repeat: bool) {
    let next = d.now_ms();
    d.mouse.jobs.push(Job { owner, evs, at: 0, repeat, next, down: Vec::new() });
    tick(d);
}

/// Stops the jobs of `owner`, letting go of what they hold.
fn stop(d: &mut Desk, owner: u16) {
    let t = d.now_ms();
    let mut i = 0;
    while i < d.mouse.jobs.len() {
        if d.mouse.jobs[i].owner != owner {
            i += 1;
            continue;
        }
        let j = d.mouse.jobs.remove(i);
        for e in j.down.into_iter().rev() {
            emit(d, match e {
                Ev::Key(c, _) => Ev::Key(c, false),
                Ev::Btn(b, _) => Ev::Btn(b, false),
                w => w,
            }, t);
        }
    }
}

fn emit(d: &mut Desk, e: Ev, t: u32) {
    match e {
        Ev::Key(c, p) => crate::input::key(d, c, p, t),
        Ev::Btn(b, p) => crate::input::pointer_button(d, b as u32, p, t),
        Ev::Wait(_) => {}
    }
}

/// Runs what is due of the autoclickers and macros (the main loop, every turn).
pub fn tick(d: &mut Desk) {
    if d.mouse.jobs.is_empty() {
        return;
    }
    let now = d.now_ms();
    let mut i = 0;
    while i < d.mouse.jobs.len() {
        // At most a few hundred events a turn (a looping macro of no pauses).
        for _ in 0..256 {
            let j = &mut d.mouse.jobs[i];
            if j.next > now {
                break;
            }
            if j.at >= j.evs.len() {
                if !j.repeat {
                    break;
                }
                j.at = 0;
            }
            let e = j.evs[j.at];
            j.at += 1;
            match e {
                Ev::Wait(ms) => j.next = j.next.max(now.saturating_sub(50)) + ms,
                Ev::Key(_, p) | Ev::Btn(_, p) => {
                    let key = |x: &Ev| match (x, &e) {
                        (Ev::Key(a, _), Ev::Key(b, _)) => a == b,
                        (Ev::Btn(a, _), Ev::Btn(b, _)) => a == b,
                        _ => false,
                    };
                    j.down.retain(|x| !key(x));
                    if p {
                        j.down.push(e);
                    }
                    emit(d, e, now);
                }
            }
        }
        let j = &d.mouse.jobs[i];
        if !j.repeat && j.at >= j.evs.len() {
            d.mouse.jobs.remove(i);
        } else {
            i += 1;
        }
    }
}

/// How long the main loop may sleep for the autoclickers and macros (ms; None: no limit).
pub fn wait_ms(d: &Desk) -> Option<i32> {
    let now = d.now_ms();
    d.mouse.jobs.iter().map(|j| j.next.saturating_sub(now) as i32).min()
}

fn path() -> std::path::PathBuf {
    let cfg = std::env::var("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    cfg.join("resc-desk").join("mouse.conf")
}

pub fn load() -> Binds {
    let mut b = Binds::default();
    let Ok(t) = std::fs::read_to_string(path()) else { return b };
    for l in t.lines() {
        let mut it = l.splitn(4, ' ');
        if let (Some("bind"), Some(name), Some(layer), Some(act)) = (it.next(), it.next(), it.next(), it.next()) {
            if let Ok(layer) = layer.parse::<u8>() {
                let a = Act::parse(act);
                if a != Act::Default {
                    b.map.insert((name.to_string(), layer.min(1)), a);
                }
            }
        }
    }
    b
}

fn save(b: &Binds) {
    let mut v: Vec<_> = b.map.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    let mut s = String::from("# resc-desk mouse buttons: bind NAME LAYER(0 as is, 1 with G-Shift) ACTION (default, none, gshift, key ctrl+c, cmd COMMAND, click left 15 [toggle], macro ctrl+c, 50, ctrl+v, macro-loop click left, 100)\n");
    for ((n, l), a) in v {
        s.push_str(&format!("bind {n} {l} {}\n", a.spec()));
    }
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, s);
}

/// `ctl mouse-bind NAME LAYER ACTION…`.
pub fn set(d: &mut Desk, name: &str, layer: u8, act: Act) {
    // The left button as is is never taken away (nothing could be clicked after).
    if name == "left" && layer == 0 {
        return;
    }
    let key = (name.to_string(), layer.min(1));
    if act == Act::Default {
        d.mouse.map.remove(&key);
    } else {
        d.mouse.map.insert(key, act);
    }
    save(&d.mouse);
}

/// The state lines for the settings: `mbind NAME LAYER ACTION`.
pub fn state(d: &Desk) -> String {
    let mut s = String::new();
    for ((n, l), a) in &d.mouse.map {
        s.push_str(&format!("mbind {n} {l} {}\n", a.spec()));
    }
    s
}

fn lookup(d: &Desk, name: &str) -> Act {
    let get = |l: u8| d.mouse.map.get(&(name.to_string(), l)).cloned();
    if d.mouse.gshift > 0 {
        match get(1) {
            Some(a) if a != Act::Default => return a,
            _ => {}
        }
    }
    get(0).unwrap_or(Act::Default)
}

fn run(d: &mut Desk, owner: u16, act: &Act, pressed: bool, t: u32) {
    match act {
        Act::Keys(k) => {
            if pressed {
                for c in k {
                    crate::input::key(d, *c, true, t);
                }
            } else {
                for c in k.iter().rev() {
                    crate::input::key(d, *c, false, t);
                }
            }
        }
        Act::Cmd(c) if pressed => d.spawn(c),
        Act::Click { btn, cps, toggle } => {
            let running = d.mouse.jobs.iter().any(|j| j.owner == owner);
            let half = (1000 / *cps / 2).max(1);
            let evs = vec![Ev::Btn(*btn, true), Ev::Wait(half.min(HOLD_MS)), Ev::Btn(*btn, false), Ev::Wait((1000 / *cps).saturating_sub(half.min(HOLD_MS)).max(1))];
            match (*toggle, pressed, running) {
                (true, true, true) | (false, false, _) => stop(d, owner),
                (_, true, false) => start(d, owner, evs, true),
                _ => {}
            }
        }
        Act::Macro { steps, repeat } => match (pressed, *repeat) {
            (true, _) if !d.mouse.jobs.iter().any(|j| j.owner == owner) => start(d, owner, expand(steps), *repeat),
            (false, true) => stop(d, owner),
            _ => {}
        },
        Act::GShift => {
            if pressed {
                d.mouse.gshift += 1;
            } else {
                d.mouse.gshift = d.mouse.gshift.saturating_sub(1);
            }
        }
        _ => {}
    }
}

/// A mouse button (evdev code). True when its binding took it (not passed on).
pub fn button(d: &mut Desk, code: u16, pressed: bool, t: u32) -> bool {
    let Some((name, ..)) = BUTTONS.iter().find(|b| b.1 == code && b.1 != 0) else { return false };
    if pressed {
        let act = lookup(d, name);
        d.mouse.held.insert(code, act.clone());
        if act == Act::Default {
            return false;
        }
        run(d, code, &act, true, t);
        true
    } else {
        match d.mouse.held.remove(&code) {
            None | Some(Act::Default) => false,
            Some(act) => {
                run(d, code, &act, false, t);
                true
            }
        }
    }
}

/// `name` (a tilt, the wheel) does something of its own now.
pub fn bound(d: &Desk, name: &str) -> bool {
    if name.starts_with("wheel") && d.mouse.gshift == 0 {
        return false;
    }
    !matches!(lookup(d, name), Act::Default | Act::GShift)
}

/// A tilt (-1 left, 1 right) or, with G-Shift, a wheel notch (-1 up, 1 down): a press and a
/// release of its binding. True when taken.
pub fn tap(d: &mut Desk, name: &str, t: u32) -> bool {
    if name.starts_with("wheel") && d.mouse.gshift == 0 {
        return false;
    }
    let act = lookup(d, name);
    if act == Act::Default || act == Act::GShift {
        return false;
    }
    // (A tilt has no release of its own: a looping macro or a held autoclicker runs one
    // round; a toggled one switches.)
    let owner = if name == "tilt_left" { 1 } else if name == "tilt_right" { 2 } else { 3 };
    match &act {
        Act::Click { toggle: false, btn, .. } => {
            let b = *btn;
            start(d, owner, vec![Ev::Btn(b, true), Ev::Wait(HOLD_MS), Ev::Btn(b, false)], false);
        }
        Act::Macro { steps, repeat: true } => start(d, owner, expand(steps), false),
        _ => {
            run(d, owner, &act, true, t);
            run(d, owner, &act, false, t);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actions_read_and_write_back() {
        for s in ["none", "gshift", "key ctrl+shift+t", "key super+tab", "key playpause", "cmd firefox --new-window"] {
            assert_eq!(Act::parse(s).spec(), s);
        }
        assert_eq!(Act::parse("key ctrl+c"), Act::Keys(vec![29, 46]));
        assert_eq!(Act::parse("key nonsense+c"), Act::Default);
        assert_eq!(Act::parse("key 1+0"), Act::Keys(vec![2, 11]));
        for s in ["key f19", "click left 15", "click right 30 toggle", "macro ctrl+c, 50, ctrl+v", "macro-loop click left, 100, e"] {
            assert_eq!(Act::parse(s).spec(), s);
        }
        assert_eq!(Act::parse("macro ctrl+c, 50ms, click"), Act::Macro { steps: vec![Step::Tap(vec![29, 46]), Step::Wait(50), Step::Click(0x110)], repeat: false });
        assert_eq!(Act::parse("macro nonsense"), Act::Default);
    }
}
