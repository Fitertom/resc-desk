//! linux-dmabuf: clients hand over GPU buffers (dma-buf fds) instead of shared memory. Games,
//! browsers and Xwayland's glamor render on the GPU and need this; without it they draw on
//! the CPU or copy every frame.
//!
//! What can be imported is what the renderer says (gpu/: the formats and modifiers Vulkan can
//! sample, and which /dev/dri node it is). With no Vulkan but a GPU in /dev/dri, linear
//! ARGB/XRGB buffers are still taken: the CPU renderer reads them through mmap.
use crate::desk::Desk;
use std::cell::RefCell;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

const fn fourcc(s: &[u8; 4]) -> u32 {
    s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}
pub const ARGB8888: u32 = fourcc(b"AR24");
pub const XRGB8888: u32 = fourcc(b"XR24");
pub const ABGR8888: u32 = fourcc(b"AB24");
pub const XBGR8888: u32 = fourcc(b"XB24");
/// The formats we take, all 8 bits per channel in one plane.
pub const FORMATS: [u32; 4] = [ARGB8888, XRGB8888, ABGR8888, XBGR8888];
pub const MOD_LINEAR: u64 = 0;

pub fn has_alpha(format: u32) -> bool {
    format == ARGB8888 || format == ABGR8888
}

pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// A client's dma-buf behind a wl_buffer.
pub struct Dmabuf {
    pub format: u32,
    pub modifier: u64,
    pub planes: Vec<Plane>,
    pub w: i32,
    pub h: i32,
    pub y_invert: bool,
    map: RefCell<Option<sys::Mmap>>,
}

/// zwp_linux_buffer_params_v1 while the client fills it in.
#[derive(Default)]
pub struct Params {
    pub planes: [Option<(OwnedFd, u32, u32, u64)>; 4],
    pub used: bool,
}

/// What we can import, for the feedback and the format events.
pub struct Support {
    /// dev_t of the GPU's node.
    pub dev: u64,
    pub formats: Vec<(u32, u64)>,
    /// The format table (format u32, padding, modifier u64 per entry), sealed.
    pub table: OwnedFd,
    pub table_size: u32,
}

/// After the renderer is up: advertise zwp_linux_dmabuf_v1 if anything can be imported.
pub fn init(d: &mut Desk) {
    let (dev, formats) = match d.gpu.as_ref().and_then(|g| g.dmabuf_support()) {
        Some(s) => s,
        None => match render_node() {
            // No Vulkan, but a GPU: linear buffers, read by the CPU.
            Some(dev) => (dev, vec![(ARGB8888, MOD_LINEAR), (XRGB8888, MOD_LINEAR)]),
            // Tests without a GPU (RESC_DMABUF_TEST=1): a memfd passes for a linear dma-buf.
            None if std::env::var_os("RESC_DMABUF_TEST").is_some() => (0, vec![(ARGB8888, MOD_LINEAR), (XRGB8888, MOD_LINEAR)]),
            None => return,
        },
    };
    if formats.is_empty() {
        return;
    }
    let mut bytes = Vec::with_capacity(formats.len() * 16);
    for (f, m) in &formats {
        bytes.extend_from_slice(&f.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&m.to_ne_bytes());
    }
    let Ok(table) = sys::memfd("resc-desk-dmabuf-formats", 0) else { return };
    if sys::write_all(table.as_fd(), &bytes).is_err() {
        return;
    }
    sys::seal_readonly(table.as_fd());
    eprintln!("dmabuf: {} format/modifier pairs, device {}:{}", formats.len(), major(dev), minor(dev));
    d.dmabuf = Some(Support { dev, formats, table, table_size: bytes.len() as u32 });
    d.add_global(wl::Iface::ZwpLinuxDmabufV1, 4, None);
}

/// glibc's makedev / major / minor.
pub fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32) | ((major & 0xfff) << 8) | ((minor & 0xffff_ff00) << 12) | (minor & 0xff)
}
fn major(dev: u64) -> u64 {
    ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0xfff)
}
fn minor(dev: u64) -> u64 {
    ((dev >> 12) & 0xffff_ff00) | (dev & 0xff)
}

fn render_node() -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let mut nodes: Vec<_> = std::fs::read_dir("/dev/dri").ok()?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    nodes.sort();
    nodes.iter().filter(|p| p.file_name().map_or(false, |n| n.to_string_lossy().starts_with("renderD"))).find_map(|p| std::fs::metadata(p).ok().map(|m| m.rdev()))
}

