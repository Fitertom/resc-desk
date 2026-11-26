//! Win+Shift+S: a snip of the screen to the clipboard. The screens dim, a drag picks the
//! area (it stays clear), letting go copies it as a PNG and ends; Esc or a right click
//! cancels. No buttons, no editor, nothing saved.
//!
//! The picture is not taken from the dimmed screens: the dim and the cursor go first, and
//! once every monitor under the area has shown a frame without them, their pixels are read
//! (the GPU's frames: games and video included) and cut to the area at the sharpest of
//! their scales.
use crate::backend::Backend;
use crate::desk::Desk;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};
use crate::state::DataSource;
use std::sync::Arc;

#[derive(Default)]
pub struct Snip {
    /// Waiting for the drag / dragging from (x, y) (logical).
    pub on: bool,
    pub from: Option<(f64, f64)>,
    pub to: (f64, f64),
    /// Let go: the area, the frame count of each output then, and when (gives up after 1 s).
    taking: Option<(Rect, Vec<u64>, u64)>,
}

pub fn toggle(d: &mut Desk) {
    if d.snip.on || d.snip.taking.is_some() {
        cancel(d);
        return;
    }
    d.snip = Snip { on: true, from: None, to: (d.seat.x, d.seat.y), taking: None };
    d.damage_all();
}

pub fn active(d: &Desk) -> bool {
    d.snip.on
}

/// While taking the picture the cursor is left out.
pub fn hides_cursor(d: &Desk) -> bool {
    d.snip.taking.is_some()
}

pub fn cancel(d: &mut Desk) {
    d.snip = Snip::default();
    d.damage_all();
}

fn area(s: &Snip) -> Option<Rect> {
    let (x0, y0) = s.from?;
    let (x1, y1) = s.to;
    let r = Rect::from_edges(x0.min(x1).floor() as i32, y0.min(y1).floor() as i32, x0.max(x1).ceil() as i32, y0.max(y1).ceil() as i32);
    Some(r)
}

/// Keys while snipping: Esc cancels, other presses are swallowed (releases go on, so
/// the window does not keep Win / Shift held).
pub fn key(d: &mut Desk, code: u32, pressed: bool) -> bool {
    if !d.snip.on || !pressed {
        return false;
    }
    if pressed && code == crate::input::key::ESC {
        cancel(d);
    }
    true
}

/// A press while snipping (true: taken). The left button starts the area, others cancel.
pub fn press(d: &mut Desk, button: u32) -> bool {
    if !d.snip.on {
        return false;
    }
    if button != crate::input::BTN_LEFT {
        cancel(d);
        return true;
    }
    d.snip.from = Some((d.seat.x, d.seat.y));
    d.snip.to = (d.seat.x, d.seat.y);
    true
}

pub fn motion(d: &mut Desk, x: f64, y: f64) {
    if !d.snip.on || d.snip.from.is_none() {
        return;
    }
    let old = area(&d.snip);
    d.snip.to = (x, y);
    let new = area(&d.snip);
    match (old, new) {
        (Some(a), Some(b)) => d.damage(a.union(&b).inflate(4)),
        _ => d.damage_all(),
    }
}

pub fn release(d: &mut Desk) {
    if !d.snip.on {
        return;
    }
    let Some(r) = area(&d.snip) else { return };
    if r.w < 2 || r.h < 2 {
        // A click without a drag: nothing picked, keep waiting.
        d.snip.from = None;
        d.damage_all();
        return;
    }
    let frames = d.outputs.iter().map(|o| o.frames).collect();
    d.snip = Snip { on: false, from: None, to: (0.0, 0.0), taking: Some((r, frames, sys::now_ns())) };
    d.damage_all();
}

/// In the main loop after presenting: once the monitors under the area show a clean frame,
/// cut it out and make it the clipboard.
pub fn process(d: &mut Desk, b: &mut dyn Backend) {
    let Some((r, frames, at)) = d.snip.taking.clone() else { return };
    let under: Vec<usize> = (0..d.outputs.len()).filter(|&i| d.outputs[i].rect().overlaps(&r)).collect();
    let late = sys::now_ns().saturating_sub(at) > 1_000_000_000;
    if !late && under.iter().any(|&i| d.outputs[i].frames <= frames.get(i).copied().unwrap_or(0)) {
        return;
    }
    d.snip.taking = None;
    d.damage_all();
    if under.is_empty() {
        return;
    }
    let s = under.iter().map(|&i| d.outputs[i].scale).fold(1.0f64, f64::max);
    let (w, h) = (((r.w as f64) * s).round().max(1.0) as i32, ((r.h as f64) * s).round().max(1.0) as i32);
    let mut img = vec![0xff00_0000u32; (w * h) as usize];
    for &oi in &under {
        let Some((px, pw, ph)) = b.read_pixels(d, oi) else { continue };
        let o = &d.outputs[oi];
        let (orect, os) = (o.rect(), o.scale);
        for iy in 0..h {
            let ly = r.y as f64 + (iy as f64 + 0.5) / s;
            if ly < orect.y as f64 || ly >= orect.y1() as f64 {
                continue;
            }
            let sy = ((ly - orect.y as f64) * os) as i32;
            if sy < 0 || sy >= ph {
                continue;
            }
            for ix in 0..w {
                let lx = r.x as f64 + (ix as f64 + 0.5) / s;
                if lx < orect.x as f64 || lx >= orect.x1() as f64 {
                    continue;
                }
                let sx = ((lx - orect.x as f64) * os) as i32;
                if sx >= 0 && sx < pw {
                    img[(iy * w + ix) as usize] = px[(sy * pw + sx) as usize] | 0xff00_0000;
                }
            }
        }
    }
    let png = crate::png::encode(&img, w, h, w as usize, 1);
    let mimes = vec!["image/png".to_string()];
    let dk = d.sources.insert(DataSource { client: Default::default(), obj: 0, mimes, primary: false, own: Some(Arc::new(png)), x11: false, actions: 0 });
    crate::server::set_selection(d, Some(dk));
    eprintln!("snip: {}x{} at {},{} copied", w, h, r.x, r.y);
}

/// Over everything on output `oi`: the dim, the clear area and its frame.
pub fn draw(d: &Desk, oi: usize, xf: Xf, cv: &mut Canvas, clip: Rect) {
    if !d.snip.on {
        return;
    }
    let o = d.outputs[oi].rect();
    let dim = argb(110, 0, 0, 0);
    let Some(a) = area(&d.snip).filter(|a| a.overlaps(&o)) else {
        cv.fill(clip, xf.r(o), dim);
        return;
    };
    // Dim around the area: above, below, left, right.
    for r in [
        Rect::from_edges(o.x, o.y, o.x1(), a.y),
        Rect::from_edges(o.x, a.y1(), o.x1(), o.y1()),
        Rect::from_edges(o.x, a.y.max(o.y), a.x, a.y1().min(o.y1())),
        Rect::from_edges(a.x1(), a.y.max(o.y), o.x1(), a.y1().min(o.y1())),
    ] {
        if !r.is_empty() {
            cv.fill(clip, xf.r(r.intersect(&o)), dim);
        }
    }
    let pa = xf.r(a);
    let line = 0xffff_ffff;
    let t = (xf.scale.round() as i32).max(1);
    for r in [
        Rect::new(pa.x - t, pa.y - t, pa.w + 2 * t, t),
        Rect::new(pa.x - t, pa.y1(), pa.w + 2 * t, t),
        Rect::new(pa.x - t, pa.y, t, pa.h),
        Rect::new(pa.x1(), pa.y, t, pa.h),
    ] {
        cv.fill(clip, r, line);
    }
}
