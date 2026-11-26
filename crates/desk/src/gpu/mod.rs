//! Vulkan renderer (RADV on AMD, ANV on Intel, lavapipe for tests).
//!
//! What the GPU holds: the wallpaper (one texture per output), every client surface (one
//! texture, only the rects the client damaged go up), every window's frame and shadow (one
//! texture per window and output, drawn by the CPU only when the title, focus, a hovered
//! button or the size changes), and two CPU layers: what lies on the taskbar's glass, and the
//! overlays (start menu, clipboard, Alt+Tab content, the cursor) — redrawn and uploaded only
//! where they have content. Moving a window costs the CPU nothing: the same textures are
//! drawn elsewhere.
//!
//! Frame: in every rect the damage grew to (whole glass panels and what their frost reads),
//! bottom to top: wallpaper, windows (frame, then surfaces), the taskbar's glass, layer 0,
//! the overlays' glass, layer 1. Glass is Liquify's shaders as they are
//! (`shaders/liquify/ui.frag` kind 2 over a mip-mapped copy of what lies under the panel,
//! the shadow of `glass.frag`). The result comes back to the backend's buffer (headless,
//! nested); a KMS backend will scan the frame out instead.
//!
//! Clients' dma-bufs (games, browsers, Xwayland) are not copied: each becomes an image over
//! the same memory (DRM format modifiers), sampled where it is. The client's GPU work on it
//! is waited for on the GPU (its fence as a sync_file, Linux 6.0+); the image is taken from
//! the "foreign" queue family before drawing and given back after.
mod vk;

use crate::desk::Desk;
use crate::geom::{Damage, Rect};
use crate::render::{Canvas, GpuItem, Xf};
use crate::state::{SKey, WKey};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::rc::{Rc, Weak};
use vk::*;

const SPV_FULLSCREEN: &[u8] = include_bytes!("../../../../shaders/spv/fullscreen.vert.spv");
const SPV_LAYER: &[u8] = include_bytes!("../../../../shaders/spv/layer.frag.spv");
const SPV_GLASS_VERT: &[u8] = include_bytes!("../../../../shaders/spv/glass.vert.spv");
const SPV_SHADOW: &[u8] = include_bytes!("../../../../shaders/spv/shadow.frag.spv");
const SPV_UI_VERT: &[u8] = include_bytes!("../../../../shaders/spv/ui.vert.spv");
const SPV_UI_FRAG: &[u8] = include_bytes!("../../../../shaders/spv/ui.frag.spv");
const SPV_GLASS_FRAG: &[u8] = include_bytes!("../../../../shaders/spv/glass.frag.spv");
const SPV_SURF_VERT: &[u8] = include_bytes!("../../../../shaders/spv/surface.vert.spv");
const SPV_SURF_FRAG: &[u8] = include_bytes!("../../../../shaders/spv/surface.frag.spv");
const SPV_BLUR: &[u8] = include_bytes!("../../../../shaders/spv/blur.frag.spv");
const SPV_DROP_VERT: &[u8] = include_bytes!("../../../../shaders/spv/drop.vert.spv");
const SPV_DROP_FRAG: &[u8] = include_bytes!("../../../../shaders/spv/drop.frag.spv");
/// Descriptor sets: three per output plus one per surface and window frame.
const MAX_SETS: u32 = 4096;

/// ui.vert `Inst`: 12 vec4.
const INST_SIZE: u64 = 12 * 16;
const MAX_GLASS: u64 = 64;
/// common.glsl push constants (std430): the largest block, the layout's one range.
const PC_SIZE: u32 = 104;
const FMT: u32 = FORMAT_B8G8R8A8_UNORM;

type R<T> = Result<T, String>;

struct Image {
    img: H,
    mem: H,
    view: H,
}

struct Buffer {
    buf: H,
    mem: H,
    ptr: *mut u8,
    size: u64,
}

/// A sampled texture: a client surface, a window frame, the wallpaper.
struct Tex {
    img: Image,
    set: H,
    w: i32,
    h: i32,
    /// XRGB: alpha reads 1 (the view's swizzle).
    opaque: bool,
    /// Never written: UNDEFINED, not to be drawn until its first upload.
    fresh: bool,
}

/// A window's frame and shadow on one output: where it goes (physical) and what it shows.
struct FrameTex {
    tex: Tex,
    at: Rect,
    key: u64,
}

/// Rects of an upload buffer going into an image this frame.
struct Upload {
    img: H,
    fresh: bool,
    buf: H,
    copies: Vec<BufferImageCopy>,
}

/// Something drawn for the windows, bottom to top: a texture or a solid rect.
enum Draw {
    Tex { set: H, dst: Rect, uv: [f32; 4], round: [f32; 6], alpha: f32 },
    Color { dst: Rect, color: [f32; 4], round: [f32; 6], alpha: f32 },
}

/// surface.frag's rounded clip (physical x, y, w, h, top corners, bottom corners); corners
/// 0: none.
fn round_pc(xf: crate::render::Xf, round: Option<(Rect, i32)>) -> [f32; 6] {
    match round {
        Some((r, rad)) if rad > 0 => {
            let p = xf.r(r);
            let rad = rad as f32 * xf.scale as f32;
            [p.x as f32, p.y as f32, p.w as f32, p.h as f32, rad, rad]
        }
        _ => [0.0; 6],
    }
}

/// Everything one output needs, sized to it.
struct Out {
    id: u32,
    w: i32,
    h: i32,
    /// CPU layers (0: taskbar content, 1: overlays and cursor): the CPU draws straight into
    /// the mapped buffer, the image is what the GPU samples.
    layers: [(Image, Buffer, H); 2],
    /// Where each layer had content last frame: it is cleared there when that goes.
    ext: [Vec<Rect>; 2],
    wall: Tex,
    wall_key: Option<(u64, i32, i32)>,
    frames: HashMap<WKey, FrameTex>,
    target: Image,
    backdrop: Image,
    levels: u32,
    bd_set: H,
    /// The frost: a separable gaussian on a level of the backdrop (blur.frag, horizontal
    /// into [0], vertical into [1]; ui.frag reads [1]), one per level a sigma may need
    /// (each glass place blurs its own amount): `blurs[l]`, l ≥ 1.
    blurs: Vec<Option<Blur>>,
    readback: Buffer,
    inst: Buffer,
    /// Not drawn yet: every image starts UNDEFINED and the whole output is damaged.
    fresh: bool,
    /// Windows' frosts kept between frames (GlassOp::id).
    gcache: HashMap<u64, GCache>,
}

/// A window glass's frost kept between frames (GlassOp::id): the blurred level, read by
/// ui.frag like Blur's [1].
struct GCache {
    img: Image,
    set: H,
    level: u32,
    under: u64,
    fresh: bool,
}

/// Backdrops captured and blurred, and kept frosts used instead (ctl stats).
pub static BLURS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static KEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Frames drawn: what cannot be known to be the same (a drop's backdrop) hashes this.
static FRAME_NO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn hash_draw(h: &mut std::collections::hash_map::DefaultHasher, dr: &Draw, gen: u64) {
    use std::hash::Hash;
    let r = |h: &mut std::collections::hash_map::DefaultHasher, r: &Rect| (r.x, r.y, r.w, r.h).hash(h);
    let fs = |h: &mut std::collections::hash_map::DefaultHasher, v: &[f32]| v.iter().for_each(|x| x.to_bits().hash(h));
    match dr {
        Draw::Tex { set, dst, uv, round, alpha } => {
            (0u8, *set, gen).hash(h);
            r(h, dst);
            fs(h, uv);
            fs(h, round);
            fs(h, &[*alpha]);
        }
        Draw::Color { dst, color, round, alpha } => {
            1u8.hash(h);
            r(h, dst);
            fs(h, color);
            fs(h, round);
            fs(h, &[*alpha]);
        }
    }
}

fn draw_dst(dr: &Draw) -> Rect {
    match dr {
        Draw::Tex { dst, .. } | Draw::Color { dst, .. } => *dst,
    }
}

/// What lies under a glass whose frost reads `zone` (physical), drawn before it: the
/// wallpaper, `draws` with their pictures' generations, and the glass below (`ops`).
fn under_key(zone: Rect, sigma: f32, wall: (u64, i32, i32), draws: &[Draw], gens: &[u64], ops: &[GlassOp]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (zone.x, zone.y, zone.w, zone.h, sigma.to_bits(), wall).hash(&mut h);
    for (i, dr) in draws.iter().enumerate() {
        if draw_dst(dr).overlaps(&zone) {
            hash_draw(&mut h, dr, gens.get(i).copied().unwrap_or(0));
        }
    }
    for op in ops.iter().filter(|o| o.rect.overlaps(&zone)) {
        (op.rect.x, op.rect.y, op.rect.w, op.rect.h, op.id).hash(&mut h);
        op.radii.iter().chain(op.tint.iter()).chain([op.bezel, op.sigma].iter()).for_each(|x| x.to_bits().hash(&mut h));
        if let Some(inst) = &op.drop {
            inst.iter().flatten().for_each(|x| x.to_bits().hash(&mut h));
        }
        // A glass whose own backdrop is not known: this frame only.
        let u = if op.id != 0 { op.under } else { FRAME_NO.load(std::sync::atomic::Ordering::Relaxed) };
        u.hash(&mut h);
    }
    h.finish() | 1
}

/// The frost's images and sets (see Out::blur).
struct Blur {
    /// The backdrop's level it blurs.
    level: u32,
    w: i32,
    h: i32,
    imgs: [Image; 2],
    /// A view of the backdrop's level the blur reads.
    src_view: H,
    /// Reads the backdrop's level, reads [0], reads [1] (all five bindings: ui.frag).
    sets: [H; 3],
}

/// The frost's level and kernel for a sigma of `sigma` physical px: the level whose texel
/// is about a quarter sigma (the mip boxes and the bilinear upscale stay well under the
/// kernel: no blocks, no ghost copies), so the kernel is 4 to 8 texels.
fn blur_plan(sigma: f32, w: i32, h: i32) -> Option<(u32, f32, f32)> {
    if sigma < 1.0 {
        return None;
    }
    let full = 32 - (w.max(h) as u32).leading_zeros();
    // Level 1 at least: no full-size blur images (a small sigma is a 1–2 texel kernel there).
    let l = ((sigma / 4.0).log2().floor().max(1.0) as u32).min(full.saturating_sub(1));
    let sl = sigma / (1u32 << l) as f32;
    // Minus what the box chain and the bilinear upscale add (about a quarter texel squared).
    let k = if l > 0 { (sl * sl - 0.25).max(0.25).sqrt() } else { sl };
    Some((l, k, (3.0 * k).ceil().min(48.0)))
}

pub struct Gpu {
    pub name: String,
    i: Instance,
    f: Device,
    inst: H,
    dev: H,
    queue: H,
    mem: MemoryProperties,
    pool: H,
    cmd: H,
    fence: H,
    dsl: H,
    layout: H,
    dpool: H,
    s_near: H,
    s_mip: H,
    s_lin: H,
    p_copy: H,
    p_over: H,
    p_shadow: H,
    p_glass: H,
    p_tex: H,
    /// blur.frag: one direction of the frost's gaussian.
    p_blur: H,
    /// Liquify's liquid glass (glass.frag): clear glass (settings), refraction along the edge.
    p_lens: H,
    /// drop.frag: a window or the preview card moving as a drop, bridged to the hover drop.
    p_drop: H,
    outs: Vec<Out>,
    surfs: HashMap<SKey, Tex>,
    /// Closed windows' last pictures (life.rs `holds`): their surfaces are gone.
    ghost_tex: HashMap<SKey, Tex>,
    /// Upload buffers for this frame's texture data, each with how much of it is taken.
    arena: Vec<(Buffer, u64)>,
    family: u32,
    /// dma-buf import: the entry points, the GPU's /dev/dri node, what can be imported.
    dma: Option<DmaFns>,
    sem_fns: Option<SemFns>,
    drm_dev: Option<u64>,
    dma_formats: Vec<(u32, u64)>,
    /// Imported dma-bufs, per wl_buffer (its address; the Weak says when it is gone).
    imports: HashMap<usize, Import>,
}

/// A client's dma-buf as an image.
struct Import {
    buf: Weak<crate::shm::Buffer>,
    tex: Tex,
    /// For the client's fence (imported anew each frame it is drawn).
    sem: H,
    /// Taken from the foreign queue at least once (its layout is GENERAL from then on).
    used: bool,
}

/// DRM fourcc → Vulkan format (one plane, 8 bits per channel).
fn vk_format(drm: u32) -> u32 {
    use crate::dmabuf::*;
    match drm {
        ABGR8888 | XBGR8888 => FORMAT_R8G8B8A8_UNORM,
        _ => FMT,
    }
}

const DMA_EXTS: [&[u8]; 4] = [b"VK_KHR_external_memory_fd\0", b"VK_EXT_external_memory_dma_buf\0", b"VK_EXT_image_drm_format_modifier\0", b"VK_EXT_queue_family_foreign\0"];
const SEM_EXT: &[u8] = b"VK_KHR_external_semaphore_fd\0";
const DRM_EXT: &[u8] = b"VK_EXT_physical_device_drm\0";
const KHR_DYNAMIC_RENDERING: &[u8] = b"VK_KHR_dynamic_rendering\0";

/// One glass panel of the frame, physical px.
#[derive(Default)]
pub struct GlassOp {
    pub rect: Rect,
    /// Corner radii: top-left, top-right, bottom-right, bottom-left.
    pub radii: [f32; 4],
    /// Drawn before this CPU layer (1: the taskbar's, 2: the overlays'); 0: a window's glass,
    /// drawn before the window draw at `before` (so over the windows below it).
    pub layer: usize,
    pub before: usize,
    /// Premultiplied tint.
    pub tint: [f32; 4],
    /// Refraction band along the edge (physical px).
    pub bezel: f32,
    /// The panel's own soft shadow and hairline (windows have theirs in the frame).
    pub panel: bool,
    /// Up to two rects (physical) the window's opaque content covers: where it is all that
    /// shows, the glass is not drawn (nor does damage there widen to it).
    pub cover: [Rect; 2],
    /// A drop (drop.frag's instance, winanim.rs) instead of a box.
    pub drop: Option<[[f32; 4]; 12]>,
    /// Its frost's sigma, physical px (each glass place has its own, look.rs; 0: none).
    pub sigma: f32,
    /// Its frost kept between frames (a window's glass): who it is (0: not kept), what lay
    /// under it (a hash of everything drawn below it in its zone), and whether the kept frost
    /// is of that very picture (then it is neither captured nor blurred again, and damage
    /// inside it does not grow to the whole panel).
    pub id: u64,
    pub under: u64,
    pub hit: bool,
}

/// Window glass: deeper than the panels', so titles stay readable over anything.
const TINT_WINDOW: [f32; 4] = [20.0 / 255.0 * 0.62, 18.0 / 255.0 * 0.62, 28.0 / 255.0 * 0.62, 0.62];

/// The glass under a window with our frame (frosted glass only): the whole frame, a drop of
/// glass with the title bar on it and a 1 px rim all round the content. An opaque client
/// covers the rest: its own drawing there leaves the glass be.
fn window_glass(d: &Desk, wk: WKey, xf: Xf, before: usize) -> Option<GlassOp> {
    if !crate::glass::frosted() {
        return None;
    }
    let w = d.windows.get(wk)?;
    if w.toast {
        // A notification: a rim of glass around it.
        let f = crate::wm::frame_rect(d, wk)?.inflate(crate::toast::RIM);
        let s = xf.scale as f32;
        let g = crate::look::glass(crate::look::Place::of_app(&w.app_id));
        let tint = crate::glass::tint_at(TINT_WINDOW, g.dim);
        let r = crate::toast::RADIUS as f32 * s;
        return Some(GlassOp { rect: xf.r(f), radii: [r; 4], layer: 0, before, tint, bezel: g.bezel * s, panel: false, cover: [Rect::default(); 2], drop: None, sigma: g.blur * s, ..Default::default() });
    }
    if w.tile == crate::state::Tile::Fullscreen {
        return None;
    }
    // A frameless one only when it asks for the glass under all of it and is see-through.
    let bare = !w.ssd;
    if bare && !(crate::look::glass_under(&w.app_id) && d.surfaces.get(w.surf).is_some_and(|s| !s.is_opaque())) {
        return None;
    }
    let f = crate::wm::frame_rect(d, wk)?;
    let s = xf.scale as f32;
    let rad = crate::shell::window_radius(d, wk);
    let r = rad as f32 * s;
    let opaque = d.surfaces.get(w.surf).map_or(true, |s| s.is_opaque());
    let mut cover = [Rect::default(); 2];
    if let Some(c) = crate::winanim::content_rect(d, wk).filter(|_| opaque) {
        // All of it but its bottom corners (rounded off: the glass shows there).
        let (c, n) = (xf.r(c), xf.len(rad));
        cover = [Rect::new(c.x, c.y, c.w, c.h - n), Rect::new(c.x + n, c.y, c.w - 2 * n, c.h)];
    }
    // The place's own dim, blur and edge (look.rs): «Проводник», «Параметры», other windows.
    let g = crate::look::glass(crate::look::Place::of_app(&w.app_id));
    let tint = crate::glass::tint_at(TINT_WINDOW, g.dim);
    Some(GlassOp { rect: xf.r(f), radii: [r; 4], layer: 0, before, tint, bezel: g.bezel * s, panel: false, cover, drop: None, sigma: g.blur * s, ..Default::default() })
}

/// The drop a moving drop bridges to: the hover one or a drop of its own (life.rs) at its
/// button, unless another drop has it. Taken all the way, k = 0 at the ends too: drawn on its
/// own (layer 1, over the windows) it would frost the drop's stretched content right under it
/// into a white blot.
fn bridge(lenses: &[Rect], p: &crate::winanim::Pose, taken: &mut Vec<Rect>) -> Option<Rect> {
    let l = lenses.iter().copied().find(|l| !taken.contains(l) && l.overlaps(&p.btn))?;
    taken.push(l);
    Some(l)
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// A moving drop's glass (drop.frag) where `p` is now, on output `xf`. Its look goes from a
/// window's (the deep tint, its frame's shadow and rim light; `active`: focused) to a panel's
/// (the card) by `p.look.panel`; `hover`: the taskbar's hover drop it bridges to, in its own
/// look (shell.rs `Lens`).
fn drop_op(xf: Xf, p: &crate::winanim::Pose, active: bool, hover: Option<Rect>, layer: usize, before: usize) -> GlassOp {
    use crate::glass;
    let s = xf.scale as f32;
    let l = &p.look;
    let phys = |b: [f32; 4]| [xf.fx(b[0] as f64) as f32, xf.fy(b[1] as f64) as f32, xf.fx(b[2] as f64) as f32, xf.fy(b[3] as f64) as f32];
    let a = phys(l.b);
    let (b, rb) = match hover {
        Some(h) => (phys([h.x as f32, h.y as f32, h.x1() as f32, h.y1() as f32]), crate::shell::lens_radius(h) as f32 * s),
        None => ([0.0; 4], 0.0),
    };
    let t = l.panel;
    // shell.rs `draw_shadow` (squared fall to its reach) and glass.frag's (sigma 20, 54 px).
    let (dy, sigma, strength) = if active { (6.0, 10.0, 0.45) } else { (3.0, 7.0, 0.28) };
    let win_sh = [strength, sigma * s, dy * s, (crate::shell::SHADOW as f32 - dy) * s];
    let panel_sh = [glass::SHADOW * 0.6, 20.0 * s, 6.0 * s, 54.0 * s];
    let mut sh_a = mix4(win_sh, panel_sh, t);
    sh_a[0] *= l.sh;
    // shell.rs `draw_deco`'s rim light; the panels' inset highlight and hairline.
    let rim = if active { [60.0, 22.0] } else { [30.0, 14.0] };
    let look_a = [rim[0] / 255.0 * (1.0 - t), rim[1] / 255.0 * (1.0 - t), glass::HIGHLIGHT * t, glass::BORDER_A * t];
    let look_b = [0.0, 0.0, glass::HIGHLIGHT, glass::BORDER_A];
    let bez = [glass::win_bezel() * s + (glass::bezel() - glass::win_bezel()) * s * t, glass::bezel() * s, 1.0 - t, 0.0];
    let mut bb = a;
    if hover.is_some() {
        bb = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    }
    let m = 64.0 * s;
    let tint = mix4(TINT_WINDOW, glass::TINT_DEEP, t);
    let inst = [
        [bb[0] - m, bb[1] - m, bb[2] + m, bb[3] + m],
        a,
        b,
        [l.r * s, rb, p.k * s, s],
        tint,
        glass::TINT_LENS,
        sh_a,
        panel_sh,
        look_a,
        look_b,
        bez,
        [glass::THICKNESS * s, glass::BORDER_W * s, 0.0, 0.0],
    ];
    let rect = Rect::from_edges(bb[0].floor() as i32, bb[1].floor() as i32, bb[2].ceil() as i32, bb[3].ceil() as i32);
    GlassOp { rect, radii: [l.r * s; 4], layer, before, tint, bezel: bez[0], panel: false, cover: [Rect::default(); 2], drop: Some(inst), sigma: glass::frost() * s, ..Default::default() }
}

/// One of «Пуск»'s drops (startanim.rs) on output `xf`, in a panel's look; `hover`: the
/// taskbar's hover drop it bridges to (the menu's glass, at the start button).
fn start_op(xf: Xf, sp: &crate::startanim::DropSpec, hover: Option<Rect>) -> GlassOp {
    use crate::glass;
    let s = xf.scale as f32;
    let phys = |b: [f32; 4]| [xf.fx(b[0] as f64) as f32, xf.fy(b[1] as f64) as f32, xf.fx(b[2] as f64) as f32, xf.fy(b[3] as f64) as f32];
    let sh = [sp.sh[0], sp.sh[1] * s, sp.sh[2] * s, sp.sh[3] * s];
    let a = phys(sp.a);
    let (b, rb, tint_b, sh_b, bez_b) = match (sp.b, hover) {
        (Some((r, rad)), _) => (phys(r), rad * s, sp.tint[1], sh, sp.bezel * s),
        (None, Some(h)) => {
            let lens_sh = [glass::SHADOW * 0.6, 20.0 * s, 6.0 * s, 54.0 * s];
            (phys([h.x as f32, h.y as f32, h.x1() as f32, h.y1() as f32]), crate::shell::lens_radius(h) as f32 * s, glass::TINT_LENS, lens_sh, glass::bezel() * s)
        }
        _ => ([0.0; 4], 0.0, sp.tint[1], sh, sp.bezel * s),
    };
    let mut bb = a;
    if b[2] - b[0] >= 1.0 && b[3] - b[1] >= 1.0 {
        bb = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    }
    let m = (sp.sh[3] + 10.0) * s;
    let look = [0.0, 0.0, glass::HIGHLIGHT, glass::BORDER_A];
    let inst = [
        [bb[0] - m, bb[1] - m, bb[2] + m, bb[3] + m],
        a,
        b,
        [sp.ra * s, rb, sp.k * s, s],
        sp.tint[0],
        tint_b,
        sh,
        sh_b,
        look,
        look,
        [sp.bezel * s, bez_b, 0.0, 0.0],
        [glass::THICKNESS * s, glass::BORDER_W * s, 0.0, 0.0],
    ];
    let rect = Rect::from_edges(bb[0].floor() as i32, bb[1].floor() as i32, bb[2].ceil() as i32, bb[3].ceil() as i32);
    GlassOp { rect, radii: [sp.ra * s; 4], layer: sp.layer, before: 0, tint: sp.tint[0], bezel: sp.bezel * s, panel: false, cover: [Rect::default(); 2], drop: Some(inst), sigma: crate::glass::frost() * s, ..Default::default() }
}

fn cstr(s: &[u8]) -> *const std::ffi::c_char {
    s.as_ptr() as _
}

/// Premultiplied ARGB as the shader's RGBA.
fn unpack(c: u32) -> [f32; 4] {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), ch(24)]
}

