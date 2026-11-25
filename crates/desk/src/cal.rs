//! The calendar flyout over the taskbar clock, with what «Glint Calendar»
//! (Fitertom/resc_calendar) does:
//! - right click on a day: a red mark; again: off;
//! - middle click: a note (Enter keeps it, Esc drops it); middle click on a noted day
//!   deletes the note;
//! - pointing at a noted day shows the note; noted days have a dot, today a circle,
//!   weekends are tinted;
//! - the wheel, PgUp / PgDn: months; the month's title, Home: back to today;
//! - the pointer leaving the flyout (once it was in), Esc: closes it.
//!
//! Below the month: a timer (presets, the wheel over the digits, start / pause / reset)
//! and an alarm (the wheel over the hours and minutes, on / off, «in 15 minutes»). When
//! either goes off the flyout opens, a chime plays every few seconds and the taskbar
//! clock says so, until «Стоп» or the flyout is closed (two minutes at most).
//!
//! Kept in ~/.config/resc-desk/: calendar.txt (`YYYY-MM-DD<TAB>mark|note<TAB>text`, as
//! in Glint Calendar) and alarm.conf.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::desk::Desk;
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::shell::{self, col, phys};
use crate::textedit::TextField;
use crate::tray::{days_from_civil, weekday};

const PAD: i32 = 16;
const CW: i32 = 44;
const RH: i32 = 40;
pub const W: i32 = 7 * CW + 2 * PAD;
const Y_HEAD: i32 = 58;
const Y_WD: i32 = Y_HEAD + 34;
const Y_GRID: i32 = Y_WD + 26;
const Y_NOTE: i32 = Y_GRID + 6 * RH + 4;
const Y_SEP: i32 = Y_NOTE + 44;
const Y_TABS: i32 = Y_SEP + 12;
const Y_BIG: i32 = Y_TABS + 40;
const Y_CHIPS: i32 = Y_BIG + 56;
pub const H: i32 = Y_CHIPS + 30 + PAD;

const KEY_HOME: u32 = 102;
const BIG: f32 = 30.0;
/// The chime repeats this often while ringing, and rings this long at most.
const CHIME_EVERY: Duration = Duration::from_millis(3500);
const RING_MAX: Duration = Duration::from_secs(120);
const TIMER_PRESETS: [u32; 5] = [1, 5, 10, 15, 30];
const ALARM_IN: [u32; 4] = [5, 15, 30, 60];
const MAX_TIMER: u32 = 24 * 3600 - 1;

const RED: u32 = 0xffff_5a5a;
/// Weekend days (premultiplied, opaque).
const WEEKEND: u32 = 0xffff_9a9a;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Day {
    pub mark: bool,
    pub note: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Tab {
    #[default]
    Timer,
    Alarm,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Run {
    Idle,
    Running(Instant),
    Paused(Duration),
}

pub struct Ring {
    pub what: Tab,
    since: Instant,
    next_chime: Instant,
}

/// What a hit on the flyout is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CHot {
    Today,
    Head,
    Prev,
    Next,
    Day(u8),
    Note,
    Tab(Tab),
    /// A group of the big digits (hours / minutes / seconds): the wheel sets it.
    Part(u8),
    Start,
    Reset,
    Switch,
    Chip(u8),
}

pub struct Cal {
    /// Months away from this one.
    pub month: i32,
    pub days: BTreeMap<i64, Day>,
    /// A note being written: for which day.
    pub edit: Option<(i64, TextField)>,
    /// The pointer has been on the flyout (leaving it then closes it).
    pub entered: bool,
    pub tab: Tab,
    /// The timer's length, seconds.
    pub timer_s: u32,
    pub run: Run,
    pub alarm: (u32, u32),
    pub alarm_on: bool,
    /// The minute the alarm last went off (day, hour, minute): once per minute.
    fired: Option<(i64, u32, u32)>,
    pub ring: Option<Ring>,
    scroll_acc: f64,
    /// Where things are kept (None: nowhere, as in tests).
    dir: Option<PathBuf>,
}

impl Default for Cal {
    fn default() -> Self {
        Cal {
            month: 0,
            days: BTreeMap::new(),
            edit: None,
            entered: false,
            tab: Tab::Timer,
            timer_s: 300,
            run: Run::Idle,
            alarm: (7, 0),
            alarm_on: false,
            fired: None,
            ring: None,
            scroll_acc: 0.0,
            dir: None,
        }
    }
}

// ------------------------------------------------------------------ keeping

fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(base).join("resc-desk")
}

