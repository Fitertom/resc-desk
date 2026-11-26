//! The mouse's buttons bound to other things («Параметры» → «Мышь», as Logitech's G Hub
//! does without it): each button has an action as it is and one while a G-Shift button is
//! held (a second layer). Actions: a key combination (held as long as the button is), a
//! command, nothing, G-Shift itself, or the button as it was.
//!
//! The side buttons, the wheel's click and tilt, and the wheel itself (only with G-Shift)
//! are bound here (evdev.rs asks before passing them on). Bindings live in
//! ~/.config/resc-desk/mouse.conf: `bind NAME LAYER ACTION` (LAYER 0: as is, 1: with
//! G-Shift; ACTION: default, none, gshift, `key ctrl+c`, `cmd COMMAND`).
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
}

impl Act {
    pub fn parse(s: &str) -> Act {
        let s = s.trim();
        match s.split_once(' ').map_or((s, ""), |(a, b)| (a, b.trim())) {
            ("none", _) => Act::None,
            ("gshift", _) => Act::GShift,
            ("key", k) => keys(k).map_or(Act::Default, Act::Keys),
            ("cmd", c) if !c.is_empty() => Act::Cmd(c.to_string()),
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
        }
    }
}

/// Key names (as in `key ctrl+shift+t`) and their evdev codes.
const KEYS: [(&str, u32); 52] = [
    ("ctrl", 29), ("shift", 42), ("alt", 56), ("super", 125), ("altgr", 100),
    ("esc", 1), ("tab", 15), ("enter", 28), ("space", 57), ("backspace", 14), ("delete", 111), ("insert", 110),
    ("home", 102), ("end", 107), ("pageup", 104), ("pagedown", 109),
    ("up", 103), ("down", 108), ("left", 105), ("right", 106),
    ("f1", 59), ("f2", 60), ("f3", 61), ("f4", 62), ("f5", 63), ("f6", 64), ("f7", 65), ("f8", 66), ("f9", 67), ("f10", 68), ("f11", 87), ("f12", 88),
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
    let mut s = String::from("# resc-desk mouse buttons: bind NAME LAYER(0 as is, 1 with G-Shift) ACTION (default, none, gshift, key ctrl+c, cmd COMMAND)\n");
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

fn run(d: &mut Desk, act: &Act, pressed: bool, t: u32) {
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
        run(d, &act, true, t);
        true
    } else {
        match d.mouse.held.remove(&code) {
            None | Some(Act::Default) => false,
            Some(act) => {
                run(d, &act, false, t);
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
    run(d, &act, true, t);
    run(d, &act, false, t);
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
    }
}