/// Everything a window's frame texture shows: when it is the same, the texture is reused
/// wherever the window is (at fractional scales, the position's fraction of a physical
/// pixel counts: rounding inside the frame depends on it).
fn frame_key(d: &Desk, wk: WKey, xf: Xf, ext: Rect, at: Rect) -> u64 {
    use std::hash::{Hash, Hasher};
    let w = &d.windows[wk];
    let hover = d.seat.hover.filter(|(k, _)| *k == wk).map(|(_, h)| h);
    let pressed = match d.seat.grab {
        crate::input::Grab::Deco { wk: g, hit } if g == wk => Some(hit),
        _ => None,
    };
    let q = |v: f64| (v * 1e6).round() as i64;
    let fx = q(xf.fx(ext.x as f64) - at.x as f64);
    let fy = q(xf.fy(ext.y as f64) - at.y as f64);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}", w.tile, w.ssd, w.activated, w.title, w.app_id, hover, pressed, xf.scale.to_bits(), fx, fy, ext.w * 65536 + ext.h, crate::glass::frosted())
        .hash(&mut h);
    h.finish()
}

static RENDERER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// What draws the screen, for people ("Vulkan · <device>" or the CPU).
pub fn renderer() -> String {
    let r = RENDERER.lock().map(|s| s.clone()).unwrap_or_default();
    if r.is_empty() { "процессор (без видеокарты)".into() } else { r }
}

/// The renderer fell back to the CPU.
pub fn renderer_lost() {
    if let Ok(mut s) = RENDERER.lock() {
        s.clear();
    }
}

/// `RESC_RENDER`: `cpu`, `vulkan` (any device, lavapipe included), or unset: Vulkan on a real GPU.
pub fn init() -> Option<Box<Gpu>> {
    let mode = std::env::var("RESC_RENDER").unwrap_or_default();
    if mode == "cpu" {
        return None;
    }
    match unsafe { Gpu::new(mode == "vulkan") } {
        Ok(g) => {
            eprintln!("resc-desk: render: Vulkan, {}", g.name);
            if let Ok(mut s) = RENDERER.lock() {
                *s = format!("Vulkan · {}", g.name);
            }
            Some(Box::new(g))
        }
        Err(e) => {
            eprintln!("resc-desk: render: CPU ({e})");
            None
        }
    }
}

impl Gpu {
    unsafe fn new(allow_cpu: bool) -> R<Gpu> {
        let gipa = vk::loader()?;
        let g = Global::load(|n| gipa(0, cstr(n)))?;
        let mut ver = 0u32;
        (g.vkEnumerateInstanceVersion)(&mut ver);
        if ver < API_1_3 {
            return Err(format!("Vulkan loader {}.{} < 1.3", ver >> 22, (ver >> 12) & 0x3ff));
        }
        let app = ApplicationInfo {
            s_type: st::APPLICATION_INFO,
            p_next: std::ptr::null(),
            app_name: cstr(b"resc-desk\0"),
            app_version: 1,
            engine_name: cstr(b"resc\0"),
            engine_version: 1,
            api_version: API_1_3,
        };
        let validate = std::env::var_os("RESC_VK_VALIDATE").is_some();
        let layers = [cstr(b"VK_LAYER_KHRONOS_validation\0")];
        let mut ci = InstanceCreateInfo {
            s_type: st::INSTANCE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            app_info: &app,
            layer_count: validate as u32,
            layers: layers.as_ptr(),
            ext_count: 0,
            exts: std::ptr::null(),
        };
        let mut inst = 0;
        if (g.vkCreateInstance)(&ci, std::ptr::null(), &mut inst) != SUCCESS {
            if validate {
                eprintln!("resc-desk: render: no VK_LAYER_KHRONOS_validation, running without it");
            }
            ci.layer_count = 0;
            check((g.vkCreateInstance)(&ci, std::ptr::null(), &mut inst), "vkCreateInstance")?;
        }
        let i = Instance::load(|n| gipa(inst, cstr(n)))?;
        match Self::open(&i, inst, allow_cpu) {
            Ok(mut gpu) => {
                if ci.layer_count > 0 {
                    gpu.name += " + validation";
                }
                Ok(gpu)
            }
            Err(e) => {
                (i.vkDestroyInstance)(inst, std::ptr::null());
                Err(e)
            }
        }
    }

    unsafe fn has_ext(i: &Instance, pd: H, want: &[u8]) -> bool {
        let mut n = 0u32;
        (i.vkEnumerateDeviceExtensionProperties)(pd, std::ptr::null(), &mut n, std::ptr::null_mut());
        let mut props: Vec<ExtensionProperties> = (0..n).map(|_| std::mem::zeroed()).collect();
        (i.vkEnumerateDeviceExtensionProperties)(pd, std::ptr::null(), &mut n, props.as_mut_ptr());
        props.iter().any(|e| std::ffi::CStr::from_ptr(e.name.as_ptr()).to_bytes_with_nul() == want)
    }

