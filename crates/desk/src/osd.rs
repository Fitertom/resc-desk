//! The volume keys' feedback: liquify's volume control (the speaker and the slider after it)
//! over the screen, above the taskbar, with no panel behind it. As there the slider flows to
//! the new level on a spring, a mute flows it to nothing and back, the speaker crossfades to
//! its crossed-out twin and pops (from small, with a push) when muted or unmuted. It fades in
//! on a key and out 1.4 s after the last one.
use crate::anim::Spring;
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};

/// liquify's sizes (kit::volume) a quarter bigger: it is read from across the desk.
const K: f32 = 1.25;
const ICON: f32 = 18.0 * K;
const BOX: f32 = 32.0 * K;
const GAP: f32 = 6.0 * K;
const BAR_W: f32 = 120.0 * K;
const H: f32 = 24.0 * K;
const BAR: f32 = 4.0 * K;
const KNOB: f32 = 12.0 * K;
/// Above the taskbar.
const LIFT: i32 = 18;
const HOLD_MS: u32 = 1400;

pub struct Osd {
    /// The output it shows on (the pointer's, or the main one without a taskbar there).
    out: usize,
    /// 0 gone .. 1 shown.
    shown: Spring,
    /// liquify's motion.volume, motion.mute, motion.mute_pop.
    volume: Spring,
    mute: Spring,
    mute_pop: Spring,
    muted: Option<bool>,
    /// When the last key came (ms).
    at: u32,
}

impl Default for Osd {
    fn default() -> Self {
        Osd {
            out: 0,
            shown: Spring::new(0.0, 0.28, 1.0),
            volume: Spring::new(0.0, 0.36, 1.0),
            mute: Spring::new(0.0, 0.36, 1.0),
            mute_pop: Spring::new(1.0, 0.36, 0.38),
            muted: None,
            at: 0,
        }
    }
}

/// A volume key was pressed: show the sink's level as it is now.
pub fn show(d: &mut Desk) {
    let Some(s) = d.tray.audio.sink().cloned() else { return };
    let level = s.percent().min(crate::pulse::MAX_PERCENT) as f32 / crate::pulse::MAX_PERCENT as f32;
    let before = rect(d);
    let oi = crate::shell::bar_output(d, d.output_at(d.seat.x, d.seat.y).unwrap_or(0));
    let now = d.now_ms();
    let o = &mut d.osd;
    let fresh = o.shown.value < 0.02;
    if fresh {
        // From hidden: the slider starts where it is (no sweep from the last time).
        o.out = oi;
        o.volume.snap(if s.mute { 0.0 } else { level });
        o.mute.snap(if s.mute { 1.0 } else { 0.0 });
    }
    // Muted shows as nothing, as in liquify (and the level comes back on unmute).
    o.volume.to(if s.mute { 0.0 } else { level });
    o.mute.to(if s.mute { 1.0 } else { 0.0 });
    if o.muted.is_some_and(|m| m != s.mute) && !fresh {
        o.mute_pop.snap(0.62);
        o.mute_pop.velocity = 7.0;
        o.mute_pop.to(1.0);
    }
    o.muted = Some(s.mute);
    o.shown.to(1.0);
    o.at = now;
    if let Some(r) = before {
        d.damage(r);
    }
    if let Some(r) = rect(d) {
        d.damage(r);
    }
    if !d.overlay.animating {
        d.overlay.anim_ms = d.now_ms();
    }
    d.overlay.animating = true;
}

/// Where it is drawn (logical, with room for its shadow), while shown.
pub fn rect(d: &Desk) -> Option<Rect> {
    let o = &d.osd;
    if o.shown.value <= 0.001 && o.shown.target == 0.0 || o.out >= d.outputs.len() {
        return None;
    }
    let or = d.outputs[o.out].rect();
    let tb = crate::shell::taskbar_rect(d, o.out);
    let w = (BOX + GAP + BAR_W).ceil() as i32;
    let h = BOX.ceil() as i32;
    let bottom = if tb.h > 0 { tb.y } else { or.y1() - 24 } - LIFT;
    Some(Rect::new(or.x + (or.w - w) / 2, bottom - h, w, h).inflate(6))
}

pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let before = rect(d);
    let now = d.now_ms();
    let o = &mut d.osd;
    if o.shown.target > 0.0 && now.wrapping_sub(o.at) > HOLD_MS {
        o.shown.to(0.0);
    }
    let mut moving = false;
    for s in [&mut o.shown, &mut o.volume, &mut o.mute, &mut o.mute_pop] {
        moving |= s.step(dt);
    }
    let waiting = o.shown.target > 0.0;
    if moving {
        if let Some(r) = before.or_else(|| rect(d)) {
            d.damage(r);
        }
    }
    if d.osd.shown.value <= 0.001 && d.osd.shown.target == 0.0 {
        d.osd.muted = None;
    }
    moving || waiting
}

