//! DRM/KMS output on the kernel's own ioctls (drm.h / drm_mode.h), no libdrm: every
//! connected connector gets a CRTC and its preferred mode, two dumb buffers and page flips
//! with an event. The frame is drawn into system memory (the renderers blend, and reading
//! write-combined video memory crawls) and only what changed is copied to the buffer being
//! flipped in, together with what that buffer missed while the other was shown.
//!
//! Legacy modesetting (SETCRTC + PAGE_FLIP) works on every driver; atomic, planes, a
//! hardware cursor, VRR and direct scanout of GPU frames come later (README, item 3).
use crate::desk::Desk;
use crate::geom::{Damage, Rect};
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use sys::{ioc, ioctl_ptr, IOC_READ, IOC_WRITE};

const D: u8 = b'd';
const RW: u64 = IOC_READ | IOC_WRITE;
const SET_MASTER: u64 = ioc(0, D, 0x1e, 0);
const DROP_MASTER: u64 = ioc(0, D, 0x1f, 0);
const GET_CAP: u64 = ioc(RW, D, 0x0c, 16);
const GETRESOURCES: u64 = ioc(RW, D, 0xA0, 64);
const GETCRTC: u64 = ioc(RW, D, 0xA1, 104);
const SETCRTC: u64 = ioc(RW, D, 0xA2, 104);
const GETENCODER: u64 = ioc(RW, D, 0xA6, 20);
const GETCONNECTOR: u64 = ioc(RW, D, 0xA7, 80);
const ADDFB: u64 = ioc(RW, D, 0xAE, 28);
const RMFB: u64 = ioc(RW, D, 0xAF, 4);
const PAGE_FLIP: u64 = ioc(RW, D, 0xB0, 24);
const CREATE_DUMB: u64 = ioc(RW, D, 0xB2, 32);
const MAP_DUMB: u64 = ioc(RW, D, 0xB3, 16);
const DESTROY_DUMB: u64 = ioc(RW, D, 0xB4, 4);