pub fn date_str(n: i64) -> String {
    let (y, m, d) = civil(n);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > crate::tray::month_len(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// calendar.txt's lines into days.
pub fn parse_days(text: &str) -> BTreeMap<i64, Day> {
    let mut days: BTreeMap<i64, Day> = BTreeMap::new();
    for line in text.lines() {
        let mut f = line.splitn(3, '\t');
        let (Some(date), Some(kind)) = (f.next(), f.next()) else { continue };
        let Some(n) = parse_date(date.trim()) else { continue };
        let text = f.next().unwrap_or("").trim();
        match kind.trim() {
            "mark" => days.entry(n).or_default().mark = true,
            "note" if !text.is_empty() => days.entry(n).or_default().note = Some(text.to_string()),
            _ => {}
        }
    }
    days
}

pub fn days_text(days: &BTreeMap<i64, Day>) -> String {
    let mut s = String::new();
    for (n, d) in days {
        if d.mark {
            s += &format!("{}\tmark\t\n", date_str(*n));
        }
        if let Some(t) = &d.note {
            let t: String = t.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect();
            s += &format!("{}\tnote\t{}\n", date_str(*n), t);
        }
    }
    s
}

fn write_atomic(path: &PathBuf, text: &str) {
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

impl Cal {
    /// Reads calendar.txt and alarm.conf from `dir`.
    pub fn load(dir: PathBuf) -> Cal {
        let mut c = Cal { dir: Some(dir.clone()), ..Default::default() };
        if let Ok(t) = std::fs::read_to_string(dir.join("calendar.txt")) {
            c.days = parse_days(&t);
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("alarm.conf")) {
            for line in t.lines() {
                let mut f = line.split_whitespace();
                match (f.next(), f.next(), f.next()) {
                    (Some("alarm"), Some(hm), on) => {
                        if let Some((h, m)) = hm.split_once(':') {
                            if let (Ok(h), Ok(m)) = (h.parse::<u32>(), m.parse::<u32>()) {
                                if h < 24 && m < 60 {
                                    c.alarm = (h, m);
                                    c.alarm_on = on == Some("on");
                                }
                            }
                        }
                    }
                    (Some("timer"), Some(s), _) => {
                        if let Ok(s) = s.parse::<u32>() {
                            c.timer_s = s.clamp(1, MAX_TIMER);
                        }
                    }
                    _ => {}
                }
            }
        }
        c
    }

    fn save_days(&self) {
        if let Some(d) = &self.dir {
            write_atomic(&d.join("calendar.txt"), &days_text(&self.days));
        }
    }

    fn save_alarm(&self) {
        if let Some(d) = &self.dir {
            let text = format!(
                "alarm {:02}:{:02} {}\ntimer {}\n",
                self.alarm.0,
                self.alarm.1,
                if self.alarm_on { "on" } else { "off" },
                self.timer_s
            );
            write_atomic(&d.join("alarm.conf"), &text);
        }
    }

    pub fn toggle_mark(&mut self, n: i64) {
        let e = self.days.entry(n).or_default();
        e.mark = !e.mark;
        self.tidy(n);
        self.save_days();
    }

    pub fn set_note(&mut self, n: i64, text: Option<String>) {
        let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        self.days.entry(n).or_default().note = text;
        self.tidy(n);
        self.save_days();
    }

    fn tidy(&mut self, n: i64) {
        if self.days.get(&n).map_or(false, |d| !d.mark && d.note.is_none()) {
            self.days.remove(&n);
        }
    }

    pub fn note(&self, n: i64) -> Option<&str> {
        self.days.get(&n).and_then(|d| d.note.as_deref())
    }

    // -------------------------------------------------------------- timer and alarm

    /// The timer's time left (or its length when idle), seconds, rounded up.
    pub fn timer_left(&self, now: Instant) -> u32 {
        let d = match self.run {
            Run::Idle => return self.timer_s,
            Run::Running(end) => end.saturating_duration_since(now),
            Run::Paused(left) => left,
        };
        ((d.as_millis() + 999) / 1000) as u32
    }

    /// Start / pause / go on.
    pub fn start_pause(&mut self, now: Instant) {
        self.run = match self.run {
            Run::Idle if self.timer_s > 0 => Run::Running(now + Duration::from_secs(self.timer_s as u64)),
            Run::Idle => Run::Idle,
            Run::Running(end) => Run::Paused(end.saturating_duration_since(now)),
            Run::Paused(left) => Run::Running(now + left),
        };
    }

    pub fn set_timer(&mut self, secs: u32, now: Instant) {
        self.timer_s = secs.clamp(1, MAX_TIMER);
        self.run = Run::Idle;
        self.start_pause(now);
        self.save_alarm();
    }

    /// The alarm `mins` from now (`hm`: the time now), switched on.
    pub fn alarm_in(&mut self, hm: (u32, u32), mins: u32) {
        let t = (hm.0 * 60 + hm.1 + mins) % (24 * 60);
        self.alarm = (t / 60, t % 60);
        self.alarm_on = true;
        self.fired = None;
        self.save_alarm();
    }

    /// The wheel over group `k` of the big digits; `up`: more.
    pub fn adjust(&mut self, k: u8, up: bool) {
        if self.ring.is_some() {
            return;
        }
        match self.tab {
            Tab::Timer => {
                if self.run != Run::Idle {
                    return;
                }
                let long = self.timer_s >= 3600;
                let step: i64 = match (long, k) {
                    (true, 0) => 3600,
                    (true, 1) | (false, 0) => 60,
                    _ => 10,
                };
                let v = self.timer_s as i64 + if up { step } else { -step };
                self.timer_s = v.clamp(10, MAX_TIMER as i64) as u32;
            }
            Tab::Alarm => {
                let (h, m) = self.alarm;
                self.alarm = match k {
                    0 => ((h + if up { 1 } else { 23 }) % 24, m),
                    _ => (h, (m + if up { 1 } else { 59 }) % 60),
                };
                self.fired = None;
            }
        }
        self.save_alarm();
    }

    /// What goes off now: the timer ran out, or it is the alarm's minute (`local`: today's
    /// day number, hour, minute).
    pub fn due(&mut self, now: Instant, local: (i64, u32, u32)) -> Option<Tab> {
        if let Run::Running(end) = self.run {
            if now >= end {
                self.run = Run::Idle;
                return Some(Tab::Timer);
            }
        }
        if self.alarm_on && (local.1, local.2) == self.alarm && self.fired != Some(local) {
            self.fired = Some(local);
            // Once, as Windows' «Только один раз».
            self.alarm_on = false;
            self.save_alarm();
            return Some(Tab::Alarm);
        }
        None
    }

    pub fn start_ring(&mut self, what: Tab, now: Instant) {
        self.ring = Some(Ring { what, since: now, next_chime: now });
        self.tab = what;
        self.month = 0;
    }

    /// Whether a chime is due now (and when the next one is).
    fn chime_due(&mut self, now: Instant) -> bool {
        match self.ring.as_mut() {
            Some(r) if now >= r.next_chime => {
                r.next_chime = now + CHIME_EVERY;
                true
            }
            _ => false,
        }
    }

    /// Big digits groups: the timer's [h,] mm, ss or the alarm's hh, mm.
    pub fn parts(&self, now: Instant) -> Vec<String> {
        match self.tab {
            Tab::Timer => {
                let s = self.timer_left(now);
                if s >= 3600 {
                    vec![format!("{}", s / 3600), format!("{:02}", s / 60 % 60), format!("{:02}", s % 60)]
                } else {
                    vec![format!("{:02}", s / 60), format!("{:02}", s % 60)]
                }
            }
            Tab::Alarm => vec![format!("{:02}", self.alarm.0), format!("{:02}", self.alarm.1)],
        }
    }
}

/// A timer's time as on the clock: 4:05, 1:02:03.
pub fn short_time(s: u32) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Date of day `n` since 1970-01-01.
pub fn civil(n: i64) -> (i32, u32, u32) {
    let z = n + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if month <= 2 { y + 1 } else { y }) as i32, month, day)
}

fn today() -> (i64, u32, u32) {
    let (hh, mm, day, mon, year) = crate::clock::local_now();
    (days_from_civil(year, mon, day), hh, mm)
}

/// The month shown (year, month) and the day number of the grid's first cell.
fn shown(c: &Cal) -> (i32, u32, i64) {
    let (_, _, _, mon, year) = crate::clock::local_now();
    let total = year * 12 + mon as i32 - 1 + c.month;
    let (sy, sm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    (sy, sm, days_from_civil(sy, sm, 1) - weekday(sy, sm, 1) as i64)
}

// ------------------------------------------------------------------ the desk's side

pub fn init(d: &mut Desk) {
    d.tray.cal = Cal::load(config_dir());
}

fn damage(d: &mut Desk) {
    crate::tray::damage(d);
}

/// Once a second.
pub fn tick(d: &mut Desk) {
    let now = Instant::now();
    if let Some(what) = d.tray.cal.due(now, today()) {
        d.tray.cal.start_ring(what, now);
        d.tray.cal.edit = None;
        // Show it where the pointer is.
        let oi = d.outputs.iter().position(|o| o.rect().contains(d.seat.x, d.seat.y)).unwrap_or(0);
        if !crate::tray::is_open(d, crate::tray::Fly::Calendar) {
            crate::tray::toggle(d, oi, crate::tray::Fly::Calendar);
            d.tray.cal.start_ring(what, now);
        }
    }
    if let Some(r) = &d.tray.cal.ring {
        if now.duration_since(r.since) > RING_MAX {
            d.tray.cal.ring = None;
        }
    }
    if d.tray.cal.chime_due(now) {
        chime(d);
    }
    if d.tray.cal.run != Run::Idle || d.tray.cal.ring.is_some() {
        shell::damage_clocks(d);
        if crate::tray::is_open(d, crate::tray::Fly::Calendar) {
            damage(d);
        }
    }
}

/// Flyout opened.
pub fn opened(d: &mut Desk) {
    let c = &mut d.tray.cal;
    c.month = 0;
    c.entered = false;
    c.edit = None;
}

/// Flyout closed: the ringing stops, an unfinished note is dropped.
pub fn closed(d: &mut Desk) {
    let c = &mut d.tray.cal;
    c.edit = None;
    if c.ring.take().is_some() {
        shell::damage_clocks(d);
    }
}

/// The chime: made once as a WAV (two soft bell tones), played by whatever player there
/// is.
fn chime(d: &mut Desk) {
    let dir = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".cache"))
        .join("resc-desk");
    let f = dir.join("chime.wav");
    if !f.exists() {
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(&f, chime_wav()).is_err() {
            return;
        }
    }
    let q = f.to_string_lossy().replace('\'', r"'\''");
    d.spawn(&format!("pw-play '{q}' 2>/dev/null || paplay '{q}' 2>/dev/null || aplay -q '{q}' 2>/dev/null"));
}

