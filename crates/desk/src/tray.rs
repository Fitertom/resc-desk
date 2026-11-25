//! The taskbar's right island: the keyboard layout, network, sound and the clock, and the
//! glass flyouts they open above it, like Windows' quick settings and calendar:
//! - network: Wi-Fi on/off, the networks around (connect, a password for a new secured
//!   one, disconnect), the cable; NetworkManager over our D-Bus client (net.rs), polled;
//! - sound: the output's volume (a slider; the speaker mutes), the outputs to choose from,
//!   the same for the microphone, and a way to the sound settings;
//! - calendar: today's date, a month with marks and notes, a timer and an alarm (cal.rs).
//!
//! Sound goes through our PulseAudio client (pulse.rs): one connection, subscribed to
//! changes, whose socket sits in the main loop (TOK_PULSE).

use crate::desk::{Desk, TOK_PULSE};
use crate::font;
use crate::geom::Rect;
use crate::net::{self, Net};
use crate::pulse::{Audio, Device, Pulse};
use crate::render::{argb, Canvas, Xf};
use crate::shell::{self, col, phys};

pub const FLY_R: i32 = 20;
const FLY_GAP: i32 = 10;
const PAD: i32 = 16;
/// Volume step of the keys and the wheel, percent.
const STEP: i32 = 5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fly {
    Net,
    Sound,
    Calendar,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hot {
    /// The speaker / microphone button (mute). `true`: the input.
    Mute(bool),
    Slider(bool),
    Device(bool, u32),
    Settings,
    /// On the calendar flyout (cal.rs).
    Cal(crate::cal::CHot),
    WifiToggle,
    /// A network row (its index in the list shown).
    Ap(u16),
    Disconnect,
    PwField,
    PwConnect,
    NetSettings,
}

#[derive(Default)]
pub struct Tray {
    pub fly: Option<(usize, Fly)>,
    /// The calendar flyout's state: marks, notes, the timer and the alarm.
    pub cal: crate::cal::Cal,
    /// A volume slider held (input?), and its value while held.
    drag: Option<bool>,
    hot: Option<Hot>,
    conn: Option<Pulse>,
    pub audio: Audio,
    retry_ms: u32,
    // Network.
    net_bus: Option<crate::dbus::Bus>,
    pub net: Net,
    net_poll_ms: u32,
    net_retry_ms: u32,
    /// The password being typed, and for which network.
    pw: crate::textedit::TextField,
    pw_for: Option<String>,
    /// A connection started: to which network, when.
    pending: Option<(String, u32)>,
    net_msg: Option<String>,
}

// ------------------------------------------------------------------ sound connection

pub fn init(d: &mut Desk) {
    connect(d);
    crate::cal::init(d);
}

fn connect(d: &mut Desk) {
    d.tray.retry_ms = d.now_ms();
    let res = Pulse::connect("resc-desk").and_then(|mut p| {
        p.subscribe()?;
        let a = p.state()?;
        Ok((p, a))
    });
    match res {
        Ok((p, a)) => {
            let _ = d.ep.add(p.fd(), TOK_PULSE, sys::EPOLLIN);
            d.tray.conn = Some(p);
            d.tray.audio = a;
            damage(d);
        }
        Err(e) => {
            if d.tray.retry_ms == 0 || std::env::var("RESC_DEBUG").is_ok() {
                eprintln!("sound: {e}");
            }
        }
    }
}

fn drop_conn(d: &mut Desk) {
    if let Some(p) = d.tray.conn.take() {
        d.ep.remove(p.fd());
    }
    d.tray.retry_ms = d.now_ms();
    d.tray.audio = Audio::default();
    damage(d);
}

/// The server's socket is readable: change events.
pub fn readable(d: &mut Desk) {
    let Some(p) = d.tray.conn.as_mut() else { return };
    match p.drain() {
        Ok(true) => refresh(d),
        Ok(false) => {}
        Err(_) => drop_conn(d),
    }
}

/// Once a second: reconnect after the server went away (every 5 s).
pub fn housekeeping(d: &mut Desk) {
    if d.tray.conn.is_none() && d.now_ms().wrapping_sub(d.tray.retry_ms) >= 5000 {
        connect(d);
    }
    net_poll(d, false);
    crate::cal::tick(d);
}

// ------------------------------------------------------------------ network

/// Reads NetworkManager's state: every 5 s, every 1.5 s while the flyout is open or a
/// connection is being made; `force`: now.
fn net_poll(d: &mut Desk, force: bool) {
    let now = d.now_ms();
    let open = is_open(d, Fly::Net);
    let every = if open || d.tray.pending.is_some() { 1500 } else { 5000 };
    if !force && now.wrapping_sub(d.tray.net_poll_ms) < every {
        return;
    }
    d.tray.net_poll_ms = now;
    if d.tray.net_bus.is_none() {
        if !force && now.wrapping_sub(d.tray.net_retry_ms) < 10_000 && d.tray.net_retry_ms != 0 {
            return;
        }
        d.tray.net_retry_ms = now.max(1);
        match crate::dbus::Bus::system() {
            Ok(b) => d.tray.net_bus = Some(b),
            Err(_) => {
                set_net(d, Net::default());
                return;
            }
        }
    }
    let bus = d.tray.net_bus.as_mut().unwrap();
    match net::state(bus, open) {
        Ok(mut n) => {
            if !open {
                for a in &mut n.aps {
                    a.saved = d.tray.net.aps.iter().any(|x| x.ssid == a.ssid && x.saved);
                }
            }
            net_pending(d, &n, now);
            set_net(d, n);
        }
        Err(_) => {
            d.tray.net_bus = None;
            d.tray.net_retry_ms = now.max(1);
            set_net(d, Net::default());
        }
    }
}

fn set_net(d: &mut Desk, n: Net) {
    if n != d.tray.net {
        d.tray.net = n;
        damage(d);
    }
}

/// Follows a connection being made: done, or failed (a password again for a secured one).
fn net_pending(d: &mut Desk, n: &Net, now: u32) {
    let Some((ssid, since)) = d.tray.pending.clone() else { return };
    let on = n.active().map_or(false, |a| a.ssid == ssid);
    if on && n.wifi_state == net::DEV_ACTIVATED {
        d.tray.pending = None;
        d.tray.net_msg = None;
        d.tray.pw_for = None;
        return;
    }
    let waited = now.wrapping_sub(since);
    let failed = waited > 3000 && (n.wifi_state == net::DEV_FAILED || (!n.connecting() && !on)) || waited > 45_000;
    if failed {
        d.tray.pending = None;
        d.tray.net_msg = Some(format!("Не удалось подключиться к «{ssid}»"));
        if n.aps.iter().any(|a| a.ssid == ssid && a.secure) {
            d.tray.pw_for = Some(ssid);
            d.tray.pw.clear();
        }
        damage_fly(d);
    }
}

fn net_op(d: &mut Desk, f: impl FnOnce(&mut crate::dbus::Bus, &Net) -> std::io::Result<()>) {
    let n = d.tray.net.clone();
    let Some(bus) = d.tray.net_bus.as_mut() else { return };
    if let Err(e) = f(bus, &n) {
        d.tray.net_msg = Some(e.to_string());
    }
    net_poll(d, true);
    damage_fly(d);
}

fn net_connect(d: &mut Desk, ssid: &str, password: Option<String>) {
    let Some(ap) = d.tray.net.aps.iter().find(|a| a.ssid == ssid).cloned() else { return };
    let Some(dev) = d.tray.net.wifi_dev.clone() else { return };
    d.tray.net_msg = None;
    d.tray.pending = Some((ssid.to_string(), d.now_ms()));
    d.tray.pw_for = None;
    net_op(d, |bus, _| net::connect(bus, &dev, &ap, password.as_deref()));
}

/// The password field has the keyboard.
pub fn wants_keys(d: &Desk) -> bool {
    (is_open(d, Fly::Net) && d.tray.pw_for.is_some()) || is_open(d, Fly::Calendar)
}

/// A key while a flyout wants them. True if used.
pub fn key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    if is_open(d, Fly::Calendar) {
        return crate::cal::key(d, code, pressed);
    }
    if !pressed {
        return true;
    }
    use crate::input::key;
    match code {
        key::ESC => {
            d.seat.swallowed.push(code);
            d.tray.pw_for = None;
        }
        key::ENTER => {
            d.seat.swallowed.push(code);
            if let Some(ssid) = d.tray.pw_for.clone() {
                let pw = d.tray.pw.text();
                if pw.chars().count() >= 8 {
                    net_connect(d, &ssid, Some(pw));
                } else {
                    d.tray.net_msg = Some("Пароль Wi-Fi — не короче 8 символов".into());
                }
            }
        }
        _ => {
            let m = crate::input::text_mods(d);
            let _ = d.tray.pw.key(code, m);
        }
    }
    damage_fly(d);
    true
}

