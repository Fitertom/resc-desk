//! wl_shm pools and buffers. Pixels are read in place (zero copy) under a SIGBUS guard.
use std::cell::{Cell, RefCell};
use std::os::fd::{AsFd, OwnedFd};

pub const FMT_ARGB8888: u32 = 0;
pub const FMT_XRGB8888: u32 = 1;

pub struct Pool {
    fd: OwnedFd,
    map: RefCell<Option<sys::Mmap>>,
    size: Cell<usize>,
}

impl Pool {
    pub fn new(fd: OwnedFd, size: usize) -> std::io::Result<Self> {
        let m = sys::Mmap::map(fd.as_fd(), size, false, true)?;
        Ok(Self { fd, map: RefCell::new(Some(m)), size: Cell::new(size) })
    }
    pub fn resize(&self, size: usize) -> std::io::Result<()> {
        if size <= self.size.get() {
            return Ok(()); // pools may only grow
        }
        let m = sys::Mmap::map(self.fd.as_fd(), size, false, true)?;
        *self.map.borrow_mut() = Some(m);
        self.size.set(size);
        Ok(())
    }
    pub fn size(&self) -> usize {
        self.size.get()
    }
}

pub struct Buffer {
    pub pool: Option<std::rc::Rc<Pool>>,
    pub offset: usize,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub format: u32,
    pub broken: Cell<bool>,
    /// Solid color buffers (wp_single_pixel_buffer) have no pool memory.
    pub solid: Option<u32>,
    /// A GPU buffer (linux-dmabuf): the GPU samples it in place, the CPU only reads linear ones.
    pub dmabuf: Option<crate::dmabuf::Dmabuf>,
}

impl Buffer {
    pub fn opaque_format(&self) -> bool {
        match self.solid {
            Some(c) => c >> 24 == 0xff,
            None => self.format == FMT_XRGB8888,
        }
    }
    pub fn valid_for_pool(&self) -> bool {
        let Some(pool) = &self.pool else { return true };
        let need = self.offset as u64 + self.stride as u64 * self.h as u64;
        self.w > 0 && self.h > 0 && self.stride >= self.w * 4 && need <= pool.size() as u64
    }
    /// Calls `f(pixels_u32, stride_px)` with the buffer content. Returns None if broken.
    pub fn with_pixels<R>(&self, f: impl FnOnce(&[u32], usize) -> R) -> Option<R> {
        if let Some(dm) = &self.dmabuf {
            return dm.with_pixels(f);
        }
        if self.broken.get() || self.stride % 4 != 0 {
            return None;
        }
        let pool = self.pool.as_ref()?;
        let map = pool.map.borrow();
        let m = map.as_ref()?;
        let len = self.stride as usize * self.h as usize;
        if self.offset + len > m.len || self.offset % 4 != 0 {
            return None;
        }
        let px = unsafe { std::slice::from_raw_parts(m.ptr.add(self.offset) as *const u32, len / 4) };
        let (r, faulted) = sys::with_sigbus_guard(m.ptr, m.len, || f(px, self.stride as usize / 4));
        if faulted {
            self.broken.set(true);
        }
        Some(r)
    }
}