/// A premultiplied colour `a` times as opaque (every channel scales).
fn fade(c: u32, a: f32) -> u32 {
    let a = a.clamp(0.0, 1.0);
    let ch = |sh: u32| ((((c >> sh) & 0xff) as f32 * a).round() as u32) << sh;
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// The speaker (Lucide's volume-2 / volume-x on a 24 grid) at (x, y) logical, `size` big.
fn speaker(cv: &mut Canvas, clip: Rect, xf: Xf, x: f32, y: f32, size: f32, off: bool, color: u32, width: f32) {
    let s = size / 24.0;
    let p = |px: f32, py: f32| -> (f32, f32) {
        let (lx, ly) = (x + px * s, y + py * s);
        (((lx as f64 - xf.ox as f64) * xf.scale) as f32 - xf.px as f32, ((ly as f64 - xf.oy as f64) * xf.scale) as f32 - xf.py as f32)
    };
    let w = width * s * xf.scale as f32;
    let body = [(11.0, 5.0), (6.0, 9.0), (2.0, 9.0), (2.0, 15.0), (6.0, 15.0), (11.0, 19.0)];
    for i in 0..body.len() {
        let (a, b) = (body[i], body[(i + 1) % body.len()]);
        cv.line(clip, p(a.0, a.1), p(b.0, b.1), w, color);
    }
    if off {
        cv.line(clip, p(22.0, 9.0), p(16.0, 15.0), w, color);
        cv.line(clip, p(16.0, 9.0), p(22.0, 15.0), w, color);
    } else {
        // The wave: a 5 px arc round (12, 12), -45° .. 45°.
        let n = 10;
        for i in 0..n {
            let a0 = -std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_2 * i as f32 / n as f32;
            let a1 = -std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_2 * (i + 1) as f32 / n as f32;
            cv.line(clip, p(12.0 + 5.0 * a0.cos(), 12.0 + 5.0 * a0.sin()), p(12.0 + 5.0 * a1.cos(), 12.0 + 5.0 * a1.sin()), w, color);
        }
    }
}

pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    let o = &d.osd;
    if oi != o.out {
        return;
    }
    let Some(r) = rect(d) else { return };
    let a = o.shown.value.clamp(0.0, 1.0);
    if a <= 0.001 {
        return;
    }
    let r = r.inflate(-6);
    let (x0, y0) = (r.x as f32, r.y as f32);
    let k = o.volume.value.clamp(0.0, 1.0);
    let off = o.mute.value.clamp(0.0, 1.0);
    // Comes up a little as it fades in.
    let rise = (1.0 - a) * 8.0;
    let y0 = y0 + rise;
    // No panel: a soft shadow under each part keeps it readable on light windows.
    let shadow = |c: u32| fade(c, 0.35 * a);
    // The speaker, popping about its middle.
    let pop = o.mute_pop.value;
    let size = ICON * pop;
    let (ix, iy) = (x0 + (BOX - size) * 0.5, y0 + (BOX - size) * 0.5);
    for (sh, dx) in [(true, 1.0f32), (false, 0.0)] {
        let base = |c: u32| if sh { shadow(argb(255, 0, 0, 0)) } else { fade(c, a) };
        let wid = if sh { 3.5 } else { 2.0 };
        let (sx, sy) = (ix + dx * 0.0, iy + dx);
        if 1.0 - off > 0.01 {
            speaker(cv, clip, xf, sx, sy, size, false, base(fade(0xffff_ffff, 0.9 * (1.0 - off))), wid);
        }
        if off > 0.01 {
            speaker(cv, clip, xf, sx, sy, size, true, base(fade(0xffff_ffff, 0.9 * off)), wid);
        }
    }
    // The slider.
    let bx = x0 + BOX + GAP;
    let by = y0 + (BOX - H) * 0.5;
    let track = Rect::new(bx.round() as i32, (by + (H - BAR) * 0.5).round() as i32, BAR_W.round() as i32, BAR.round() as i32);
    let fill = Rect::new(track.x, track.y, (BAR_W * k).round() as i32, track.h);
    let kx = (BAR_W * k - KNOB * 0.5).clamp(0.0, BAR_W - KNOB);
    let knob = Rect::new((bx + kx).round() as i32, (by + (H - KNOB) * 0.5).round() as i32, KNOB.round() as i32, KNOB.round() as i32);
    let rad = |r: Rect| (xf.len((r.h / 2) as i32)).max(1);
    cv.rrect(clip, xf.r(track.inflate(1).translate(0, 1)), rad(track.inflate(1)), shadow(argb(255, 0, 0, 0)));
    cv.rrect(clip, xf.r(knob.inflate(1).translate(0, 1)), rad(knob.inflate(1)), shadow(argb(255, 0, 0, 0)));
    cv.rrect(clip, xf.r(track), rad(track), fade(0xffff_ffff, 0.18 * a));
    if fill.w > 0 {
        cv.rrect(clip, xf.r(fill), rad(fill), fade(0xffff_ffff, 0.85 * a));
    }
    cv.rrect(clip, xf.r(knob), rad(knob), fade(0xffff_ffff, a));
}