fn open_net_settings(d: &mut Desk) {
    close(d);
    let t = d.cfg.terminal.clone();
    let first = t.split_whitespace().next().unwrap_or("foot").to_string();
    if first.ends_with("foot") {
        d.spawn(&format!("{first} nmtui"));
    } else {
        d.spawn(&format!("{first} -e nmtui"));
    }
}

fn refresh(d: &mut Desk) {
    let Some(p) = d.tray.conn.as_mut() else { return };
    match p.state() {
        Ok(a) => {
            if a != d.tray.audio {
                d.tray.audio = a;
                damage(d);
            }
        }
        Err(_) => drop_conn(d),
    }
}

fn op(d: &mut Desk, f: impl FnOnce(&mut Pulse, &Audio) -> std::io::Result<()>) {
    let audio = d.tray.audio.clone();
    let Some(p) = d.tray.conn.as_mut() else { return };
    if f(p, &audio).is_err() {
        drop_conn(d);
        return;
    }
    refresh(d);
}

fn dev(a: &Audio, input: bool) -> Option<Device> {
    if input { a.source() } else { a.sink() }.cloned()
}

fn set_volume(d: &mut Desk, input: bool, percent: u32) {
    op(d, |p, a| {
        let Some(s) = dev(a, input) else { return Ok(()) };
        if s.percent() == percent {
            return Ok(());
        }
        p.set_volume(&s, input, percent)
    });
}

/// Volume keys and the wheel: `delta` percent, onto the step grid; louder unmutes.
pub fn volume_step(d: &mut Desk, delta: i32) {
    op(d, |p, a| {
        let Some(s) = a.sink().cloned() else { return Ok(()) };
        let v = ((s.percent() as i32 + delta) as f32 / STEP as f32).round() as i32 * STEP;
        let v = v.clamp(0, crate::pulse::MAX_PERCENT as i32) as u32;
        if s.mute && delta > 0 {
            p.set_mute(&s, false, false)?;
        }
        p.set_volume(&s, false, v)
    });
}