pub fn chime_wav() -> Vec<u8> {
    let rate = 48000u32;
    let n = (rate as f32 * 1.4) as usize;
    let mut s = vec![0f32; n];
    // Two bell-ish notes: A5 then E6, each with a quiet octave partial, decaying.
    for &(f, at, len) in &[(880.0f32, 0.0f32, 1.0f32), (1318.5, 0.22, 1.15)] {
        let i0 = (at * rate as f32) as usize;
        for i in 0..((len * rate as f32) as usize).min(n - i0) {
            let t = i as f32 / rate as f32;
            let attack = (t / 0.006).min(1.0);
            let env = attack * (-t * 4.2).exp();
            let w = std::f32::consts::TAU * f * t;
            s[i0 + i] += env * (w.sin() + 0.25 * (2.0 * w).sin() + 0.08 * (3.0 * w).sin());
        }
    }
    let mut out = Vec::with_capacity(44 + n * 2);
    let data = (n * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for v in s {
        let v = (v * 0.28).clamp(-1.0, 1.0);
        out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
    }
    out
}

// ------------------------------------------------------------------ layout

fn chips(c: &Cal) -> Vec<String> {
    match (c.tab, c.ring.as_ref().map(|r| r.what)) {
        (Tab::Alarm, Some(Tab::Alarm)) => vec!["Отложить на 5 мин".into()],
        (Tab::Timer, _) => TIMER_PRESETS.iter().map(|m| format!("{m} мин")).collect(),
        (Tab::Alarm, _) => ALARM_IN.iter().map(|&m| if m >= 60 { format!("+{} ч", m / 60) } else { format!("+{m} мин") }).collect(),
    }
}

/// The digits' groups and colons, left to right: (group or None for a colon, rect).
fn digit_rects(p: Rect, parts: &[String]) -> Vec<(Option<u8>, Rect, String)> {
    let mut x = p.x + PAD;
    let mut v = Vec::new();
    for (k, s) in parts.iter().enumerate() {
        if k > 0 {
            let w = font::text_width_w(":", BIG, font::W::Bold) + 2;
            v.push((None, Rect::new(x, p.y + Y_BIG, w, 48), ":".to_string()));
            x += w;
        }
        let w = font::text_width_w(s, BIG, font::W::Bold);
        v.push((Some(k as u8), Rect::new(x, p.y + Y_BIG, w, 48), s.clone()));
        x += w;
    }
    v
}

pub fn hits(d: &Desk, p: Rect) -> Vec<(CHot, Rect)> {
    let c = &d.tray.cal;
    let mut v = Vec::new();
    let (sy, sm, _) = shown(c);
    v.push((CHot::Today, Rect::new(p.x + 8, p.y + 6, p.w - 110, 40)));
    let hw = font::text_width_w(&format!("{} {}", MONTHS[sm as usize - 1], sy), 14.0, font::W::Semi);
    v.push((CHot::Head, Rect::new(p.x + PAD - 8, p.y + Y_HEAD + 2, hw + 16, 30)));
    v.push((CHot::Prev, Rect::new(p.x1() - PAD - 68, p.y + Y_HEAD + 2, 30, 30)));
    v.push((CHot::Next, Rect::new(p.x1() - PAD - 32, p.y + Y_HEAD + 2, 30, 30)));
    for i in 0..42 {
        v.push((CHot::Day(i as u8), Rect::new(p.x + PAD + (i % 7) * CW, p.y + Y_GRID + (i / 7) * RH, CW, RH)));
    }
    if c.edit.is_some() {
        v.push((CHot::Note, Rect::new(p.x + PAD, p.y + Y_NOTE + 2, p.w - 2 * PAD, 34)));
    }
    v.push((CHot::Tab(Tab::Timer), Rect::new(p.x + PAD + 2, p.y + Y_TABS + 2, 96, 26)));
    v.push((CHot::Tab(Tab::Alarm), Rect::new(p.x + PAD + 100, p.y + Y_TABS + 2, 104, 26)));
    let now = Instant::now();
    if c.ring.is_none() {
        for (k, r, _) in digit_rects(p, &c.parts(now)) {
            if let Some(k) = k {
                v.push((CHot::Part(k), r.inflate(4)));
            }
        }
    }
    let ringing = c.ring.is_some();
    if c.tab == Tab::Timer || ringing {
        v.push((CHot::Start, Rect::new(p.x1() - PAD - 34 - 8 - 96, p.y + Y_BIG + 7, 96, 34)));
    }
    if c.tab == Tab::Timer && !ringing {
        v.push((CHot::Reset, Rect::new(p.x1() - PAD - 34, p.y + Y_BIG + 7, 34, 34)));
    }
    if c.tab == Tab::Alarm && !ringing {
        v.push((CHot::Switch, Rect::new(p.x1() - PAD - 46, p.y + Y_BIG + 12, 46, 24)));
    }
    let ch = chips(c);
    let n = ch.len() as i32;
    let cw = (p.w - 2 * PAD - 6 * (n - 1)) / n;
    for k in 0..n {
        v.push((CHot::Chip(k as u8), Rect::new(p.x + PAD + k * (cw + 6), p.y + Y_CHIPS, cw, 30)));
    }
    v
}

// ------------------------------------------------------------------ input

fn commit_edit(d: &mut Desk) {
    if let Some((n, f)) = d.tray.cal.edit.take() {
        d.tray.cal.set_note(n, Some(f.text()));
    }
}

/// A press on hit `h`.
pub fn click(d: &mut Desk, h: CHot, button: u32) {
    use crate::input::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    let now = Instant::now();
    let c = &mut d.tray.cal;
    if let CHot::Day(i) = h {
        let (_, sm, start) = shown(c);
        let n = start + i as i64;
        match button {
            BTN_RIGHT => {
                c.toggle_mark(n);
            }
            BTN_MIDDLE => {
                if c.edit.as_ref().map(|e| e.0) == Some(n) {
                    c.edit = None;
                } else {
                    commit_edit(d);
                    let c = &mut d.tray.cal;
                    if c.note(n).is_some() {
                        c.set_note(n, None);
                    } else {
                        c.edit = Some((n, TextField::default()));
                    }
                }
            }
            BTN_LEFT => {
                // A day of the month before or after: go there.
                let (_, m, _) = civil(n);
                if m != sm {
                    c.month += if i < 7 { -1 } else { 1 };
                }
            }
            _ => {}
        }
        damage(d);
        return;
    }
    if button != BTN_LEFT {
        return;
    }
    match h {
        CHot::Today | CHot::Head => c.month = 0,
        CHot::Prev => c.month -= 1,
        CHot::Next => c.month += 1,
        CHot::Day(_) | CHot::Note | CHot::Part(_) => {}
        CHot::Tab(t) => c.tab = t,
        CHot::Start => {
            if c.ring.take().is_some() {
                shell::damage_clocks(d);
            } else {
                c.start_pause(now);
            }
        }
        CHot::Reset => c.run = Run::Idle,
        CHot::Switch => {
            c.alarm_on = !c.alarm_on;
            c.fired = None;
            c.save_alarm();
        }
        CHot::Chip(k) => {
            let k = k as usize;
            match (c.tab, c.ring.as_ref().map(|r| r.what)) {
                (Tab::Alarm, Some(Tab::Alarm)) => {
                    let (_, hh, mm) = today();
                    c.ring = None;
                    c.alarm_in((hh, mm), 5);
                }
                (Tab::Timer, _) => {
                    c.ring = None;
                    c.set_timer(TIMER_PRESETS[k.min(TIMER_PRESETS.len() - 1)] * 60, now);
                }
                (Tab::Alarm, _) => {
                    let (_, hh, mm) = today();
                    c.alarm_in((hh, mm), ALARM_IN[k.min(ALARM_IN.len() - 1)]);
                }
            }
            shell::damage_clocks(d);
        }
    }
    damage(d);
}

/// The wheel on the flyout: over the digits it sets them, elsewhere it turns months.
pub fn scroll(d: &mut Desk, h: Option<CHot>, value: f64) {
    let c = &mut d.tray.cal;
    c.scroll_acc += value;
    // A notch is 10–15; touchpads send many small steps.
    if c.scroll_acc.abs() < 8.0 {
        return;
    }
    let down = c.scroll_acc > 0.0;
    c.scroll_acc = 0.0;
    match h {
        Some(CHot::Part(k)) => c.adjust(k, !down),
        _ => c.month += if down { 1 } else { -1 },
    }
    damage(d);
}

/// The pointer moved (`on`: over the flyout). True: it left, close the flyout.
pub fn hover(d: &mut Desk, on: bool) -> bool {
    let c = &mut d.tray.cal;
    if on {
        c.entered = true;
        return false;
    }
    c.entered && c.edit.is_none() && c.ring.is_none()
}

/// A key while the flyout is open. True if used.
pub fn key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    use crate::input::key;
    if !pressed {
        return false;
    }
    if d.tray.cal.edit.is_some() {
        match code {
            key::ESC => d.tray.cal.edit = None,
            key::ENTER => commit_edit(d),
            _ => {
                let m = crate::input::text_mods(d);
                if let Some((_, f)) = d.tray.cal.edit.as_mut() {
                    let _ = f.key(code, m);
                }
            }
        }
        d.seat.swallowed.push(code);
        damage(d);
        return true;
    }
    let c = &mut d.tray.cal;
    match code {
        key::ESC => {
            d.seat.swallowed.push(code);
            crate::tray::close(d);
            return true;
        }
        key::PAGEUP => c.month -= 1,
        key::PAGEDOWN => c.month += 1,
        KEY_HOME => c.month = 0,
        _ => return false,
    }
    d.seat.swallowed.push(code);
    damage(d);
    true
}