const CAP_DUMB_BUFFER: u64 = 1;
const PAGE_FLIP_EVENT: u32 = 1;
const EVENT_FLIP_COMPLETE: u32 = 2;
const MODE_TYPE_PREFERRED: u32 = 1 << 3;
const CONNECTED: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    ty: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CrtcReq {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
struct PageFlip {
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    reserved: u32,
    user_data: u64,
}

#[repr(C)]
#[derive(Default)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

// The kernel's sizes (x86_64 and aarch64 alike).
const _: () = assert!(std::mem::size_of::<ModeInfo>() == 68);
const _: () = assert!(std::mem::size_of::<CrtcReq>() == 104);
const _: () = assert!(std::mem::size_of::<CardRes>() == 64);
const _: () = assert!(std::mem::size_of::<GetConnector>() == 80);
const _: () = assert!(std::mem::size_of::<GetEncoder>() == 20);
const _: () = assert!(std::mem::size_of::<FbCmd>() == 28);
const _: () = assert!(std::mem::size_of::<PageFlip>() == 24);
const _: () = assert!(std::mem::size_of::<CreateDumb>() == 32);
const _: () = assert!(std::mem::size_of::<MapDumb>() == 16);

fn zero_mode() -> ModeInfo {
    unsafe { std::mem::zeroed() }
}

fn zero_crtc() -> CrtcReq {
    unsafe { std::mem::zeroed() }
}

/// drm_connector_type names, as the kernel and every compositor print them.
fn connector_name(ty: u32, id: u32) -> String {
    let n = match ty {
        1 => "VGA",
        2 => "DVI-I",
        3 => "DVI-D",
        4 => "DVI-A",
        5 => "Composite",
        6 => "SVIDEO",
        7 => "LVDS",
        8 => "Component",
        9 => "DIN",
        10 => "DP",
        11 => "HDMI-A",
        12 => "HDMI-B",
        13 => "TV",
        14 => "eDP",
        15 => "Virtual",
        16 => "DSI",
        17 => "DPI",
        18 => "Writeback",
        19 => "SPI",
        20 => "USB",
        _ => "Unknown",
    };
    format!("{n}-{id}")
}

struct DumbBuf {
    handle: u32,
    fb: u32,
    pitch: u32,
    map: sys::Mmap,
    /// What changed on screen while this buffer was not being drawn.
    missed: Damage,
}

pub struct Monitor {
    pub out_id: u32,
    pub name: String,
    conn: u32,
    crtc: u32,
    mode: ModeInfo,
    pub w: i32,
    pub h: i32,
    pub mm: Option<(f64, f64)>,
    pub refresh_mhz: i32,
    bufs: Vec<DumbBuf>,
    front: usize,
    flip_pending: bool,
    /// The mode is set (SETCRTC done since we got the device last).
    set: bool,
    /// The CRTC as it was before us, put back on exit.
    saved: CrtcReq,
    /// The whole frame in system memory.
    shadow: Vec<u32>,
}

pub struct Drm {
    pub fd: OwnedFd,
    pub path: String,
    pub mons: Vec<Monitor>,
    pub paused: bool,
    /// The output kept with no monitor behind it (all unplugged).
    pub orphan: Option<u32>,
}

impl Drm {
    /// The first card with a connected monitor (`RESC_DRM_DEVICE` picks one).
    pub fn open() -> std::io::Result<Drm> {
        let mut tried = Vec::new();
        let paths: Vec<String> = match std::env::var("RESC_DRM_DEVICE") {
            Ok(p) => vec![p],
            Err(_) => {
                let dir = std::fs::read_dir("/dev/dri").map_err(|e| std::io::Error::new(e.kind(), format!("no /dev/dri ({e}): no GPU driver here")))?;
                let mut v: Vec<String> = dir
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().to_string_lossy().into_owned())
                    .filter(|p| p.rsplit('/').next().map_or(false, |n| n.starts_with("card")))
                    .collect();
                v.sort();
                v
            }
        };
        for path in paths {
            match Self::open_one(&path) {
                Ok(d) if !d.mons.is_empty() => return Ok(d),
                Ok(_) => tried.push(format!("{path}: no monitor connected")),
                Err(e) => tried.push(format!("{path}: {e}")),
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, if tried.is_empty() { "no /dev/dri/card*".into() } else { tried.join("; ") }))
    }

    fn open_one(path: &str) -> std::io::Result<Drm> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(0o4000 | 0o2000000).open(path)?;
        let fd: OwnedFd = f.into();
        let raw = fd.as_raw_fd();
        // Opening a card no one drives makes us its master; when another one is, this asks.
        let _ = sys::ioctl_int(raw, SET_MASTER, 0);
        let mut cap = [CAP_DUMB_BUFFER, 0u64];
        unsafe { ioctl_ptr(raw, GET_CAP, cap.as_mut_ptr())? };
        if cap[1] == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no dumb buffers"));
        }
        let mut drm = Drm { fd, path: path.to_string(), mons: Vec::new(), paused: false, orphan: None };
        drm.scan()?;
        Ok(drm)
    }

    /// The card's CRTCs and connectors.
    fn resources(&self) -> std::io::Result<(Vec<u32>, Vec<u32>)> {
        let raw = self.fd.as_raw_fd();
        let mut res = CardRes::default();
        unsafe { ioctl_ptr(raw, GETRESOURCES, &mut res)? };
        let mut crtcs = vec![0u32; res.count_crtcs as usize];
        let mut conns = vec![0u32; res.count_connectors as usize];
        let mut encs = vec![0u32; res.count_encoders as usize];
        let mut fbs = vec![0u32; res.count_fbs as usize];
        let mut res2 = CardRes {
            fb_id_ptr: fbs.as_mut_ptr() as u64,
            crtc_id_ptr: crtcs.as_mut_ptr() as u64,
            connector_id_ptr: conns.as_mut_ptr() as u64,
            encoder_id_ptr: encs.as_mut_ptr() as u64,
            count_fbs: fbs.len() as u32,
            count_crtcs: crtcs.len() as u32,
            count_connectors: conns.len() as u32,
            count_encoders: encs.len() as u32,
            ..Default::default()
        };
        unsafe { ioctl_ptr(raw, GETRESOURCES, &mut res2)? };
        crtcs.truncate(res2.count_crtcs as usize);
        conns.truncate(res2.count_connectors as usize);
        Ok((crtcs, conns))
    }

    /// Connected connectors, each with a free CRTC and its preferred mode.
    fn scan(&mut self) -> std::io::Result<()> {
        let (crtcs, conns) = self.resources()?;
        for cid in conns {
            if let Some(m) = self.probe(cid, &crtcs)? {
                self.mons.push(m);
            }
        }
        Ok(())
    }

    /// Connector `cid` as a monitor, if something is plugged into it and a CRTC is free.
    fn probe(&self, cid: u32, crtcs: &[u32]) -> std::io::Result<Option<Monitor>> {
        let raw = self.fd.as_raw_fd();
        let Some((c, modes, enc_ids)) = get_connector(raw, cid) else { return Ok(None) };
        if c.connection != CONNECTED || modes.is_empty() {
            return Ok(None);
        }
        let name = connector_name(c.connector_type, c.connector_type_id);
        let mode = *modes.iter().find(|m| m.ty & MODE_TYPE_PREFERRED != 0).unwrap_or(&modes[0]);
        // The CRTC it is on now if free, else any free one its encoders can drive.
        let used: Vec<u32> = self.mons.iter().map(|m| m.crtc).collect();
        let mut cands: Vec<u32> = Vec::new();
        for &eid in std::iter::once(&c.encoder_id).chain(enc_ids.iter()) {
            if eid == 0 {
                continue;
            }
            let mut e = GetEncoder { encoder_id: eid, ..Default::default() };
            if unsafe { ioctl_ptr(raw, GETENCODER, &mut e) }.is_err() {
                continue;
            }
            if eid == c.encoder_id && e.crtc_id != 0 {
                cands.insert(0, e.crtc_id);
            }
            for (i, &k) in crtcs.iter().enumerate() {
                if e.possible_crtcs & (1 << i) != 0 {
                    cands.push(k);
                }
            }
        }
        let Some(crtc) = cands.into_iter().find(|k| !used.contains(k)) else {
            eprintln!("drm: {name}: no free CRTC");
            return Ok(None);
        };
        let mut saved = zero_crtc();
        saved.crtc_id = crtc;
        let _ = unsafe { ioctl_ptr(raw, GETCRTC, &mut saved) };
        let (w, h) = (mode.hdisplay as i32, mode.vdisplay as i32);
        let total = mode.htotal as u64 * mode.vtotal as u64;
        let refresh_mhz = if total > 0 { (mode.clock as u64 * 1_000_000 / total) as i32 } else { mode.vrefresh as i32 * 1000 };
        let mm = (c.mm_width > 0 && c.mm_height > 0).then(|| (c.mm_width as f64, c.mm_height as f64));
        let bufs = vec![self.dumb(w, h)?, self.dumb(w, h)?];
        Ok(Some(Monitor {
            out_id: 0,
            name,
            conn: cid,
            crtc,
            mode,
            w,
            h,
            mm,
            refresh_mhz,
            bufs,
            front: 0,
            flip_pending: false,
            set: false,
            saved,
            shadow: vec![0xff00_0000; (w * h) as usize],
        }))
    }

    /// A monitor plugged in or out (a uevent said so): new ones become outputs, gone ones
    /// stop being outputs (their windows move to the others). True when anything changed.
    pub fn hotplug(&mut self, d: &mut Desk) -> bool {
        let Ok((crtcs, conns)) = self.resources() else { return false };
        let raw = self.fd.as_raw_fd();
        let mut changed = false;
        // Gone: not connected any more (or the connector itself went, a DP-MST hub). Their
        // CRTCs are free for what is new; their outputs go once the new ones are there.
        let mut gone = Vec::new();
        let mut i = 0;
        while i < self.mons.len() {
            let cid = self.mons[i].conn;
            let still = conns.contains(&cid) && get_connector(raw, cid).map_or(false, |(c, m, _)| c.connection == CONNECTED && !m.is_empty());
            if still {
                i += 1;
                continue;
            }
            let m = self.mons.remove(i);
            eprintln!("kms: {} unplugged", m.name);
            // The CRTC off: nothing scans out of buffers about to be freed.
            let mut req = zero_crtc();
            req.crtc_id = m.crtc;
            let _ = unsafe { ioctl_ptr(raw, SETCRTC, &mut req) };
            gone.push(m.out_id);
            self.free(m);
            changed = true;
        }
        for cid in conns {
            if self.mons.iter().any(|m| m.conn == cid) {
                continue;
            }
            match self.probe(cid, &crtcs) {
                Ok(Some(mut m)) => {
                    m.out_id = add_output(d, &m, &self.path);
                    self.mons.push(m);
                    changed = true;
                }
                Ok(None) => {}
                Err(e) => eprintln!("drm: connector {cid}: {e}"),
            }
        }
        // The desk always keeps an output: with no monitor left, the last one stays (dark,
        // not drawn) until one is plugged in again.
        if let Some(keep) = self.orphan.take() {
            gone.push(keep);
        }
        for id in gone {
            if self.mons.is_empty() && self.orphan.is_none() {
                self.orphan = Some(id);
                continue;
            }
            if let Some(oi) = d.output_index(id) {
                d.remove_output(oi);
            }
        }
        changed
    }

    fn free(&self, mut m: Monitor) {
        let raw = self.fd.as_raw_fd();
        for b in m.bufs.drain(..) {
            let mut fb = b.fb;
            let _ = unsafe { ioctl_ptr(raw, RMFB, &mut fb) };
            let mut hd = b.handle;
            drop(b.map);
            let _ = unsafe { ioctl_ptr(raw, DESTROY_DUMB, &mut hd) };
        }
    }

    fn dumb(&self, w: i32, h: i32) -> std::io::Result<DumbBuf> {
        let raw = self.fd.as_raw_fd();
        let mut c = CreateDumb { width: w as u32, height: h as u32, bpp: 32, ..Default::default() };
        unsafe { ioctl_ptr(raw, CREATE_DUMB, &mut c)? };
        let mut fb = FbCmd { width: w as u32, height: h as u32, pitch: c.pitch, bpp: 32, depth: 24, handle: c.handle, ..Default::default() };
        if let Err(e) = unsafe { ioctl_ptr(raw, ADDFB, &mut fb) } {
            let mut hd = c.handle;
            let _ = unsafe { ioctl_ptr(raw, DESTROY_DUMB, &mut hd) };
            return Err(e);
        }
        let mut m = MapDumb { handle: c.handle, ..Default::default() };
        unsafe { ioctl_ptr(raw, MAP_DUMB, &mut m)? };
        let map = sys::Mmap::map_at(self.fd.as_fd(), c.size as usize, m.offset, true, true)?;
        // Black, and everything missed: the first frame goes up whole.
        unsafe { std::ptr::write_bytes(map.ptr, 0, c.size as usize) };
        let mut missed = Damage::default();
        missed.add(Rect::new(0, 0, w, h));
        Ok(DumbBuf { handle: c.handle, fb: fb.fb_id, pitch: c.pitch, map, missed })
    }

    /// Draws output `oi` (monitor `mi`) and flips it in; with nothing to flip in yet, sets the mode.
    pub fn present(&mut self, d: &mut Desk, oi: usize, mi: usize) {
        if self.paused || self.mons[mi].flip_pending {
            d.outputs[oi].frame_pending = true;
            return;
        }
        let raw = self.fd.as_raw_fd();
        let dmg = super::take_damage(d, oi);
        let m = &mut self.mons[mi];
        let (w, h) = (m.w, m.h);
        let mut cv = crate::render::Canvas { px: &mut m.shadow, w, h, stride: w as usize };
        let painted = crate::render::render_output(d, oi, &mut cv, &dmg);
        let back = if m.set { 1 - m.front } else { m.front };
        for (i, b) in m.bufs.iter_mut().enumerate() {
            for r in &painted {
                b.missed.add(*r);
            }
            if i != back {
                continue;
            }
            let rects = std::mem::take(&mut b.missed);
            for r in rects.rects.iter().map(|r| r.intersect(&Rect::new(0, 0, w, h))).filter(|r| !r.is_empty()) {
                for y in r.y..r.y1() {
                    let src = &m.shadow[(y * w + r.x) as usize..(y * w + r.x1()) as usize];
                    unsafe {
                        let dst = b.map.ptr.add(y as usize * b.pitch as usize + r.x as usize * 4) as *mut u32;
                        std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
                    }
                }
            }
        }
        let fb = m.bufs[back].fb;
        let o = &mut d.outputs[oi];
        o.frames += 1;
        if !m.set {
            let mut conn = m.conn;
            let mut req = zero_crtc();
            req.set_connectors_ptr = &mut conn as *mut u32 as u64;
            req.count_connectors = 1;
            req.crtc_id = m.crtc;
            req.fb_id = fb;
            req.mode_valid = 1;
            req.mode = m.mode;
            match unsafe { ioctl_ptr(raw, SETCRTC, &mut req) } {
                Ok(_) => m.set = true,
                Err(e) => eprintln!("drm: {}: setting {}x{} failed: {e}", m.name, w, h),
            }
            m.front = back;
            o.frame_pending = false;
            return;
        }
        let mut flip = PageFlip { crtc_id: m.crtc, fb_id: fb, flags: PAGE_FLIP_EVENT, user_data: mi as u64, ..Default::default() };
        match unsafe { ioctl_ptr(raw, PAGE_FLIP, &mut flip) } {
            Ok(_) => {
                m.flip_pending = true;
                o.frame_pending = true;
            }
            Err(e) => {
                // EBUSY and the like: the buffer stays unshown; redraw it next time.
                eprintln!("drm: {}: page flip: {e}", m.name);
                m.bufs[back].missed.add(Rect::new(0, 0, w, h));
                o.frame_pending = false;
            }
        }
    }

    /// Reads DRM events: a finished flip frees its output for the next frame.
    pub fn dispatch(&mut self, d: &mut Desk) {
        let raw = self.fd.as_raw_fd();
        let mut buf = [0u8; 1024];
        loop {
            let n = unsafe { libc_read(raw, &mut buf) };
            if n <= 0 {
                return;
            }
            let mut at = 0usize;
            while at + 8 <= n as usize {
                let ty = u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap());
                let len = u32::from_ne_bytes(buf[at + 4..at + 8].try_into().unwrap()) as usize;
                if len < 8 || at + len > n as usize {
                    break;
                }
                if ty == EVENT_FLIP_COMPLETE && len >= 16 {
                    let mi = u64::from_ne_bytes(buf[at + 8..at + 16].try_into().unwrap()) as usize;
                    if let Some(m) = self.mons.get_mut(mi) {
                        m.flip_pending = false;
                        m.front = 1 - m.front;
                        if let Some(oi) = d.output_index(m.out_id) {
                            d.outputs[oi].frame_pending = false;
                        }
                    }
                }
                at += len;
            }
        }
    }

    /// Another session takes the screen (VT switch): give up being master.
    pub fn pause(&mut self) {
        self.paused = true;
        let _ = sys::ioctl_int(self.fd.as_raw_fd(), DROP_MASTER, 0);
    }

    /// Back on our VT: master again, modes set anew, everything redrawn.
    pub fn resume(&mut self, d: &mut Desk) {
        if let Err(e) = sys::ioctl_int(self.fd.as_raw_fd(), SET_MASTER, 0) {
            eprintln!("drm: cannot become master again: {e}");
        }
        self.paused = false;
        for m in &mut self.mons {
            m.set = false;
            m.flip_pending = false;
            for b in &mut m.bufs {
                b.missed.add(Rect::new(0, 0, m.w, m.h));
            }
        }
        for o in d.outputs.iter_mut() {
            o.frame_pending = false;
        }
        d.damage_all();
    }

    pub fn pixels(&self, mi: usize) -> (Vec<u32>, i32, i32) {
        let m = &self.mons[mi];
        (m.shadow.clone(), m.w, m.h)
    }
}