pub fn toggle_mute(d: &mut Desk, input: bool) {
    op(d, |p, a| match dev(a, input) {
        Some(s) => p.set_mute(&s, input, !s.mute),
        None => Ok(()),
    });
}

fn set_default(d: &mut Desk, input: bool, index: u32) {
    op(d, |p, a| {
        let list = if input { &a.sources } else { &a.sinks };
        match list.iter().find(|s| s.index == index) {
            Some(s) => p.set_default(&s.name, input),
            None => Ok(()),
        }
    });
}

// ------------------------------------------------------------------ the island

/// The island's width: layout, network, sound, clock.
pub const ISLAND_W: i32 = 4 + 36 + 2 + 40 + 2 + 40 + 2 + 96 + 4;

/// Redraws the island and the open flyout.
pub fn damage(d: &mut Desk) {
    for oi in 0..d.outputs.len() {
        let r = shell::islands(d, oi)[1];
        d.damage(r.inflate(shell::GLASS_REACH));
    }
    damage_fly(d);
}

fn damage_fly(d: &mut Desk) {
    if let Some(r) = fly_rect(d) {
        d.damage(r.inflate(shell::GLASS_REACH));
    }
}

/// The island's speaker: a body and cone, up to three waves by the volume, or a cross when
/// muted; centred on logical (`cx`, `cy`).
pub fn draw_speaker(cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, dev: Option<&Device>, c: u32) {
    let lw = 1.6 * xf.scale as f32;
    let (x0, x1) = (cx - 7.0, cx - 1.0);
    // Body and cone, filled with thin vertical strokes.
    let mut x = x0;
    while x <= x1 {
        let h = if x < cx - 4.0 { 2.6 } else { 2.6 + (x - (cx - 4.0)) * 1.4 };
        cv.line(clip, phys(xf, x, cy - h), phys(xf, x, cy + h), 1.1 * xf.scale as f32, c);
        x += 0.5;
    }
    let Some(dev) = dev else {
        // No sound server: a slash.
        cv.line(clip, phys(xf, cx - 9.0, cy - 8.0), phys(xf, cx + 8.0, cy + 8.0), lw, c);
        return;
    };
    if dev.mute {
        let (mx, my) = (cx + 5.5, cy);
        cv.line(clip, phys(xf, mx - 3.0, my - 3.0), phys(xf, mx + 3.0, my + 3.0), lw, c);
        cv.line(clip, phys(xf, mx - 3.0, my + 3.0), phys(xf, mx + 3.0, my - 3.0), lw, c);
        return;
    }
    let p = dev.percent();
    let waves = if p == 0 { 0 } else if p < 34 { 1 } else if p < 67 { 2 } else { 3 };
    for i in 0..waves {
        let r = 4.0 + i as f32 * 3.6;
        arc(cv, clip, xf, (cx, cy), r, 0.85, lw, c);
    }
}

/// An arc of `radius` from angle `a0` to `a1` (radians, y down).
fn arc_at(cv: &mut Canvas, clip: Rect, xf: Xf, c: (f32, f32), radius: f32, a0: f32, a1: f32, w: f32, color: u32) {
    let n = 12;
    let pt = |i: i32| {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        phys(xf, c.0 + radius * a.cos(), c.1 + radius * a.sin())
    };
    for i in 0..n {
        cv.line(clip, pt(i), pt(i + 1), w, color);
    }
}

/// Wi-Fi: a dot and three arcs, `level` (0–4) of them lit; a slash when `off`.
pub fn draw_wifi(cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, level: u8, off: bool, c: u32) {
    let lw = 1.7 * xf.scale as f32;
    let dim = argb(80, 255, 255, 255);
    let o = (cx, cy + 6.0);
    let up = -std::f32::consts::FRAC_PI_2;
    let dot = Rect::new((o.0 - 1.5) as i32, (o.1 - 2.5) as i32, 3, 3);
    cv.rrect(clip, xf.r(dot), xf.len(2), if level >= 1 && !off { c } else { dim });
    for (i, r) in [5.0f32, 9.0, 13.0].into_iter().enumerate() {
        let lit = !off && level as usize >= i + 2;
        arc_at(cv, clip, xf, o, r, up - 0.78, up + 0.78, lw, if lit { c } else { dim });
    }
    if off {
        cv.line(clip, phys(xf, cx - 9.0, cy - 7.0), phys(xf, cx + 9.0, cy + 7.0), lw, c);
    }
}

/// A cable connection: three boxes joined.
fn draw_ethernet(cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, c: u32) {
    let lw = 1.6 * xf.scale as f32;
    let (x, y) = (cx as i32, cy as i32);
    cv.rrect(clip, xf.r(Rect::new(x - 4, y - 9, 8, 6)), xf.len(1), c);
    cv.line(clip, phys(xf, cx, cy - 3.0), phys(xf, cx, cy + 1.0), lw, c);
    cv.line(clip, phys(xf, cx - 6.0, cy + 1.0), phys(xf, cx + 6.0, cy + 1.0), lw, c);
    cv.line(clip, phys(xf, cx - 6.0, cy + 1.0), phys(xf, cx - 6.0, cy + 4.0), lw, c);
    cv.line(clip, phys(xf, cx + 6.0, cy + 1.0), phys(xf, cx + 6.0, cy + 4.0), lw, c);
    cv.rrect(clip, xf.r(Rect::new(x - 9, y + 4, 6, 5)), xf.len(1), c);
    cv.rrect(clip, xf.r(Rect::new(x + 3, y + 4, 6, 5)), xf.len(1), c);
}