    unsafe fn open(i: &Instance, inst: H, allow_cpu: bool) -> R<Gpu> {
        let mut n = 0u32;
        (i.vkEnumeratePhysicalDevices)(inst, &mut n, std::ptr::null_mut());
        let mut pds = vec![0; n as usize];
        (i.vkEnumeratePhysicalDevices)(inst, &mut n, pds.as_mut_ptr());
        // Best first: a discrete GPU, then an integrated one, then anything else.
        let rank = |t: u32| match t {
            DEVICE_TYPE_DISCRETE => 0,
            DEVICE_TYPE_INTEGRATED => 1,
            DEVICE_TYPE_VIRTUAL => 2,
            DEVICE_TYPE_CPU => 3,
            _ => 4,
        };
        // Vulkan 1.3, or 1.2 with VK_KHR_dynamic_rendering (dzn: Direct3D 12 under WSL2).
        let mut best: Option<(u32, H, String, u32, bool)> = None;
        let mut seen = Vec::new();
        for pd in pds {
            let mut props = DeviceProps([0; 1024]);
            (i.vkGetPhysicalDeviceProperties)(pd, &mut props);
            let (t, name) = (props.device_type(), props.name());
            seen.push(name.clone());
            let khr = props.api_version() < API_1_3;
            if props.api_version() < API_1_2 || (t == DEVICE_TYPE_CPU && !allow_cpu) {
                continue;
            }
            if khr && !Self::has_ext(i, pd, KHR_DYNAMIC_RENDERING) {
                continue;
            }
            let mut qn = 0u32;
            (i.vkGetPhysicalDeviceQueueFamilyProperties)(pd, &mut qn, std::ptr::null_mut());
            let mut qs = vec![QueueFamilyProperties::default(); qn as usize];
            (i.vkGetPhysicalDeviceQueueFamilyProperties)(pd, &mut qn, qs.as_mut_ptr());
            let Some(fam) = qs.iter().position(|q| q.flags & QUEUE_GRAPHICS != 0) else { continue };
            let mut dr = DynamicRenderingFeatures { s_type: ST_DYNAMIC_RENDERING_FEATURES, p_next: std::ptr::null_mut(), dynamic_rendering: 0 };
            let mut f2 = Features2 { s_type: st::PHYSICAL_DEVICE_FEATURES_2, p_next: &mut dr as *mut _ as *mut c_void, features: [0; 55] };
            (i.vkGetPhysicalDeviceFeatures2)(pd, &mut f2);
            if dr.dynamic_rendering == 0 {
                continue;
            }
            if best.as_ref().map_or(true, |b| rank(t) < rank(b.0)) {
                best = Some((t, pd, name, fam as u32, khr));
            }
        }
        let Some((_, pd, name, family, khr)) = best else {
            return Err(format!("no Vulkan GPU with dynamic rendering (devices: {})", if seen.is_empty() { "none".into() } else { seen.join(", ") }));
        };
        // dma-buf import, when the driver has all of it.
        let mut en = 0u32;
        (i.vkEnumerateDeviceExtensionProperties)(pd, std::ptr::null(), &mut en, std::ptr::null_mut());
        let mut eprops: Vec<ExtensionProperties> = (0..en).map(|_| std::mem::zeroed()).collect();
        (i.vkEnumerateDeviceExtensionProperties)(pd, std::ptr::null(), &mut en, eprops.as_mut_ptr());
        let names: Vec<Vec<u8>> = eprops.iter().map(|e| std::ffi::CStr::from_ptr(e.name.as_ptr()).to_bytes_with_nul().to_vec()).collect();
        let has = |e: &[u8]| names.iter().any(|n| n.as_slice() == e);
        let dma_ok = DMA_EXTS.iter().all(|e| has(e));
        let sem_ok = dma_ok && has(SEM_EXT);
        let mut exts: Vec<*const std::ffi::c_char> = Vec::new();
        if dma_ok {
            exts.extend(DMA_EXTS.iter().map(|e| cstr(e)));
        }
        if sem_ok {
            exts.push(cstr(SEM_EXT));
        }
        if khr {
            exts.push(cstr(KHR_DYNAMIC_RENDERING));
        }
        let drm_dev = if has(DRM_EXT) {
            let mut drm = DrmProperties { s_type: sx::PHYSICAL_DEVICE_DRM_PROPERTIES, p_next: std::ptr::null_mut(), has_primary: 0, has_render: 0, primary_major: 0, primary_minor: 0, render_major: 0, render_minor: 0 };
            let mut p2 = Properties2 { s_type: sx::PHYSICAL_DEVICE_PROPERTIES_2, p_next: &mut drm as *mut _ as *mut c_void, props: DeviceProps([0; 1024]) };
            (i.vkGetPhysicalDeviceProperties2)(pd, &mut p2);
            if drm.has_render != 0 {
                Some(crate::dmabuf::makedev(drm.render_major as u64, drm.render_minor as u64))
            } else if drm.has_primary != 0 {
                Some(crate::dmabuf::makedev(drm.primary_major as u64, drm.primary_minor as u64))
            } else {
                None
            }
        } else {
            None
        };
        let dma_formats = if dma_ok && drm_dev.is_some() { Self::dma_formats(i, pd) } else { Vec::new() };
        let prio = 1.0f32;
        let qci = DeviceQueueCreateInfo { s_type: st::DEVICE_QUEUE_CREATE_INFO, p_next: std::ptr::null(), flags: 0, family, count: 1, priorities: &prio };
        let dr = DynamicRenderingFeatures { s_type: ST_DYNAMIC_RENDERING_FEATURES, p_next: std::ptr::null_mut(), dynamic_rendering: 1 };
        let dci = DeviceCreateInfo {
            s_type: st::DEVICE_CREATE_INFO,
            p_next: &dr as *const _ as P,
            flags: 0,
            queue_count: 1,
            queues: &qci,
            layer_count: 0,
            layers: std::ptr::null(),
            ext_count: exts.len() as u32,
            exts: exts.as_ptr(),
            features: std::ptr::null(),
        };
        let mut dev = 0;
        check((i.vkCreateDevice)(pd, &dci, std::ptr::null(), &mut dev), "vkCreateDevice")?;
        let gdpa = i.vkGetDeviceProcAddr;
        // On 1.2 the rendering entry points carry the extension's suffix.
        let f = Device::load(|n| {
            let p = gdpa(dev, cstr(n));
            if !p.is_null() || !khr {
                return p;
            }
            let mut k = n[..n.len() - 1].to_vec();
            k.extend_from_slice(b"KHR\0");
            gdpa(dev, cstr(&k))
        })?;
        let dma = if dma_ok { DmaFns::load(|n| gdpa(dev, cstr(n))).ok() } else { None };
        let sem_fns = if sem_ok { SemFns::load(|n| gdpa(dev, cstr(n))).ok() } else { None };
        let mut mem = std::mem::zeroed::<MemoryProperties>();
        (i.vkGetPhysicalDeviceMemoryProperties)(pd, &mut mem);
        let mut queue = 0;
        (f.vkGetDeviceQueue)(dev, family, 0, &mut queue);

        let mut gpu = Gpu {
            name,
            i: *i,
            f,
            inst,
            dev,
            queue,
            mem,
            pool: 0,
            cmd: 0,
            fence: 0,
            dsl: 0,
            layout: 0,
            dpool: 0,
            s_near: 0,
            s_mip: 0,
            s_lin: 0,
            p_copy: 0,
            p_over: 0,
            p_shadow: 0,
            p_glass: 0,
            p_blur: 0,
            p_tex: 0,
            p_lens: 0,
            p_drop: 0,
            outs: Vec::new(),
            surfs: HashMap::new(),
            ghost_tex: HashMap::new(),
            arena: Vec::new(),
            family,
            dma_formats: if dma.is_some() { dma_formats } else { Vec::new() },
            dma,
            sem_fns,
            drm_dev,
            imports: HashMap::new(),
        };
        gpu.setup(family)?;
        Ok(gpu)
    }

    unsafe fn setup(&mut self, family: u32) -> R<()> {
        let (f, dev) = (&self.f, self.dev);
        let null = std::ptr::null();
        let pci = CommandPoolCreateInfo { s_type: st::COMMAND_POOL_CREATE_INFO, p_next: null, flags: POOL_RESET_COMMAND_BUFFER, family };
        check((f.vkCreateCommandPool)(dev, &pci, null, &mut self.pool), "vkCreateCommandPool")?;
        let cai = CommandBufferAllocateInfo { s_type: st::COMMAND_BUFFER_ALLOCATE_INFO, p_next: null, pool: self.pool, level: 0, count: 1 };
        check((f.vkAllocateCommandBuffers)(dev, &cai, &mut self.cmd), "vkAllocateCommandBuffers")?;
        let fci = FenceCreateInfo { s_type: st::FENCE_CREATE_INFO, p_next: null, flags: 0 };
        check((f.vkCreateFence)(dev, &fci, null, &mut self.fence), "vkCreateFence")?;

        // One set layout for everything: ui.frag's five samplers (layer.frag reads the first).
        let bindings: Vec<DescriptorSetLayoutBinding> = (0..5)
            .map(|b| DescriptorSetLayoutBinding { binding: b, ty: DESCRIPTOR_COMBINED_IMAGE_SAMPLER, count: 1, stages: SHADER_FRAGMENT, immutable_samplers: std::ptr::null() })
            .collect();
        let dslci = DescriptorSetLayoutCreateInfo { s_type: st::DESCRIPTOR_SET_LAYOUT_CREATE_INFO, p_next: null, flags: 0, count: 5, bindings: bindings.as_ptr() };
        check((f.vkCreateDescriptorSetLayout)(dev, &dslci, null, &mut self.dsl), "vkCreateDescriptorSetLayout")?;
        let pcr = PushConstantRange { stages: SHADER_VERTEX | SHADER_FRAGMENT, offset: 0, size: PC_SIZE };
        let plci = PipelineLayoutCreateInfo { s_type: st::PIPELINE_LAYOUT_CREATE_INFO, p_next: null, flags: 0, set_count: 1, sets: &self.dsl, pc_count: 1, pcs: &pcr };
        check((f.vkCreatePipelineLayout)(dev, &plci, null, &mut self.layout), "vkCreatePipelineLayout")?;
        let size = DescriptorPoolSize { ty: DESCRIPTOR_COMBINED_IMAGE_SAMPLER, count: 5 * MAX_SETS };
        let dpci = DescriptorPoolCreateInfo { s_type: st::DESCRIPTOR_POOL_CREATE_INFO, p_next: null, flags: POOL_FREE_DESCRIPTOR_SET, max_sets: MAX_SETS, size_count: 1, sizes: &size };
        check((f.vkCreateDescriptorPool)(dev, &dpci, null, &mut self.dpool), "vkCreateDescriptorPool")?;

        let sampler = |filter: u32, max_lod: f32| -> R<H> {
            let sci = SamplerCreateInfo {
                s_type: st::SAMPLER_CREATE_INFO,
                p_next: null,
                flags: 0,
                mag: filter,
                min: filter,
                mipmap_mode: filter,
                address: [ADDRESS_CLAMP_TO_EDGE; 3],
                lod_bias: 0.0,
                anisotropy_enable: 0,
                max_anisotropy: 1.0,
                compare_enable: 0,
                compare_op: 0,
                min_lod: 0.0,
                max_lod,
                border_color: 0,
                unnormalized: 0,
            };
            let mut s = 0;
            check((f.vkCreateSampler)(dev, &sci, null, &mut s), "vkCreateSampler")?;
            Ok(s)
        };
        self.s_near = sampler(FILTER_NEAREST, 0.0)?;
        self.s_mip = sampler(FILTER_LINEAR, 1000.0)?;
        self.s_lin = sampler(FILTER_LINEAR, 0.0)?;

        let module = |spv: &[u8]| -> R<H> {
            let words: Vec<u32> = spv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            let ci = ShaderModuleCreateInfo { s_type: st::SHADER_MODULE_CREATE_INFO, p_next: null, flags: 0, code_size: spv.len(), code: words.as_ptr() };
            let mut m = 0;
            check((f.vkCreateShaderModule)(dev, &ci, null, &mut m), "vkCreateShaderModule")?;
            Ok(m)
        };
        let m_full = module(SPV_FULLSCREEN)?;
        let m_layer = module(SPV_LAYER)?;
        let m_gvert = module(SPV_GLASS_VERT)?;
        let m_shadow = module(SPV_SHADOW)?;
        let m_uivert = module(SPV_UI_VERT)?;
        let m_uifrag = module(SPV_UI_FRAG)?;
        let m_svert = module(SPV_SURF_VERT)?;
        let m_sfrag = module(SPV_SURF_FRAG)?;
        let m_gfrag = module(SPV_GLASS_FRAG)?;
        let m_blur = module(SPV_BLUR)?;
        let m_dvert = module(SPV_DROP_VERT)?;
        let m_dfrag = module(SPV_DROP_FRAG)?;
        self.p_copy = self.pipeline(m_full, m_layer, false, false, TOPOLOGY_TRIANGLE_LIST)?;
        self.p_over = self.pipeline(m_full, m_layer, true, false, TOPOLOGY_TRIANGLE_LIST)?;
        self.p_shadow = self.pipeline(m_gvert, m_shadow, true, false, TOPOLOGY_TRIANGLE_STRIP)?;
        self.p_glass = self.pipeline(m_uivert, m_uifrag, true, true, TOPOLOGY_TRIANGLE_STRIP)?;
        self.p_tex = self.pipeline(m_svert, m_sfrag, true, false, TOPOLOGY_TRIANGLE_STRIP)?;
        self.p_lens = self.pipeline(m_gvert, m_gfrag, true, false, TOPOLOGY_TRIANGLE_STRIP)?;
        self.p_blur = self.pipeline(m_full, m_blur, false, false, TOPOLOGY_TRIANGLE_LIST)?;
        // Instanced like the glass (12 vec4 per drop).
        self.p_drop = self.pipeline(m_dvert, m_dfrag, true, true, TOPOLOGY_TRIANGLE_STRIP)?;
        for m in [m_full, m_layer, m_gvert, m_shadow, m_uivert, m_uifrag, m_svert, m_sfrag, m_gfrag, m_blur, m_dvert, m_dfrag] {
            (self.f.vkDestroyShaderModule)(dev, m, null);
        }
        Ok(())
    }

    /// A pipeline drawing into a B8G8R8A8 frame. `blend`: premultiplied over; `glass`: ui.vert's
    /// instance attributes and ui.frag specialized to kind 2.
    unsafe fn pipeline(&self, vs: H, fs: H, blend: bool, glass: bool, topology: u32) -> R<H> {
        let null = std::ptr::null();
        let kind: i32 = 2;
        let entry = SpecializationMapEntry { id: 0, offset: 0, size: 4 };
        let spec = SpecializationInfo { count: 1, entries: &entry, data_size: 4, data: &kind as *const i32 as P };
        let spec_p = if glass { &spec as *const _ } else { std::ptr::null() };
        let stages = [
            PipelineShaderStageCreateInfo { s_type: st::PIPELINE_SHADER_STAGE_CREATE_INFO, p_next: null, flags: 0, stage: SHADER_VERTEX, module: vs, name: cstr(b"main\0"), spec: spec_p },
            PipelineShaderStageCreateInfo { s_type: st::PIPELINE_SHADER_STAGE_CREATE_INFO, p_next: null, flags: 0, stage: SHADER_FRAGMENT, module: fs, name: cstr(b"main\0"), spec: spec_p },
        ];
        let vb = VertexInputBindingDescription { binding: 0, stride: INST_SIZE as u32, rate: INPUT_RATE_INSTANCE };
        let attrs: Vec<VertexInputAttributeDescription> =
            (0..12).map(|l| VertexInputAttributeDescription { location: l, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: l * 16 }).collect();
        let vi = PipelineVertexInputStateCreateInfo {
            s_type: st::PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            p_next: null,
            flags: 0,
            binding_count: glass as u32,
            bindings: &vb,
            attr_count: if glass { 12 } else { 0 },
            attrs: attrs.as_ptr(),
        };
        let ia = PipelineInputAssemblyStateCreateInfo { s_type: st::PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO, p_next: null, flags: 0, topology, restart: 0 };
        let vp = PipelineViewportStateCreateInfo { s_type: st::PIPELINE_VIEWPORT_STATE_CREATE_INFO, p_next: null, flags: 0, viewport_count: 1, viewports: null, scissor_count: 1, scissors: null };
        let rs = PipelineRasterizationStateCreateInfo {
            s_type: st::PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            p_next: null,
            flags: 0,
            depth_clamp: 0,
            discard: 0,
            polygon_mode: 0,
            cull_mode: 0,
            front_face: 0,
            depth_bias: 0,
            bias_constant: 0.0,
            bias_clamp: 0.0,
            bias_slope: 0.0,
            line_width: 1.0,
        };
        let ms = PipelineMultisampleStateCreateInfo {
            s_type: st::PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            p_next: null,
            flags: 0,
            samples: 1,
            sample_shading: 0,
            min_sample_shading: 0.0,
            sample_mask: std::ptr::null(),
            alpha_to_coverage: 0,
            alpha_to_one: 0,
        };
        let att = PipelineColorBlendAttachmentState {
            enable: blend as u32,
            src_color: BLEND_ONE,
            dst_color: BLEND_ONE_MINUS_SRC_ALPHA,
            color_op: 0,
            src_alpha: BLEND_ONE,
            dst_alpha: BLEND_ONE_MINUS_SRC_ALPHA,
            alpha_op: 0,
            write_mask: 0xf,
        };
        let cb = PipelineColorBlendStateCreateInfo { s_type: st::PIPELINE_COLOR_BLEND_STATE_CREATE_INFO, p_next: null, flags: 0, logic_op_enable: 0, logic_op: 0, count: 1, attachments: &att, constants: [0.0; 4] };
        let dyns = [DYNAMIC_VIEWPORT, DYNAMIC_SCISSOR];
        let ds = PipelineDynamicStateCreateInfo { s_type: st::PIPELINE_DYNAMIC_STATE_CREATE_INFO, p_next: null, flags: 0, count: 2, states: dyns.as_ptr() };
        let fmt = FMT;
        let rci = PipelineRenderingCreateInfo { s_type: st::PIPELINE_RENDERING_CREATE_INFO, p_next: null, view_mask: 0, color_count: 1, color_formats: &fmt, depth_format: 0, stencil_format: 0 };
        let ci = GraphicsPipelineCreateInfo {
            s_type: st::GRAPHICS_PIPELINE_CREATE_INFO,
            p_next: &rci as *const _ as P,
            flags: 0,
            stage_count: 2,
            stages: stages.as_ptr(),
            vertex_input: &vi,
            input_assembly: &ia,
            tessellation: null,
            viewport: &vp,
            rasterization: &rs,
            multisample: &ms,
            depth_stencil: null,
            color_blend: &cb,
            dynamic: &ds,
            layout: self.layout,
            render_pass: 0,
            subpass: 0,
            base: 0,
            base_index: -1,
        };
        let mut p = 0;
        check((self.f.vkCreateGraphicsPipelines)(self.dev, 0, 1, &ci, null, &mut p), "vkCreateGraphicsPipelines")?;
        Ok(p)
    }

