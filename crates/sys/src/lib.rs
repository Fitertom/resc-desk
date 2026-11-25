//! Thin Linux syscall layer for the things std does not expose:
//! fd passing (SCM_RIGHTS), epoll, mmap, memfd, timerfd, flock, SIGBUS guard.
//! No external crates; declarations follow glibc x86_64/aarch64 ABI.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::ffi::{c_void, CString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub mod ffi {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct iovec {
        pub iov_base: *mut c_void,
        pub iov_len: usize,
    }
    #[repr(C)]
    pub struct msghdr {
        pub msg_name: *mut c_void,
        pub msg_namelen: u32,
        pub msg_iov: *mut iovec,
        pub msg_iovlen: usize,
        pub msg_control: *mut c_void,
        pub msg_controllen: usize,
        pub msg_flags: i32,
    }
    #[repr(C)]
    pub struct cmsghdr {
        pub cmsg_len: usize,
        pub cmsg_level: i32,
        pub cmsg_type: i32,
    }
    #[cfg_attr(target_arch = "x86_64", repr(C, packed))]
    #[cfg_attr(not(target_arch = "x86_64"), repr(C))]
    #[derive(Clone, Copy)]
    pub struct epoll_event {
        pub events: u32,
        pub data: u64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct timespec {
        pub tv_sec: i64,
        pub tv_nsec: i64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct itimerspec {
        pub it_interval: timespec,
        pub it_value: timespec,
    }
    /// glibc `struct sigaction` (x86_64 / aarch64): handler, 128-byte mask, flags, restorer.
    #[repr(C)]
    pub struct sigaction_t {
        pub sa_sigaction: usize,
        pub sa_mask: [u64; 16],
        pub sa_flags: i32,
        pub sa_restorer: usize,
    }

    pub const SOL_SOCKET: i32 = 1;
    pub const SCM_RIGHTS: i32 = 1;
    pub const MSG_DONTWAIT: i32 = 0x40;
    pub const MSG_NOSIGNAL: i32 = 0x4000;
    pub const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
    pub const EPOLL_CLOEXEC: i32 = 0x80000;
    pub const EPOLL_CTL_ADD: i32 = 1;
    pub const EPOLL_CTL_DEL: i32 = 2;
    pub const EPOLL_CTL_MOD: i32 = 3;
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    pub const MAP_SHARED: i32 = 1;
    pub const MAP_PRIVATE: i32 = 2;
    pub const MAP_FIXED: i32 = 0x10;
    pub const MAP_ANONYMOUS: i32 = 0x20;
    pub const MFD_CLOEXEC: u32 = 1;
    pub const MFD_ALLOW_SEALING: u32 = 2;
    pub const CLOCK_MONOTONIC: i32 = 1;
    pub const TFD_NONBLOCK: i32 = 0o4000;
    pub const TFD_CLOEXEC: i32 = 0o2000000;
    pub const LOCK_EX: i32 = 2;
    pub const LOCK_NB: i32 = 4;
    pub const SIGBUS: i32 = 7;
    pub const SIGPIPE: i32 = 13;
    pub const SIG_IGN: usize = 1;
    pub const SIG_DFL: usize = 0;
    pub const SA_SIGINFO: i32 = 4;
    pub const SA_NODEFER: i32 = 0x4000_0000;
    pub const SA_RESTART: i32 = 0x1000_0000;
    pub const F_ADD_SEALS: i32 = 1033;
    pub const F_SEAL_SEAL: i32 = 1;
    pub const F_SEAL_SHRINK: i32 = 2;
    pub const F_SEAL_GROW: i32 = 4;

    extern "C" {
        pub fn sendmsg(fd: i32, msg: *const msghdr, flags: i32) -> isize;
        pub fn recvmsg(fd: i32, msg: *mut msghdr, flags: i32) -> isize;
        pub fn epoll_create1(flags: i32) -> i32;
        pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, ev: *mut epoll_event) -> i32;
        pub fn epoll_wait(epfd: i32, evs: *mut epoll_event, max: i32, timeout: i32) -> i32;
        pub fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> i32;
        pub fn memfd_create(name: *const i8, flags: u32) -> i32;
        pub fn ftruncate(fd: i32, len: i64) -> i32;
        pub fn timerfd_create(clock: i32, flags: i32) -> i32;
        pub fn timerfd_settime(fd: i32, flags: i32, new: *const itimerspec, old: *mut itimerspec) -> i32;
        pub fn clock_gettime(clk: i32, ts: *mut timespec) -> i32;
        pub fn flock(fd: i32, op: i32) -> i32;
        pub fn signal(sig: i32, handler: usize) -> usize;
        pub fn sigaction(sig: i32, act: *const sigaction_t, old: *mut sigaction_t) -> i32;
        pub fn read(fd: i32, buf: *mut c_void, n: usize) -> isize;
        pub fn write(fd: i32, buf: *const c_void, n: usize) -> isize;
        pub fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        pub fn pipe2(fds: *mut i32, flags: i32) -> i32;
        pub fn getuid() -> u32;
        pub fn ioctl(fd: i32, req: u64, ...) -> i32;
        pub fn sigprocmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
        pub fn signalfd(fd: i32, mask: *const [u64; 16], flags: i32) -> i32;
        pub fn socket(domain: i32, ty: i32, proto: i32) -> i32;
        pub fn bind(fd: i32, addr: *const c_void, len: u32) -> i32;
    }
}

