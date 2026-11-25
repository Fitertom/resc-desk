//! DRM/KMS output on the kernel's own ioctls (drm.h / drm_mode.h), no libdrm: every
//! connected connector gets a CRTC and its preferred mode, two dumb buffers and page flips
//! with an event. The frame is drawn into system memory (the renderers blend, and reading
//! write-combined video memory crawls) and only what changed is copied to the buffer being
//! flipped in, together with what that buffer missed while the other was shown.
//!
//! Legacy modesetting (SETCRTC + PAGE_FLIP) works on every driver; atomic, planes, a
//! hardware cursor and direct scanout of GPU frames come later (README, item 3).
//!
//! Each monitor's settings (monitors.rs) go on here: its mode, on or off, turned (the
//! frame is drawn upright and turned on the way into the buffer), a mirror of another one
//! (its frame scaled in), VRR (the CRTC's `VRR_ENABLED`) and HDR (10-bit buffers, the
//! connector's `max bpc`, `Colorspace` BT2020_RGB and `HDR_OUTPUT_METADATA` with PQ).
//! The properties go through the legacy property ioctls (an atomic driver commits them).
use crate::desk::Desk;
use crate::monitors::{self, Role};
use std::collections::HashMap;
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
const GETPROPERTY: u64 = ioc(RW, D, 0xAA, 64);
const GETPROPBLOB: u64 = ioc(RW, D, 0xAC, 16);
const OBJ_GETPROPERTIES: u64 = ioc(RW, D, 0xB9, 32);
const OBJ_SETPROPERTY: u64 = ioc(RW, D, 0xBA, 24);
const CREATEPROPBLOB: u64 = ioc(RW, D, 0xBD, 16);
const DESTROYPROPBLOB: u64 = ioc(RW, D, 0xBE, 4);
const OBJ_CRTC: u32 = 0xcccc_cccc;
const OBJ_CONNECTOR: u32 = 0xc0c0_c0c0;
const PROP_ENUM: u32 = 1 << 3;
const PROP_BITMASK: u32 = 1 << 5;

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

#[repr(C)]
#[derive(Default)]
struct GetProperty {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PropEnum {
    value: u64,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct GetBlob {
    blob_id: u32,
    length: u32,
    data: u64,
}

#[repr(C)]
#[derive(Default)]
struct ObjGetProps {
    props_ptr: u64,
    values_ptr: u64,
    count: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
#[derive(Default)]
struct ObjSetProp {
    value: u64,
    prop_id: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
#[derive(Default)]
struct CreateBlob {
    data: u64,
    length: u32,
    blob_id: u32,
}

/// struct hdr_output_metadata (static metadata type 1, CTA-861-G).
#[repr(C)]
#[derive(Default)]
struct HdrMetadata {
    metadata_type: u32,
    eotf: u8,
    kind: u8,
    primaries: [[u16; 2]; 3],
    white: [u16; 2],
    max_mastering: u16,
    min_mastering: u16,
    max_cll: u16,
    max_fall: u16,
}

// The kernel's sizes (x86_64 and aarch64 alike).
const _: () = assert!(std::mem::size_of::<GetProperty>() == 64);
const _: () = assert!(std::mem::size_of::<PropEnum>() == 40);
const _: () = assert!(std::mem::size_of::<GetBlob>() == 16);
const _: () = assert!(std::mem::size_of::<ObjGetProps>() == 32);
const _: () = assert!(std::mem::size_of::<ObjSetProp>() == 24);
const _: () = assert!(std::mem::size_of::<CreateBlob>() == 16);
const _: () = assert!(std::mem::size_of::<HdrMetadata>() == 32);
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
    /// The whole frame in system memory (upright: `dims()`).
    shadow: Vec<u32>,
    /// Its modes (one per size and refresh) and the preferred one.
    modes: Vec<ModeInfo>,
    pref: usize,
    pub rot: u16,
    pub role: Role,
    /// The connector's properties as they were when it came, and the CRTC's VRR switch.
    props: HashMap<String, Prop>,
    vrr_prop: Option<u32>,
    pub vrr_cap: bool,
    pub hdr_cap: bool,
    pub vrr: bool,
    /// HDR: 10-bit buffers, PQ.
    pub deep: bool,
    hdr_blob: u32,
    /// Flips asked for with older buffers are let go by (bumped whenever they change).
    gen: u32,
}

impl Monitor {
    /// The desk's size of it (the panel turned).
    pub fn dims(&self) -> (i32, i32) {
        monitors::rot_dims(self.w, self.h, self.rot)
    }