    fn mem_type(&self, bits: u32, want: &[u32]) -> R<u32> {
        for flags in want {
            for t in 0..self.mem.type_count {
                if bits & (1 << t) != 0 && self.mem.types[t as usize].flags & flags == *flags {
                    return Ok(t);
                }
            }
        }
        Err("no suitable memory type".into())
    }

    unsafe fn alloc(&self, req: &MemoryRequirements, want: &[u32]) -> R<H> {
        let ai = MemoryAllocateInfo { s_type: st::MEMORY_ALLOCATE_INFO, p_next: std::ptr::null(), size: req.size, type_index: self.mem_type(req.type_bits, want)? };
        let mut m = 0;
        check((self.f.vkAllocateMemory)(self.dev, &ai, std::ptr::null(), &mut m), "vkAllocateMemory")?;
        Ok(m)
    }

    unsafe fn image(&self, w: i32, h: i32, levels: u32, usage: u32) -> R<Image> {
        self.image_sw(w, h, levels, usage, false)
    }

    /// `opaque`: the view reads alpha as 1 (XRGB buffers).
    unsafe fn image_sw(&self, w: i32, h: i32, levels: u32, usage: u32, opaque: bool) -> R<Image> {
        let null = std::ptr::null();
        let ci = ImageCreateInfo {
            s_type: st::IMAGE_CREATE_INFO,
            p_next: null,
            flags: 0,
            image_type: IMAGE_TYPE_2D,
            format: FMT,
            extent: [w as u32, h as u32, 1],
            mip_levels: levels,
            array_layers: 1,
            samples: 1,
            tiling: 0,
            usage,
            sharing: 0,
            qf_count: 0,
            qfs: std::ptr::null(),
            initial_layout: layout::UNDEFINED,
        };
        let mut img = 0;
        check((self.f.vkCreateImage)(self.dev, &ci, null, &mut img), "vkCreateImage")?;
        let mut req = MemoryRequirements::default();
        (self.f.vkGetImageMemoryRequirements)(self.dev, img, &mut req);
        let mem = self.alloc(&req, &[mem::DEVICE_LOCAL, 0])?;
        check((self.f.vkBindImageMemory)(self.dev, img, mem, 0), "vkBindImageMemory")?;
        let vci = ImageViewCreateInfo {
            s_type: st::IMAGE_VIEW_CREATE_INFO,
            p_next: null,
            flags: 0,
            image: img,
            view_type: VIEW_TYPE_2D,
            format: FMT,
            // IDENTITY ×3, then ONE for alpha when opaque.
            components: [0, 0, 0, if opaque { 2 } else { 0 }],
            range: SubresourceRange::color(0, levels),
        };
        let mut view = 0;
        check((self.f.vkCreateImageView)(self.dev, &vci, null, &mut view), "vkCreateImageView")?;
        Ok(Image { img, mem, view })
    }

    /// Host-visible buffer, mapped for good. Cached memory first: the CPU reads it back
    /// (blending, readback), and reads from write-combined memory crawl.
    unsafe fn buffer(&self, size: u64, usage: u32) -> R<Buffer> {
        let null = std::ptr::null();
        let ci = BufferCreateInfo { s_type: st::BUFFER_CREATE_INFO, p_next: null, flags: 0, size, usage, sharing: 0, qf_count: 0, qfs: std::ptr::null() };
        let mut buf = 0;
        check((self.f.vkCreateBuffer)(self.dev, &ci, null, &mut buf), "vkCreateBuffer")?;
        let mut req = MemoryRequirements::default();
        (self.f.vkGetBufferMemoryRequirements)(self.dev, buf, &mut req);
        let hv = mem::HOST_VISIBLE | mem::HOST_COHERENT;
        let mem = self.alloc(&req, &[hv | mem::HOST_CACHED, hv])?;
        check((self.f.vkBindBufferMemory)(self.dev, buf, mem, 0), "vkBindBufferMemory")?;
        let mut ptr = std::ptr::null_mut();
        check((self.f.vkMapMemory)(self.dev, mem, 0, !0, 0, &mut ptr), "vkMapMemory")?;        Ok(Buffer { buf, mem, ptr: ptr as *mut u8, size })
    }

    /// A view of `count` levels of `img` from `base`.
    unsafe fn view(&self, img: H, base: u32, count: u32) -> R<H> {
        let vci = ImageViewCreateInfo {
            s_type: st::IMAGE_VIEW_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            image: img,
            view_type: VIEW_TYPE_2D,
            format: FMT,
            components: [0; 4],
            range: SubresourceRange::color(base, count),
        };
        let mut view = 0;
        check((self.f.vkCreateImageView)(self.dev, &vci, std::ptr::null(), &mut view), "vkCreateImageView")?;
        Ok(view)
    }

    unsafe fn free_image(&self, i: &Image) {
        let null = std::ptr::null();
        (self.f.vkDestroyImageView)(self.dev, i.view, null);
        (self.f.vkDestroyImage)(self.dev, i.img, null);
        (self.f.vkFreeMemory)(self.dev, i.mem, null);
    }

    unsafe fn free_buffer(&self, b: &Buffer) {
        let null = std::ptr::null();
        (self.f.vkDestroyBuffer)(self.dev, b.buf, null);
        (self.f.vkFreeMemory)(self.dev, b.mem, null);
    }

    unsafe fn free_out(&self, o: &Out) {
        (self.f.vkDeviceWaitIdle)(self.dev);
        let mut sets = vec![o.bd_set];
        for b in o.blurs.iter().flatten() {
            sets.extend(b.sets);
            for i in &b.imgs {
                self.free_image(i);
            }
            (self.f.vkDestroyImageView)(self.dev, b.src_view, std::ptr::null());
        }
        for (img, buf, set) in &o.layers {
            self.free_image(img);
            self.free_buffer(buf);
            sets.push(*set);
        }
        (self.f.vkFreeDescriptorSets)(self.dev, self.dpool, sets.len() as u32, sets.as_ptr());
        self.free_tex(&o.wall);
        for ft in o.frames.values() {
            self.free_tex(&ft.tex);
        }
        for c in o.gcache.values() {
            (self.f.vkFreeDescriptorSets)(self.dev, self.dpool, 1, &c.set);
            self.free_image(&c.img);
        }
        self.free_image(&o.target);
        self.free_image(&o.backdrop);
        self.free_buffer(&o.readback);
        self.free_buffer(&o.inst);
    }

    /// A texture to upload into and sample: linear filter, or nearest for `exact`
    /// (sampled 1:1 by layer.frag's texelFetch).
    unsafe fn tex(&self, w: i32, h: i32, opaque: bool, exact: bool) -> R<Tex> {
        let img = self.image_sw(w, h, 1, usage::TRANSFER_DST | usage::SAMPLED, opaque)?;
        let set = match self.set(img.view, if exact { self.s_near } else { self.s_lin }, false) {
            Ok(s) => s,
            Err(e) => {
                self.free_image(&img);
                return Err(e);
            }
        };
        Ok(Tex { img, set, w, h, opaque, fresh: true })
    }

    unsafe fn free_tex(&self, t: &Tex) {
        self.free_image(&t.img);
        (self.f.vkFreeDescriptorSets)(self.dev, self.dpool, 1, &t.set);
    }

    /// `bytes` of upload space for this frame: (arena buffer, offset). The previous frame is
    /// done with all of it (the frame is waited for).
    unsafe fn stage(&mut self, bytes: u64) -> R<(usize, u64)> {
        let bytes = (bytes + 15) & !15;
        if let Some(i) = self.arena.iter().position(|(b, used)| b.size - used >= bytes) {
            let off = self.arena[i].1;
            self.arena[i].1 += bytes;
            return Ok((i, off));
        }
        let size = bytes.max(8 << 20);
        let b = self.buffer(size, usage::TRANSFER_SRC)?;
        self.arena.push((b, bytes));
        Ok((self.arena.len() - 1, 0))
    }

    /// A `w`×`h` canvas in upload space, cleared: the upload copies it whole into an image.
    unsafe fn stage_canvas(&mut self, img: H, fresh: bool, w: i32, h: i32) -> R<(Upload, Canvas<'static>)> {
        let n = (w as usize) * (h as usize);
        let (ai, off) = self.stage(n as u64 * 4)?;
        let b = &self.arena[ai].0;
        let px = std::slice::from_raw_parts_mut(b.ptr.add(off as usize) as *mut u32, n);
        px.fill(0);
        let copy = BufferImageCopy {
            buffer_offset: off,
            row_length: 0,
            image_height: 0,
            sub: SubresourceLayers::color(0),
            offset: [0, 0, 0],
            extent: [w as u32, h as u32, 1],
        };
        Ok((Upload { img, fresh, buf: b.buf, copies: vec![copy] }, Canvas { px, w, h, stride: w as usize }))
    }

    /// Drops textures of surfaces that are gone or have no buffer any more.
    unsafe fn gc(&mut self, d: &Desk) {
        let gone: Vec<SKey> = self
            .surfs
            .keys()
            .copied()
            .filter(|k| d.surfaces.get(*k).map_or(true, |s| s.destroyed || s.buffer.as_ref().map_or(true, |(b, _)| b.solid.is_some() || (b.dmabuf.is_some() && self.dma.is_some()))))
            .collect();
        for k in gone {
            if let Some(t) = self.surfs.remove(&k) {
                if crate::life::holds(d, k) {
                    self.ghost_tex.insert(k, t);
                } else {
                    self.free_tex(&t);
                }
            }
        }
        let let_go: Vec<SKey> = self.ghost_tex.keys().copied().filter(|k| !crate::life::holds(d, *k)).collect();
        for k in let_go {
            if let Some(t) = self.ghost_tex.remove(&k) {
                self.free_tex(&t);
            }
        }
        let dead: Vec<usize> = self.imports.iter().filter(|(_, i)| i.buf.strong_count() == 0).map(|(k, _)| *k).collect();
        for k in dead {
            if let Some(i) = self.imports.remove(&k) {
                self.free_import(&i);
            }
        }
    }

    unsafe fn free_import(&self, i: &Import) {
        self.free_tex(&i.tex);
        if i.sem != 0 {
            (self.f.vkDestroySemaphore)(self.dev, i.sem, std::ptr::null());
        }
    }

    /// The dma-buf formats and modifiers this GPU samples and imports.
    unsafe fn dma_formats(i: &Instance, pd: H) -> Vec<(u32, u64)> {
        let mut out = Vec::new();
        for drm in crate::dmabuf::FORMATS {
            let vf = vk_format(drm);
            let mut list = DrmModifierList { s_type: sx::DRM_FORMAT_MODIFIER_PROPERTIES_LIST, p_next: std::ptr::null_mut(), count: 0, props: std::ptr::null_mut() };
            let mut fp = FormatProperties2 { s_type: sx::FORMAT_PROPERTIES_2, p_next: &mut list as *mut _ as *mut c_void, linear: 0, optimal: 0, buffer: 0 };
            (i.vkGetPhysicalDeviceFormatProperties2)(pd, vf, &mut fp);
            let mut mods = vec![DrmModifierProps::default(); list.count as usize];
            list.props = mods.as_mut_ptr();
            (i.vkGetPhysicalDeviceFormatProperties2)(pd, vf, &mut fp);
            mods.truncate(list.count as usize);
            for m in mods {
                if m.plane_count != 1 || m.features & FORMAT_FEATURE_SAMPLED_IMAGE == 0 {
                    continue;
                }
                // Importable from a dma-buf with this modifier, as a sampled image?
                let mi = ImageDrmModifierInfo { s_type: sx::PHYSICAL_DEVICE_IMAGE_DRM_FORMAT_MODIFIER_INFO, p_next: std::ptr::null(), modifier: m.modifier, sharing: 0, qf_count: 0, qfs: std::ptr::null() };
                let ei = ExternalImageFormatInfo { s_type: sx::PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO, p_next: &mi as *const _ as P, handle_type: HANDLE_DMA_BUF };
                let info = ImageFormatInfo2 { s_type: sx::PHYSICAL_DEVICE_IMAGE_FORMAT_INFO_2, p_next: &ei as *const _ as P, format: vf, image_type: IMAGE_TYPE_2D, tiling: TILING_DRM_FORMAT_MODIFIER, usage: usage::SAMPLED, flags: 0 };
                let mut ep = ExternalImageFormatProperties { s_type: sx::EXTERNAL_IMAGE_FORMAT_PROPERTIES, p_next: std::ptr::null_mut(), features: 0, export_from: 0, compatible: 0 };
                let mut ip = ImageFormatProperties2 { s_type: sx::IMAGE_FORMAT_PROPERTIES_2, p_next: &mut ep as *mut _ as *mut c_void, props: [0; 4] };
                if (i.vkGetPhysicalDeviceImageFormatProperties2)(pd, &info, &mut ip) == SUCCESS && ep.features & EXTERNAL_MEMORY_IMPORTABLE != 0 {
                    out.push((drm, m.modifier));
                }
            }
        }
        out
    }

    /// For dmabuf.rs: the GPU's dev_t and what it imports (None: no dma-buf import here).
    pub fn dmabuf_support(&self) -> Option<(u64, Vec<(u32, u64)>)> {
        if self.dma.is_none() || self.dma_formats.is_empty() {
            return None;
        }
        Some((self.drm_dev?, self.dma_formats.clone()))
    }