/// The taskbar clock's second line while the timer runs or something rings.
pub fn clock_line(d: &Desk) -> Option<(String, u32)> {
    let c = &d.tray.cal;
    match (&c.ring, c.run) {
        (Some(r), _) => {
            let t = match r.what {
                Tab::Timer => "Время вышло".to_string(),
                Tab::Alarm => format!("Будильник {:02}:{:02}", c.alarm.0, c.alarm.1),
            };
            // Blinks with the seconds.
            let on = r.since.elapsed().as_millis() / 1000 % 2 == 0;
            Some((t, if on { col::ACCENT } else { col::TEXT }))
        }
        (None, Run::Running(_)) => Some((format!("Таймер {}", short_time(c.timer_left(Instant::now()))), col::ACCENT)),
        (None, Run::Paused(_)) => Some((format!("Пауза {}", short_time(c.timer_left(Instant::now()))), col::TEXT_DIM)),
        _ => None,
    }
}

// ------------------------------------------------------------------ drawing

const MONTHS: [&str; 12] = ["Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь"];
const MONTHS_GEN: [&str; 12] = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];
const WEEKDAYS: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];
const WD_SHORT: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

fn day_name(n: i64) -> String {
    let (_, m, dd) = civil(n);
    format!("{dd} {}", MONTHS_GEN[m as usize - 1])
}