/// Checks a finished params object and makes the buffer. Err: (error code, message), or
/// code u32::MAX for "failed" (a buffer we cannot take, not a protocol error).
pub fn create(sup: &Support, p: &mut Params, w: i32, h: i32, format: u32, flags: u32) -> Result<Dmabuf, (u32, String)> {
    use wl::protocol::zwp_linux_buffer_params_v1::error as e;
    if p.used {
        return Err((e::ALREADY_USED, "params already used".into()));
    }
    p.used = true;
    if w <= 0 || h <= 0 {
        return Err((e::INVALID_DIMENSIONS, "size must be positive".into()));
    }
    let n = p.planes.iter().take_while(|x| x.is_some()).count();
    if n == 0 || p.planes[n..].iter().any(|x| x.is_some()) {
        return Err((e::INCOMPLETE, "planes missing".into()));
    }
    let modifier = p.planes[0].as_ref().unwrap().3;
    if p.planes[..n].iter().any(|x| x.as_ref().unwrap().3 != modifier) {
        return Err((e::INVALID_FORMAT, "planes with different modifiers".into()));
    }
    if n != 1 || !sup.formats.contains(&(format, modifier)) {
        return Err((u32::MAX, String::new()));
    }
    let (fd, offset, stride, _) = p.planes[0].take().unwrap();
    if (stride as u64) < w as u64 * 4 && modifier == MOD_LINEAR {
        return Err((e::OUT_OF_BOUNDS, "stride too small".into()));
    }
    // The dma-buf must hold what the plane says (when its size can be asked).
    if let Some(size) = dmabuf_size(&fd) {
        if offset as u64 + stride as u64 * h as u64 > size && modifier == MOD_LINEAR {
            return Err((e::OUT_OF_BOUNDS, "plane outside the buffer".into()));
        }
    }
    Ok(Dmabuf {
        format,
        modifier,
        planes: vec![Plane { fd, offset, stride }],
        w,
        h,
        y_invert: flags & wl::protocol::zwp_linux_buffer_params_v1::flags::Y_INVERT != 0,
        map: RefCell::new(None),
    })
}

fn dmabuf_size(fd: &OwnedFd) -> Option<u64> {
    use std::io::Seek;
    let mut f = std::fs::File::from(fd.try_clone().ok()?);
    let n = f.seek(std::io::SeekFrom::End(0)).ok()?;
    (n > 0).then_some(n)
}

// DMA_BUF_IOCTL_SYNC: around CPU access to a mapped dma-buf.
const DMA_BUF_IOCTL_SYNC: u64 = sys::ioc(sys::IOC_WRITE, b'b', 0, 8);
const SYNC_READ: u64 = 1;
const SYNC_END: u64 = 4;
// DMA_BUF_IOCTL_EXPORT_SYNC_FILE (Linux 6.0): the fence a reader has to wait for.
const DMA_BUF_IOCTL_EXPORT_SYNC_FILE: u64 = sys::ioc(sys::IOC_READ | sys::IOC_WRITE, b'b', 2, 8);

impl Dmabuf {
    /// The client's pending writes as a sync_file (to wait for on the GPU). None on old
    /// kernels: the driver's implicit sync is all there is then.
    pub fn read_fence(&self) -> Option<OwnedFd> {
        #[repr(C)]
        struct Export {
            flags: u32,
            fd: i32,
        }
        let mut e = Export { flags: SYNC_READ as u32, fd: -1 };
        let p = self.planes.first()?;
        unsafe {
            sys::ioctl_ptr(p.fd.as_raw_fd(), DMA_BUF_IOCTL_EXPORT_SYNC_FILE, &mut e).ok()?;
            use std::os::fd::FromRawFd;
            (e.fd >= 0).then(|| OwnedFd::from_raw_fd(e.fd))
        }
    }

    /// The pixels for the CPU renderer: linear ARGB/XRGB only (tiled layouts are the GPU's).
    pub fn with_pixels<R>(&self, f: impl FnOnce(&[u32], usize) -> R) -> Option<R> {
        if self.modifier != MOD_LINEAR || !(self.format == ARGB8888 || self.format == XRGB8888) || self.planes.len() != 1 {
            return None;
        }
        let p = &self.planes[0];
        if p.stride % 4 != 0 || p.offset % 4 != 0 {
            return None;
        }
        let len = p.offset as usize + p.stride as usize * self.h as usize;
        let mut map = self.map.borrow_mut();
        if map.is_none() {
            let size = dmabuf_size(&p.fd).map_or(len, |s| s as usize);
            if size < len {
                return None;
            }
            *map = Some(sys::Mmap::map(p.fd.as_fd(), size, false, true).ok()?);
        }
        let m = map.as_ref()?;
        let fd = p.fd.as_raw_fd();
        let mut flags = SYNC_READ;
        unsafe {
            let _ = sys::ioctl_ptr(fd, DMA_BUF_IOCTL_SYNC, &mut flags);
        }
        let px = unsafe { std::slice::from_raw_parts(m.ptr.add(p.offset as usize) as *const u32, (len - p.offset as usize) / 4) };
        let (r, _) = sys::with_sigbus_guard(m.ptr, m.len, || f(px, p.stride as usize / 4));
        let mut flags = SYNC_READ | SYNC_END;
        unsafe {
            let _ = sys::ioctl_ptr(fd, DMA_BUF_IOCTL_SYNC, &mut flags);
        }
        Some(r)
    }
}