fn draw_lock(cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, c: u32) {
    let lw = 1.4 * xf.scale as f32;
    arc_at(cv, clip, xf, (cx, cy - 1.0), 3.0, std::f32::consts::PI, 2.0 * std::f32::consts::PI, lw, c);
    cv.line(clip, phys(xf, cx - 3.0, cy - 1.0), phys(xf, cx - 3.0, cy + 1.0), lw, c);
    cv.line(clip, phys(xf, cx + 3.0, cy - 1.0), phys(xf, cx + 3.0, cy + 1.0), lw, c);
    cv.rrect(clip, xf.r(Rect::new(cx as i32 - 5, cy as i32 + 1, 10, 7)), xf.len(2), c);
}

fn bars(strength: u8) -> u8 {
    match strength {
        76.. => 4,
        51..=75 => 3,
        26..=50 => 2,
        _ => 1,
    }
}

/// The island's network icon.
pub fn draw_net_icon(d: &Desk, cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, c: u32) {
    let n = &d.tray.net;
    if n.wired && n.active().is_none() {
        draw_ethernet(cv, clip, xf, cx, cy, c);
    } else if let Some(a) = n.active() {
        draw_wifi(cv, clip, xf, cx, cy, bars(a.strength), false, c);
    } else {
        draw_wifi(cv, clip, xf, cx, cy, 0, !n.wifi_enabled || !n.available, c);
    }
}

/// An arc of `radius` from -`half` to +`half` radians around the +x axis.
fn arc(cv: &mut Canvas, clip: Rect, xf: Xf, c: (f32, f32), radius: f32, half: f32, w: f32, color: u32) {
    let n = 10;
    let pt = |i: i32| {
        let a = -half + 2.0 * half * i as f32 / n as f32;
        phys(xf, c.0 + radius * a.cos(), c.1 + radius * a.sin())
    };
    for i in 0..n {
        cv.line(clip, pt(i), pt(i + 1), w, color);
    }
}

/// A microphone: a capsule, its cradle and stand.
fn draw_mic(cv: &mut Canvas, clip: Rect, xf: Xf, cx: f32, cy: f32, dev: Option<&Device>, c: u32) {
    let lw = 1.6 * xf.scale as f32;
    let cap = Rect::new((cx - 3.0) as i32, (cy - 8.0) as i32, 6, 11);
    cv.rrect(clip, xf.r(cap), xf.len(3), c);
    let n = 8;
    let pt = |i: i32| {
        let a = std::f32::consts::PI * i as f32 / n as f32;
        phys(xf, cx + 6.0 * a.cos(), cy - 1.0 + 5.5 * a.sin())
    };
    for i in 0..n {
        cv.line(clip, pt(i), pt(i + 1), lw, c);
    }
    cv.line(clip, phys(xf, cx, cy + 4.5), phys(xf, cx, cy + 8.0), lw, c);
    if dev.map_or(true, |d| d.mute) {
        cv.line(clip, phys(xf, cx - 8.0, cy - 8.0), phys(xf, cx + 8.0, cy + 8.0), lw, c);
    }
}

// ------------------------------------------------------------------ flyouts

pub fn toggle(d: &mut Desk, oi: usize, f: Fly) {
    shell::close_menus(d);
    damage_fly(d);
    if d.tray.fly.map(|x| x.1) == Some(f) {
        d.tray.fly = None;
        return;
    }
    d.tray.fly = Some((oi, f));
    d.tray.hot = None;
    if f == Fly::Calendar {
        crate::cal::opened(d);
    }
    if f == Fly::Sound {
        refresh(d);
    }
    if f == Fly::Net {
        d.tray.pw_for = None;
        d.tray.net_msg = None;
        net_poll(d, true);
        if let Some(dev) = d.tray.net.wifi_dev.clone() {
            if let Some(bus) = d.tray.net_bus.as_mut() {
                let _ = net::scan(bus, &dev);
            }
        }
    }
    damage_fly(d);
    shell::damage_taskbars(d);
}

pub fn close(d: &mut Desk) {
    if d.tray.fly.is_some() {
        damage_fly(d);
        d.tray.fly = None;
        d.tray.drag = None;
        d.tray.pw_for = None;
        crate::cal::closed(d);
        shell::damage_taskbars(d);
    }
}

pub fn is_open(d: &Desk, f: Fly) -> bool {
    d.tray.fly.map(|x| x.1) == Some(f)
}

struct Lay {
    panel: Rect,
    hits: Vec<(Hot, Rect)>,
}

const ROW: i32 = 36;
const VOL_H: i32 = 44;

fn sound_height(a: &Audio) -> i32 {
    let mut h = PAD + 22 + 10 + VOL_H + 22 + a.sinks.len().max(1) as i32 * ROW;
    if !a.sources.is_empty() {
        h += 14 + VOL_H + 22 + a.sources.len() as i32 * ROW;
    }
    h + 12 + 40 + 6
}

const NET_TOP: i32 = PAD + 50;
const NET_ROW: i32 = 44;
const NET_PW: i32 = 48;
const NET_MAX: usize = 7;