    /// A client's dma-buf as a sampled image over its memory.
    unsafe fn import(&self, dm: &crate::dmabuf::Dmabuf) -> R<Tex> {
        let dma = self.dma.as_ref().ok_or("no dma-buf import")?;
        let null = std::ptr::null();
        let vf = vk_format(dm.format);
        let opaque = !crate::dmabuf::has_alpha(dm.format);
        let layouts: Vec<SubresourceLayout> = dm
            .planes
            .iter()
            .map(|p| SubresourceLayout { offset: p.offset as u64, size: 0, row_pitch: p.stride as u64, array_pitch: 0, depth_pitch: 0 })
            .collect();
        let explicit = DrmModifierExplicitCreateInfo { s_type: sx::IMAGE_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO, p_next: null, modifier: dm.modifier, plane_count: layouts.len() as u32, layouts: layouts.as_ptr() };
        let ext = ExternalMemoryImageCreateInfo { s_type: sx::EXTERNAL_MEMORY_IMAGE_CREATE_INFO, p_next: &explicit as *const _ as P, handle_types: HANDLE_DMA_BUF };
        let ci = ImageCreateInfo {
            s_type: st::IMAGE_CREATE_INFO,
            p_next: &ext as *const _ as P,
            flags: 0,
            image_type: IMAGE_TYPE_2D,
            format: vf,
            extent: [dm.w as u32, dm.h as u32, 1],
            mip_levels: 1,
            array_layers: 1,
            samples: 1,
            tiling: TILING_DRM_FORMAT_MODIFIER,
            usage: usage::SAMPLED,
            sharing: 0,
            qf_count: 0,
            qfs: null as *const u32,
            initial_layout: layout::UNDEFINED,
        };
        let mut img = 0;
        check((self.f.vkCreateImage)(self.dev, &ci, null, &mut img), "vkCreateImage (dma-buf)")?;
        let fail = |e: String| {
            (self.f.vkDestroyImage)(self.dev, img, null);
            Err(e)
        };
        let fd = match dm.planes[0].fd.try_clone() {
            Ok(f) => f.into_raw_fd(),
            Err(e) => return fail(format!("dup: {e}")),
        };
        let mut fp = MemoryFdProperties { s_type: sx::MEMORY_FD_PROPERTIES, p_next: std::ptr::null_mut(), type_bits: 0 };
        (dma.vkGetMemoryFdPropertiesKHR)(self.dev, HANDLE_DMA_BUF, fd, &mut fp);
        let mut req = MemoryRequirements::default();
        (self.f.vkGetImageMemoryRequirements)(self.dev, img, &mut req);
        let ty = match self.mem_type(req.type_bits & fp.type_bits, &[0]) {
            Ok(t) => t,
            Err(e) => {
                drop(OwnedFd::from_raw_fd(fd));
                return fail(e);
            }
        };
        let ded = MemoryDedicatedAllocateInfo { s_type: sx::MEMORY_DEDICATED_ALLOCATE_INFO, p_next: null, image: img, buffer: 0 };
        let imp = ImportMemoryFdInfo { s_type: sx::IMPORT_MEMORY_FD_INFO, p_next: &ded as *const _ as P, handle_type: HANDLE_DMA_BUF, fd };
        let ai = MemoryAllocateInfo { s_type: st::MEMORY_ALLOCATE_INFO, p_next: &imp as *const _ as P, size: req.size, type_index: ty };
        let mut mem = 0;
        if (self.f.vkAllocateMemory)(self.dev, &ai, null, &mut mem) != SUCCESS {
            // Not taken: the fd is still ours.
            drop(OwnedFd::from_raw_fd(fd));
            return fail("vkAllocateMemory (dma-buf import)".into());
        }
        if (self.f.vkBindImageMemory)(self.dev, img, mem, 0) != SUCCESS {
            (self.f.vkFreeMemory)(self.dev, mem, null);
            return fail("vkBindImageMemory (dma-buf)".into());
        }
        let vci = ImageViewCreateInfo {
            s_type: st::IMAGE_VIEW_CREATE_INFO,
            p_next: null,
            flags: 0,
            image: img,
            view_type: VIEW_TYPE_2D,
            format: vf,
            components: [0, 0, 0, if opaque { 2 } else { 0 }],
            range: SubresourceRange::color(0, 1),
        };
        let mut view = 0;
        if (self.f.vkCreateImageView)(self.dev, &vci, null, &mut view) != SUCCESS {
            (self.f.vkFreeMemory)(self.dev, mem, null);
            return fail("vkCreateImageView (dma-buf)".into());
        }
        let image = Image { img, mem, view };
        let set = match self.set(view, self.s_lin, false) {
            Ok(s) => s,
            Err(e) => {
                self.free_image(&image);
                return Err(e);
            }
        };
        Ok(Tex { img: image, set, w: dm.w, h: dm.h, opaque, fresh: false })
    }

    /// The dma-buf behind `b` ready to draw: (set, size, image, first use, semaphore to wait
    /// on for the client's rendering, if any).
    unsafe fn sync_dmabuf(&mut self, b: &Rc<crate::shm::Buffer>) -> R<Option<(H, i32, i32, H, bool, Option<H>)>> {
        let Some(dm) = &b.dmabuf else { return Ok(None) };
        let key = Rc::as_ptr(b) as usize;
        if !self.imports.get(&key).is_some_and(|i| i.buf.strong_count() > 0 && std::ptr::eq(i.buf.as_ptr(), Rc::as_ptr(b))) {
            if let Some(old) = self.imports.remove(&key) {
                self.free_import(&old);
            }
            let tex = self.import(dm)?;
            self.imports.insert(key, Import { buf: Rc::downgrade(b), tex, sem: 0, used: false });
        }
        // The client's GPU work on it, waited for on our queue.
        let mut wait = None;
        if let (Some(sf), Some(fence)) = (self.sem_fns.as_ref(), dm.read_fence()) {
            let imp = self.imports.get_mut(&key).unwrap();
            if imp.sem == 0 {
                let ci = SemaphoreCreateInfo { s_type: sx::SEMAPHORE_CREATE_INFO, p_next: std::ptr::null(), flags: 0 };
                (self.f.vkCreateSemaphore)(self.dev, &ci, std::ptr::null(), &mut imp.sem);
            }
            if imp.sem != 0 {
                let fd = fence.into_raw_fd();
                let info = ImportSemaphoreFdInfo { s_type: sx::IMPORT_SEMAPHORE_FD_INFO, p_next: std::ptr::null(), semaphore: imp.sem, flags: SEMAPHORE_IMPORT_TEMPORARY, handle_type: SEMAPHORE_HANDLE_SYNC_FD, fd };
                if (sf.vkImportSemaphoreFdKHR)(self.dev, &info) == SUCCESS {
                    wait = Some(imp.sem);
                } else {
                    drop(OwnedFd::from_raw_fd(fd));
                }
            }
        }
        let imp = self.imports.get_mut(&key).unwrap();
        let first = !imp.used;
        imp.used = true;
        Ok(Some((imp.tex.set, imp.tex.w, imp.tex.h, imp.tex.img.img, first, wait)))
    }

    /// Surface `sk`'s texture brought up to date (only what the client damaged goes up).
    /// None: nothing to draw yet.
    unsafe fn sync_surface(&mut self, d: &mut Desk, sk: SKey, ups: &mut Vec<Upload>) -> R<Option<(H, i32, i32)>> {
        let Some(s) = d.surfaces.get_mut(sk) else { return Ok(None) };
        let Some((b, _)) = &s.buffer else { return Ok(None) };
        let b = b.clone();
        let mut full = std::mem::take(&mut s.tex_full);
        let dmg = std::mem::take(&mut s.tex_dmg);
        let opaque = b.opaque_format();
        let ok = self.surfs.get(&sk).map_or(false, |t| t.w == b.w && t.h == b.h && t.opaque == opaque);
        if !ok {
            if let Some(t) = self.surfs.remove(&sk) {
                self.free_tex(&t);
            }
            if b.w <= 0 || b.h <= 0 {
                return Ok(None);
            }
            let t = self.tex(b.w, b.h, opaque, false)?;
            self.surfs.insert(sk, t);
        }
        let t = &self.surfs[&sk];
        let (img, set, fresh) = (t.img.img, t.set, t.fresh);
        full |= fresh;
        let bounds = Rect::new(0, 0, b.w, b.h);
        let rects: Vec<Rect> = if full { vec![bounds] } else { dmg.iter().map(|r| r.intersect(&bounds)).filter(|r| !r.is_empty()).collect() };
        if !rects.is_empty() {
            let total: u64 = rects.iter().map(|r| r.w as u64 * r.h as u64 * 4).sum();
            let (ai, off) = self.stage(total)?;
            let ab = &self.arena[ai].0;
            let (base, abuf) = (ab.ptr.add(off as usize), ab.buf);
            let copies = b.with_pixels(|px, stride| {
                let mut at = 0usize;
                let mut copies = Vec::with_capacity(rects.len());
                for r in &rects {
                    let dst = base.add(at) as *mut u32;
                    for y in 0..r.h as usize {
                        let s0 = (r.y as usize + y) * stride + r.x as usize;
                        std::ptr::copy_nonoverlapping(px[s0..s0 + r.w as usize].as_ptr(), dst.add(y * r.w as usize), r.w as usize);
                    }
                    copies.push(BufferImageCopy {
                        buffer_offset: off + at as u64,
                        row_length: 0,
                        image_height: 0,
                        sub: SubresourceLayers::color(0),
                        offset: [r.x, r.y, 0],
                        extent: [r.w as u32, r.h as u32, 1],
                    });
                    at += r.w as usize * r.h as usize * 4;
                }
                copies
            });
            match copies {
                Some(copies) => {
                    ups.push(Upload { img, fresh, buf: abuf, copies });
                    self.surfs.get_mut(&sk).unwrap().fresh = false;
                }
                // A broken pool: whatever was there stays.
                None if fresh => return Ok(None),
                None => {}
            }
        }
        Ok(Some((set, b.w, b.h)))
    }

    /// Window `wk`'s frame texture on output `k`, redrawn only when what it shows changed.
    unsafe fn sync_frame(&mut self, d: &Desk, k: usize, wk: WKey, xf: Xf, ups: &mut Vec<Upload>) -> R<Option<(H, Rect)>> {
        let Some(ext) = crate::render::frame_extent(d, wk) else { return Ok(None) };
        let at = xf.r(ext);
        if at.is_empty() {
            return Ok(None);
        }
        let key = frame_key(d, wk, xf, ext, at);
        let ok = self.outs[k].frames.get(&wk).map_or(false, |f| f.tex.w == at.w && f.tex.h == at.h);
        if !ok {
            if let Some(f) = self.outs[k].frames.remove(&wk) {
                self.free_tex(&f.tex);
            }
            let tex = self.tex(at.w, at.h, false, false)?;
            self.outs[k].frames.insert(wk, FrameTex { tex, at, key: !key });
        }
        let f = &self.outs[k].frames[&wk];
        let (img, set, fresh, old) = (f.tex.img.img, f.tex.set, f.tex.fresh, f.key);
        if fresh || old != key {
            let (up, mut cv) = self.stage_canvas(img, fresh, at.w, at.h)?;
            crate::render::draw_frame(d, wk, xf.shifted(at.x, at.y), &mut cv);
            ups.push(up);
        }
        let f = self.outs[k].frames.get_mut(&wk).unwrap();
        f.tex.fresh = false;
        f.key = key;
        f.at = at;
        Ok(Some((set, at)))
    }

    unsafe fn set(&self, view: H, sampler: H, all: bool) -> R<H> {
        self.set_with(view, sampler, if all { 5 } else { 1 }, if all { layout::GENERAL } else { layout::SHADER_READ })
    }

    /// `view` in the first `n` bindings, read in layout `lay`.
    unsafe fn set_with(&self, view: H, sampler: H, n: u32, lay: u32) -> R<H> {
        let ai = DescriptorSetAllocateInfo { s_type: st::DESCRIPTOR_SET_ALLOCATE_INFO, p_next: std::ptr::null(), pool: self.dpool, count: 1, layouts: &self.dsl };
        let mut set = 0;
        check((self.f.vkAllocateDescriptorSets)(self.dev, &ai, &mut set), "vkAllocateDescriptorSets")?;
        let info = DescriptorImageInfo { sampler, view, layout: lay };
        let writes: Vec<WriteDescriptorSet> = (0..n)
            .map(|b| WriteDescriptorSet {
                s_type: st::WRITE_DESCRIPTOR_SET,
                p_next: std::ptr::null(),
                set,
                binding: b,
                array_element: 0,
                count: 1,
                ty: DESCRIPTOR_COMBINED_IMAGE_SAMPLER,
                image_info: &info,
                buffer_info: std::ptr::null(),
                texel_views: std::ptr::null(),
            })
            .collect();
        (self.f.vkUpdateDescriptorSets)(self.dev, writes.len() as u32, writes.as_ptr(), 0, std::ptr::null());
        Ok(set)
    }

    /// Mip levels of the backdrop: down to the one the frost blurs (blur_plan).
    fn levels(w: i32, h: i32, scale: f32) -> u32 {
        blur_plan(crate::glass::max_frost() * scale, w, h).map_or(1, |(l, _, _)| l + 1)
    }

    /// The blur for a sigma of `sigma` physical px on `out`, with its sigma and radius in
    /// that level's texels (None: no frost).
    fn blur_for(out: &Out, sigma: f32) -> Option<(&Blur, f32, f32)> {
        let (l, k, radius) = blur_plan(sigma, out.w, out.h)?;
        out.blurs.get(l as usize).and_then(|b| b.as_ref()).map(|b| (b, k, radius))
    }

    unsafe fn out(&mut self, id: u32, w: i32, h: i32, scale: f32) -> R<usize> {
        let levels = Self::levels(w, h, scale);
        if let Some(i) = self.outs.iter().position(|o| o.id == id) {
            let o = &self.outs[i];
            if o.w == w && o.h == h && o.levels == levels {
                return Ok(i);
            }
            let o = self.outs.remove(i);
            self.free_out(&o);
        }
        let px = (w as u64) * (h as u64) * 4;
        let mut layers = Vec::new();
        for _ in 0..2 {
            let img = self.image(w, h, 1, usage::TRANSFER_DST | usage::SAMPLED)?;
            let buf = self.buffer(px, usage::TRANSFER_SRC)?;
            std::ptr::write_bytes(buf.ptr, 0, px as usize);
            let set = self.set(img.view, self.s_near, false)?;
            layers.push((img, buf, set));
        }
        let target = self.image(w, h, 1, usage::COLOR_ATTACHMENT | usage::TRANSFER_SRC)?;
        let backdrop = self.image(w, h, levels, usage::TRANSFER_SRC | usage::TRANSFER_DST | usage::SAMPLED)?;
        let bd_set = self.set(backdrop.view, self.s_mip, true)?;
        let mut blurs = vec![None];
        for l in 1..levels {
            let (bw, bh) = ((w >> l).max(1), (h >> l).max(1));
            let a = self.image(bw, bh, 1, usage::COLOR_ATTACHMENT | usage::SAMPLED)?;
            let b = self.image(bw, bh, 1, usage::COLOR_ATTACHMENT | usage::SAMPLED)?;
            let src_view = self.view(backdrop.img, l, 1)?;
            let sets = [
                self.set(src_view, self.s_lin, true)?,
                self.set_with(a.view, self.s_lin, 5, layout::SHADER_READ)?,
                self.set_with(b.view, self.s_lin, 5, layout::SHADER_READ)?,
            ];
            blurs.push(Some(Blur { level: l, w: bw, h: bh, imgs: [a, b], src_view, sets }));
        }
        let readback = self.buffer(px, usage::TRANSFER_DST)?;
        let inst = self.buffer(INST_SIZE * MAX_GLASS, usage::VERTEX_BUFFER)?;
        let wall = self.tex(w, h, true, true)?;
        let mut it = layers.into_iter();
        self.outs.push(Out {
            id,
            w,
            h,
            layers: [it.next().unwrap(), it.next().unwrap()],
            ext: [Vec::new(), Vec::new()],
            wall,
            wall_key: None,
            frames: HashMap::new(),
            target,
            backdrop,
            levels,
            bd_set,
            blurs,
            readback,
            inst,
            fresh: true,
            gcache: HashMap::new(),
        });
        Ok(self.outs.len() - 1)
    }