fn chevron(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, right: bool) {
    let (cx, cy) = r.center();
    let (cx, cy) = (cx as f32, cy as f32);
    let s = if right { 1.0 } else { -1.0 };
    let lw = 1.8 * xf.scale as f32;
    cv.line(clip, phys(xf, cx - 2.5 * s, cy - 5.0), phys(xf, cx + 2.5 * s, cy), lw, col::TEXT);
    cv.line(clip, phys(xf, cx + 2.5 * s, cy), phys(xf, cx - 2.5 * s, cy + 5.0), lw, col::TEXT);
}

pub fn draw(d: &Desk, p: Rect, hot: Option<CHot>, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let c = &d.tray.cal;
    let now = Instant::now();
    let (_, _, day, mon, year) = crate::clock::local_now();
    let today_n = days_from_civil(year, mon, day);
    let hits = hits(d, p);
    let rect_of = |h: CHot| hits.iter().find(|(x, _)| *x == h).map(|(_, r)| *r);
    let hov = |h: CHot| hot == Some(h);
    let s = xf.scale as f32;

    // Today, in full; the month's title (both lead back to today).
    let wd = weekday(year, mon, day);
    let today = format!("{}, {} {}", WEEKDAYS[wd], day, MONTHS_GEN[mon as usize - 1]);
    let tc = if hov(CHot::Today) && c.month != 0 { col::ACCENT } else { col::TEXT };
    font::draw_text_w(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 18, &today, 15.0, tc, p.w - 2 * PAD, font::W::Semi);
    let (sy, sm, start) = shown(c);
    let head = format!("{} {}", MONTHS[sm as usize - 1], sy);
    if let Some(r) = rect_of(CHot::Head) {
        if hov(CHot::Head) {
            cv.rrect(clip, xf.r(r), xf.len(8), argb(24, 255, 255, 255));
        }
        let hc = if hov(CHot::Head) && c.month != 0 { col::ACCENT } else { col::TEXT };
        font::draw_text_w(cv, clip, xf, r.x + 8, p.y + Y_HEAD + 22, &head, 14.0, hc, p.w, font::W::Semi);
    }
    for (h, right) in [(CHot::Prev, false), (CHot::Next, true)] {
        if let Some(r) = rect_of(h) {
            if hov(h) {
                cv.rrect(clip, xf.r(r), xf.len(10), argb(24, 255, 255, 255));
            }
            chevron(cv, clip, xf, r, right);
        }
    }
    let gx = p.x + PAD;
    for (i, n) in WD_SHORT.iter().enumerate() {
        let tw = font::text_width(n, 12.0);
        let cl = if i >= 5 { argb(170, 255, 154, 154) } else { col::TEXT_DIM };
        font::draw_text(cv, clip, xf, gx + i as i32 * CW + (CW - tw) / 2, p.y + Y_WD + 16, n, 12.0, cl, CW);
    }
    // Six weeks from the Monday on or before the 1st.
    for i in 0..42i64 {
        let n = start + i;
        let (colm, row) = ((i % 7) as i32, (i / 7) as i32);
        let cell = Rect::new(gx + colm * CW, p.y + Y_GRID + row * RH, CW, RH);
        let (dy, dm, dd) = civil(n);
        let inside = dy == sy && dm == sm;
        let weekend = colm >= 5;
        let disc = Rect::new(cell.x + (CW - 34) / 2, cell.y + 3, 34, 34);
        let (cx, cy) = (disc.x as f32 + 17.0, disc.y as f32 + 17.0);
        let mut tc = match (inside, weekend) {
            (true, false) => col::TEXT,
            (true, true) => WEEKEND,
            (false, false) => argb(70, 255, 255, 255),
            (false, true) => argb(70, 255, 154, 154),
        };
        if hov(CHot::Day(i as u8)) && n != today_n {
            cv.rrect(clip, xf.r(disc), xf.len(17), argb(26, 255, 255, 255));
        }
        if c.edit.as_ref().map(|e| e.0) == Some(n) {
            cv.rrect_stroke(clip, xf.r(disc), xf.len(17), 1.5 * s, 0xa78bfa, 255, 255);
        }
        if n == today_n {
            cv.rrect(clip, xf.r(disc), xf.len(17), col::ACCENT);
            tc = col::ACCENT_INK;
        }
        let day = c.days.get(&n);
        if day.map_or(false, |x| x.mark) {
            let rr = if n == today_n { 19.0 } else { 16.0 };
            let a = if inside { 255 } else { 120 };
            cv.ring(clip, phys(xf, cx, cy), rr * s, 2.0 * s, argb(a, 255, 90, 90));
        }
        let t = format!("{dd}");
        let tw = font::text_width_w(&t, 13.0, font::W::Semi);
        font::draw_text_w(cv, clip, xf, cell.x + (CW - tw) / 2, cell.y + 25, &t, 13.0, tc, CW, font::W::Semi);
        if day.map_or(false, |x| x.note.is_some()) {
            let dc = if n == today_n { col::ACCENT_INK } else if inside { col::ACCENT } else { argb(110, 167, 139, 250) };
            let dot = Rect::new(cell.x + CW / 2 - 2, cell.y + 30, 4, 4);
            cv.rrect(clip, xf.r(dot), xf.len(2), dc);
        }
    }
    // The note line: writing one, the one pointed at, or how to make them.
    let ny = p.y + Y_NOTE;
    let hovered = match hot {
        Some(CHot::Day(i)) => Some(start + i as i64),
        _ => None,
    };
    if let Some((n, f)) = &c.edit {
        let r = rect_of(CHot::Note).unwrap_or(Rect::new(p.x + PAD, ny + 2, p.w - 2 * PAD, 34));
        cv.rrect(clip, xf.r(r), xf.len(10), argb(22, 255, 255, 255));
        cv.rrect_stroke(clip, xf.r(r), xf.len(10), 1.5 * s, 0xa78bfa, 200, 200);
        if f.is_empty() {
            let ph = format!("Заметка на {}", day_name(*n));
            font::draw_text(cv, clip, xf, r.x + 12, r.y + 22, &ph, 13.0, col::TEXT_DIM, r.w - 24);
        }
        f.draw(cv, clip, xf, r.x + 12, r.y + 22, r.w - 24, 13.0, col::TEXT, font::W::Text);
    } else if let Some((n, note)) = hovered.and_then(|n| c.note(n).map(|t| (n, t))) {
        font::draw_text(cv, clip, xf, p.x + PAD + 2, ny + 13, &day_name(n), 11.5, col::TEXT_DIM, p.w);
        let t = shell::ellipsize(note, 13.0, font::W::Text, p.w - 2 * PAD - 4);
        font::draw_text(cv, clip, xf, p.x + PAD + 2, ny + 31, &t, 13.0, col::TEXT, p.w - 2 * PAD);
    } else {
        let t = "ПКМ по дню — отметка, СКМ — заметка";
        font::draw_text(cv, clip, xf, p.x + PAD + 2, ny + 24, t, 11.5, argb(100, 255, 255, 255), p.w - 2 * PAD);
    }
    cv.fill(clip, xf.r(Rect::new(p.x + PAD, p.y + Y_SEP, p.w - 2 * PAD, 1)), argb(26, 255, 255, 255));

    // Timer / alarm switcher, and the other one's state on the right.
    let seg = Rect::new(p.x + PAD, p.y + Y_TABS, 206, 30);
    cv.rrect(clip, xf.r(seg), xf.len(15), argb(18, 255, 255, 255));
    for (t, label) in [(Tab::Timer, "Таймер"), (Tab::Alarm, "Будильник")] {
        let Some(r) = rect_of(CHot::Tab(t)) else { continue };
        if c.tab == t {
            cv.rrect(clip, xf.r(r), xf.len(13), argb(46, 255, 255, 255));
        } else if hov(CHot::Tab(t)) {
            cv.rrect(clip, xf.r(r), xf.len(13), argb(20, 255, 255, 255));
        }
        let tw = font::text_width_w(label, 12.5, font::W::Semi);
        let lc = if c.tab == t { col::TEXT } else { col::TEXT_DIM };
        font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + 18, label, 12.5, lc, r.w, font::W::Semi);
    }
    let other = match c.tab {
        Tab::Timer if c.alarm_on => Some(format!("Будильник {:02}:{:02}", c.alarm.0, c.alarm.1)),
        Tab::Alarm if c.run != Run::Idle => Some(format!("Таймер {}", short_time(c.timer_left(now)))),
        _ => None,
    };
    if let Some(o) = other {
        let max = p.x1() - PAD - seg.x1() - 8;
        let o = shell::ellipsize(&o, 11.5, font::W::Text, max);
        let ow = font::text_width(&o, 11.5);
        font::draw_text(cv, clip, xf, p.x1() - PAD - ow, p.y + Y_TABS + 20, &o, 11.5, col::TEXT_DIM, max);
    }

    // The big digits, or what went off.
    if let Some(r) = &c.ring {
        let msg = match r.what {
            Tab::Timer => "Время вышло".to_string(),
            Tab::Alarm => format!("Будильник {:02}:{:02}", c.alarm.0, c.alarm.1),
        };
        let pulse = (r.since.elapsed().as_secs_f32() * 3.0).sin() * 0.5 + 0.5;
        let mc = argb(255, 255, (200.0 - 40.0 * pulse) as u32, 255);
        let mw = p.w - 2 * PAD - 96 - 12;
        font::draw_text_w(cv, clip, xf, p.x + PAD, p.y + Y_BIG + 32, &shell::ellipsize(&msg, 20.0, font::W::Bold, mw), 20.0, mc, mw, font::W::Bold);
    } else {
        let adjustable = c.tab == Tab::Alarm || c.run == Run::Idle;
        let dc = match (c.tab, c.run) {
            (Tab::Timer, Run::Paused(_)) => col::TEXT_DIM,
            (Tab::Alarm, _) if !c.alarm_on => argb(200, 255, 255, 255),
            _ => col::TEXT,
        };
        for (k, r, t) in digit_rects(p, &c.parts(now)) {
            if let Some(k) = k {
                if adjustable && hov(CHot::Part(k)) {
                    cv.rrect(clip, xf.r(r.inflate(4)), xf.len(8), argb(24, 255, 255, 255));
                    // Up / down: the wheel sets it.
                    let mx = r.x as f32 + r.w as f32 / 2.0;
                    let lw = 1.4 * s;
                    let (t0, b0) = (r.y as f32 - 1.0, r.y1() as f32 + 1.0);
                    cv.line(clip, phys(xf, mx - 4.0, t0 + 3.0), phys(xf, mx, t0), lw, col::TEXT_DIM);
                    cv.line(clip, phys(xf, mx, t0), phys(xf, mx + 4.0, t0 + 3.0), lw, col::TEXT_DIM);
                    cv.line(clip, phys(xf, mx - 4.0, b0 - 3.0), phys(xf, mx, b0), lw, col::TEXT_DIM);
                    cv.line(clip, phys(xf, mx, b0), phys(xf, mx + 4.0, b0 - 3.0), lw, col::TEXT_DIM);
                }
            }
            let x = if k.is_none() { r.x + 1 } else { r.x };
            font::draw_text_w(cv, clip, xf, x, r.y + 36, &t, BIG, dc, r.w + 4, font::W::Bold);
        }
    }
    if let Some(r) = rect_of(CHot::Start) {
        let label = match (&c.ring, c.run) {
            (Some(_), _) => "Стоп",
            (None, Run::Idle) => "Старт",
            (None, Run::Running(_)) => "Пауза",
            (None, Run::Paused(_)) => "Дальше",
        };
        if c.ring.is_some() || c.run == Run::Idle {
            cv.rrect(clip, xf.r(r), xf.len(10), if hov(CHot::Start) { argb(255, 186, 166, 255) } else { col::ACCENT });
            let tw = font::text_width_w(label, 13.0, font::W::Semi);
            font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + 22, label, 13.0, col::ACCENT_INK, r.w, font::W::Semi);
        } else {
            crate::settings::button_pub(cv, clip, xf, r, label, hov(CHot::Start));
        }
    }
    if let Some(r) = rect_of(CHot::Reset) {
        let on = c.run != Run::Idle;
        if on && hov(CHot::Reset) {
            cv.rrect(clip, xf.r(r), xf.len(10), argb(24, 255, 255, 255));
        }
        // Back to the start: an arc turning back, its head at the top left.
        let (cx, cy) = (r.x as f32 + 17.0, r.y as f32 + 17.0);
        let rc = if on { col::TEXT } else { argb(80, 255, 255, 255) };
        let (a0, a1) = ((-155.0f32).to_radians(), 125.0f32.to_radians());
        cv.arc(clip, phys(xf, cx, cy), 6.0 * s, 1.6 * s, a0, a1, rc);
        let (ca, sa) = (a0.cos(), a0.sin());
        let tip = (cx + 6.0 * ca + sa * 1.0, cy + 6.0 * sa - ca * 1.0);
        let back = (tip.0 - sa * 3.4, tip.1 + ca * 3.4);
        cv.line(clip, phys(xf, tip.0, tip.1), phys(xf, back.0 - ca * 3.0, back.1 - sa * 3.0), 1.6 * s, rc);
        cv.line(clip, phys(xf, tip.0, tip.1), phys(xf, back.0 + ca * 3.0, back.1 + sa * 3.0), 1.6 * s, rc);
    }
    if let Some(r) = rect_of(CHot::Switch) {
        let on = c.alarm_on;
        if on {
            cv.rrect(clip, xf.r(r), xf.len(12), if hov(CHot::Switch) { argb(255, 186, 166, 255) } else { col::ACCENT });
        } else {
            cv.rrect_stroke(clip, xf.r(r), xf.len(12), 1.5 * s, 0xffffff, 150, 150);
        }
        let k = if on { Rect::new(r.x1() - 20, r.y + 4, 16, 16) } else { Rect::new(r.x + 5, r.y + 5, 14, 14) };
        cv.rrect(clip, xf.r(k), xf.len(8), if on { col::ACCENT_INK } else { argb(200, 255, 255, 255) });
        let t = if on { "Вкл." } else { "Выкл." };
        let tw = font::text_width(t, 12.0);
        font::draw_text(cv, clip, xf, r.x - tw - 10, r.y + 17, t, 12.0, col::TEXT_DIM, 60);
    }
    let labels = chips(c);
    for (k, label) in labels.iter().enumerate() {
        let Some(r) = rect_of(CHot::Chip(k as u8)) else { continue };
        let sel = c.tab == Tab::Timer && c.ring.is_none() && TIMER_PRESETS.get(k).map_or(false, |m| m * 60 == c.timer_s);
        cv.rrect(clip, xf.r(r), xf.len(15), argb(if hov(CHot::Chip(k as u8)) { 38 } else { 18 }, 255, 255, 255));
        if sel {
            cv.rrect_stroke(clip, xf.r(r), xf.len(15), 1.5 * s, 0xa78bfa, 220, 220);
        }
        let tw = font::text_width_w(label, 12.0, font::W::Semi);
        font::draw_text_w(cv, clip, xf, r.x + (r.w - tw) / 2, r.y + 19, label, 12.0, col::TEXT, r.w, font::W::Semi);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_file_round_trip() {
        let text = "2026-11-25\tmark\t\n2026-11-25\tnote\tКупить хлеба\n2026-12-31\tnote\tНовый год\nмусор\n2026-02-30\tmark\t\n";
        let days = parse_days(text);
        assert_eq!(days.len(), 2, "a bad date and a bad line are skipped");
        let n = days_from_civil(2026, 11, 25);
        assert_eq!(days[&n], Day { mark: true, note: Some("Купить хлеба".into()) });
        assert_eq!(parse_days(&days_text(&days)), days);
        assert_eq!(date_str(n), "2026-11-25");
    }

    #[test]
    fn marks_and_notes_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("resc-cal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let n = days_from_civil(2026, 11, 25);
        let mut c = Cal::load(dir.clone());
        c.toggle_mark(n);
        c.set_note(n, Some("  с\tтабом \n".into()));
        c.alarm_in((23, 50), 15);
        let c2 = Cal::load(dir.clone());
        assert_eq!(c2.days[&n], Day { mark: true, note: Some("с табом".into()) }, "kept on one line");
        assert_eq!((c2.alarm, c2.alarm_on), ((0, 5), true));
        // Off again and empty: the day is gone.
        let mut c3 = c2;
        c3.toggle_mark(n);
        c3.set_note(n, Some("   ".into()));
        assert!(c3.days.is_empty());
        assert!(Cal::load(dir.clone()).days.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timer_runs_pauses_and_goes_off() {
        let t0 = Instant::now();
        let mut c = Cal::default();
        c.timer_s = 90;
        assert_eq!(c.parts(t0), ["01", "30"]);
        c.start_pause(t0);
        assert_eq!(c.timer_left(t0 + Duration::from_millis(500)), 90);
        assert_eq!(c.timer_left(t0 + Duration::from_secs(30)), 60);
        // Paused at 60: time doesn't move.
        c.start_pause(t0 + Duration::from_secs(30));
        assert_eq!(c.timer_left(t0 + Duration::from_secs(500)), 60);
        assert_eq!(c.due(t0 + Duration::from_secs(500), (0, 0, 0)), None);
        c.start_pause(t0 + Duration::from_secs(500));
        assert_eq!(c.due(t0 + Duration::from_secs(559), (0, 0, 0)), None);
        assert_eq!(c.due(t0 + Duration::from_secs(560), (0, 0, 0)), Some(Tab::Timer));
        assert_eq!(c.run, Run::Idle);
        assert_eq!(c.timer_left(t0), 90, "idle again, at its length");
        // The wheel: minutes and tens of seconds, never below 10 s; not while running.
        c.adjust(0, true);
        c.adjust(1, false);
        assert_eq!(c.timer_s, 140);
        for _ in 0..30 {
            c.adjust(0, false);
        }
        assert_eq!(c.timer_s, 10);
        c.timer_s = 3600;
        assert_eq!(c.parts(t0), ["1", "00", "00"]);
        c.adjust(0, true);
        assert_eq!(c.timer_s, 7200);
        c.start_pause(t0);
        c.adjust(0, true);
        assert_eq!(c.timer_s, 7200);
        assert_eq!(short_time(3725), "1:02:05");
        assert_eq!(short_time(65), "1:05");
    }

    #[test]
    fn alarm_goes_off_once() {
        let t0 = Instant::now();
        let mut c = Cal::default();
        c.tab = Tab::Alarm;
        c.adjust(0, false);
        c.adjust(1, false);
        assert_eq!(c.alarm, (6, 59), "the wheel wraps");
        c.alarm_in((23, 30), 45);
        assert_eq!(c.alarm, (0, 15), "past midnight");
        assert!(c.alarm_on);
        assert_eq!(c.due(t0, (100, 0, 14)), None);
        assert_eq!(c.due(t0, (100, 0, 15)), Some(Tab::Alarm));
        assert!(!c.alarm_on, "once");
        assert_eq!(c.due(t0, (100, 0, 15)), None);
        // On again in the same minute: not again until it is that time again.
        c.alarm_on = true;
        assert_eq!(c.due(t0, (100, 0, 15)), None);
        assert_eq!(c.due(t0, (101, 0, 15)), Some(Tab::Alarm));
    }

    #[test]
    fn ringing_chimes_now_and_then() {
        let t0 = Instant::now();
        let mut c = Cal::default();
        c.start_ring(Tab::Timer, t0);
        assert!(c.chime_due(t0));
        assert!(!c.chime_due(t0 + Duration::from_secs(1)));
        assert!(c.chime_due(t0 + Duration::from_secs(4)));
        let w = chime_wav();
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize, w.len() - 8);
    }
}