/// The networks listed (Wi-Fi on).
fn net_rows(d: &Desk) -> Vec<net::Ap> {
    let n = &d.tray.net;
    if !n.available || !n.wifi_enabled || n.wifi_dev.is_none() {
        return Vec::new();
    }
    n.aps.iter().take(NET_MAX).cloned().collect()
}

fn net_height(d: &Desk, rows: &[net::Ap]) -> i32 {
    let mut h = NET_TOP + rows.len() as i32 * NET_ROW;
    if rows.is_empty() {
        h += 56;
    }
    if d.tray.pw_for.as_ref().map_or(false, |s| rows.iter().any(|a| &a.ssid == s)) {
        h += NET_PW;
    }
    if d.tray.net_msg.is_some() {
        h += 26;
    }
    h + 12 + 40 + 6
}

fn fly_rect(d: &Desk) -> Option<Rect> {
    lay(d).map(|l| l.panel)
}

fn lay(d: &Desk) -> Option<Lay> {
    let (oi, f) = d.tray.fly?;
    if oi >= d.outputs.len() {
        return None;
    }
    let island = shell::islands(d, oi)[1];
    let o = d.outputs[oi].rect();
    let net_rows = net_rows(d);
    let (w, h) = match f {
        Fly::Net => (360, net_height(d, &net_rows)),
        Fly::Sound => (360, sound_height(&d.tray.audio)),
        Fly::Calendar => (crate::cal::W, crate::cal::H),
    };
    let x = (island.x1() - w).clamp(o.x + 8, (o.x1() - 8 - w).max(o.x + 8));
    let panel = Rect::new(x, island.y - FLY_GAP - h, w, h);
    let mut hits = Vec::new();
    match f {
        Fly::Net => {
            hits.push((Hot::WifiToggle, Rect::new(panel.x1() - PAD - 44, panel.y + PAD - 4, 44, 26)));
            let mut y = panel.y + NET_TOP;
            for (i, a) in net_rows.iter().enumerate() {
                let row = Rect::new(panel.x + 8, y, panel.w - 16, NET_ROW);
                if a.active {
                    hits.push((Hot::Disconnect, Rect::new(row.x1() - 104, row.y + 8, 96, 28)));
                }
                hits.push((Hot::Ap(i as u16), row));
                y += NET_ROW;
                if d.tray.pw_for.as_deref() == Some(a.ssid.as_str()) {
                    hits.push((Hot::PwField, Rect::new(panel.x + 16, y + 6, panel.w - 32 - 112, 34)));
                    hits.push((Hot::PwConnect, Rect::new(panel.x1() - 16 - 104, y + 6, 104, 34)));
                    y += NET_PW;
                }
            }
            hits.push((Hot::NetSettings, Rect::new(panel.x + 8, panel.y + h - 6 - 40, panel.w - 16, 40)));
        }
        Fly::Sound => {
            let mut y = panel.y + PAD + 22 + 10;
            let a = &d.tray.audio;
            let mut section = |input: bool, list: &Vec<Device>, y: &mut i32, hits: &mut Vec<(Hot, Rect)>| {
                hits.push((Hot::Mute(input), Rect::new(panel.x + 10, *y + 4, 36, 36)));
                hits.push((Hot::Slider(input), Rect::new(panel.x + 56, *y + 7, panel.w - 56 - 58, 30)));
                *y += VOL_H + 22;
                for s in list {
                    hits.push((Hot::Device(input, s.index), Rect::new(panel.x + 8, *y, panel.w - 16, ROW)));
                    *y += ROW;
                }
                if list.is_empty() {
                    *y += ROW;
                }
            };
            section(false, &a.sinks, &mut y, &mut hits);
            if !a.sources.is_empty() {
                y += 14;
                section(true, &a.sources, &mut y, &mut hits);
            }
            y += 12;
            hits.push((Hot::Settings, Rect::new(panel.x + 8, y, panel.w - 16, 40)));
        }
        Fly::Calendar => {
            hits.extend(crate::cal::hits(d, panel).into_iter().map(|(h, r)| (Hot::Cal(h), r)));
        }
    }
    Some(Lay { panel, hits })
}

/// The flyout's glass panel (shell::glass_panels).
pub fn glass(d: &Desk, oi: usize) -> Option<Rect> {
    lay(d).filter(|_| d.tray.fly.map(|f| f.0) == Some(oi)).map(|l| l.panel)
}

pub fn contains(d: &Desk, x: f64, y: f64) -> bool {
    fly_rect(d).map_or(false, |r| r.contains(x, y))
}

fn slider_value(d: &Desk, input: bool, x: f64) -> Option<u32> {
    let l = lay(d)?;
    let (_, r) = l.hits.iter().find(|(h, _)| *h == Hot::Slider(input))?;
    let k = ((x - r.x as f64) / r.w.max(1) as f64).clamp(0.0, 1.0);
    Some((k * crate::pulse::MAX_PERCENT as f64).round() as u32)
}