    /// Draws output `oi` into `cv` (its whole buffer): textures and CPU layers brought up to
    /// date, then the frame on the GPU and back where the damage grew to. Returns the rects
    /// that changed.
    pub fn render(&mut self, d: &mut Desk, oi: usize, cv: &mut Canvas, damage: &Damage) -> R<Vec<Rect>> {
        let o = &d.outputs[oi];
        let xf = Xf::new(o.x, o.y, o.scale);
        let (w, h) = (cv.w, cv.h);
        let bounds = Rect::new(0, 0, w, h);
        let k = unsafe { self.out(o.id, w, h, o.scale as f32)? };
        let fresh = self.outs[k].fresh;
        let mut damage = damage.clone();
        if fresh {
            damage.add(bounds);
        }
        let raw: Vec<Rect> = damage.rects.iter().map(|r| r.intersect(&bounds)).filter(|r| !r.is_empty()).collect();
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        for a in &mut self.arena {
            a.1 = 0;
        }
        let mut ups = Vec::new();
        unsafe {
            self.gc(d);
            // The wallpaper: drawn once per output size and wallpaper.
            let wkey = (d.wall.gen.get(), w, h);
            if self.outs[k].wall_key != Some(wkey) {
                let (img, wfresh) = (self.outs[k].wall.img.img, self.outs[k].wall.fresh);
                let (up, mut wcv) = self.stage_canvas(img, wfresh, w, h)?;
                crate::shell::draw_background(d, oi, xf, &mut wcv, bounds);
                ups.push(up);
                self.outs[k].wall.fresh = false;
                self.outs[k].wall_key = Some(wkey);
            }
        }

        // The windows: frames and surfaces, bottom to top. A window growing out of its taskbar
        // button, shrinking into it or spreading out of the preview card is a drop
        // (winanim.rs): its glass (drop.frag) in place of its frame and frame glass, its
        // surfaces stretched into it; its head is on the overlays' layer.
        let items = crate::render::gpu_items(d, oi);
        let poses: Vec<(WKey, crate::winanim::Pose)> =
            d.windows.iter().filter(|(_, w)| w.morph.is_some()).filter_map(|(k, _)| crate::winanim::pose(d, k).map(|p| (k, p))).collect();
        let pose_of = |wk: WKey| poses.iter().find(|(k, _)| *k == wk).map(|(_, p)| *p);
        // The taskbar's hover drop: part of a drop that bridges to it, then not drawn alone.
        let lens = crate::shell::lens_rect(d, oi);
        // Every drop a moving one may bridge to: the hover one, the buttons' own, the ones
        // under clicked icons (life.rs).
        let mut lenses: Vec<Rect> = lens.into_iter().collect();
        lenses.extend(crate::life::button_drops(d, oi));
        lenses.extend(crate::life::origin_drops(d, oi));
        let mut taken: Vec<Rect> = Vec::new();
        let mut dropped: Vec<WKey> = Vec::new();
        let mut draws = Vec::with_capacity(items.len());
        // Clients' dma-bufs drawn this frame: (image, first use), and their fences.
        let mut dmas: Vec<(H, bool)> = Vec::new();
        let mut waits: Vec<H> = Vec::new();
        let mut frames_seen = Vec::new();
        let mut win_ops: Vec<GlassOp> = Vec::new();
        // The pictures' generations of `draws` (a surface's commits, a frame's key).
        let mut gens: Vec<u64> = Vec::new();
        let wall_key = self.outs[k].wall_key.unwrap_or_default();
        let out_fresh = self.outs[k].fresh;
        let m_px = crate::glass::margin(xf.scale as f32);
        // Glass the damage widens to (and what of it a window's opaque content covers).
        let mut zones: Vec<(Rect, [Rect; 2])> = Vec::new();
        for it in items {
            let win = match it {
                GpuItem::Frame(wk) => Some(wk),
                GpuItem::Surface(_, _, _, w) => w,
            };
            let pose = win.and_then(pose_of);
            if let (Some(wk), Some(p)) = (win, pose.as_ref()) {
                if !dropped.contains(&wk) {
                    dropped.push(wk);
                    let hover = bridge(&lenses, p, &mut taken);
                    let op = drop_op(xf, p, d.windows[wk].activated, hover, 0, draws.len());
                    zones.push((op.rect, op.cover));
                    win_ops.push(op);
                }
            }
            match it {
                GpuItem::Frame(wk) => {
                    if pose.is_some() {
                        // Its frame is the drop now: kept for when it lands.
                        frames_seen.push(wk);
                        continue;
                    }
                    if let Some(mut op) = window_glass(d, wk, xf, draws.len()) {
                        // Its frost kept: still good if nothing under it changed.
                        let zone = op.rect.inflate(m_px).intersect(&bounds);
                        op.id = {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            ("win", wk).hash(&mut h);
                            h.finish() | 1
                        };
                        op.under = under_key(zone, op.sigma, wall_key, &draws, &gens, &win_ops);
                        let level = blur_plan(op.sigma, w, h).map(|p| p.0);
                        op.hit = !out_fresh
                            && level.is_some()
                            && self.outs[k].gcache.get(&op.id).map_or(false, |c| !c.fresh && c.under == op.under && Some(c.level) == level);
                        if !op.hit {
                            zones.push((op.rect, op.cover));
                        }
                        win_ops.push(op);
                    }
                    if let Some((set, at)) = unsafe { self.sync_frame(d, k, wk, xf, &mut ups)? } {
                        draws.push(Draw::Tex { set, dst: at, uv: [0.0, 0.0, 1.0, 1.0], round: [0.0; 6], alpha: 1.0 });
                        gens.push(self.outs[k].frames.get(&wk).map_or(0, |f| f.key));
                        frames_seen.push(wk);
                    }
                }
                GpuItem::Surface(sk, r, round, _) => {
                    // A notification (no frame of ours): its rim of glass under its first
                    // surface, its corners rounded inside the rim.
                    let toast = win.filter(|k| d.windows.get(*k).is_some_and(|w| w.toast));
                    let mut round = round;
                    if let Some(wk) = toast {
                        if !frames_seen.contains(&wk) {
                            frames_seen.push(wk);
                            if let Some(op) = window_glass(d, wk, xf, draws.len()) {
                                zones.push((op.rect, op.cover));
                                win_ops.push(op);
                            }
                        }
                        round = crate::wm::frame_rect(d, wk).map(|f| (f, crate::toast::RADIUS - crate::toast::RIM));
                    }
                    // Moving: opaque all the way, stretched with the drop, each axis on its own.
                    let (dst, round) = match &pose {
                        Some(p) => (p.map_px(xf, r), p.clip_px(xf)),
                        None => (xf.r(r), round_pc(xf, round)),
                    };
                    if let Some(dr) = unsafe { self.surface_draw(d, sk, dst, round, 1.0, &mut ups, &mut dmas, &mut waits)? } {
                        draws.push(dr);
                        gens.push(d.surfaces.get(sk).map_or(0, |s| s.gen));
                    }
                }
            }
        }
        // Closed windows going away (life.rs): a drop each, over the windows, with the last
        // picture of its surfaces.
        for (g, p) in crate::life::ghosts(d, oi) {
            let hover = bridge(&lenses, &p, &mut taken);
            let op = drop_op(xf, &p, g.active, hover, 0, draws.len());
            zones.push((op.rect, op.cover));
            win_ops.push(op);
            if p.look.img_a <= 0.0 {
                continue;
            }
            let round = p.clip_px(xf);
            for (sk, r) in &g.surfs {
                if let Some(t) = self.ghost_tex.get(sk).or_else(|| self.surfs.get(sk)).filter(|t| !t.fresh) {
                    draws.push(Draw::Tex { set: t.set, dst: p.map_px(xf, *r), uv: [0.0, 0.0, 1.0, 1.0], round, alpha: p.look.img_a });
                }
            }
        }
        // The preview card: moving, a drop of its own (at rest a glass panel, shell.rs
        // `glass_panels`). Its content is the window's, drawn right over its glass.
        let mut card_op = None;
        let mut ldraws = Vec::new();
        if let Some(c) = crate::shell::card(d).filter(|c| c.oi == oi) {
            if c.moving {
                let hover = bridge(&lenses, &c.pose, &mut taken);
                let op = drop_op(xf, &c.pose, true, hover, 1, 0);
                zones.push((op.rect, op.cover));
                card_op = Some(op);
            }
            if c.pose.look.img_a > 0.0 {
                let clip = c.pose.clip_px(xf);
                for (sk, r) in crate::render::window_surfaces(d, c.wk) {
                    let dst = c.pose.map_px(xf, r);
                    if let Some(dr) = unsafe { self.surface_draw(d, sk, dst, clip, c.pose.look.img_a, &mut ups, &mut dmas, &mut waits)? } {
                        ldraws.push(dr);
                    }
                }
            }
        }
        // «Пуск»: its glass and the drops on it (startanim.rs), the glass bridging to the hover
        // drop at the start button.
        let mut start_ops = Vec::new();
        for sp in crate::startanim::drops(d, oi) {
            let hover = match sp.bridge {
                Some(btn) => lens.filter(|l| !taken.contains(l) && l.overlaps(&btn)),
                _ => None,
            };
            taken.extend(hover);
            let op = start_op(xf, &sp, hover);
            zones.push((op.rect, op.cover));
            start_ops.push(op);
        }
        let start_rect = (crate::startanim::shown(d) == Some(oi)).then(|| crate::start::rect(d, oi));
        // Alt+Tab (alttab.rs): the panel's rim, the chosen card's drop, and the cards'
        // thumbnails (the windows' own textures) right over them, under the overlays' layer.
        let mut at_ops = Vec::new();
        let s0 = xf.scale as f32;
        if let Some(sp) = crate::alttab::panel_drop(d, oi) {
            let mut op = start_op(xf, &sp, None);
            if let Some(inst) = op.drop.as_mut() {
                // drop.frag: only the rim, the inside left out.
                inst[11][2] = 1.0;
            }
            zones.push((op.rect, op.cover));
            at_ops.push(op);
        }
        if let Some((r, rad)) = crate::alttab::lens(d, oi) {
            let op = GlassOp {
                rect: xf.r(r),
                radii: [rad * s0; 4],
                layer: 2,
                before: 0,
                tint: crate::glass::TINT_LENS,
                bezel: crate::glass::bezel() * s0,
                panel: true,
                cover: [Rect::default(); 2],
                drop: None,
                sigma: crate::glass::frost() * s0, ..Default::default() };
            zones.push((op.rect, op.cover));
            at_ops.push(op);
        }
        let mut l2draws = Vec::new();
        for (wk, p) in crate::alttab::thumbs(d, oi) {
            let clip = p.clip_px(xf);
            for (sk, r) in crate::render::window_surfaces(d, wk) {
                if let Some(dr) = unsafe { self.surface_draw(d, sk, p.map_px(xf, r), clip, p.look.img_a, &mut ups, &mut dmas, &mut waits)? } {
                    l2draws.push(dr);
                }
            }
        }
        // Frames of windows no longer on this output.
        let gone: Vec<WKey> = self.outs[k].frames.keys().copied().filter(|wk| !frames_seen.contains(wk)).collect();
        for wk in gone {
            if let Some(f) = self.outs[k].frames.remove(&wk) {
                unsafe { self.free_tex(&f.tex) };
            }
        }

        // CPU layers, straight into their upload buffers, only where they have (or had) content.
        let ext_now: [Vec<Rect>; 2] = crate::render::layer_extents(d, oi).map(|v| v.into_iter().map(|r| r.intersect(&bounds)).filter(|r| !r.is_empty()).collect());
        let mut ldmg: [Vec<Rect>; 2] = [Vec::new(), Vec::new()];
        for li in 0..2 {
            let mut region = Damage::default();
            for e in ext_now[li].iter().chain(self.outs[k].ext[li].iter()) {
                for r in &raw {
                    let c = r.intersect(e);
                    if !c.is_empty() {
                        region.add(c);
                    }
                }
            }
            ldmg[li] = region.rects;
        }
        {
            let out = &self.outs[k];
            let n = (w * h) as usize;
            let mk = |b: &Buffer| Canvas { px: unsafe { std::slice::from_raw_parts_mut(b.ptr as *mut u32, n) }, w, h, stride: w as usize };
            let mut layers = [mk(&out.layers[0].1), mk(&out.layers[1].1)];
            let t = std::time::Instant::now();
            crate::render::render_layers(d, oi, &mut layers, &ldmg);
            if crate::prof::on() {
                crate::prof::since(&format!("{} cpu layers", d.outputs[oi].name), t);
            }
        }
        self.outs[k].ext = ext_now;
        // A fresh image is UNDEFINED everywhere: all of it goes up (the buffer is zero
        // outside the content).
        let lcopy: [Vec<Rect>; 2] = if fresh { [vec![bounds], vec![bounds]] } else { ldmg };

        let rects = crate::render::widen_for_glass(d, oi, xf, &damage, bounds, &zones);
        let s = xf.scale as f32;
        let mut ops = win_ops;
        // The calendar's and the timer's flyouts: their places' look (look.rs).
        let cal = crate::tray::glass(d, oi).zip(crate::tray::cal_fly(d)).map(|(r, f)| {
            let p = if f == crate::tray::Fly::Timer { crate::look::Place::Timer } else { crate::look::Place::Calendar };
            (r, crate::look::glass(p))
        });
        ops.extend(
            crate::shell::glass_panels(d, oi)
                .into_iter()
                .filter(|(r, _, _)| !taken.contains(r) && Some(*r) != start_rect)
                .map(|(r, radius, layer)| {
                    let place = cal.filter(|(c, _)| *c == r).map(|(_, g)| g);
                    GlassOp {
                        rect: xf.r(r),
                        radii: [radius as f32 * s; 4],
                        layer,
                        before: 0,
                        // The taskbar's hover drop: clear glass on the island's dark glass.
                        tint: match place {
                            Some(g) => crate::glass::tint_at(crate::glass::TINT_DEEP, g.dim),
                            None if lenses.contains(&r) => crate::glass::TINT_LENS,
                            None => crate::glass::TINT_DEEP,
                        },
                        bezel: place.map_or(crate::glass::bezel(), |g| g.bezel) * s,
                        panel: true,
                        cover: [Rect::default(); 2],
                        drop: None,
                        sigma: place.map_or(crate::glass::frost(), |g| if crate::glass::frosted() { g.blur } else { 0.0 }) * s, ..Default::default() }
                }),
        );
        ops.extend(card_op);
        ops.extend(start_ops);
        ops.extend(at_ops);
        ops.truncate(MAX_GLASS as usize);
        let captured: Vec<bool>;
        unsafe {
            // Kept frosts: one image per window glass at its frost's level; the ones of
            // windows gone from here go too.
            let live: Vec<u64> = ops.iter().filter(|o| o.id != 0).map(|o| o.id).collect();
            let dead: Vec<u64> = self.outs[k].gcache.keys().copied().filter(|id| !live.contains(id)).collect();
            for id in dead {
                if let Some(c) = self.outs[k].gcache.remove(&id) {
                    (self.f.vkFreeDescriptorSets)(self.dev, self.dpool, 1, &c.set);
                    self.free_image(&c.img);
                }
            }
            for op in ops.iter().filter(|o| o.id != 0) {
                let Some((b, _, _)) = Self::blur_for(&self.outs[k], op.sigma) else { continue };
                let (level, bw, bh) = (b.level, b.w, b.h);
                if self.outs[k].gcache.get(&op.id).map_or(false, |c| c.level == level) {
                    continue;
                }
                if let Some(c) = self.outs[k].gcache.remove(&op.id) {
                    (self.f.vkFreeDescriptorSets)(self.dev, self.dpool, 1, &c.set);
                    self.free_image(&c.img);
                }
                let img = self.image(bw, bh, 1, usage::COLOR_ATTACHMENT | usage::SAMPLED)?;
                let set = self.set_with(img.view, self.s_lin, 5, layout::SHADER_READ)?;
                self.outs[k].gcache.insert(op.id, GCache { img, set, level, under: 0, fresh: true });
            }
            self.write_instances(k, &ops, s);
            captured = self.record(k, &ups, &lcopy, &rects, &ops, &draws, &[ldraws, l2draws], &dmas, s)?;
            let stages = vec![stage::FRAGMENT; waits.len()];
            let si = SubmitInfo {
                s_type: st::SUBMIT_INFO,
                p_next: std::ptr::null(),
                wait_count: waits.len() as u32,
                wait_sems: waits.as_ptr(),
                wait_stages: stages.as_ptr(),
                cmd_count: 1,
                cmds: &self.cmd,
                signal_count: 0,
                signal_sems: std::ptr::null(),
            };
            let t = std::time::Instant::now();
            check((self.f.vkQueueSubmit)(self.queue, 1, &si, self.fence), "vkQueueSubmit")?;
            check((self.f.vkWaitForFences)(self.dev, 1, &self.fence, 1, 5_000_000_000), "vkWaitForFences")?;
            if crate::prof::on() {
                crate::prof::since(&format!("{} gpu wait", d.outputs[oi].name), t);
            }
            (self.f.vkResetFences)(self.dev, 1, &self.fence);
        }
        self.outs[k].fresh = false;
        FRAME_NO.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // What the kept frosts are of now: blurred whole this frame, or not to be trusted.
        for (i, op) in ops.iter().enumerate().filter(|(_, o)| o.id != 0) {
            if let Some(c) = self.outs[k].gcache.get_mut(&op.id) {
                c.fresh = false;
                if captured.get(i).copied().unwrap_or(false) {
                    c.under = op.under;
                } else if !op.hit {
                    c.under = 0;
                }
            }
        }

        // The frame back into the backend's buffer.
        let t_rb = std::time::Instant::now();
        let rb = unsafe { std::slice::from_raw_parts(self.outs[k].readback.ptr as *const u32, (w * h) as usize) };
        for r in &rects {
            for y in r.y..r.y1() {
                let o = y as usize * w as usize;
                let t = y as usize * cv.stride;
                cv.px[t + r.x as usize..t + r.x1() as usize].copy_from_slice(&rb[o + r.x as usize..o + r.x1() as usize]);
            }
        }
        if crate::prof::on() {
            let name = &d.outputs[oi].name;
            crate::prof::since(&format!("{name} readback copy"), t_rb);
            let px: i64 = rects.iter().map(|r| r.w as i64 * r.h as i64).sum();
            crate::prof::add(&format!("{name} Mpx per frame"), px as f64 / 1e6);
        }
        Ok(rects)
    }

