//! Programs' own notification windows (Steam's «notificationtoasts_N_desktop», X windows of
//! type _NET_WM_WINDOW_TYPE_NOTIFICATION): in the main monitor's bottom right corner over
//! the taskbar's place, the newest lowest, each rising from under the screen's edge on a
//! spring, in a rim of glass (gpu.rs `window_glass`). They take no focus, have no taskbar
//! button and stay over the windows.
use crate::anim::Spring;
use crate::desk::Desk;
use crate::state::WKey;

/// From the work area's edges, and between two of them (logical px).
const MARGIN: i32 = 14;
const GAP: i32 = 10;
/// The glass rim around one (logical px) and its corners.
pub const RIM: i32 = 8;
pub const RADIUS: i32 = 18;

pub struct Toast {
    pub wk: WKey,
    /// Its top edge (logical px), springing to where it goes.
    y: Spring,
}

/// A program's notification by its X title or type.
pub fn is_toast(title: &str, notification_type: bool) -> bool {
    notification_type || title.starts_with("notificationtoasts")
}

/// A toast window was mapped: it starts under the main monitor's bottom edge.
pub fn mapped(d: &mut Desk, wk: WKey) {
    d.windows[wk].placed = true;
    let oi = crate::monitors::primary_index(d);
    let Some(o) = d.outputs.get(oi) else { return };
    let bottom = o.rect().y1();
    let mut y = Spring::new(bottom as f32 + RIM as f32, 0.36, 0.84);
    y.rest = 0.5;
    d.toasts.retain(|t| t.wk != wk);
    d.toasts.push(Toast { wk, y });
    place(d);
    crate::shell::taskbar_poke(d);
}

/// Where each goes: stacked up from the bottom right, the newest lowest.
fn place(d: &mut Desk) {
    let oi = crate::monitors::primary_index(d);
    let wa = crate::wm::panel_area(d, oi);
    let mut bottom = wa.y1() - MARGIN;
    for i in (0..d.toasts.len()).rev() {
        let wk = d.toasts[i].wk;
        let Some(w) = d.windows.get(wk) else { continue };
        let g = d.surfaces.get(w.surf).map(|s| s.geom()).unwrap_or_default();
        let top = bottom - g.h;
        d.toasts[i].y.to(top as f32);
        bottom = top - GAP - 2 * RIM;
        let x = wa.x1() - MARGIN - RIM - g.w;
        if d.windows[wk].x != x {
            crate::wm::damage_window(d, wk);
            d.windows[wk].x = x;
        }
    }
}

/// Moves them toward their places. True while one moves.
pub fn tick(d: &mut Desk, dt: f32) -> bool {
    let before = d.toasts.len();
    d.toasts.retain(|t| d.windows.get(t.wk).is_some_and(|w| d.surfaces.get(w.surf).is_some_and(|s| s.mapped)));
    if d.toasts.is_empty() {
        return false;
    }
    if d.toasts.len() != before {
        place(d);
    } else {
        // Sizes change as they draw: their places follow.
        place(d);
    }
    let mut moving = false;
    for i in 0..d.toasts.len() {
        let m = d.toasts[i].y.step(dt);
        moving |= m;
        let wk = d.toasts[i].wk;
        let y = d.toasts[i].y.value.round() as i32;
        if d.windows[wk].y != y {
            crate::wm::damage_window(d, wk);
            d.windows[wk].y = y;
            crate::wm::damage_window(d, wk);
            crate::wm::update_window_outputs(d, wk);
        }
    }
    moving
}