/// A press. True if it was ours (on the flyout, or closing it).
pub fn click(d: &mut Desk, x: f64, y: f64, button: u32) -> bool {
    let Some(l) = lay(d) else { return false };
    if !l.panel.contains(x, y) {
        // Outside: away it goes. On its own island button the click ends here (else that
        // button would open it again).
        let f = d.tray.fly.map(|f| f.1);
        close(d);
        let own = shell::tray_item_at(d, x, y);
        return own.is_some() && own == f;
    }
    let Some(hot) = l.hits.iter().find(|(_, r)| r.contains(x, y)).map(|(h, _)| *h) else { return true };
    if let Hot::Cal(h) = hot {
        crate::cal::click(d, h, button);
        return true;
    }
    if button != crate::input::BTN_LEFT {
        return true;
    }
    match hot {
        Hot::Mute(input) => toggle_mute(d, input),
        Hot::Slider(input) => {
            d.tray.drag = Some(input);
            if let Some(v) = slider_value(d, input, x) {
                set_volume(d, input, v);
            }
        }
        Hot::Device(input, idx) => set_default(d, input, idx),
        Hot::Settings => {
            close(d);
            open_sound_settings(d);
        }
        Hot::Cal(_) => {}
        Hot::WifiToggle => {
            let on = !d.tray.net.wifi_enabled;
            net_op(d, |bus, _| net::set_wifi(bus, on));
        }
        Hot::Disconnect => {
            if let Some(dev) = d.tray.net.wifi_dev.clone() {
                net_op(d, |bus, _| net::disconnect(bus, &dev));
            }
        }
        Hot::Ap(i) => {
            let rows = net_rows(d);
            if let Some(a) = rows.get(i as usize) {
                if a.active {
                    // Already on it.
                } else if a.enterprise {
                    d.tray.net_msg = Some("Корпоративная сеть: подключитесь через «Параметры сети»".into());
                } else if a.secure && !a.saved {
                    d.tray.pw_for = Some(a.ssid.clone());
                    d.tray.pw.clear();
                    d.tray.net_msg = None;
                } else {
                    let s = a.ssid.clone();
                    net_connect(d, &s, None);
                }
            }
        }
        Hot::PwField => {}
        Hot::PwConnect => {
            if let Some(ssid) = d.tray.pw_for.clone() {
                let pw = d.tray.pw.text();
                if pw.chars().count() >= 8 {
                    net_connect(d, &ssid, Some(pw));
                } else {
                    d.tray.net_msg = Some("Пароль Wi-Fi — не короче 8 символов".into());
                }
            }
        }
        Hot::NetSettings => open_net_settings(d),
    }
    damage_fly(d);
    true
}

/// The pointer moved with a button held after a press on the shell.
pub fn drag(d: &mut Desk, x: f64) {
    let Some(input) = d.tray.drag else { return };
    if let Some(v) = slider_value(d, input, x) {
        set_volume(d, input, v);
    }
}

pub fn release(d: &mut Desk) {
    d.tray.drag = None;
}

pub fn hover(d: &mut Desk, x: f64, y: f64) {
    // The calendar goes away when the pointer leaves it, as Glint Calendar does.
    if is_open(d, Fly::Calendar) && crate::cal::hover(d, contains(d, x, y)) {
        close(d);
        return;
    }
    let hot = lay(d).and_then(|l| l.hits.into_iter().find(|(_, r)| r.contains(x, y)).map(|(h, _)| h));
    if hot != d.tray.hot {
        d.tray.hot = hot;
        damage_fly(d);
    }
}

/// The wheel: over the sound button or the flyout's sliders, the volume. True if used.
pub fn scroll(d: &mut Desk, x: f64, y: f64, value: f64) -> bool {
    if value == 0.0 {
        return false;
    }
    if is_open(d, Fly::Calendar) && contains(d, x, y) {
        let h = lay(d).and_then(|l| l.hits.into_iter().find(|(_, r)| r.contains(x, y))).and_then(|(h, _)| match h {
            Hot::Cal(c) => Some(c),
            _ => None,
        });
        crate::cal::scroll(d, h, value);
        return true;
    }
    let delta = if value > 0.0 { -STEP } else { STEP };
    let on_slider = lay(d).and_then(|l| l.hits.into_iter().find(|(h, r)| matches!(h, Hot::Slider(_) | Hot::Mute(_)) && r.contains(x, y)));
    match on_slider {
        Some((Hot::Slider(true) | Hot::Mute(true), _)) => {
            let v = d.tray.audio.source().map_or(0, |s| s.percent()) as i32 + delta;
            set_volume(d, true, v.clamp(0, 100) as u32);
            true
        }
        Some(_) => {
            volume_step(d, delta);
            true
        }
        None if shell::tray_item_at(d, x, y) == Some(Fly::Sound) || contains(d, x, y) => {
            if contains(d, x, y) {
                return true;
            }
            volume_step(d, delta);
            true
        }
        None => false,
    }
}

fn open_sound_settings(d: &mut Desk) {
    shell::open_settings_page(d, "sound");
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let Some((foi, f)) = d.tray.fly else { return };
    if foi != oi {
        return;
    }
    let Some(l) = lay(d) else { return };
    shell::draw_glass(cv, clip, xf, l.panel, FLY_R);
    match f {
        Fly::Net => draw_net(d, &l, xf, cv, clip),
        Fly::Sound => draw_sound(d, &l, xf, cv, clip),
        Fly::Calendar => {
            let hot = match d.tray.hot {
                Some(Hot::Cal(h)) => Some(h),
                _ => None,
            };
            crate::cal::draw(d, l.panel, hot, xf, cv, clip)
        }
    }
}

fn hot_bg(cv: &mut Canvas, clip: Rect, xf: Xf, r: Rect, rad: i32, on: bool) {
    if on {
        cv.rrect(clip, xf.r(r), xf.len(rad), argb(24, 255, 255, 255));
    }
}