    /// A surface's draw at `dst` (physical) with clip `round`: its texture brought up to date
    /// (a dma-buf imported where Vulkan can, its client's fence waited for). None: nothing to
    /// draw yet.
    #[allow(clippy::too_many_arguments)]
    unsafe fn surface_draw(&mut self, d: &mut Desk, sk: SKey, dst: Rect, round: [f32; 6], alpha: f32, ups: &mut Vec<Upload>, dmas: &mut Vec<(H, bool)>, waits: &mut Vec<H>) -> R<Option<Draw>> {
        let Some(s) = d.surfaces.get(sk) else { return Ok(None) };
        let Some((b, _)) = &s.buffer else { return Ok(None) };
        if let Some(c) = b.solid {
            return Ok(Some(Draw::Color { dst, color: unpack(c), round, alpha }));
        }
        let src = crate::render::surface_src(s, b);
        if src.is_empty() {
            return Ok(None);
        }
        // Imported where Vulkan can; otherwise (linear only) read through mmap and uploaded
        // like shared memory.
        if let Some(dm) = b.dmabuf.as_ref().filter(|_| self.dma.is_some()) {
            let (b, flip) = (b.clone(), dm.y_invert);
            return match self.sync_dmabuf(&b) {
                Ok(Some((set, tw, th, img, first, wait))) => {
                    if !dmas.iter().any(|(i, _)| *i == img) {
                        dmas.push((img, first));
                        waits.extend(wait);
                    }
                    let (tw, th) = (tw as f32, th as f32);
                    let (v0, v1) = (src.y as f32 / th, src.y1() as f32 / th);
                    let (v0, v1) = if flip { (1.0 - v0, 1.0 - v1) } else { (v0, v1) };
                    Ok(Some(Draw::Tex { set, dst, uv: [src.x as f32 / tw, v0, src.x1() as f32 / tw, v1], round, alpha }))
                }
                Ok(None) => Ok(None),
                // A buffer the driver refuses: not drawn (the client can tell nothing).
                Err(e) => {
                    eprintln!("dmabuf: {e}");
                    Ok(None)
                }
            };
        }
        Ok(self.sync_surface(d, sk, ups)?.map(|(set, tw, th)| {
            let (tw, th) = (tw as f32, th as f32);
            let uv = [src.x as f32 / tw, src.y as f32 / th, src.x1() as f32 / tw, src.y1() as f32 / th];
            Draw::Tex { set, dst, uv, round, alpha }
        }))
    }

    /// ui.vert instances for the glass panels: Liquify's glass card, as glass.rs draws it.
    /// A drop's (drop.vert) is ready made.
    unsafe fn write_instances(&self, k: usize, ops: &[GlassOp], s: f32) {
        let out = &self.outs[k];
        for (i, op) in ops.iter().enumerate() {
            if let Some(v) = &op.drop {
                std::ptr::copy_nonoverlapping(v.as_ptr() as *const u8, out.inst.ptr.add(i * INST_SIZE as usize), INST_SIZE as usize);
                continue;
            }
            let t = op.tint;
            let a = t[3];
            let r = op.rect;
            let (border, highlight) = if op.panel { (crate::glass::BORDER_A, crate::glass::HIGHLIGHT) } else { (0.0, 0.0) };
            let v: [[f32; 4]; 12] = [
                [r.x as f32, r.y as f32, r.w as f32, r.h as f32],     // iRect
                [0.0, 0.0, out.w as f32, out.h as f32],                 // iClip
                [t[0] / a, t[1] / a, t[2] / a, a],                      // iC0: the tint, straight alpha
                [0.0; 4],                                               // iC1
                [0.0; 4],                                               // iGrad
                [1.0, 1.0, 1.0, border],                                // iBorder: hairline white
                [0.0; 4],                                               // iUv
                op.radii,                                               // iRadii
                [crate::glass::BORDER_W * s, 2.0, 0.0, 1.0],            // iP: border, kind, solid fill, opacity
                [highlight, s, crate::glass::THICKNESS * s, op.bezel],  // iQ
                [0.0; 4],                                               // iRClip
                [0.0; 4],                                               // iRRad
            ];
            std::ptr::copy_nonoverlapping(v.as_ptr() as *const u8, out.inst.ptr.add(i * INST_SIZE as usize), INST_SIZE as usize);
        }
    }

    unsafe fn barrier(&self, img: H, range: SubresourceRange, src: (u32, u32, u32), dst: (u32, u32, u32)) {
        let b = ImageMemoryBarrier::new(img, range, src.1, dst.1, src.2, dst.2);
        (self.f.vkCmdPipelineBarrier)(self.cmd, src.0, dst.0, 0, 0, std::ptr::null(), 0, std::ptr::null(), 1, &b);
    }

    unsafe fn begin_rendering(&self, k: usize, scissor: Rect) {
        let out = &self.outs[k];
        let att = RenderingAttachmentInfo {
            s_type: st::RENDERING_ATTACHMENT_INFO,
            p_next: std::ptr::null(),
            view: out.target.view,
            layout: layout::COLOR_ATTACHMENT,
            resolve_mode: 0,
            resolve_view: 0,
            resolve_layout: 0,
            load_op: LOAD_OP_LOAD,
            store_op: STORE_OP_STORE,
            clear: [0.0; 4],
        };
        let area = Rect2D { x: 0, y: 0, w: out.w as u32, h: out.h as u32 };
        let ri = RenderingInfo {
            s_type: st::RENDERING_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            area,
            layer_count: 1,
            view_mask: 0,
            color_count: 1,
            colors: &att,
            depth: std::ptr::null(),
            stencil: std::ptr::null(),
        };
        (self.f.vkCmdBeginRendering)(self.cmd, &ri);
        let vp = Viewport { x: 0.0, y: 0.0, w: out.w as f32, h: out.h as f32, min_depth: 0.0, max_depth: 1.0 };
        (self.f.vkCmdSetViewport)(self.cmd, 0, 1, &vp);
        let sc = Rect2D { x: scissor.x, y: scissor.y, w: scissor.w as u32, h: scissor.h as u32 };
        (self.f.vkCmdSetScissor)(self.cmd, 0, 1, &sc);
    }

    /// A whole-output image 1:1 (layer.frag): `over` blends it, else it replaces.
    unsafe fn draw_full(&self, set: H, over: bool) {
        let p = if over { self.p_over } else { self.p_copy };
        (self.f.vkCmdBindPipeline)(self.cmd, BIND_GRAPHICS, p);
        (self.f.vkCmdBindDescriptorSets)(self.cmd, BIND_GRAPHICS, self.layout, 0, 1, &set, 0, std::ptr::null());
        (self.f.vkCmdDraw)(self.cmd, 3, 1, 0, 0);
    }

    /// The windows' draws that touch `scissor` (surface.vert / surface.frag).
    unsafe fn draw_windows(&self, k: usize, draws: &[Draw], scissor: Rect) {
        let out = &self.outs[k];
        let (f, cmd) = (&self.f, self.cmd);
        let mut bound = false;
        let mut last_set = 0;
        for dr in draws {
            let (dst, set) = match dr {
                Draw::Tex { set, dst, .. } => (*dst, *set),
                // Solid rects sample nothing, but the set must be bound: any will do.
                Draw::Color { dst, .. } => (*dst, out.wall.set),
            };
            if !dst.overlaps(&scissor) {
                continue;
            }
            if !bound {
                (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_tex);
                bound = true;
            }
            if set != last_set {
                (f.vkCmdBindDescriptorSets)(cmd, BIND_GRAPHICS, self.layout, 0, 1, &set, 0, std::ptr::null());
                last_set = set;
            }
            // surface.vert PC (std430): screen, dst @16, uv @32, color @48, mode @64, alpha @68,
            // clip @80, radius @96, bottom radius @100.
            let mut pc = [0f32; 26];
            pc[0] = out.w as f32;
            pc[1] = out.h as f32;
            pc[4..8].copy_from_slice(&[dst.x as f32, dst.y as f32, dst.w as f32, dst.h as f32]);
            let (round, alpha) = match dr {
                Draw::Tex { uv, round, alpha, .. } => {
                    pc[8..12].copy_from_slice(uv);
                    (round, *alpha)
                }
                Draw::Color { color, round, alpha, .. } => {
                    pc[12..16].copy_from_slice(color);
                    pc[16] = 1.0;
                    (round, *alpha)
                }
            };
            pc[20..26].copy_from_slice(round);
            pc[17] = alpha;
            (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, pc.as_ptr() as P);
            (f.vkCmdDraw)(cmd, 4, 1, 0, 0);
        }
    }

    /// What lies under glass panel `op` (the frame so far), copied to the backdrop and
    /// mip-mapped over the part its frost reads. Outside a render pass.
    unsafe fn capture_backdrop(&self, k: usize, op: &GlassOp, s: f32) {
        BLURS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let out = &self.outs[k];
        let (f, cmd) = (&self.f, self.cmd);
        let bounds = Rect::new(0, 0, out.w, out.h);
        let al = 1i32 << (out.levels - 1);
        let z = op.rect.inflate(crate::glass::margin(s)).intersect(&bounds);
        if z.is_empty() {
            return;
        }
        // Aligned to the coarsest level, so every level's texels are whole.
        let z = Rect::from_edges(z.x.div_euclid(al) * al, z.y.div_euclid(al) * al, (z.x1() + al - 1) / al * al, (z.y1() + al - 1) / al * al).intersect(&bounds);
        let all = SubresourceRange::color(0, REMAINING);
        let col = SubresourceRange::color(0, 1);
        self.barrier(out.target.img, col, (stage::COLOR_OUTPUT, access::COLOR_WRITE, layout::COLOR_ATTACHMENT), (stage::TRANSFER, access::TRANSFER_READ, layout::TRANSFER_SRC));
        // The last glass may still read the backdrop.
        self.barrier(out.backdrop.img, all, (stage::FRAGMENT, access::SHADER_READ, layout::GENERAL), (stage::TRANSFER, access::TRANSFER_WRITE, layout::GENERAL));
        let copy = ImageCopy {
            src_sub: SubresourceLayers::color(0),
            src_offset: [z.x, z.y, 0],
            dst_sub: SubresourceLayers::color(0),
            dst_offset: [z.x, z.y, 0],
            extent: [z.w as u32, z.h as u32, 1],
        };
        (f.vkCmdCopyImage)(cmd, out.target.img, layout::TRANSFER_SRC, out.backdrop.img, layout::GENERAL, 1, &copy);
        let size = |l: u32| ((out.w >> l).max(1), (out.h >> l).max(1));
        for l in 1..out.levels {
            self.barrier(
                out.backdrop.img,
                SubresourceRange::color(l - 1, 1),
                (stage::TRANSFER, access::TRANSFER_WRITE, layout::GENERAL),
                (stage::TRANSFER, access::TRANSFER_READ, layout::GENERAL),
            );
            let (sw, sh) = size(l - 1);
            let (dw, dh) = size(l);
            let d0 = (z.x >> l, z.y >> l);
            let d1 = (((z.x1() + (1 << l) - 1) >> l).min(dw), ((z.y1() + (1 << l) - 1) >> l).min(dh));
            if d1.0 <= d0.0 || d1.1 <= d0.1 {
                continue;
            }
            let blit = ImageBlit {
                src_sub: SubresourceLayers::color(l - 1),
                src: [[d0.0 * 2, d0.1 * 2, 0], [(d1.0 * 2).min(sw), (d1.1 * 2).min(sh), 1]],
                dst_sub: SubresourceLayers::color(l),
                dst: [[d0.0, d0.1, 0], [d1.0, d1.1, 1]],
            };
            (f.vkCmdBlitImage)(cmd, out.backdrop.img, layout::GENERAL, out.backdrop.img, layout::GENERAL, 1, &blit, FILTER_LINEAR);
        }
        self.barrier(out.backdrop.img, all, (stage::TRANSFER, access::TRANSFER_WRITE, layout::GENERAL), (stage::FRAGMENT, access::SHADER_READ, layout::GENERAL));
        if let Some((b, sigma, radius)) = Self::blur_for(out, op.sigma) {
            // The zone on the level (whole texels: it is aligned to the coarsest).
            let l = b.level;
            let zl = Rect::from_edges(z.x >> l, z.y >> l, (z.x1() + (1 << l) - 1) >> l, (z.y1() + (1 << l) - 1) >> l).intersect(&Rect::new(0, 0, b.w, b.h));
            let read = (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ);
            let write = (stage::COLOR_OUTPUT, access::COLOR_WRITE, layout::COLOR_ATTACHMENT);
            let keep = (op.id != 0).then(|| out.gcache.get(&op.id)).flatten().filter(|c| c.level == l);
            for (pass, dir) in [[1.0f32, 0.0], [0.0, 1.0]].into_iter().enumerate() {
                let dst = match keep {
                    Some(c) if pass == 1 => &c.img,
                    _ => &b.imgs[pass],
                };
                self.barrier(dst.img, col, read, write);
                self.blur_pass(b, dst.view, b.sets[pass], zl, [dir[0], dir[1], sigma, radius]);
                self.barrier(dst.img, col, write, read);
            }
        }
        self.barrier(out.target.img, col, (stage::TRANSFER, access::TRANSFER_READ, layout::TRANSFER_SRC), (stage::COLOR_OUTPUT, access::COLOR_WRITE | access::COLOR_READ, layout::COLOR_ATTACHMENT));
    }

