//! Input straight from evdev (/dev/input/event*), no libinput: keyboards, mice and
//! touchpads, grabbed (EVIOCGRAB) so the text console under us gets nothing.
//!
//! Mouse: relative motion with a mild speed-dependent acceleration, buttons, wheel (hi-res
//! where the device has it). Touchpad: one finger moves the pointer (millimetres from the
//! device's resolution, accelerated), two fingers scroll, a short touch taps (one finger:
//! left, two: right, three: middle), and a click with two fingers on a clickpad is a right
//! click, as in Windows. Devices are looked for again every couple of seconds (plugging).
use crate::desk::{Desk, TOK_BACKEND};
use std::os::fd::{AsRawFd, OwnedFd};
use sys::{ioc, ioctl_ptr, IOC_READ, IOC_WRITE};

const E: u8 = b'E';
const fn eviocgbit(ev: u8, len: usize) -> u64 {
    ioc(IOC_READ, E, 0x20 + ev, len)
}
const fn eviocgabs(abs: u8) -> u64 {
    ioc(IOC_READ, E, 0x40 + abs, 24)
}
const EVIOCGPROP: u64 = ioc(IOC_READ, E, 0x09, 4);
const EVIOCGNAME: u64 = ioc(IOC_READ, E, 0x06, 128);
const EVIOCGRAB: u64 = ioc(IOC_WRITE, E, 0x90, 4);

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const KEY_A: usize = 30;
const KEY_Z: usize = 44;
const BTN_MISC: u16 = 0x100;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOOL_QUINTTAP: u16 = 0x148;
const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
const BTN_TOOL_QUADTAP: u16 = 0x14f;
const INPUT_PROP_DIRECT: u32 = 1;
/// Epoll sub-tokens of devices start here (the backend's own fds come first).
pub const SLOT_BASE: u64 = 3;

/// Pointer speed: touchpad px per mm at rest, mouse gain; acceleration on top.
const TP_PX_PER_MM: f64 = 7.0;
const TAP_MS: u64 = 180;
const TAP_MOVE_MM: f64 = 1.5;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Keyboard,
    Mouse,
    Touchpad,
}

#[derive(Default)]
struct Touch {
    down: bool,
    fingers: u32,
    max_fingers: u32,
    x: Option<i32>,
    y: Option<i32>,
    last: Option<(i32, i32)>,
    start_ms: u64,
    moved_mm: f64,
    clicked: bool,
    /// The clickpad's button went down as this (left, or right with two fingers).
    pad_button: Option<u32>,
    scrolling: bool,
    last_ms: u64,
}

struct Dev {
    fd: OwnedFd,
    path: String,
    name: String,
    kinds: Vec<Kind>,
    hires_wheel: bool,
    /// Touchpad units per mm (x, y).
    res: (f64, f64),
    dx: i32,
    dy: i32,
    wheel: [i32; 2],
    wheel_hi: [i32; 2],
    tp: Touch,
}

pub struct Input {
    devs: Vec<Option<Dev>>,
    pub paused: bool,
}

fn test_bit(bits: &[u8], n: usize) -> bool {
    bits.get(n / 8).map_or(false, |b| b & (1 << (n % 8)) != 0)
}

impl Input {
    pub fn new() -> Input {
        Input { devs: Vec::new(), paused: false }
    }