fn draw_sound(d: &Desk, l: &Lay, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let p = l.panel;
    let a = &d.tray.audio;
    let hot = d.tray.hot;
    font::draw_text_w(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 16, "Звук", 15.0, col::TEXT, p.w, font::W::Semi);
    if d.tray.conn.is_none() {
        font::draw_text(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 60, "Нет связи со звуковым сервером", 12.5, col::TEXT_DIM, p.w - 2 * PAD);
        font::draw_text(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 80, "(PulseAudio или pipewire-pulse)", 12.5, col::TEXT_DIM, p.w - 2 * PAD);
    }
    for &(h, r) in &l.hits {
        match h {
            Hot::Mute(input) => {
                let dv = dev(a, input);
                hot_bg(cv, clip, xf, r, 10, hot == Some(h));
                let (cx, cy) = r.center();
                if input {
                    draw_mic(cv, clip, xf, cx as f32, cy as f32, dv.as_ref(), col::TEXT);
                } else {
                    draw_speaker(cv, clip, xf, cx as f32 + 1.0, cy as f32, dv.as_ref(), col::TEXT);
                }
            }
            Hot::Slider(input) => {
                let pct = dev(a, input).map_or(0, |s| s.percent());
                crate::settings::slider(cv, clip, xf, r, pct as f32 / crate::pulse::MAX_PERCENT as f32);
                let t = format!("{pct}");
                let tw = font::text_width_w(&t, 13.0, font::W::Bold);
                font::draw_text_w(cv, clip, xf, p.x1() - PAD - tw, r.y + 20, &t, 13.0, col::TEXT, 40, font::W::Bold);
                let label = if input { "Устройство ввода" } else { "Устройство вывода" };
                font::draw_text(cv, clip, xf, p.x + PAD + 2, r.y + 30 + 7 + 16, label, 12.0, col::TEXT_DIM, p.w);
            }
            Hot::Device(input, idx) => {
                hot_bg(cv, clip, xf, r, 10, hot == Some(h));
                let list = if input { &a.sources } else { &a.sinks };
                let Some(s) = list.iter().find(|s| s.index == idx) else { continue };
                let cur = Some(&s.name) == (if input { a.source() } else { a.sink() }).map(|x| &x.name);
                let (cx, cy) = (r.x as f32 + 18.0, r.y as f32 + r.h as f32 / 2.0);
                cv.ring(clip, phys(xf, cx, cy), 7.0 * xf.scale as f32, 1.5 * xf.scale as f32, if cur { col::ACCENT } else { col::TEXT_DIM });
                if cur {
                    let dot = Rect::new(cx as i32 - 3, cy as i32 - 3, 7, 7);
                    cv.rrect(clip, xf.r(dot), xf.len(4), col::ACCENT);
                }
                let tmax = r.w - 44;
                let name = shell::ellipsize(&s.desc, 13.0, font::W::Text, tmax);
                font::draw_text(cv, clip, xf, r.x + 34, r.y + 23, &name, 13.0, col::TEXT, tmax);
            }
            Hot::Settings => {
                cv.fill(clip, xf.r(Rect::new(p.x + PAD, r.y - 6, p.w - 2 * PAD, 1)), argb(26, 255, 255, 255));
                hot_bg(cv, clip, xf, r, 10, hot == Some(h));
                font::draw_text_w(cv, clip, xf, r.x + 10, r.y + 25, "Параметры звука", 13.0, col::TEXT, r.w, font::W::Semi);
                let (ax, ay) = (r.x1() as f32 - 18.0, r.y as f32 + 20.0);
                let lw = 1.6 * xf.scale as f32;
                cv.line(clip, phys(xf, ax - 3.0, ay - 5.0), phys(xf, ax + 2.0, ay), lw, col::TEXT_DIM);
                cv.line(clip, phys(xf, ax + 2.0, ay), phys(xf, ax - 3.0, ay + 5.0), lw, col::TEXT_DIM);
            }
            _ => {}
        }
    }
}