unsafe fn libc_read(fd: RawFd, buf: &mut [u8]) -> isize {
    extern "C" {
        fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
    }
    read(fd, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len())
}

fn get_connector(raw: RawFd, id: u32) -> Option<(GetConnector, Vec<ModeInfo>, Vec<u32>)> {
    for _ in 0..4 {
        let mut c = GetConnector { connector_id: id, ..Default::default() };
        unsafe { ioctl_ptr(raw, GETCONNECTOR, &mut c).ok()? };
        let mut modes = vec![zero_mode(); c.count_modes as usize];
        let mut encs = vec![0u32; c.count_encoders as usize];
        let mut props = vec![0u32; c.count_props as usize];
        let mut vals = vec![0u64; c.count_props as usize];
        let (nm, ne, np) = (c.count_modes, c.count_encoders, c.count_props);
        let mut c2 = GetConnector {
            connector_id: id,
            modes_ptr: modes.as_mut_ptr() as u64,
            encoders_ptr: encs.as_mut_ptr() as u64,
            props_ptr: props.as_mut_ptr() as u64,
            prop_values_ptr: vals.as_mut_ptr() as u64,
            count_modes: nm,
            count_encoders: ne,
            count_props: np,
            ..Default::default()
        };
        unsafe { ioctl_ptr(raw, GETCONNECTOR, &mut c2).ok()? };
        // A monitor plugged in between the two calls: ask again.
        if c2.count_modes > nm || c2.count_encoders > ne || c2.count_props > np {
            continue;
        }
        modes.truncate(c2.count_modes as usize);
        encs.truncate(c2.count_encoders as usize);
        return Some((c2, modes, encs));
    }
    None
}

