//! The X11 wire protocol, the part a window manager needs (X11R7 protocol spec, little
//! endian): connection setup, requests, and replies, errors and events read back. No xcb.
//!
//! Requests are queued and flushed by the event loop. A request with a reply gets a tag;
//! when its reply (or error) comes, it is handed back with that tag, in order. Nothing
//! waits, not even the setup: the window manager must never block on Xwayland, which is
//! our Wayland client and may itself be waiting for us.
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

pub type Xid = u32;
pub type Atom = u32;

// Predefined atoms.
pub const PRIMARY: Atom = 1;
pub const ATOM: Atom = 4;
pub const CARDINAL: Atom = 6;
pub const STRING: Atom = 31;
pub const WINDOW: Atom = 33;
pub const WM_HINTS: Atom = 35;
pub const WM_NAME: Atom = 39;
pub const WM_NORMAL_HINTS: Atom = 40;
pub const WM_CLASS: Atom = 67;
pub const WM_TRANSIENT_FOR: Atom = 68;

// Event masks.
pub const EM_STRUCTURE_NOTIFY: u32 = 0x2_0000;
pub const EM_SUBSTRUCTURE_NOTIFY: u32 = 0x8_0000;
pub const EM_SUBSTRUCTURE_REDIRECT: u32 = 0x10_0000;
pub const EM_FOCUS_CHANGE: u32 = 0x20_0000;
pub const EM_PROPERTY_CHANGE: u32 = 0x40_0000;

// Event codes.
pub const CREATE_NOTIFY: u8 = 16;
pub const DESTROY_NOTIFY: u8 = 17;
pub const UNMAP_NOTIFY: u8 = 18;
pub const MAP_REQUEST: u8 = 20;
pub const CONFIGURE_NOTIFY: u8 = 22;
pub const CONFIGURE_REQUEST: u8 = 23;
pub const PROPERTY_NOTIFY: u8 = 28;
pub const SELECTION_REQUEST: u8 = 30;
pub const SELECTION_NOTIFY: u8 = 31;
pub const CLIENT_MESSAGE: u8 = 33;

// ConfigureWindow value mask.
pub const CW_X: u16 = 1;
pub const CW_Y: u16 = 2;
pub const CW_WIDTH: u16 = 4;
pub const CW_HEIGHT: u16 = 8;
pub const CW_BORDER: u16 = 16;
pub const CW_STACK_MODE: u16 = 64;

pub fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
pub fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// What the server said about itself.
#[derive(Default, Clone)]
pub struct Setup {
    pub id_base: u32,
    pub id_mask: u32,
    pub root: Xid,
    /// The longest request the server takes, bytes.
    pub max_request: usize,
}

/// Something read from the server.
pub enum Msg<T> {
    /// A reply to the request tagged `T`: its whole bytes (32 + extra).
    Reply(T, Vec<u8>),
    /// An error for a request: its tag if it had one, the error code, the request's opcodes.
    Error(Option<T>, u8, u8, u16),
    /// An event (32 bytes; the code has the "sent" bit cleared).
    Event(u8, [u8; 32]),
}

pub struct Conn<T> {
    stream: UnixStream,
    out: Vec<u8>,
    inb: Vec<u8>,
    /// Requests sent so far (the server's sequence numbers count them).
    seq: u64,
    /// Requests waiting for a reply: their full sequence number and tag.
    waiting: VecDeque<(u64, T)>,
    next_id: u32,
    pub setup: Setup,
}

impl<T> Conn<T> {
    /// Queues the setup request; `try_setup` reads the answer.
    pub fn new(fd: OwnedFd) -> std::io::Result<Self> {
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(true)?;
        let mut c = Conn { stream, out: Vec::new(), inb: Vec::new(), seq: 0, waiting: VecDeque::new(), next_id: 0, setup: Setup::default() };
        // 'l': little endian, protocol 11.0, no authorization (the WM socket is trusted).
        c.out.extend_from_slice(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        Ok(c)
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// Reads the setup reply if it is all there. Ok(false): not yet (call again when the
    /// fd is readable). Never blocks: Xwayland talks to us as a Wayland client while it starts.
    pub fn try_setup(&mut self) -> std::io::Result<bool> {
        self.flush()?;
        self.fill()?;
        if self.inb.len() < 8 {
            return Ok(false);
        }
        let need = 8 + u16_at(&self.inb, 6) as usize * 4;
        if self.inb.len() < need {
            return Ok(false);
        }
        let b: Vec<u8> = self.inb.drain(..need).collect();
        if b[0] != 1 {
            let n = b[1] as usize;
            let why = String::from_utf8_lossy(&b[8..(8 + n).min(b.len())]).into_owned();
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("X setup refused: {why}")));
        }
        let vendor_len = u16_at(&b, 24) as usize;
        let nformats = b[29] as usize;
        let screen = 40 + pad4(vendor_len) + nformats * 8;
        self.setup = Setup {
            id_base: u32_at(&b, 12),
            id_mask: u32_at(&b, 16),
            root: u32_at(&b, screen),
            max_request: u16_at(&b, 26) as usize * 4,
        };
        Ok(true)
    }