fn draw_net(d: &Desk, l: &Lay, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let p = l.panel;
    let n = &d.tray.net;
    let hot = d.tray.hot;
    font::draw_text_w(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 16, "Сеть", 15.0, col::TEXT, p.w, font::W::Semi);
    // What we are on.
    let status = if !n.available {
        "NetworkManager не запущен".to_string()
    } else if let Some((s, _)) = &d.tray.pending {
        format!("Подключение к «{s}»…")
    } else if let Some(a) = n.active() {
        format!("Wi-Fi: {}", a.ssid)
    } else if n.wired {
        "Подключено по кабелю".into()
    } else {
        "Нет подключения".into()
    };
    let st = shell::ellipsize(&status, 12.5, font::W::Text, p.w - 2 * PAD - 60);
    font::draw_text(cv, clip, xf, p.x + PAD + 2, p.y + PAD + 36, &st, 12.5, col::TEXT_DIM, p.w - 2 * PAD - 60);
    let rows = net_rows(d);
    let mut msg_y = p.y + NET_TOP + rows.len() as i32 * NET_ROW + 18;
    if rows.is_empty() {
        let (a, b) = if !n.available {
            ("Включите службу:", "sudo systemctl enable --now NetworkManager")
        } else if n.wifi_dev.is_none() {
            ("Нет адаптера Wi-Fi", "")
        } else if !n.wifi_enabled {
            ("Wi-Fi выключен", "")
        } else {
            ("Сети не найдены", "")
        };
        font::draw_text(cv, clip, xf, p.x + PAD + 2, p.y + NET_TOP + 22, a, 13.0, col::TEXT, p.w - 2 * PAD);
        font::draw_text(cv, clip, xf, p.x + PAD + 2, p.y + NET_TOP + 42, b, 12.0, col::TEXT_DIM, p.w - 2 * PAD);
        msg_y = p.y + NET_TOP + 56 + 18;
    }
    for &(h, r) in &l.hits {
        match h {
            Hot::WifiToggle if n.available && n.wifi_dev.is_some() => {
                let t = if n.wifi_enabled { 1.0 } else { 0.0 };
                crate::settings::switch(cv, clip, xf, r.x as f32, r.y as f32, t, 0.0);
            }
            Hot::Ap(i) => {
                let Some(a) = rows.get(i as usize) else { continue };
                hot_bg(cv, clip, xf, r, 10, hot == Some(h) || hot == Some(Hot::Disconnect) && a.active);
                if a.active {
                    cv.rrect(clip, xf.r(r), xf.len(10), argb(26, 167, 139, 250));
                }
                draw_wifi(cv, clip, xf, r.x as f32 + 20.0, r.y as f32 + r.h as f32 / 2.0 - 1.0, bars(a.strength), false, col::TEXT);
                let tmax = r.w - 44 - if a.active { 110 } else { 30 };
                let name = shell::ellipsize(&a.ssid, 13.0, font::W::Semi, tmax);
                font::draw_text_w(cv, clip, xf, r.x + 42, r.y + 19, &name, 13.0, col::TEXT, tmax, font::W::Semi);
                let pending = d.tray.pending.as_ref().map_or(false, |(s, _)| *s == a.ssid);
                let sub = if a.active && !pending {
                    "Подключено"
                } else if pending {
                    "Подключение…"
                } else if a.saved {
                    "Сохранена"
                } else if a.secure {
                    "Защищена"
                } else {
                    "Открытая"
                };
                font::draw_text(cv, clip, xf, r.x + 42, r.y + 35, sub, 11.5, col::TEXT_DIM, tmax);
                if a.secure && !a.active {
                    draw_lock(cv, clip, xf, r.x1() as f32 - 18.0, r.y as f32 + 18.0, col::TEXT_DIM);
                }
            }
            Hot::Disconnect => {
                crate::settings::button_pub(cv, clip, xf, r, "Отключить", hot == Some(h));
            }
            Hot::PwField => {
                cv.rrect(clip, xf.r(r), xf.len(10), argb(22, 255, 255, 255));
                cv.rrect_stroke(clip, xf.r(r), xf.len(10), 1.5 * xf.scale as f32, 0xa78bfa, 200, 200);
                let n = d.tray.pw.text().chars().count();
                if n == 0 {
                    font::draw_text(cv, clip, xf, r.x + 12, r.y + 22, "Пароль", 13.0, col::TEXT_DIM, r.w - 20);
                }
                let mut masked = crate::textedit::TextField::default();
                masked.insert(&"•".repeat(n));
                masked.caret = d.tray.pw.caret.min(n);
                masked.draw(cv, clip, xf, r.x + 12, r.y + 22, r.w - 20, 13.0, col::TEXT, font::W::Semi);
            }
            Hot::PwConnect => {
                crate::settings::button_pub(cv, clip, xf, r, "Подключить", hot == Some(h));
            }
            Hot::NetSettings => {
                cv.fill(clip, xf.r(Rect::new(p.x + PAD, r.y - 6, p.w - 2 * PAD, 1)), argb(26, 255, 255, 255));
                hot_bg(cv, clip, xf, r, 10, hot == Some(h));
                font::draw_text_w(cv, clip, xf, r.x + 10, r.y + 25, "Параметры сети", 13.0, col::TEXT, r.w, font::W::Semi);
                let (ax, ay) = (r.x1() as f32 - 18.0, r.y as f32 + 20.0);
                let lw = 1.6 * xf.scale as f32;
                cv.line(clip, phys(xf, ax - 3.0, ay - 5.0), phys(xf, ax + 2.0, ay), lw, col::TEXT_DIM);
                cv.line(clip, phys(xf, ax + 2.0, ay), phys(xf, ax - 3.0, ay + 5.0), lw, col::TEXT_DIM);
            }
            _ => {}
        }
    }
    if let Some(m) = &d.tray.net_msg {
        if d.tray.pw_for.is_some() {
            msg_y += NET_PW;
        }
        let m = shell::ellipsize(m, 12.0, font::W::Text, p.w - 2 * PAD);
        font::draw_text(cv, clip, xf, p.x + PAD + 2, msg_y, &m, 12.0, argb(255, 255, 140, 140), p.w - 2 * PAD);
    }
}

/// Days since 1970-01-01 (Howard Hinnant's days_from_civil).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 0 Monday … 6 Sunday.
pub fn weekday(y: i32, m: u32, d: u32) -> usize {
    (days_from_civil(y, m, d) + 3).rem_euclid(7) as usize
}

pub fn month_len(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_math() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(1970, 1, 1), 3); // Thursday
        assert_eq!(weekday(2026, 11, 24), 1); // Tuesday
        assert_eq!(crate::cal::civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(month_len(2024, 2), 29);
        assert_eq!(month_len(2100, 2), 28);
    }
}