impl Drop for Drm {
    fn drop(&mut self) {
        let raw = self.fd.as_raw_fd();
        for m in std::mem::take(&mut self.mons) {
            // What was on the CRTC before us (the console's framebuffer), if anything was.
            if m.saved.fb_id != 0 {
                let mut conn = m.conn;
                let mut req = m.saved;
                req.set_connectors_ptr = &mut conn as *mut u32 as u64;
                req.count_connectors = 1;
                let _ = unsafe { ioctl_ptr(raw, SETCRTC, &mut req) };
            }
            self.free(m);
        }
        let _ = sys::ioctl_int(raw, DROP_MASTER, 0);
    }
}

/// Monitor `m` as a desk output, everything damaged: its id.
pub fn add_output(d: &mut Desk, m: &Monitor, card: &str) -> u32 {
    let spec = super::OutSpec { w: m.w, h: m.h, scale: 1.0, mm: m.mm, pos_mm: None };
    let idx = d.add_output(m.name.clone(), m.w, m.h, &spec, m.refresh_mhz);
    let r = d.outputs[idx].rect();
    d.outputs[idx].damage.add(r);
    eprintln!(
        "kms: {} {}x{} @ {:.2} Hz{} on {card}",
        m.name,
        m.w,
        m.h,
        m.refresh_mhz as f64 / 1000.0,
        m.mm.map_or(String::new(), |(w, h)| format!(", {w}x{h} mm")),
    );
    d.outputs[idx].id
}
