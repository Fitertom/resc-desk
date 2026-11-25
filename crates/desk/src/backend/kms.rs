//! The real thing: resc-desk as the session on a Linux console. Monitors through DRM/KMS
//! (drm.rs), input from evdev (evdev.rs), the VT it was started on (vt.rs), and the
//! kernel's uevents for what gets plugged in or out (monitors, keyboards, mice).
//!
//! Start it from a text console (log in on Ctrl+Alt+F3, or have the console log in by
//! itself, README) with `resc-desk`. The card and the input devices are the console user's
//! through logind's ACLs on systemd distributions; otherwise the `video` and `input` groups.
use super::drm::Drm;
use super::evdev::{Action, Input, SLOT_BASE};
use super::vt::Vt;
use super::Backend;
use crate::desk::{Desk, TOK_BACKEND, TOK_KIND_MASK};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

const SUB_DRM: u64 = 0;
const SUB_SIGNAL: u64 = 1;
const SUB_UEVENT: u64 = 2;

pub struct Kms {
    // Dropped in this order: modes put back while still master, then the console.
    drm: Drm,
    input: Input,
    vt: Option<Vt>,
    sig: sys::SignalFd,
    uevents: Option<OwnedFd>,
}

impl Kms {
    pub fn new(d: &mut Desk) -> std::io::Result<Kms> {
        // The card first: without one nothing else is touched (signals stay as they are for
        // the headless fallback).
        let mut drm = Drm::open()?;
        let sig = sys::SignalFd::new(&[sys::SIGUSR1, sys::SIGUSR2, sys::SIGINT, sys::SIGTERM, sys::SIGHUP])?;
        let vt = match Vt::open() {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("kms: no VT ({e}): the console may draw over us, and VT switching is off");
                None
            }
        };
        d.ep.add(drm.fd.as_raw_fd(), TOK_BACKEND | SUB_DRM, sys::EPOLLIN)?;
        d.ep.add(sig.fd.as_raw_fd(), TOK_BACKEND | SUB_SIGNAL, sys::EPOLLIN)?;
        let uevents = match sys::uevent_socket() {
            Ok(fd) => {
                d.ep.add(fd.as_raw_fd(), TOK_BACKEND | SUB_UEVENT, sys::EPOLLIN)?;
                Some(fd)
            }
            Err(e) => {
                eprintln!("kms: no uevents ({e}): monitors plugged in later are not seen");
                None
            }
        };
        // Each monitor's settings and Win+P's way: the outputs.
        drm.apply(d);
        let mut input = Input::new();
        if input.scan(d) == 0 && std::env::var("RESC_ALLOW_NO_INPUT").is_err() {
            // Without a keyboard nothing could ever take the screen back: give it back now
            // (the card and the VT are let go on the way out).
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "no keyboard or mouse could be opened (/dev/input/event*). Add yourself to the `input` group: sudo usermod -aG input $USER, then log in again (RESC_ALLOW_NO_INPUT=1 starts anyway)",
            ));
        }
        Ok(Kms { drm, input, vt, sig, uevents })
    }

    fn signals(&mut self, d: &mut Desk) {
        while let Some(s) = self.sig.read() {
            match s as i32 {
                sys::SIGUSR1 => {
                    // Leaving our VT: let go of the screen and the devices first.
                    self.drm.pause();
                    self.input.pause(d);
                    if let Some(vt) = &self.vt {
                        vt.allow_release();
                    }
                }
                sys::SIGUSR2 => {
                    if let Some(vt) = &self.vt {
                        vt.ack_acquire();
                    }
                    self.drm.resume(d);
                    self.input.resume();
                    // Plugged in or out while we were away.
                    self.drm.hotplug(d);
                    self.input.scan(d);
                }
                _ => d.running = false,
            }
        }
    }

    fn uevents(&mut self, d: &mut Desk) {
        let Some(fd) = &self.uevents else { return };
        let (mut monitors, mut inputs) = (false, false);
        while let Some(ev) = sys::read_uevent(fd.as_fd()) {
            let get = |k: &str| ev.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
            match get("SUBSYSTEM") {
                Some("drm") if get("HOTPLUG") == Some("1") => monitors = true,
                Some("input") if get("ACTION") == Some("add") && get("DEVNAME").map_or(false, |n| n.contains("event")) => inputs = true,
                _ => {}
            }
        }
        if monitors && !self.drm.paused && self.drm.hotplug(d) {
            d.damage_all();
        }
        if inputs {
            self.input.scan(d);
        }
    }
}

impl Backend for Kms {
    fn dispatch(&mut self, d: &mut Desk, token: u64) {
        match token & !TOK_KIND_MASK {
            SUB_DRM => self.drm.dispatch(d),
            SUB_SIGNAL => self.signals(d),
            SUB_UEVENT => self.uevents(d),
            slot => {
                for a in self.input.dispatch(d, (slot - SLOT_BASE) as usize) {
                    match a {
                        Action::SwitchVt(n) => {
                            if let Some(vt) = &self.vt {
                                vt.activate(n);
                            }
                        }
                    }
                }
            }
        }
    }

    fn present(&mut self, d: &mut Desk, oi: usize) {
        let id = d.outputs[oi].id;
        match self.drm.mons.iter().position(|m| m.out_id == id) {
            Some(mi) => self.drm.present(d, oi, mi),
            // The output kept with every monitor unplugged: nothing to draw into.
            None => d.outputs[oi].frame_pending = true,
        }
    }

    fn read_pixels(&mut self, d: &mut Desk, oi: usize) -> Option<(Vec<u32>, i32, i32)> {
        let id = d.outputs.get(oi)?.id;
        let mi = self.drm.mons.iter().position(|m| m.out_id == id)?;
        Some(self.drm.pixels(mi))
    }

    fn housekeeping(&mut self, d: &mut Desk) {
        // Without uevents (a container), look for new keyboards and mice now and then.
        if self.uevents.is_none() {
            self.input.scan(d);
        }
    }

    fn name(&self) -> &'static str {
        "kms"
    }

    fn monitors(&self, _d: &Desk) -> Vec<crate::monitors::MonInfo> {
        self.drm.info()
    }

    fn reconfigure(&mut self, d: &mut Desk) {
        if !self.drm.paused {
            self.drm.apply(d);
        }
    }
}