/// The kernel's uevents (what udev listens to): device added, removed, changed — a monitor
/// plugged in (SUBSYSTEM=drm, HOTPLUG=1), a keyboard (SUBSYSTEM=input). Non-blocking.
pub fn uevent_socket() -> io::Result<OwnedFd> {
    // AF_NETLINK, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, NETLINK_KOBJECT_UEVENT.
    let fd = cvt(unsafe { ffi::socket(16, 2 | 0o4000 | 0o2000000, 15) })?;
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // sockaddr_nl: family, pad, pid (0: the kernel picks), groups (1: kernel events).
    #[repr(C)]
    struct SockaddrNl {
        family: u16,
        pad: u16,
        pid: u32,
        groups: u32,
    }
    let a = SockaddrNl { family: 16, pad: 0, pid: 0, groups: 1 };
    cvt(unsafe { ffi::bind(fd.as_raw_fd(), &a as *const _ as *const c_void, 12) })?;
    Ok(fd)
}

/// One uevent from `uevent_socket`, as KEY=VALUE pairs (ACTION, SUBSYSTEM, DEVNAME, ...).
pub fn read_uevent(fd: BorrowedFd<'_>) -> Option<Vec<(String, String)>> {
    let mut buf = [0u8; 8192];
    let n = unsafe { ffi::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut c_void, buf.len()) };
    if n <= 0 {
        return None;
    }
    Some(
        buf[..n as usize]
            .split(|b| *b == 0)
            .filter_map(|f| std::str::from_utf8(f).ok()?.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

// ---------------------------------------------------------------- ioctl, signals

/// ioctl with a pointer argument (DRM, evdev, VT structs).
pub unsafe fn ioctl_ptr<T>(fd: RawFd, req: u64, arg: *mut T) -> io::Result<i32> {
    loop {
        let r = ffi::ioctl(fd, req, arg as *mut c_void);
        if r >= 0 {
            return Ok(r);
        }
        let e = io::Error::last_os_error();
        // EINTR / EAGAIN: libdrm retries both.
        if !matches!(e.raw_os_error(), Some(4) | Some(11)) {
            return Err(e);
        }
    }
}

/// ioctl with an integer argument (KDSETMODE, VT_ACTIVATE, ...).
pub fn ioctl_int(fd: RawFd, req: u64, arg: usize) -> io::Result<i32> {
    cvt(unsafe { ffi::ioctl(fd, req, arg) })
}

/// `_IOC(dir, type, nr, size)` as the kernel's ioctl.h builds it.
pub const fn ioc(dir: u64, ty: u8, nr: u8, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64
}
pub const IOC_WRITE: u64 = 1;
pub const IOC_READ: u64 = 2;

/// Signals delivered through a file descriptor in the event loop (they are blocked for
/// the process; children start with them unblocked, std resets the mask in the child).
pub struct SignalFd {
    pub fd: OwnedFd,
}

impl SignalFd {
    pub fn new(signals: &[i32]) -> io::Result<Self> {
        let mut set = [0u64; 16];
        for s in signals {
            let b = (*s - 1) as usize;
            set[b / 64] |= 1 << (b % 64);
        }
        cvt(unsafe { ffi::sigprocmask(0, &set, std::ptr::null_mut()) })?;
        let fd = cvt(unsafe { ffi::signalfd(-1, &set, 0o4000 | 0o2000000) })?;
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// The next pending signal's number, if any.
    pub fn read(&self) -> Option<u32> {
        let mut info = [0u8; 128];
        let n = unsafe { ffi::read(self.fd.as_raw_fd(), info.as_mut_ptr() as *mut c_void, 128) };
        (n == 128).then(|| u32::from_ne_bytes([info[0], info[1], info[2], info[3]]))
    }
}

pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGUSR1: i32 = 10;
pub const SIGUSR2: i32 = 12;
pub const SIGTERM: i32 = 15;

fn cvt(r: i32) -> io::Result<i32> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

// ---------------------------------------------------------------- fd passing

/// Max fds per sendmsg, same limit libwayland uses on the receive side.
pub const MAX_FDS: usize = 28;

/// Non-blocking send of `data` with `fds` attached. Returns bytes written.
pub fn send_with_fds(fd: BorrowedFd<'_>, data: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let mut iov = ffi::iovec { iov_base: data.as_ptr() as *mut c_void, iov_len: data.len() };
    let mut cbuf = [0u64; 20]; // 160 bytes >= CMSG_SPACE(28*4)=128
    let mut msg = ffi::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: std::ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0,
    };
    if !fds.is_empty() {
        assert!(fds.len() <= MAX_FDS);
        let hdr = std::mem::size_of::<ffi::cmsghdr>();
        let payload = fds.len() * 4;
        let space = hdr + ((payload + 7) & !7);
        unsafe {
            let c = cbuf.as_mut_ptr() as *mut ffi::cmsghdr;
            (*c).cmsg_len = hdr + payload;
            (*c).cmsg_level = ffi::SOL_SOCKET;
            (*c).cmsg_type = ffi::SCM_RIGHTS;
            std::ptr::copy_nonoverlapping(fds.as_ptr() as *const u8, (c as *mut u8).add(hdr), payload);
        }
        msg.msg_control = cbuf.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = space;
    }
    let r = unsafe { ffi::sendmsg(fd.as_raw_fd(), &msg, ffi::MSG_DONTWAIT | ffi::MSG_NOSIGNAL) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r as usize) }
}

/// Non-blocking receive; received fds are appended to `fds_out` (CLOEXEC).
pub fn recv_with_fds(fd: BorrowedFd<'_>, buf: &mut [u8], fds_out: &mut impl Extend<OwnedFd>) -> io::Result<usize> {
    let mut iov = ffi::iovec { iov_base: buf.as_mut_ptr() as *mut c_void, iov_len: buf.len() };
    let mut cbuf = [0u64; 40];
    let mut msg = ffi::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: cbuf.as_mut_ptr() as *mut c_void,
        msg_controllen: std::mem::size_of_val(&cbuf),
        msg_flags: 0,
    };
    let r = unsafe { ffi::recvmsg(fd.as_raw_fd(), &mut msg, ffi::MSG_DONTWAIT | ffi::MSG_CMSG_CLOEXEC) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    // Walk the control messages.
    let hdr = std::mem::size_of::<ffi::cmsghdr>();
    let base = cbuf.as_ptr() as *const u8;
    let mut off = 0usize;
    while off + hdr <= msg.msg_controllen {
        unsafe {
            let c = base.add(off) as *const ffi::cmsghdr;
            let len = (*c).cmsg_len;
            if len < hdr || off + len > msg.msg_controllen {
                break;
            }
            if (*c).cmsg_level == ffi::SOL_SOCKET && (*c).cmsg_type == ffi::SCM_RIGHTS {
                let n = (len - hdr) / 4;
                let p = base.add(off + hdr) as *const i32;
                for i in 0..n {
                    fds_out.extend(Some(OwnedFd::from_raw_fd(p.add(i).read_unaligned())));
                }
            }
            off += (len + 7) & !7;
        }
    }
    Ok(r as usize)
}

// ---------------------------------------------------------------- epoll

pub const EPOLLIN: u32 = 0x1;
pub const EPOLLOUT: u32 = 0x4;
pub const EPOLLERR: u32 = 0x8;
pub const EPOLLHUP: u32 = 0x10;

pub struct Epoll {
    fd: OwnedFd,
    buf: Vec<ffi::epoll_event>,
}

impl Epoll {
    pub fn new() -> io::Result<Self> {
        let fd = cvt(unsafe { ffi::epoll_create1(ffi::EPOLL_CLOEXEC) })?;
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) }, buf: vec![ffi::epoll_event { events: 0, data: 0 }; 64] })
    }
    fn ctl(&self, op: i32, fd: RawFd, token: u64, events: u32) -> io::Result<()> {
        let mut ev = ffi::epoll_event { events, data: token };
        cvt(unsafe { ffi::epoll_ctl(self.fd.as_raw_fd(), op, fd, &mut ev) }).map(|_| ())
    }
    pub fn add(&self, fd: RawFd, token: u64, events: u32) -> io::Result<()> {
        self.ctl(ffi::EPOLL_CTL_ADD, fd, token, events)
    }
    pub fn modify(&self, fd: RawFd, token: u64, events: u32) -> io::Result<()> {
        self.ctl(ffi::EPOLL_CTL_MOD, fd, token, events)
    }
    pub fn remove(&self, fd: RawFd) {
        let _ = self.ctl(ffi::EPOLL_CTL_DEL, fd, 0, 0);
    }
    /// Waits up to `timeout_ms` (-1 = forever) and fills `out` with (token, events).
    pub fn wait(&mut self, timeout_ms: i32, out: &mut Vec<(u64, u32)>) -> io::Result<()> {
        out.clear();
        let n = unsafe { ffi::epoll_wait(self.fd.as_raw_fd(), self.buf.as_mut_ptr(), self.buf.len() as i32, timeout_ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(e);
        }
        for ev in &self.buf[..n as usize] {
            let ev = *ev;
            out.push((ev.data, ev.events));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- memory

/// A mapped region; unmapped on drop.
pub struct Mmap {
    pub ptr: *mut u8,
    pub len: usize,
}

impl Mmap {
    pub fn map(fd: BorrowedFd<'_>, len: usize, writable: bool, shared: bool) -> io::Result<Self> {
        Self::map_at(fd, len, 0, writable, shared)
    }

    /// At `offset` into the file (a DRM dumb buffer's fake offset from MAP_DUMB).
    pub fn map_at(fd: BorrowedFd<'_>, len: usize, offset: u64, writable: bool, shared: bool) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let prot = ffi::PROT_READ | if writable { ffi::PROT_WRITE } else { 0 };
        let flags = if shared { ffi::MAP_SHARED } else { ffi::MAP_PRIVATE };
        let p = unsafe { ffi::mmap(std::ptr::null_mut(), len, prot, flags, fd.as_raw_fd(), offset as i64) };
        if p as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr: p as *mut u8, len })
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        unsafe { ffi::munmap(self.ptr as *mut c_void, self.len) };
    }
}

/// Anonymous shared memory file. Falls back to an unlinked temp file where
/// memfd_create is missing (WSL1, very old kernels).
pub fn memfd(name: &str, size: usize) -> io::Result<OwnedFd> {
    let c = CString::new(name).unwrap();
    let r = unsafe { ffi::memfd_create(c.as_ptr(), ffi::MFD_CLOEXEC | ffi::MFD_ALLOW_SEALING) };
    let fd = if r >= 0 {
        unsafe { OwnedFd::from_raw_fd(r) }
    } else {
        let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let mut n = 0u32;
        loop {
            let path = format!("{dir}/.{name}-{}-{n}", std::process::id());
            match std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
                Ok(f) => {
                    let _ = std::fs::remove_file(&path);
                    break OwnedFd::from(f);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                Err(e) => return Err(e),
            }
        }
    };
    cvt(unsafe { ffi::ftruncate(fd.as_raw_fd(), size as i64) })?;
    Ok(fd)
}

/// Seal a memfd so a client cannot shrink it under us (used for keymaps).
pub fn seal_readonly(fd: BorrowedFd<'_>) {
    unsafe {
        ffi::fcntl(fd.as_raw_fd(), ffi::F_ADD_SEALS, ffi::F_SEAL_SHRINK | ffi::F_SEAL_GROW | ffi::F_SEAL_SEAL);
    }
}

/// A pipe: (read end, non-blocking; write end, blocking). Both close-on-exec.
pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    const O_CLOEXEC: i32 = 0o2000000;
    const O_NONBLOCK: i32 = 0o4000;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    let mut fds = [0i32; 2];
    cvt(unsafe { ffi::pipe2(fds.as_mut_ptr(), O_CLOEXEC) })?;
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    unsafe {
        let fl = ffi::fcntl(r.as_raw_fd(), F_GETFL);
        ffi::fcntl(r.as_raw_fd(), F_SETFL, fl | O_NONBLOCK);
    }
    Ok((r, w))
}