    /// One direction of the frost's gaussian (blur.frag) from `set` into `dst`, in `zone`
    /// (texels of the level). Outside a render pass.
    unsafe fn blur_pass(&self, b: &Blur, dst: H, set: H, zone: Rect, params: [f32; 4]) {
        let (f, cmd) = (&self.f, self.cmd);
        let att = RenderingAttachmentInfo {
            s_type: st::RENDERING_ATTACHMENT_INFO,
            p_next: std::ptr::null(),
            view: dst,
            layout: layout::COLOR_ATTACHMENT,
            resolve_mode: 0,
            resolve_view: 0,
            resolve_layout: 0,
            load_op: LOAD_OP_LOAD,
            store_op: STORE_OP_STORE,
            clear: [0.0; 4],
        };
        let ri = RenderingInfo {
            s_type: st::RENDERING_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            area: Rect2D { x: 0, y: 0, w: b.w as u32, h: b.h as u32 },
            layer_count: 1,
            view_mask: 0,
            color_count: 1,
            colors: &att,
            depth: std::ptr::null(),
            stencil: std::ptr::null(),
        };
        (f.vkCmdBeginRendering)(cmd, &ri);
        let vp = Viewport { x: 0.0, y: 0.0, w: b.w as f32, h: b.h as f32, min_depth: 0.0, max_depth: 1.0 };
        (f.vkCmdSetViewport)(cmd, 0, 1, &vp);
        let sc = Rect2D { x: zone.x, y: zone.y, w: zone.w as u32, h: zone.h as u32 };
        (f.vkCmdSetScissor)(cmd, 0, 1, &sc);
        (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_blur);
        (f.vkCmdBindDescriptorSets)(cmd, BIND_GRAPHICS, self.layout, 0, 1, &set, 0, std::ptr::null());
        let mut pc = [0f32; 26];
        // blur.frag: direction, sigma, radius (texels).
        pc[..4].copy_from_slice(&params);
        (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, pc.as_ptr() as P);
        (f.vkCmdDraw)(cmd, 3, 1, 0, 0);
        (f.vkCmdEndRendering)(cmd);
    }

    /// glass.frag's shadow, then ui.frag kind 2: one panel. Inside a render pass.
    unsafe fn draw_glass(&self, k: usize, i: usize, op: &GlassOp, s: f32) {
        let out = &self.outs[k];
        let (f, cmd) = (&self.f, self.cmd);
        let r = op.rect;
        let (sw, sh) = (out.w as f32, out.h as f32);
        let off = i as u64 * INST_SIZE;
        if op.drop.is_some() {
            // drop.frag PC: screen, frost, clear. Over our blurred copy (frost 0), else over the
            // sharp one with its mips (frost: sigma), else clear glass over it as it is.
            let mut pc = [0f32; 26];
            pc[0] = sw;
            pc[1] = sh;
            let set = if !crate::glass::frosted() {
                pc[3] = 1.0;
                out.bd_set
            } else {
                match Self::blur_for(out, op.sigma) {
                    Some((b, _, _)) => b.sets[2],
                    None => out.bd_set,
                }
            };
            (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_drop);
            (f.vkCmdBindDescriptorSets)(cmd, BIND_GRAPHICS, self.layout, 0, 1, &set, 0, std::ptr::null());
            (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, pc.as_ptr() as P);
            (f.vkCmdBindVertexBuffers)(cmd, 0, 1, &out.inst.buf, &off);
            (f.vkCmdDraw)(cmd, 4, 1, 0, 0);
            return;
        }
        // common.glsl PC: screen, center, halfSize, radius, thickness, bezel, ior, blur, spec, tint,
        // shadow, disp, sat, bgAspect, scale, bgMode, lod, region (vec4), bgFade, prevAspect.
        let mut pc = [0f32; 26];
        pc[0] = sw;
        pc[1] = sh;
        pc[2] = r.x as f32 + r.w as f32 * 0.5;
        pc[3] = r.y as f32 + r.h as f32 * 0.5;
        pc[4] = r.w as f32 * 0.5;
        pc[5] = r.h as f32 * 0.5;
        pc[6] = op.radii[0];
        pc[13] = crate::glass::SHADOW;
        pc[17] = s;
        if !crate::glass::frosted() {
            // glass.frag over the backdrop (bgMode 4, the whole output, level 0): shadow,
            // refraction along the bezel, inner shadow and rim; blur 0, no tint.
            pc[7] = crate::glass::THICKNESS * s;
            pc[8] = crate::glass::bezel() * s;
            pc[9] = crate::glass::IOR;
            pc[10] = 0.0;
            pc[11] = crate::glass::SPEC;
            pc[12] = 0.0;
            pc[14] = 0.0;
            pc[15] = 1.0;
            pc[16] = sw / sh;
            pc[18] = 4.0;
            pc[19] = 0.0;
            pc[20..24].copy_from_slice(&[0.0, 0.0, sw, sh]);
            (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_lens);
            (f.vkCmdBindDescriptorSets)(cmd, BIND_GRAPHICS, self.layout, 0, 1, &out.bd_set, 0, std::ptr::null());
            (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, pc.as_ptr() as P);
            (f.vkCmdDraw)(cmd, 4, 1, 0, 0);
            return;
        }
        if op.panel {
            (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_shadow);
            (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, pc.as_ptr() as P);
            (f.vkCmdDraw)(cmd, 4, 1, 0, 0);
        }

        // ui.frag PC: screen, bgAspect, scrollY, round (vec4), roundR, lens, frost, frostQ.
        let mut ui = [0f32; 26];
        ui[0] = sw;
        ui[1] = sh;
        ui[2] = sw / sh;
        // Our gaussian already blurred it: ui.frag reads it as a sharp background (frost 0:
        // its light four-tap frost, invisible on it), with its saturation and the rest as is.
        // (No frost asked: the sharp backdrop, tinted and refracted only.)
        ui[10] = 0.0;
        let set = match Self::blur_for(out, op.sigma) {
            Some((b, _, _)) => match (op.id != 0).then(|| out.gcache.get(&op.id)).flatten().filter(|c| c.level == b.level) {
                Some(c) => c.set,
                None => b.sets[2],
            },
            None => out.bd_set,
        };
        ui[11] = 1.0;
        (f.vkCmdBindPipeline)(cmd, BIND_GRAPHICS, self.p_glass);
        (f.vkCmdBindDescriptorSets)(cmd, BIND_GRAPHICS, self.layout, 0, 1, &set, 0, std::ptr::null());
        (f.vkCmdPushConstants)(cmd, self.layout, SHADER_VERTEX | SHADER_FRAGMENT, 0, PC_SIZE, ui.as_ptr() as P);
        (f.vkCmdBindVertexBuffers)(cmd, 0, 1, &out.inst.buf, &off);
        (f.vkCmdDraw)(cmd, 4, 1, 0, 0);
    }

    #[allow(clippy::too_many_arguments)]
    /// A barrier that also moves `img` between our queue family and the foreign one.
    unsafe fn qbarrier(&self, img: H, src: (u32, u32, u32, u32), dst: (u32, u32, u32, u32)) {
        let mut b = ImageMemoryBarrier::new(img, SubresourceRange::color(0, 1), src.1, dst.1, src.2, dst.2);
        b.src_qf = src.3;
        b.dst_qf = dst.3;
        (self.f.vkCmdPipelineBarrier)(self.cmd, src.0, dst.0, 0, 0, std::ptr::null(), 0, std::ptr::null(), 1, &b);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn record(&self, k: usize, ups: &[Upload], lcopy: &[Vec<Rect>; 2], rects: &[Rect], ops: &[GlassOp], draws: &[Draw], ldraws: &[Vec<Draw>; 2], dmas: &[(H, bool)], s: f32) -> R<Vec<bool>> {
        let mut captured = vec![false; ops.len()];
        let out = &self.outs[k];
        let (f, cmd) = (&self.f, self.cmd);
        (f.vkResetCommandBuffer)(cmd, 0);
        let bi = CommandBufferBeginInfo { s_type: st::COMMAND_BUFFER_BEGIN_INFO, p_next: std::ptr::null(), flags: ONE_TIME_SUBMIT, inheritance: std::ptr::null() };
        check((f.vkBeginCommandBuffer)(cmd, &bi), "vkBeginCommandBuffer")?;
        let col = SubresourceRange::color(0, 1);
        let fresh = out.fresh;

        // 1. Textures and the layers' damage up to the GPU.
        let upload = |img: H, was_fresh: bool, buf: H, copies: &[BufferImageCopy]| {
            let old = if was_fresh { layout::UNDEFINED } else { layout::SHADER_READ };
            self.barrier(img, col, (stage::FRAGMENT, access::NONE, old), (stage::TRANSFER, access::TRANSFER_WRITE, layout::TRANSFER_DST));
            (f.vkCmdCopyBufferToImage)(cmd, buf, img, layout::TRANSFER_DST, copies.len() as u32, copies.as_ptr());
            self.barrier(img, col, (stage::TRANSFER, access::TRANSFER_WRITE, layout::TRANSFER_DST), (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ));
        };
        for u in ups {
            upload(u.img, u.fresh, u.buf, &u.copies);
        }
        for (li, (img, buf, _)) in out.layers.iter().enumerate() {
            let copies: Vec<BufferImageCopy> = lcopy[li]
                .iter()
                .map(|r| BufferImageCopy {
                    buffer_offset: ((r.y as u64) * out.w as u64 + r.x as u64) * 4,
                    row_length: out.w as u32,
                    image_height: out.h as u32,
                    sub: SubresourceLayers::color(0),
                    offset: [r.x, r.y, 0],
                    extent: [r.w as u32, r.h as u32, 1],
                })
                .collect();
            if !copies.is_empty() {
                upload(img.img, fresh, buf.buf, &copies);
            }
        }
        if fresh {
            self.barrier(out.backdrop.img, SubresourceRange::color(0, REMAINING), (stage::TOP, access::NONE, layout::UNDEFINED), (stage::FRAGMENT, access::SHADER_READ, layout::GENERAL));
            for i in out.blurs.iter().flatten().flat_map(|b| &b.imgs) {
                self.barrier(i.img, col, (stage::TOP, access::NONE, layout::UNDEFINED), (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ));
            }
        }
        for c in out.gcache.values().filter(|c| c.fresh) {
            self.barrier(c.img.img, col, (stage::TOP, access::NONE, layout::UNDEFINED), (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ));
        }
        // Clients' dma-bufs: ours to read for this frame (PREINITIALIZED the first time: the
        // contents are the client's, not to be thrown away).
        for &(img, first) in dmas {
            let old = if first { layout::PREINITIALIZED } else { layout::GENERAL };
            self.qbarrier(img, (stage::TOP, access::NONE, old, QUEUE_FAMILY_FOREIGN), (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ, self.family));
        }
        let old = if fresh { layout::UNDEFINED } else { layout::TRANSFER_SRC };
        self.barrier(out.target.img, col, (stage::TRANSFER, access::NONE, old), (stage::COLOR_OUTPUT, access::COLOR_WRITE | access::COLOR_READ, layout::COLOR_ATTACHMENT));

        // 2. The frame, bottom to top, in every rect.
        let m = crate::glass::margin(s);
        for w in rects {
            self.begin_rendering(k, *w);
            self.draw_full(out.wall.set, false);
            // The windows, bottom to top; a window's glass right before it, over what is
            // below it by then.
            let mut start = 0;
            for (i, op) in ops.iter().enumerate() {
                // (Under a window's opaque content its glass would not show.)
                if op.layer != 0 || !op.rect.inflate(m).overlaps(w) || op.cover.iter().any(|c| c.contains_rect(w)) {
                    continue;
                }
                self.draw_windows(k, &draws[start..op.before], *w);
                start = op.before;
                // Its kept frost is of what is under it now: drawn from it as it is.
                if op.hit {
                    KEPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    (f.vkCmdEndRendering)(cmd);
                    self.capture_backdrop(k, op, s);
                    self.begin_rendering(k, *w);
                    let zone = op.rect.inflate(m).intersect(&Rect::new(0, 0, out.w, out.h));
                    if w.contains_rect(&zone) {
                        captured[i] = true;
                    }
                }
                self.draw_glass(k, i, op, s);
            }
            self.draw_windows(k, &draws[start..], *w);
            // Glass of layer 1 under CPU layer 0 (the preview card's content right on its
            // glass), glass of layer 2 under CPU layer 1.
            for gl in 1..3 {
                for (i, op) in ops.iter().enumerate() {
                    if op.layer != gl || !op.rect.inflate(m).overlaps(w) {
                        continue;
                    }
                    (f.vkCmdEndRendering)(cmd);
                    self.capture_backdrop(k, op, s);
                    self.begin_rendering(k, *w);
                    self.draw_glass(k, i, op, s);
                }
                // What lies right on that glass: the preview card's content; Alt+Tab's thumbnails.
                self.draw_windows(k, &ldraws[gl - 1], *w);
                self.draw_full(out.layers[gl - 1].2, true);
            }
            (f.vkCmdEndRendering)(cmd);
        }

        // 3. Back to the CPU.
        self.barrier(out.target.img, col, (stage::COLOR_OUTPUT, access::COLOR_WRITE, layout::COLOR_ATTACHMENT), (stage::TRANSFER, access::TRANSFER_READ, layout::TRANSFER_SRC));
        let back: Vec<BufferImageCopy> = rects
            .iter()
            .map(|r| BufferImageCopy {
                buffer_offset: ((r.y as u64) * out.w as u64 + r.x as u64) * 4,
                row_length: out.w as u32,
                image_height: out.h as u32,
                sub: SubresourceLayers::color(0),
                offset: [r.x, r.y, 0],
                extent: [r.w as u32, r.h as u32, 1],
            })
            .collect();
        if !back.is_empty() {
            (f.vkCmdCopyImageToBuffer)(cmd, out.target.img, layout::TRANSFER_SRC, out.readback.buf, back.len() as u32, back.as_ptr());
        }
        // ... and back to the client.
        for &(img, _) in dmas {
            self.qbarrier(img, (stage::FRAGMENT, access::SHADER_READ, layout::SHADER_READ, self.family), (stage::BOTTOM, access::NONE, layout::GENERAL, QUEUE_FAMILY_FOREIGN));
        }
        let mb = MemoryBarrier { s_type: st::MEMORY_BARRIER, p_next: std::ptr::null(), src_access: access::TRANSFER_WRITE, dst_access: access::HOST_READ };
        (f.vkCmdPipelineBarrier)(cmd, stage::TRANSFER, stage::HOST, 0, 1, &mb, 0, std::ptr::null(), 0, std::ptr::null());
        check((f.vkEndCommandBuffer)(cmd), "vkEndCommandBuffer")?;
        Ok(captured)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let null = std::ptr::null();
            (self.f.vkDeviceWaitIdle)(self.dev);
            for o in std::mem::take(&mut self.outs) {
                self.free_out(&o);
            }
            for t in std::mem::take(&mut self.surfs).into_values().chain(std::mem::take(&mut self.ghost_tex).into_values()) {
                self.free_tex(&t);
            }
            for i in std::mem::take(&mut self.imports).into_values() {
                self.free_import(&i);
            }
            for (b, _) in std::mem::take(&mut self.arena) {
                self.free_buffer(&b);
            }
            for p in [self.p_copy, self.p_over, self.p_shadow, self.p_glass, self.p_tex, self.p_lens, self.p_blur, self.p_drop] {
                (self.f.vkDestroyPipeline)(self.dev, p, null);
            }
            for s in [self.s_near, self.s_mip, self.s_lin] {
                (self.f.vkDestroySampler)(self.dev, s, null);
            }
            (self.f.vkDestroyDescriptorPool)(self.dev, self.dpool, null);
            (self.f.vkDestroyPipelineLayout)(self.dev, self.layout, null);
            (self.f.vkDestroyDescriptorSetLayout)(self.dev, self.dsl, null);
            (self.f.vkDestroyFence)(self.dev, self.fence, null);
            (self.f.vkDestroyCommandPool)(self.dev, self.pool, null);
            (self.f.vkDestroyDevice)(self.dev, null);
            (self.i.vkDestroyInstance)(self.inst, null);
        }
    }
}