    /// Opens the devices not open yet. Returns how many there are.
    pub fn scan(&mut self, d: &mut Desk) -> usize {
        let Ok(dir) = std::fs::read_dir("/dev/input") else { return 0 };
        let mut paths: Vec<String> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().into_owned())
            .filter(|p| p.rsplit('/').next().map_or(false, |n| n.starts_with("event")))
            .collect();
        paths.sort();
        for p in paths {
            if self.devs.iter().flatten().any(|dv| dv.path == p) {
                continue;
            }
            let Some(dev) = open_dev(&p) else { continue };
            let slot = self.devs.iter().position(|s| s.is_none()).unwrap_or_else(|| {
                self.devs.push(None);
                self.devs.len() - 1
            });
            if d.ep.add(dev.fd.as_raw_fd(), TOK_BACKEND | (SLOT_BASE + slot as u64), sys::EPOLLIN).is_err() {
                continue;
            }
            if !self.paused {
                grab(&dev, true);
            }
            eprintln!("input: {} ({}) as {:?}", dev.name, dev.path, dev.kinds);
            self.devs[slot] = Some(dev);
        }
        self.devs.iter().flatten().count()
    }

    /// Another session has the seat: let go of the devices (and drop what they send).
    pub fn pause(&mut self, d: &mut Desk) {
        self.paused = true;
        for dev in self.devs.iter().flatten() {
            grab(dev, false);
        }
        crate::input::release_all_keys(d);
    }

    pub fn resume(&mut self) {
        self.paused = false;
        for dev in self.devs.iter().flatten() {
            grab(dev, true);
        }
    }

    /// Events of the device in `slot`.
    pub fn dispatch(&mut self, d: &mut Desk, slot: usize) -> Vec<Action> {
        let mut acts = Vec::new();
        let Some(Some(dev)) = self.devs.get_mut(slot) else { return acts };
        let mut buf = [0u8; 24 * 64];
        loop {
            let n = unsafe {
                extern "C" {
                    fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
                }
                read(dev.fd.as_raw_fd(), buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len())
            };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() == Some(19) {
                    // ENODEV: unplugged.
                    eprintln!("input: {} gone", dev.name);
                    d.ep.remove(dev.fd.as_raw_fd());
                    self.devs[slot] = None;
                }
                return acts;
            }
            if n == 0 {
                return acts;
            }
            for ev in buf[..n as usize].chunks_exact(24) {
                let sec = i64::from_ne_bytes(ev[0..8].try_into().unwrap());
                let usec = i64::from_ne_bytes(ev[8..16].try_into().unwrap());
                let ty = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                if self.paused {
                    continue;
                }
                let ms = (sec as u64) * 1000 + (usec as u64) / 1000;
                event(d, dev, ty, code, value, ms, &mut acts);
            }
        }
    }
}

/// What the backend does itself (not a client's business).
pub enum Action {
    /// Ctrl+Alt+F<n>: switch to virtual terminal n.
    SwitchVt(u16),
}

/// EVIOCGRAB takes its int by value.
fn grab(dev: &Dev, on: bool) {
    let _ = sys::ioctl_int(dev.fd.as_raw_fd(), EVIOCGRAB, on as usize);
}