    /// A new resource id (window, ...).
    pub fn gen_id(&mut self) -> Xid {
        let m = self.setup.id_mask;
        let low = m & m.wrapping_neg();
        self.next_id += 1;
        self.setup.id_base | ((self.next_id * low) & m)
    }

    fn fill(&mut self) -> std::io::Result<usize> {
        let mut buf = [0u8; 16384];
        let mut total = 0;
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    if total == 0 {
                        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "X server closed the connection"));
                    }
                    return Ok(total);
                }
                Ok(n) => {
                    self.inb.extend_from_slice(&buf[..n]);
                    total += n;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(total),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Writes what the socket takes now; the rest waits for the next call (the event loop
    /// asks for EPOLLOUT while `pending_out` is not 0).
    pub fn flush(&mut self) -> std::io::Result<()> {
        while !self.out.is_empty() {
            match self.stream.write(&self.out) {
                Ok(n) => {
                    self.out.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if self.out.len() > 16 << 20 {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, "X server not reading"));
                    }
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn pending_out(&self) -> usize {
        self.out.len()
    }

    /// Everything that has arrived, decoded.
    pub fn read(&mut self) -> std::io::Result<Vec<Msg<T>>> {
        self.fill()?;
        let mut out = Vec::new();
        loop {
            if self.inb.len() < 32 {
                break;
            }
            let kind = self.inb[0];
            let len = if kind == 1 { 32 + u32_at(&self.inb, 4) as usize * 4 } else { 32 };
            if self.inb.len() < len {
                break;
            }
            let b: Vec<u8> = self.inb.drain(..len).collect();
            let seq16 = u16_at(&b, 2);
            match kind {
                0 => {
                    let tag = self.take_waiting(seq16, true);
                    out.push(Msg::Error(tag, b[1], b[10], u16_at(&b, 8)));
                }
                1 => {
                    if let Some(tag) = self.take_waiting(seq16, true) {
                        out.push(Msg::Reply(tag, b));
                    }
                }
                _ => {
                    // Sequence numbers before this event's have all been answered.
                    let _ = self.take_waiting(seq16, false);
                    let mut e = [0u8; 32];
                    e.copy_from_slice(&b);
                    out.push(Msg::Event(kind & 0x7f, e));
                }
            }
        }
        Ok(out)
    }

    /// The tag of the request with this sequence number (when `exact`), dropping any
    /// earlier ones that will get no answer any more.
    fn take_waiting(&mut self, seq16: u16, exact: bool) -> Option<T> {
        while let Some(&(s, _)) = self.waiting.front() {
            let lo = s as u16;
            let behind = seq16.wrapping_sub(lo) as i16;
            if behind > 0 {
                self.waiting.pop_front();
                continue;
            }
            if behind == 0 && exact {
                return self.waiting.pop_front().map(|(_, t)| t);
            }
            break;
        }
        None
    }

    // ---------------------------------------------------------------- requests

    fn req(&mut self, opcode: u8, data: u8, body: &[u8]) {
        let len = (4 + pad4(body.len())) / 4;
        self.out.push(opcode);
        self.out.push(data);
        self.out.extend_from_slice(&(len as u16).to_le_bytes());
        self.out.extend_from_slice(body);
        self.out.resize(self.out.len() + pad4(body.len()) - body.len(), 0);
        self.seq += 1;
    }

    fn req_reply(&mut self, opcode: u8, data: u8, body: &[u8], tag: T) {
        self.req(opcode, data, body);
        self.waiting.push_back((self.seq, tag));
    }

    pub fn create_window(&mut self, wid: Xid, parent: Xid, x: i16, y: i16, w: u16, h: u16, override_redirect: bool) {
        let mut b = Vec::with_capacity(36);
        b.extend_from_slice(&wid.to_le_bytes());
        b.extend_from_slice(&parent.to_le_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // border
        b.extend_from_slice(&1u16.to_le_bytes()); // InputOutput
        b.extend_from_slice(&0u32.to_le_bytes()); // CopyFromParent visual
        b.extend_from_slice(&0x200u32.to_le_bytes()); // CWOverrideRedirect
        b.extend_from_slice(&(override_redirect as u32).to_le_bytes());
        self.req(1, 0, &b);
    }

    pub fn select_input(&mut self, w: Xid, mask: u32) {
        let mut b = Vec::with_capacity(12);
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&0x800u32.to_le_bytes()); // CWEventMask
        b.extend_from_slice(&mask.to_le_bytes());
        self.req(2, 0, &b);
    }

    pub fn get_window_attributes(&mut self, w: Xid, tag: T) {
        self.req_reply(3, 0, &w.to_le_bytes(), tag);
    }

    /// GetGeometry: x, y (i16 at 12, 14), width, height (u16 at 16, 18).
    pub fn get_geometry(&mut self, w: Xid, tag: T) {
        self.req_reply(14, 0, &w.to_le_bytes(), tag);
    }

    pub fn map_window(&mut self, w: Xid) {
        self.req(8, 0, &w.to_le_bytes());
    }

    /// ConfigureWindow: `values` in the order of the mask's bits.
    pub fn configure_window(&mut self, w: Xid, mask: u16, values: &[u32]) {
        let mut b = Vec::with_capacity(8 + values.len() * 4);
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        for v in values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.req(12, 0, &b);
    }

    pub fn query_tree(&mut self, w: Xid, tag: T) {
        self.req_reply(15, 0, &w.to_le_bytes(), tag);
    }

    pub fn intern_atom(&mut self, name: &str, tag: T) {
        let mut b = Vec::with_capacity(4 + name.len());
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(name.as_bytes());
        self.req_reply(16, 0, &b, tag);
    }

    /// ChangeProperty, replace mode. `format` 8, 16 or 32; `data` already in that format.
    pub fn change_property(&mut self, w: Xid, prop: Atom, ty: Atom, format: u8, data: &[u8]) {
        let n = data.len() / (format as usize / 8);
        let mut b = Vec::with_capacity(20 + data.len());
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&prop.to_le_bytes());
        b.extend_from_slice(&ty.to_le_bytes());
        b.push(format);
        b.extend_from_slice(&[0, 0, 0]);
        b.extend_from_slice(&(n as u32).to_le_bytes());
        b.extend_from_slice(data);
        self.req(18, 0, &b);
    }

    pub fn change_property32(&mut self, w: Xid, prop: Atom, ty: Atom, vals: &[u32]) {
        let data: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.change_property(w, prop, ty, 32, &data);
    }

    /// GetProperty of any type, all of it.
    pub fn get_property(&mut self, w: Xid, prop: Atom, tag: T) {
        self.get_property_ex(w, prop, false, tag);
    }

    /// GetProperty, deleting the property once it is read (selection transfers).
    pub fn get_property_ex(&mut self, w: Xid, prop: Atom, delete: bool, tag: T) {
        let mut b = Vec::with_capacity(20);
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&prop.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // AnyPropertyType
        b.extend_from_slice(&0u32.to_le_bytes()); // offset
        b.extend_from_slice(&(16u32 << 20).to_le_bytes()); // length, 4-byte units: all of it
        self.req_reply(20, delete as u8, &b, tag);
    }

    pub fn set_selection_owner(&mut self, owner: Xid, selection: Atom) {
        let mut b = Vec::with_capacity(12);
        b.extend_from_slice(&owner.to_le_bytes());
        b.extend_from_slice(&selection.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        self.req(22, 0, &b);
    }

    /// GetAtomName: the name's length (u16 at 8) and the name from byte 32.
    pub fn get_atom_name(&mut self, atom: Atom, tag: T) {
        self.req_reply(17, 0, &atom.to_le_bytes(), tag);
    }

    /// ConvertSelection: the owner of `selection` is asked to put it as `target` into
    /// `property` of `requestor`.
    pub fn convert_selection(&mut self, requestor: Xid, selection: Atom, target: Atom, property: Atom) {
        let mut b = Vec::with_capacity(20);
        for v in [requestor, selection, target, property, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.req(24, 0, &b);
    }

    /// The SelectionNotify an owner sends when the data is in place (`property` 0: refused).
    pub fn selection_notify(&mut self, requestor: Xid, selection: Atom, target: Atom, property: Atom, time: u32) {
        let mut e = [0u8; 32];
        e[0] = SELECTION_NOTIFY;
        for (i, v) in [time, requestor, selection, target, property].iter().enumerate() {
            e[4 + i * 4..8 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        self.send_event(requestor, 0, &e);
    }

    pub fn send_event(&mut self, dest: Xid, mask: u32, event: &[u8; 32]) {
        let mut b = Vec::with_capacity(40);
        b.extend_from_slice(&dest.to_le_bytes());
        b.extend_from_slice(&mask.to_le_bytes());
        b.extend_from_slice(event);
        self.req(25, 0, &b);
    }

    /// A ClientMessage of format 32 to `w` (WM_PROTOCOLS and the like).
    pub fn client_message(&mut self, w: Xid, ty: Atom, data: [u32; 5]) {
        let mut e = [0u8; 32];
        e[0] = CLIENT_MESSAGE;
        e[1] = 32;
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..12].copy_from_slice(&ty.to_le_bytes());
        for (i, v) in data.iter().enumerate() {
            e[12 + i * 4..16 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        self.send_event(w, 0, &e);
    }

    /// The synthetic ConfigureNotify ICCCM wants after a configure the WM decided.
    pub fn configure_notify(&mut self, w: Xid, x: i32, y: i32, wd: u32, h: u32) {
        let mut e = [0u8; 32];
        e[0] = CONFIGURE_NOTIFY;
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..12].copy_from_slice(&w.to_le_bytes());
        e[16..18].copy_from_slice(&(x as i16).to_le_bytes());
        e[18..20].copy_from_slice(&(y as i16).to_le_bytes());
        e[20..22].copy_from_slice(&(wd as u16).to_le_bytes());
        e[22..24].copy_from_slice(&(h as u16).to_le_bytes());
        self.send_event(w, EM_STRUCTURE_NOTIFY, &e);
    }

    /// SetInputFocus, revert to PointerRoot (`w` 0: none).
    pub fn set_input_focus(&mut self, w: Xid) {
        let mut b = Vec::with_capacity(8);
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        self.req(42, 1, &b);
    }

    /// GetInputFocus: a cheap request with a reply, used as a marker in the reply order.
    pub fn sync(&mut self, tag: T) {
        self.req_reply(43, 0, &[], tag);
    }

    pub fn query_extension(&mut self, name: &str, tag: T) {
        let mut b = Vec::with_capacity(4 + name.len());
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(name.as_bytes());
        self.req_reply(98, 0, &b, tag);
    }

    pub fn kill_client(&mut self, resource: Xid) {
        self.req(113, 0, &resource.to_le_bytes());
    }

    /// An extension's request (`major` from QueryExtension).
    pub fn ext(&mut self, major: u8, minor: u8, body: &[u8]) {
        self.req(major, minor, body);
    }

    pub fn ext_reply(&mut self, major: u8, minor: u8, body: &[u8], tag: T) {
        self.req_reply(major, minor, body, tag);
    }
}

/// A GetProperty reply: (type, format, value bytes).
pub fn property_value(r: &[u8]) -> (Atom, u8, &[u8]) {
    let format = r[1];
    let ty = u32_at(r, 8);
    let n = u32_at(r, 16) as usize * (format as usize / 8);
    (ty, format, &r[32..(32 + n).min(r.len())])
}

/// A property's value as 32-bit numbers.
pub fn values32(v: &[u8]) -> Vec<u32> {
    v.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_framing_and_reply_order() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut c: Conn<u32> = Conn::new(OwnedFd::from(a)).unwrap();
        c.out.clear();
        c.intern_atom("WM_S0", 7);
        c.map_window(0x200001);
        c.get_property(0x200001, WM_NAME, 9);
        // InternAtom: opcode 16, 2 + 2 words (name 5 bytes padded to 8).
        assert_eq!(&c.out[0..4], &[16, 0, 4, 0]);
        assert_eq!(&c.out[8..13], b"WM_S0");
        // MapWindow: opcode 8, length 2.
        assert_eq!(&c.out[16..20], &[8, 0, 2, 0]);
        // Replies for requests 1 and 3, an event in between.
        let mut srv = b;
        let mut r1 = [0u8; 32];
        r1[0] = 1;
        r1[2] = 1;
        r1[8..12].copy_from_slice(&300u32.to_le_bytes());
        let mut ev = [0u8; 32];
        ev[0] = 19 | 0x80; // MapNotify
        ev[2] = 2;
        let mut r3 = [0u8; 36];
        r3[0] = 1;
        r3[2] = 3;
        r3[4] = 1; // one extra word
        srv.write_all(&r1).unwrap();
        srv.write_all(&ev).unwrap();
        srv.write_all(&r3).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let msgs = c.read().unwrap();
        assert_eq!(msgs.len(), 3);
        assert!(matches!(&msgs[0], Msg::Reply(7, r) if u32_at(r, 8) == 300));
        assert!(matches!(&msgs[1], Msg::Event(19, _)));
        assert!(matches!(&msgs[2], Msg::Reply(9, r) if r.len() == 36));
    }
}
