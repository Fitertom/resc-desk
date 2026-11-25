//! The virtual terminal we run on: the text console stops drawing and reading the keyboard
//! while we are there, and VT switching (Ctrl+Alt+F<n>) is ours to do: the kernel asks with
//! SIGUSR1 before leaving our VT and tells with SIGUSR2 on coming back (VT_PROCESS).
//! Whatever happens (exit, panic, SIGTERM), the console is put back into text mode.
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

const KDSETMODE: u64 = 0x4B3A;
const KDGKBMODE: u64 = 0x4B44;
const KDSKBMODE: u64 = 0x4B45;
const KD_TEXT: usize = 0;
const KD_GRAPHICS: usize = 1;
const K_OFF: usize = 4;
const VT_SETMODE: u64 = 0x5602;
const VT_GETSTATE: u64 = 0x5603;
const VT_RELDISP: u64 = 0x5605;
const VT_ACTIVATE: u64 = 0x5606;
const VT_AUTO: i8 = 0;
const VT_PROCESS: i8 = 1;
const VT_ACKACQ: usize = 2;

#[repr(C)]
struct VtMode {
    mode: i8,
    waitv: i8,
    relsig: i16,
    acqsig: i16,
    frsig: i16,
}

/// The tty and its keyboard mode before us, for the restore in a panic hook.
static TTY: AtomicI32 = AtomicI32::new(-1);
static KB_MODE: AtomicI32 = AtomicI32::new(-1);

pub struct Vt {
    tty: OwnedFd,
    pub num: u16,
}

fn tty_number(path: &str) -> Option<u16> {
    path.strip_prefix("/dev/tty")?.parse().ok().filter(|n| *n > 0)
}

impl Vt {
    /// The VT we were started on (stdin), else the active one.
    pub fn open() -> std::io::Result<Vt> {
        use std::os::unix::fs::OpenOptionsExt;
        let stdin = std::fs::read_link("/proc/self/fd/0").ok().map(|p| p.to_string_lossy().into_owned());
        let num = match stdin.as_deref().and_then(tty_number) {
            Some(n) => n,
            None => {
                let t0 = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty0")?;
                let mut st = [0u16; 3];
                unsafe { sys::ioctl_ptr(t0.as_raw_fd(), VT_GETSTATE, st.as_mut_ptr())? };
                st[0]
            }
        };
        let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(0o2000000).open(format!("/dev/tty{num}"))?;
        let tty: OwnedFd = f.into();
        let raw = tty.as_raw_fd();
        let mut kb: i32 = 0;
        unsafe { sys::ioctl_ptr(raw, KDGKBMODE, &mut kb)? };
        TTY.store(raw, Ordering::Relaxed);
        KB_MODE.store(kb, Ordering::Relaxed);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_console();
            prev(info);
        }));
        sys::ioctl_int(raw, KDSKBMODE, K_OFF)?;
        sys::ioctl_int(raw, KDSETMODE, KD_GRAPHICS)?;
        let mut m = VtMode { mode: VT_PROCESS, waitv: 0, relsig: sys::SIGUSR1 as i16, acqsig: sys::SIGUSR2 as i16, frsig: 0 };
        unsafe { sys::ioctl_ptr(raw, VT_SETMODE, &mut m)? };
        Ok(Vt { tty, num })
    }

    fn raw(&self) -> RawFd {
        self.tty.as_raw_fd()
    }

    /// The kernel wants to leave our VT: allowed (after the caller let go of the devices).
    pub fn allow_release(&self) {
        let _ = sys::ioctl_int(self.raw(), VT_RELDISP, 1);
    }

    /// Back on our VT: acknowledged.
    pub fn ack_acquire(&self) {
        let _ = sys::ioctl_int(self.raw(), VT_RELDISP, VT_ACKACQ);
    }

    pub fn activate(&self, n: u16) {
        if n != self.num {
            let _ = sys::ioctl_int(self.raw(), VT_ACTIVATE, n as usize);
        }
    }
}

/// Text mode, the keyboard as it was, automatic VT switching: the console is usable again.
pub fn restore_console() {
    let raw = TTY.swap(-1, Ordering::Relaxed);
    if raw < 0 {
        return;
    }
    let mut m = VtMode { mode: VT_AUTO, waitv: 0, relsig: 0, acqsig: 0, frsig: 0 };
    unsafe {
        let _ = sys::ioctl_ptr(raw, VT_SETMODE, &mut m);
    }
    let kb = KB_MODE.load(Ordering::Relaxed);
    let _ = sys::ioctl_int(raw, KDSKBMODE, if kb >= 0 { kb as usize } else { 3 });
    let _ = sys::ioctl_int(raw, KDSETMODE, KD_TEXT);
}

impl Drop for Vt {
    fn drop(&mut self) {
        restore_console();
    }
}