fn open_dev(path: &str) -> Option<Dev> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(0o4000 | 0o2000000).open(path).ok()?;
    let fd: OwnedFd = f.into();
    let raw = fd.as_raw_fd();
    let mut types = [0u8; 4];
    let mut keys = [0u8; 96];
    let mut rels = [0u8; 4];
    let mut abss = [0u8; 8];
    let mut props = [0u8; 4];
    let mut name = [0u8; 128];
    unsafe {
        ioctl_ptr(raw, eviocgbit(0, 4), types.as_mut_ptr()).ok()?;
        let _ = ioctl_ptr(raw, eviocgbit(EV_KEY as u8, 96), keys.as_mut_ptr());
        let _ = ioctl_ptr(raw, eviocgbit(EV_REL as u8, 4), rels.as_mut_ptr());
        let _ = ioctl_ptr(raw, eviocgbit(EV_ABS as u8, 8), abss.as_mut_ptr());
        let _ = ioctl_ptr(raw, EVIOCGPROP, props.as_mut_ptr());
        let _ = ioctl_ptr(raw, EVIOCGNAME, name.as_mut_ptr());
    }
    let name = String::from_utf8_lossy(&name[..name.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned();
    let has = |bits: &[u8], n: u16| test_bit(bits, n as usize);
    let mut kinds = Vec::new();
    if has(&types, EV_KEY) && (KEY_A..=KEY_Z).all(|k| test_bit(&keys, k)) {
        kinds.push(Kind::Keyboard);
    }
    if has(&types, EV_REL) && has(&rels, REL_X) && has(&rels, REL_Y) && has(&keys, BTN_LEFT) {
        kinds.push(Kind::Mouse);
    }
    let direct = test_bit(&props, INPUT_PROP_DIRECT as usize);
    if has(&types, EV_ABS) && has(&abss, ABS_X) && has(&abss, ABS_Y) && has(&keys, BTN_TOOL_FINGER) && !direct {
        kinds.push(Kind::Touchpad);
    }
    if kinds.is_empty() {
        return None;
    }
    let mut res = (0.0, 0.0);
    if kinds.contains(&Kind::Touchpad) {
        // input_absinfo: value, minimum, maximum, fuzz, flat, resolution (units per mm).
        let mut ax = [0i32; 6];
        let mut ay = [0i32; 6];
        unsafe {
            let _ = ioctl_ptr(raw, eviocgabs(ABS_X as u8), ax.as_mut_ptr());
            let _ = ioctl_ptr(raw, eviocgabs(ABS_Y as u8), ay.as_mut_ptr());
        }
        // No resolution given: assume a 100 × 60 mm pad.
        let rx = if ax[5] > 0 { ax[5] as f64 } else { ((ax[2] - ax[1]).max(1) as f64) / 100.0 };
        let ry = if ay[5] > 0 { ay[5] as f64 } else { ((ay[2] - ay[1]).max(1) as f64) / 60.0 };
        res = (rx, ry);
    }
    Some(Dev {
        fd,
        path: path.to_string(),
        name,
        kinds,
        hires_wheel: has(&rels, REL_WHEEL_HI_RES),
        res,
        dx: 0,
        dy: 0,
        wheel: [0; 2],
        wheel_hi: [0; 2],
        tp: Touch::default(),
    })
}

fn pointer_by(d: &mut Desk, dx: f64, dy: f64, ms: u64) {
    // Device pixels are physical pixels of the monitor under the pointer.
    let scale = d.output_at(d.seat.x, d.seat.y).map_or(1.0, |oi| d.outputs[oi].scale);
    let (x, y) = (d.seat.x + dx / scale, d.seat.y + dy / scale);
    crate::input::pointer_motion(d, x, y, ms as u32);
}

fn click(d: &mut Desk, button: u32, ms: u64) {
    crate::input::pointer_button(d, button, true, ms as u32);
    crate::input::pointer_button(d, button, false, ms as u32);
}

fn event(d: &mut Desk, dev: &mut Dev, ty: u16, code: u16, value: i32, ms: u64, acts: &mut Vec<Action>) {
    let t = ms as u32;
    match ty {
        EV_KEY if code < BTN_MISC => {
            // Autorepeat (2) is the clients' own business.
            if value == 2 {
                return;
            }
            let pressed = value == 1;
            use crate::input::{M_ALT, M_CTRL};
            let h = d.seat.held;
            let fkey = match code {
                59..=68 => Some(code - 58),
                87 => Some(11),
                88 => Some(12),
                _ => None,
            };
            if let (true, Some(n)) = (pressed && h & M_CTRL != 0 && h & M_ALT != 0, fkey) {
                acts.push(Action::SwitchVt(n));
                return;
            }
            crate::input::key(d, code as u32, pressed, t);
        }
        EV_KEY if dev.kinds.contains(&Kind::Touchpad) => touchpad_key(d, dev, code, value, ms),
        EV_KEY if (BTN_LEFT..=BTN_LEFT + 7).contains(&code) => crate::input::pointer_button(d, code as u32, value != 0, t),
        EV_REL => match code {
            REL_X => dev.dx += value,
            REL_Y => dev.dy += value,
            REL_WHEEL => dev.wheel[0] += value,
            REL_HWHEEL => dev.wheel[1] += value,
            REL_WHEEL_HI_RES => dev.wheel_hi[0] += value,
            REL_HWHEEL_HI_RES => dev.wheel_hi[1] += value,
            _ => {}
        },
        EV_ABS if dev.kinds.contains(&Kind::Touchpad) => match code {
            ABS_X => dev.tp.x = Some(value),
            ABS_Y => dev.tp.y = Some(value),
            _ => {}
        },
        EV_SYN if code == SYN_REPORT => {
            if dev.dx != 0 || dev.dy != 0 {
                let (dx, dy) = (dev.dx as f64, dev.dy as f64);
                dev.dx = 0;
                dev.dy = 0;
                // Mild acceleration: faster flicks go further.
                let k = 1.0 + 0.06 * (dx * dx + dy * dy).sqrt().min(40.0);
                pointer_by(d, dx * k, dy * k, ms);
            }
            for axis in 0..2 {
                let (lo, hi) = (std::mem::take(&mut dev.wheel[axis]), std::mem::take(&mut dev.wheel_hi[axis]));
                let v120 = if dev.hires_wheel { hi } else { lo * 120 };
                if v120 != 0 {
                    // evdev: wheel up / tilt right positive; Wayland: down / right positive.
                    let v = if axis == 0 { -v120 } else { v120 };
                    crate::input::pointer_axis(d, t, axis as u32, v as f64 * 15.0 / 120.0, Some(v), Some(0));
                }
            }
            if dev.kinds.contains(&Kind::Touchpad) {
                touchpad_frame(d, dev, ms);
            }
        }
        _ => {}
    }
}

fn touchpad_key(d: &mut Desk, dev: &mut Dev, code: u16, value: i32, ms: u64) {
    let tp = &mut dev.tp;
    let fingers = |c: u16| match c {
        BTN_TOOL_FINGER => 1,
        BTN_TOOL_DOUBLETAP => 2,
        BTN_TOOL_TRIPLETAP => 3,
        BTN_TOOL_QUADTAP => 4,
        BTN_TOOL_QUINTTAP => 5,
        _ => 0,
    };
    match code {
        BTN_TOUCH => {
            if value != 0 {
                *tp = Touch { down: true, fingers: tp.fingers.max(1), max_fingers: tp.fingers.max(1), start_ms: ms, last_ms: ms, ..Touch::default() };
            } else {
                // A tap: short, still, no button pressed during it.
                let tap = !tp.clicked && ms.saturating_sub(tp.start_ms) < TAP_MS && tp.moved_mm < TAP_MOVE_MM;
                if tap {
                    let b = match tp.max_fingers {
                        2 => BTN_RIGHT,
                        3 => BTN_MIDDLE,
                        _ => BTN_LEFT,
                    };
                    click(d, b as u32, ms);
                }
                if tp.scrolling {
                    crate::input::pointer_axis(d, ms as u32, 0, 0.0, None, Some(1));
                    crate::input::pointer_axis(d, ms as u32, 1, 0.0, None, Some(1));
                }
                tp.down = false;
                tp.last = None;
                tp.scrolling = false;
            }
        }
        c if fingers(c) > 0 => {
            if value != 0 {
                tp.fingers = fingers(c);
                tp.max_fingers = tp.max_fingers.max(tp.fingers);
                // A finger more or less: positions jump; start over from the next report.
                tp.last = None;
            }
        }
        BTN_LEFT | BTN_RIGHT | BTN_MIDDLE => {
            if value != 0 {
                tp.clicked = true;
                // A clickpad has one button: two fingers on it make it the right one.
                let b = if code == BTN_LEFT && tp.fingers >= 2 { BTN_RIGHT } else { code };
                tp.pad_button = Some(b as u32);
                crate::input::pointer_button(d, b as u32, true, ms as u32);
            } else if let Some(b) = tp.pad_button.take() {
                crate::input::pointer_button(d, b, false, ms as u32);
            }
        }
        _ => {}
    }
}

fn touchpad_frame(d: &mut Desk, dev: &mut Dev, ms: u64) {
    let (rx, ry) = dev.res;
    let tp = &mut dev.tp;
    if !tp.down {
        return;
    }
    let (Some(x), Some(y)) = (tp.x, tp.y) else { return };
    let Some((lx, ly)) = tp.last.replace((x, y)) else {
        tp.last_ms = ms;
        return;
    };
    let (mx, my) = ((x - lx) as f64 / rx, (y - ly) as f64 / ry);
    let dist = (mx * mx + my * my).sqrt();
    tp.moved_mm += dist;
    let dt = (ms.saturating_sub(tp.last_ms)).max(1) as f64 / 1000.0;
    tp.last_ms = ms;
    if dist == 0.0 {
        return;
    }
    if tp.fingers >= 2 {
        // Two-finger scroll: content follows the fingers (Windows' default direction).
        tp.scrolling = true;
        let k = 4.0;
        if my != 0.0 {
            crate::input::pointer_axis(d, ms as u32, 0, -my * k, None, Some(1));
        }
        if mx != 0.0 {
            crate::input::pointer_axis(d, ms as u32, 1, -mx * k, None, Some(1));
        }
        return;
    }
    // Speed in mm/s: slow moves are precise, fast ones go up to 3× further.
    let v = dist / dt;
    let k = (0.6 + v / 80.0).clamp(0.6, 3.0);
    pointer_by(d, mx * TP_PX_PER_MM * k, my * TP_PX_PER_MM * k, ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers() {
        // As the kernel headers have them (x86_64).
        assert_eq!(EVIOCGRAB, 0x4004_4590);
        assert_eq!(eviocgbit(0, 4), 0x8004_4520);
        assert_eq!(eviocgabs(0), 0x8018_4540);
    }
}