pub fn write_all(fd: BorrowedFd<'_>, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let n = unsafe { ffi::write(fd.as_raw_fd(), data.as_ptr() as *const c_void, data.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        data = &data[n as usize..];
    }
    Ok(())
}

// ---------------------------------------------------------------- time

pub fn now_ns() -> u64 {
    let mut ts = ffi::timespec::default();
    unsafe { ffi::clock_gettime(ffi::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn now_ms() -> u32 {
    (now_ns() / 1_000_000) as u32
}

pub struct TimerFd {
    pub fd: OwnedFd,
}

impl TimerFd {
    pub fn new() -> io::Result<Self> {
        let fd = cvt(unsafe { ffi::timerfd_create(ffi::CLOCK_MONOTONIC, ffi::TFD_NONBLOCK | ffi::TFD_CLOEXEC) })?;
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }
    /// Arm: first expiry after `first_ns`, then every `interval_ns` (0 = one-shot).
    pub fn arm(&self, first_ns: u64, interval_ns: u64) {
        let ts = |ns: u64| ffi::timespec { tv_sec: (ns / 1_000_000_000) as i64, tv_nsec: (ns % 1_000_000_000) as i64 };
        let spec = ffi::itimerspec { it_interval: ts(interval_ns), it_value: ts(first_ns.max(1)) };
        unsafe { ffi::timerfd_settime(self.fd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
    }
    pub fn disarm(&self) {
        let spec = ffi::itimerspec::default();
        unsafe { ffi::timerfd_settime(self.fd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
    }
    /// Returns the number of expirations since last read (0 if none).
    pub fn read(&self) -> u64 {
        let mut v = 0u64;
        let n = unsafe { ffi::read(self.fd.as_raw_fd(), &mut v as *mut u64 as *mut c_void, 8) };
        if n == 8 { v } else { 0 }
    }
}

// ---------------------------------------------------------------- misc

pub fn ignore_sigpipe() {
    unsafe { ffi::signal(ffi::SIGPIPE, ffi::SIG_IGN) };
}

/// flock(LOCK_EX|LOCK_NB); Ok(false) when someone else holds it.
pub fn try_lock(fd: BorrowedFd<'_>) -> bool {
    unsafe { ffi::flock(fd.as_raw_fd(), ffi::LOCK_EX | ffi::LOCK_NB) == 0 }
}

pub fn uid() -> u32 {
    unsafe { ffi::getuid() }
}

/// Sets or clears FD_CLOEXEC (a child process that should inherit `fd` needs it clear).
/// Async-signal-safe: usable between fork and exec.
pub fn set_cloexec(fd: RawFd, on: bool) {
    const F_SETFD: i32 = 2;
    unsafe { ffi::fcntl(fd, F_SETFD, if on { 1 } else { 0 }) };
}

// ---------------------------------------------------------------- SIGBUS guard
//
// A client may truncate its shm file while we read the mapping; the read then
// raises SIGBUS. Like libwayland we catch it, map zero pages over the guarded
// range and flag the buffer as broken instead of crashing the session.

static GUARD_START: AtomicUsize = AtomicUsize::new(0);
static GUARD_LEN: AtomicUsize = AtomicUsize::new(0);
static GUARD_HIT: AtomicBool = AtomicBool::new(false);
static GUARD_INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigbus_handler(sig: i32, info: *mut u8, _ctx: *mut c_void) {
    // siginfo_t.si_addr is at offset 16 on 64-bit Linux.
    let addr = unsafe { (info.add(16) as *const usize).read() };
    let start = GUARD_START.load(Ordering::Relaxed);
    let len = GUARD_LEN.load(Ordering::Relaxed);
    if len != 0 && addr >= start && addr < start + len {
        unsafe {
            ffi::mmap(start as *mut c_void, len, ffi::PROT_READ | ffi::PROT_WRITE,
                      ffi::MAP_FIXED | ffi::MAP_PRIVATE | ffi::MAP_ANONYMOUS, -1, 0);
        }
        GUARD_HIT.store(true, Ordering::Relaxed);
        return;
    }
    // Not ours: restore default action; returning re-executes and crashes normally.
    unsafe { ffi::signal(sig, ffi::SIG_DFL) };
}

fn install_sigbus() {
    if GUARD_INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }
    let act = ffi::sigaction_t {
        sa_sigaction: sigbus_handler as usize,
        sa_mask: [0; 16],
        sa_flags: ffi::SA_SIGINFO | ffi::SA_NODEFER | ffi::SA_RESTART,
        sa_restorer: 0,
    };
    unsafe { ffi::sigaction(ffi::SIGBUS, &act, std::ptr::null_mut()) };
}

/// Runs `f` with SIGBUS protection for `[ptr, ptr+len)`. Returns (result, faulted).
pub fn with_sigbus_guard<R>(ptr: *const u8, len: usize, f: impl FnOnce() -> R) -> (R, bool) {
    install_sigbus();
    GUARD_HIT.store(false, Ordering::Relaxed);
    GUARD_START.store(ptr as usize, Ordering::Relaxed);
    GUARD_LEN.store(len, Ordering::Relaxed);
    let r = f();
    GUARD_LEN.store(0, Ordering::Relaxed);
    (r, GUARD_HIT.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn uevent_fields() {
        // A monitor plugged in, as the kernel sends it (NUL-separated, header first).
        let (r, w) = pipe().unwrap();
        let msg = b"change@/devices/pci0000:00/0000:00:02.0/drm/card0\0ACTION=change\0DEVPATH=/devices/pci0000:00/0000:00:02.0/drm/card0\0SUBSYSTEM=drm\0HOTPLUG=1\0DEVNAME=dri/card0\0SEQNUM=4242\0";
        write_all(w.as_fd(), msg).unwrap();
        let ev = read_uevent(r.as_fd()).unwrap();
        let get = |k: &str| ev.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("SUBSYSTEM"), Some("drm"));
        assert_eq!(get("HOTPLUG"), Some("1"));
        assert_eq!(get("DEVNAME"), Some("dri/card0"));
        assert_eq!(get("ACTION"), Some("change"));
    }
}