    fn mode_list(&self) -> Vec<(i32, i32, i32)> {
        self.modes.iter().map(mode_triple).collect()
    }
}

fn mode_mhz(m: &ModeInfo) -> i32 {
    let total = m.htotal as u64 * m.vtotal as u64;
    if total > 0 {
        (m.clock as u64 * 1_000_000 / total) as i32
    } else {
        m.vrefresh as i32 * 1000
    }
}

fn mode_triple(m: &ModeInfo) -> (i32, i32, i32) {
    (m.hdisplay as i32, m.vdisplay as i32, mode_mhz(m))
}

/// A property of an object: its id, value, and what it may be (a range, an enum's names).
#[derive(Clone, Debug, Default)]
struct Prop {
    id: u32,
    value: u64,
    values: Vec<u64>,
    enums: Vec<(u64, String)>,
}

fn cstr(b: &[u8]) -> String {
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

fn get_property(raw: RawFd, id: u32) -> Option<(String, Prop)> {
    let mut g = GetProperty { prop_id: id, ..Default::default() };
    unsafe { ioctl_ptr(raw, GETPROPERTY, &mut g).ok()? };
    let is_enum = g.flags & (PROP_ENUM | PROP_BITMASK) != 0;
    let mut values = vec![0u64; g.count_values as usize];
    let mut enums = vec![PropEnum { value: 0, name: [0; 32] }; if is_enum { g.count_enum_blobs as usize } else { 0 }];
    if !values.is_empty() || !enums.is_empty() {
        let mut g2 = GetProperty {
            prop_id: id,
            values_ptr: values.as_mut_ptr() as u64,
            count_values: values.len() as u32,
            enum_blob_ptr: if enums.is_empty() { 0 } else { enums.as_mut_ptr() as u64 },
            count_enum_blobs: enums.len() as u32,
            ..Default::default()
        };
        unsafe { ioctl_ptr(raw, GETPROPERTY, &mut g2).ok()? };
        values.truncate(g2.count_values as usize);
        enums.truncate(g2.count_enum_blobs as usize);
    }
    let enums = enums.iter().map(|e| (e.value, cstr(&e.name))).collect();
    Some((cstr(&g.name), Prop { id, value: 0, values, enums }))
}

/// Every property of object `obj` (a CRTC, a connector) by name.
fn obj_props(raw: RawFd, obj: u32, ty: u32) -> HashMap<String, Prop> {
    let mut out = HashMap::new();
    let mut g = ObjGetProps { obj_id: obj, obj_type: ty, ..Default::default() };
    if unsafe { ioctl_ptr(raw, OBJ_GETPROPERTIES, &mut g) }.is_err() {
        return out;
    }
    let n = g.count as usize;
    let mut ids = vec![0u32; n];
    let mut vals = vec![0u64; n];
    let mut g2 = ObjGetProps { props_ptr: ids.as_mut_ptr() as u64, values_ptr: vals.as_mut_ptr() as u64, count: n as u32, obj_id: obj, obj_type: ty };
    if unsafe { ioctl_ptr(raw, OBJ_GETPROPERTIES, &mut g2) }.is_err() {
        return out;
    }
    for i in 0..(g2.count as usize).min(n) {
        if let Some((name, mut p)) = get_property(raw, ids[i]) {
            p.value = vals[i];
            out.insert(name, p);
        }
    }
    out
}

fn set_prop(raw: RawFd, obj: u32, ty: u32, prop: u32, value: u64) -> std::io::Result<()> {
    let mut s = ObjSetProp { value, prop_id: prop, obj_id: obj, obj_type: ty };
    unsafe { ioctl_ptr(raw, OBJ_SETPROPERTY, &mut s).map(|_| ()) }
}

fn get_blob(raw: RawFd, id: u32) -> Option<Vec<u8>> {
    if id == 0 {
        return None;
    }
    let mut g = GetBlob { blob_id: id, ..Default::default() };
    unsafe { ioctl_ptr(raw, GETPROPBLOB, &mut g).ok()? };
    let mut data = vec![0u8; g.length as usize];
    let mut g2 = GetBlob { blob_id: id, length: g.length, data: data.as_mut_ptr() as u64 };
    unsafe { ioctl_ptr(raw, GETPROPBLOB, &mut g2).ok()? };
    data.truncate(g2.length as usize);
    Some(data)
}

/// The EDID says the monitor takes PQ (SMPTE ST 2084): a CTA-861 extension with an HDR
/// static metadata block whose EOTFs include it.
pub fn edid_takes_pq(edid: &[u8]) -> bool {
    for ext in edid.chunks(128).skip(1) {
        if ext.len() < 4 || ext[0] != 0x02 {
            continue;
        }
        let end = (ext[2] as usize).min(ext.len());
        let mut at = 4;
        while at < end {
            let (tag, len) = (ext[at] >> 5, (ext[at] & 31) as usize);
            if tag == 7 && len >= 2 && at + 2 < ext.len() && ext[at + 1] == 6 {
                return ext[at + 2] & 0b100 != 0;
            }
            at += 1 + len;
        }
    }
    false
}

/// A mirror's buffers: what they missed is the source's (the desk's) rects; this covers all.
const ALL: Rect = Rect { x: 0, y: 0, w: 1 << 15, h: 1 << 15 };

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
        // One mode per size and refresh (no interlaced ones), the preferred one first of its kind.
        let mut list: Vec<ModeInfo> = vec![mode];
        for m in &modes {
            if m.flags & (1 << 4) == 0 && !list.iter().any(|x| mode_triple(x) == mode_triple(m)) {
                list.push(*m);
            }
        }
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
        let refresh_mhz = mode_mhz(&mode);
        let mm = (c.mm_width > 0 && c.mm_height > 0).then(|| (c.mm_width as f64, c.mm_height as f64));
        let bufs = vec![self.dumb(w, h, false)?, self.dumb(w, h, false)?];
        let props = obj_props(raw, cid, OBJ_CONNECTOR);
        let vrr_prop = obj_props(raw, crtc, OBJ_CRTC).get("VRR_ENABLED").map(|p| p.id);
        let vrr_cap = vrr_prop.is_some() && props.get("vrr_capable").map_or(false, |p| p.value == 1);
        let edid = props.get("EDID").and_then(|p| get_blob(raw, p.value as u32)).unwrap_or_default();
        let hdr_cap = props.contains_key("HDR_OUTPUT_METADATA") && edid_takes_pq(&edid);
        eprintln!(
            "drm: {name}: {} modes, VRR {}, HDR {}",
            list.len(),
            if vrr_cap { "yes" } else { "no" },
            if hdr_cap { "yes" } else { "no" }
        );
        Ok(Some(Monitor {
            modes: list,
            pref: 0,
            rot: 0,
            role: Role::Show,
            props,
            vrr_prop,
            vrr_cap,
            hdr_cap,
            vrr: false,
            deep: false,
            hdr_blob: 0,
            gen: 0,
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
            if m.hdr_blob != 0 {
                let mut b = m.hdr_blob;
                let _ = unsafe { ioctl_ptr(raw, DESTROYPROPBLOB, &mut b) };
            }
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
                Ok(Some(m)) => {
                    self.mons.push(m);
                    changed = true;
                }
                Ok(None) => {}
                Err(e) => eprintln!("drm: connector {cid}: {e}"),
            }
        }
        if changed && !self.mons.is_empty() {
            self.apply(d);
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
        for b in m.bufs.drain(..) {
            self.free_buf(b);
        }
    }

    /// A dumb buffer as a framebuffer: XRGB8888, or XRGB2101010 when `deep` (HDR).
    fn dumb(&self, w: i32, h: i32, deep: bool) -> std::io::Result<DumbBuf> {
        let raw = self.fd.as_raw_fd();
        let mut c = CreateDumb { width: w as u32, height: h as u32, bpp: 32, ..Default::default() };
        unsafe { ioctl_ptr(raw, CREATE_DUMB, &mut c)? };
        let depth = if deep { 30 } else { 24 };
        let mut fb = FbCmd { width: w as u32, height: h as u32, pitch: c.pitch, bpp: 32, depth, handle: c.handle, ..Default::default() };
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
        let dmg = super::take_damage(d, oi);
        let m = &mut self.mons[mi];
        let (ow, oh) = m.dims();
        if m.shadow.len() != (ow * oh) as usize {
            m.shadow = vec![0xff00_0000; (ow * oh) as usize];
        }
        let mut cv = crate::render::Canvas { px: &mut m.shadow, w: ow, h: oh, stride: ow as usize };
        let painted = crate::render::render_output(d, oi, &mut cv, &dmg);
        let back = if m.set { 1 - m.front } else { m.front };
        let (rot, deep) = (m.rot, m.deep);
        for (i, b) in m.bufs.iter_mut().enumerate() {
            for r in &painted {
                b.missed.add(*r);
            }
            if i != back {
                continue;
            }
            let rects = std::mem::take(&mut b.missed);
            for r in &rects.rects {
                unsafe { monitors::copy_out(&m.shadow, ow, oh, *r, rot, b.map.ptr, b.pitch as usize, deep) };
            }
        }
        d.outputs[oi].frames += 1;
        let shown = self.show(mi, back);
        d.outputs[oi].frame_pending = shown && self.mons[mi].flip_pending;
        // Its mirrors.
        let name = self.mons[mi].name.clone();
        for j in 0..self.mons.len() {
            if self.mons[j].role == Role::Mirror(name.clone()) {
                self.feed_mirror(mi, j, &painted);
            }
        }
    }

    /// Puts buffer `back` of monitor `mi` on screen: the mode set first, else a page flip.
    /// False when it could not (the buffer then misses everything, for the next time).
    fn show(&mut self, mi: usize, back: usize) -> bool {
        let raw = self.fd.as_raw_fd();
        let m = &mut self.mons[mi];
        let fb = m.bufs[back].fb;
        if !m.set {
            let mut conn = m.conn;
            let mut req = zero_crtc();
            req.set_connectors_ptr = &mut conn as *mut u32 as u64;
            req.count_connectors = 1;
            req.crtc_id = m.crtc;
            req.fb_id = fb;
            req.mode_valid = 1;
            req.mode = m.mode;
            m.front = back;
            m.gen += 1;
            m.flip_pending = false;
            return match unsafe { ioctl_ptr(raw, SETCRTC, &mut req) } {
                Ok(_) => {
                    m.set = true;
                    true
                }
                Err(e) => {
                    eprintln!("drm: {}: setting {}x{} failed: {e}", m.name, m.w, m.h);
                    false
                }
            };
        }
        let user = mi as u64 | (m.gen as u64) << 32;
        let mut flip = PageFlip { crtc_id: m.crtc, fb_id: fb, flags: PAGE_FLIP_EVENT, user_data: user, ..Default::default() };
        match unsafe { ioctl_ptr(raw, PAGE_FLIP, &mut flip) } {
            Ok(_) => {
                m.flip_pending = true;
                true
            }
            Err(e) => {
                // EBUSY and the like: the buffer stays unshown; redraw it next time.
                eprintln!("drm: {}: page flip: {e}", m.name);
                let all = if matches!(m.role, Role::Mirror(_)) { ALL } else { Rect::new(0, 0, m.w.max(m.h), m.w.max(m.h)) };
                m.bufs[back].missed.add(all);
                false
            }
        }
    }

    /// Mirror `j` of monitor `src`: what changed on the source (`painted`, the source's
    /// rects), scaled into its buffer and flipped in when it can take a frame.
    fn feed_mirror(&mut self, src: usize, j: usize, painted: &[Rect]) {
        for b in &mut self.mons[j].bufs {
            for r in painted {
                b.missed.add(*r);
            }
        }
        if self.paused || self.mons[j].flip_pending {
            return;
        }
        let (sw, sh) = self.mons[src].dims();
        let shadow = std::mem::take(&mut self.mons[src].shadow);
        let m = &mut self.mons[j];
        let back = if m.set { 1 - m.front } else { m.front };
        let (dw, dh, deep) = (m.w, m.h, m.deep);
        let b = &mut m.bufs[back];
        let rects = std::mem::take(&mut b.missed);
        let any = !rects.rects.is_empty();
        for r in &rects.rects {
            unsafe { monitors::copy_mirror(&shadow, sw, sh, *r, b.map.ptr, b.pitch as usize, dw, dh, deep) };
        }
        self.mons[src].shadow = shadow;
        if any || !self.mons[j].set {
            self.show(j, back);
        }
    }

    /// The monitors as monitors.rs has them.
    pub fn info(&self) -> Vec<monitors::MonInfo> {
        self.mons
            .iter()
            .map(|m| monitors::MonInfo {
                name: m.name.clone(),
                modes: m.mode_list(),
                cur: mode_triple(&m.mode),
                rot: m.rot,
                on: m.role != Role::Off,
                mirror: match &m.role {
                    Role::Mirror(s) => Some(s.clone()),
                    _ => None,
                },
                vrr_cap: m.vrr_cap,
                vrr: m.vrr,
                hdr_cap: m.hdr_cap,
                hdr: m.deep,
            })
            .collect()
    }

    /// The CRTC of monitor `i` dark (its buffers stay; it is set again when shown).
    fn crtc_off(&mut self, i: usize) {
        let raw = self.fd.as_raw_fd();
        let m = &mut self.mons[i];
        let mut req = zero_crtc();
        req.crtc_id = m.crtc;
        let _ = unsafe { ioctl_ptr(raw, SETCRTC, &mut req) };
        m.set = false;
        m.flip_pending = false;
        m.gen += 1;
    }

    fn set_vrr(&mut self, i: usize, on: bool) {
        let raw = self.fd.as_raw_fd();
        let m = &mut self.mons[i];
        let on = on && m.vrr_cap;
        if let Some(p) = m.vrr_prop {
            if on != m.vrr || on {
                if let Err(e) = set_prop(raw, m.crtc, OBJ_CRTC, p, on as u64) {
                    eprintln!("drm: {}: VRR: {e}", m.name);
                    m.vrr = false;
                    return;
                }
            }
        }
        m.vrr = on;
    }

    /// HDR signalling on the connector: 10 bits a channel, BT.2020, PQ static metadata
    /// (SDR white at 203 nits, so nothing goes brighter); off: back as it was.
    fn set_hdr_props(&mut self, i: usize, on: bool) {
        let raw = self.fd.as_raw_fd();
        let m = &mut self.mons[i];
        let cid = m.conn;
        if let Some(p) = m.props.get("max bpc") {
            let (lo, hi) = (p.values.first().copied().unwrap_or(8), p.values.get(1).copied().unwrap_or(16));
            let v = if on { 10u64.clamp(lo, hi.max(lo)) } else { p.value };
            let _ = set_prop(raw, cid, OBJ_CONNECTOR, p.id, v);
        }
        if let Some(p) = m.props.get("Colorspace") {
            let want = if on { "BT2020_RGB" } else { "Default" };
            if let Some((v, _)) = p.enums.iter().find(|e| e.1 == want) {
                let _ = set_prop(raw, cid, OBJ_CONNECTOR, p.id, *v);
            }
        }
        let Some(p) = m.props.get("HDR_OUTPUT_METADATA").cloned() else { return };
        let old = m.hdr_blob;
        m.hdr_blob = 0;
        if on {
            let md = HdrMetadata {
                metadata_type: 0,
                eotf: 2,
                kind: 0,
                // BT.2020 primaries and D65, in 0.00002.
                primaries: [[35400, 14600], [8500, 39850], [6550, 2300]],
                white: [15635, 16450],
                max_mastering: 1000,
                min_mastering: 50,
                max_cll: monitors::SDR_WHITE as u16,
                max_fall: monitors::SDR_WHITE as u16,
            };
            let mut c = CreateBlob { data: &md as *const HdrMetadata as u64, length: std::mem::size_of::<HdrMetadata>() as u32, blob_id: 0 };
            if unsafe { ioctl_ptr(raw, CREATEPROPBLOB, &mut c) }.is_ok() {
                m.hdr_blob = c.blob_id;
            }
        }
        if let Err(e) = set_prop(raw, cid, OBJ_CONNECTOR, p.id, m.hdr_blob as u64) {
            eprintln!("drm: {}: HDR metadata: {e}", m.name);
        }
        if old != 0 {
            let mut b = old;
            let _ = unsafe { ioctl_ptr(raw, DESTROYPROPBLOB, &mut b) };
        }
    }

    /// Applies the monitor settings (monitors.rs): each one's role (Win+P), mode, rotation,
    /// VRR and HDR; outputs come and go with them.
    pub fn apply(&mut self, d: &mut Desk) {
        let names: Vec<String> = self.mons.iter().map(|m| m.name.clone()).collect();
        // No main one chosen: the first monitor (it stays so while others come and go).
        if d.display_cfg.primary.is_none() {
            d.display_cfg.primary = names.first().cloned();
        }
        let want = monitors::roles(&names, &d.display_cfg.mons, d.display_cfg.primary.as_deref(), d.display_cfg.project);
        let mut gone = Vec::new();
        for (i, role) in want.into_iter().enumerate() {
            let cfg = d.display_cfg.mons.get(&names[i]).cloned().unwrap_or_default();
            if role == Role::Off {
                if self.mons[i].role != Role::Off {
                    self.crtc_off(i);
                    if self.mons[i].deep {
                        self.set_hdr_props(i, false);
                    }
                }
                if self.mons[i].out_id != 0 {
                    gone.push(self.mons[i].out_id);
                    self.mons[i].out_id = 0;
                }
                self.mons[i].role = role;
                continue;
            }
            // The mode and the buffers' depth.
            let list = self.mons[i].mode_list();
            let mode = self.mons[i].modes[monitors::pick_mode(&list, cfg.mode, self.mons[i].pref)];
            let hdr = cfg.hdr && self.mons[i].hdr_cap;
            let (w, h) = (mode.hdisplay as i32, mode.vdisplay as i32);
            let m = &self.mons[i];
            if (w, h) != (m.w, m.h) || hdr != m.deep || m.role == Role::Off {
                match (self.dumb(w, h, hdr), self.dumb(w, h, hdr)) {
                    (Ok(a), Ok(b)) => {
                        let old = std::mem::replace(&mut self.mons[i].bufs, vec![a, b]);
                        let m = &mut self.mons[i];
                        m.w = w;
                        m.h = h;
                        m.deep = hdr;
                        m.set = false;
                        m.front = 0;
                        m.gen += 1;
                        m.flip_pending = false;
                        for b in old {
                            self.free_buf(b);
                        }
                    }
                    (a, b) => {
                        eprintln!("drm: {}: no {}x{}{} buffers", names[i], w, h, if hdr { " 10-bit" } else { "" });
                        for x in [a, b].into_iter().flatten() {
                            self.free_buf(x);
                        }
                        continue;
                    }
                }
            }
            if mode_triple(&mode) != mode_triple(&self.mons[i].mode) {
                self.mons[i].mode = mode;
                self.mons[i].refresh_mhz = mode_mhz(&mode);
                self.mons[i].set = false;
            }
            if self.mons[i].hdr_cap && (hdr || self.mons[i].hdr_blob != 0) {
                self.set_hdr_props(i, hdr);
                self.mons[i].set = false;
            }
            self.set_vrr(i, cfg.vrr);
            let turned = (self.mons[i].rot % 180 == 90) != (cfg.rot % 180 == 90);
            self.mons[i].rot = cfg.rot;
            let (ow, oh) = self.mons[i].dims();
            if self.mons[i].shadow.len() != (ow * oh) as usize || turned {
                self.mons[i].shadow = vec![0xff00_0000; (ow * oh) as usize];
            }
            let mirror = matches!(role, Role::Mirror(_));
            for b in &mut self.mons[i].bufs {
                if mirror {
                    // Bars around the scaled picture: black, once.
                    unsafe { std::ptr::write_bytes(b.map.ptr, 0, b.map.len) };
                }
                b.missed.add(ALL);
            }
            match &role {
                Role::Show => match d.output_index(self.mons[i].out_id).filter(|_| self.mons[i].out_id != 0) {
                    Some(oi) => {
                        if (d.outputs[oi].pw, d.outputs[oi].ph) != (ow, oh) || turned {
                            d.reshape_output(oi, ow, oh, turned);
                        }
                        d.outputs[oi].refresh_mhz = self.mons[i].refresh_mhz;
                    }
                    None => {
                        self.mons[i].out_id = add_output(d, &self.mons[i], &self.path);
                    }
                },
                _ => {
                    if self.mons[i].out_id != 0 {
                        gone.push(self.mons[i].out_id);
                        self.mons[i].out_id = 0;
                    }
                }
            }
            self.mons[i].role = role;
        }
        for id in gone {
            if let Some(oi) = d.output_index(id) {
                if d.outputs.len() > 1 {
                    d.remove_output(oi);
                }
            }
        }
        crate::layout::relayout(d);
        for o in d.outputs.iter_mut() {
            o.frame_pending = false;
        }
        d.damage_all();
    }

    fn free_buf(&self, b: DumbBuf) {
        let raw = self.fd.as_raw_fd();
        let mut fb = b.fb;
        let _ = unsafe { ioctl_ptr(raw, RMFB, &mut fb) };
        let mut hd = b.handle;
        drop(b.map);
        let _ = unsafe { ioctl_ptr(raw, DESTROY_DUMB, &mut hd) };
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
                    let user = u64::from_ne_bytes(buf[at + 8..at + 16].try_into().unwrap());
                    let (mi, gen) = ((user & 0xffff_ffff) as usize, (user >> 32) as u32);
                    if let Some(m) = self.mons.get_mut(mi).filter(|m| m.gen == gen && m.flip_pending) {
                        m.flip_pending = false;
                        m.front = 1 - m.front;
                        if m.out_id != 0 {
                            if let Some(oi) = d.output_index(m.out_id) {
                                d.outputs[oi].frame_pending = false;
                            }
                        }
                        if let Role::Mirror(src) = m.role.clone() {
                            if let Some(si) = self.mons.iter().position(|x| x.name == src) {
                                self.feed_mirror(si, mi, &[]);
                            }
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
            m.gen += 1;
            for b in &mut m.bufs {
                b.missed.add(ALL);
            }
        }
        // Another session may have changed them.
        for i in 0..self.mons.len() {
            if self.mons[i].role != Role::Off {
                let (deep, vrr) = (self.mons[i].deep, self.mons[i].vrr);
                if deep {
                    self.set_hdr_props(i, true);
                }
                if vrr {
                    self.set_vrr(i, true);
                }
            }
        }
        for o in d.outputs.iter_mut() {
            o.frame_pending = false;
        }
        d.damage_all();
    }

    pub fn pixels(&self, mi: usize) -> (Vec<u32>, i32, i32) {
        let m = &self.mons[mi];
        let (w, h) = m.dims();
        (m.shadow.clone(), w, h)
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
        for i in 0..self.mons.len() {
            if self.mons[i].deep || self.mons[i].hdr_blob != 0 {
                self.set_hdr_props(i, false);
            }
            if self.mons[i].vrr {
                self.set_vrr(i, false);
            }
        }
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
    let (w, h) = m.dims();
    let mm = if m.rot % 180 == 90 { m.mm.map(|(a, b)| (b, a)) } else { m.mm };
    let spec = super::OutSpec { w, h, scale: 1.0, mm, pos_mm: None, hz: None };
    let idx = d.add_output(m.name.clone(), w, h, &spec, m.refresh_mhz);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr_is_read_from_the_edid() {
        // A base block and a CTA-861 extension: a video block, then HDR static metadata.
        let mut edid = vec![0u8; 256];
        let ext = &mut edid[128..];
        ext[0] = 0x02;
        ext[1] = 3;
        // Video data block (tag 2, 2 bytes), HDR static metadata (tag 7, ext tag 6).
        ext[4..7].copy_from_slice(&[2 << 5 | 2, 16, 4]);
        ext[7..11].copy_from_slice(&[7 << 5 | 3, 6, 0b0000_0101, 1]);
        ext[2] = 11;
        assert!(edid_takes_pq(&edid));
        // SDR and HLG only: no PQ.
        edid[128 + 9] = 0b0000_1001;
        assert!(!edid_takes_pq(&edid));
        // No extension at all.
        assert!(!edid_takes_pq(&edid[..128]));
    }

    #[test]
    fn hdr_metadata_is_the_kernels_layout() {
        let md = HdrMetadata { eotf: 2, max_cll: 203, ..Default::default() };
        let b: &[u8] = unsafe { std::slice::from_raw_parts(&md as *const HdrMetadata as *const u8, 32) };
        assert_eq!(b[4], 2, "eotf right after the u32 type");
        assert_eq!(u16::from_ne_bytes([b[26], b[27]]), 203, "max_cll at offset 26");
    }
}
