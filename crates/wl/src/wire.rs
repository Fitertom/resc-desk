//! Message framing over a unix socket with fd passing. Used by the server
//! (our clients) and by the nested backend (we are a client of the host).
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};

pub type ObjId = u32;

/// 24.8 signed fixed point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Fixed(pub i32);

impl Fixed {
    pub fn from_f64(v: f64) -> Self {
        Fixed((v * 256.0).round() as i32)
    }
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / 256.0
    }
    pub fn from_int(v: i32) -> Self {
        Fixed(v.saturating_mul(256))
    }
}

/// `new_id` without a fixed interface (only wl_registry.bind).
#[derive(Debug)]
pub struct NewIdDyn {
    pub iface: String,
    pub version: u32,
    pub id: ObjId,
}

#[derive(Debug)]
pub enum WireError {
    Truncated,
    BadString,
    MissingFd,
    BadOpcode(u16),
    BadSize,
}

/// Cursor over one message's argument bytes.
pub struct Args<'a> {
    d: &'a [u8],
    p: usize,
    fds: &'a mut VecDeque<OwnedFd>,
}

impl<'a> Args<'a> {
    fn word(&mut self) -> Result<u32, WireError> {
        if self.p + 4 > self.d.len() {
            return Err(WireError::Truncated);
        }
        let v = u32::from_ne_bytes(self.d[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        Ok(v)
    }
    pub fn int(&mut self) -> Result<i32, WireError> {
        self.word().map(|v| v as i32)
    }
    pub fn uint(&mut self) -> Result<u32, WireError> {
        self.word()
    }
    pub fn fixed(&mut self) -> Result<Fixed, WireError> {
        self.word().map(|v| Fixed(v as i32))
    }
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let padded = (len + 3) & !3;
        if self.p + padded > self.d.len() {
            return Err(WireError::Truncated);
        }
        let s = &self.d[self.p..self.p + len];
        self.p += padded;
        Ok(s)
    }
    pub fn opt_string(&mut self) -> Result<Option<String>, WireError> {
        let len = self.word()? as usize;
        if len == 0 {
            return Ok(None);
        }
        let b = self.bytes(len)?;
        if b[len - 1] != 0 {
            return Err(WireError::BadString);
        }
        Ok(Some(String::from_utf8_lossy(&b[..len - 1]).into_owned()))
    }
    pub fn string(&mut self) -> Result<String, WireError> {
        self.opt_string()?.ok_or(WireError::BadString)
    }
    pub fn array(&mut self) -> Result<Vec<u8>, WireError> {
        let len = self.word()? as usize;
        Ok(self.bytes(len)?.to_vec())
    }
    pub fn new_id_dyn(&mut self) -> Result<NewIdDyn, WireError> {
        Ok(NewIdDyn { iface: self.string()?, version: self.uint()?, id: self.uint()? })
    }
    pub fn fd(&mut self) -> Result<OwnedFd, WireError> {
        self.fds.pop_front().ok_or(WireError::MissingFd)
    }
}

/// Header of a framed incoming message; the payload is `Conn::args(&hdr)`.
#[derive(Clone, Copy, Debug)]
pub struct MsgHdr {
    pub obj: ObjId,
    pub op: u16,
    start: usize,
    end: usize,
}

const IN_CAP: usize = 64 * 1024;
/// A client that does not drain this much queued output is disconnected.
pub const OUT_LIMIT: usize = 8 * 1024 * 1024;

pub struct Conn {
    fd: OwnedFd,
    inb: Vec<u8>,
    in_start: usize,
    in_end: usize,
    pub in_fds: VecDeque<OwnedFd>,
    out: Vec<u8>,
    /// (byte offset of the message that carries it, fd)
    out_fds: VecDeque<(usize, OwnedFd)>,
    cur_msg: usize,
}

impl Conn {
    pub fn new(fd: OwnedFd) -> Self {
        Self {
            fd,
            inb: vec![0; IN_CAP],
            in_start: 0,
            in_end: 0,
            in_fds: VecDeque::new(),
            out: Vec::with_capacity(4096),
            out_fds: VecDeque::new(),
            cur_msg: 0,
        }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Reads what is available. Ok(0) means the peer hung up.
    /// Err(WouldBlock) means nothing to read right now.
    pub fn read(&mut self) -> io::Result<usize> {
        if self.in_start > 0 {
            self.inb.copy_within(self.in_start..self.in_end, 0);
            self.in_end -= self.in_start;
            self.in_start = 0;
        }
        if self.in_end == self.inb.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "input buffer full"));
        }
        let n = sys::recv_with_fds(self.fd.as_fd(), &mut self.inb[self.in_end..], &mut self.in_fds)?;
        self.in_end += n;
        Ok(n)
    }

    /// Next complete message, if any.
    pub fn next_msg(&mut self) -> Result<Option<MsgHdr>, WireError> {
        let avail = self.in_end - self.in_start;
        if avail < 8 {
            return Ok(None);
        }
        let b = &self.inb[self.in_start..];
        let obj = u32::from_ne_bytes(b[0..4].try_into().unwrap());
        let w = u32::from_ne_bytes(b[4..8].try_into().unwrap());
        let size = (w >> 16) as usize;
        if size < 8 || size % 4 != 0 {
            return Err(WireError::BadSize);
        }
        if avail < size {
            return Ok(None);
        }
        let h = MsgHdr { obj, op: (w & 0xffff) as u16, start: self.in_start + 8, end: self.in_start + size };
        self.in_start += size;
        Ok(Some(h))
    }

    pub fn args(&mut self, h: &MsgHdr) -> Args<'_> {
        Args { d: &self.inb[h.start..h.end], p: 0, fds: &mut self.in_fds }
    }

    // ------------------------------------------------------------- output

    pub fn begin(&mut self, id: ObjId, op: u16) -> usize {
        let s = self.out.len();
        self.cur_msg = s;
        self.out.extend_from_slice(&id.to_ne_bytes());
        self.out.extend_from_slice(&(op as u32).to_ne_bytes());
        s
    }
    pub fn end(&mut self, s: usize) {
        let size = (self.out.len() - s) as u32;
        let op = u32::from_ne_bytes(self.out[s + 4..s + 8].try_into().unwrap()) & 0xffff;
        self.out[s + 4..s + 8].copy_from_slice(&((size << 16) | op).to_ne_bytes());
    }
    pub fn put_u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_ne_bytes());
    }
    pub fn put_i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_ne_bytes());
    }
    fn put_padded(&mut self, b: &[u8], extra_nul: bool) {
        let len = b.len() + extra_nul as usize;
        self.put_u32(len as u32);
        self.out.extend_from_slice(b);
        if extra_nul {
            self.out.push(0);
        }
        let pad = (4 - len % 4) % 4;
        self.out.extend(std::iter::repeat(0).take(pad));
    }
    pub fn put_str(&mut self, s: &str) {
        self.put_padded(s.as_bytes(), true);
    }
    pub fn put_opt_str(&mut self, s: Option<&str>) {
        match s {
            Some(s) => self.put_str(s),
            None => self.put_u32(0),
        }
    }
    pub fn put_array(&mut self, b: &[u8]) {
        self.put_padded(b, false);
    }
    pub fn put_fd(&mut self, fd: BorrowedFd<'_>) {
        match fd.try_clone_to_owned() {
            Ok(fd) => self.out_fds.push_back((self.cur_msg, fd)),
            Err(e) => eprintln!("wire: dup fd failed: {e}"),
        }
    }

    pub fn pending_out(&self) -> usize {
        self.out.len()
    }

    /// Writes queued output. Ok(true) when everything was sent.
    pub fn flush(&mut self) -> io::Result<bool> {
        while !self.out.is_empty() {
            let nfds = self.out_fds.len().min(sys::MAX_FDS);
            // Bytes of messages whose fds do not fit in this batch must wait.
            let limit = if self.out_fds.len() > nfds { self.out_fds[nfds].0 } else { self.out.len() };
            let limit = if limit == 0 { self.out.len() } else { limit };
            let raw: Vec<RawFd> = self.out_fds.iter().take(nfds).map(|(_, f)| f.as_raw_fd()).collect();
            match sys::send_with_fds(self.fd.as_fd(), &self.out[..limit], &raw) {
                Ok(n) => {
                    for _ in 0..nfds {
                        self.out_fds.pop_front();
                    }
                    self.out.drain(..n);
                    for (off, _) in self.out_fds.iter_mut() {
                        *off = off.saturating_sub(n);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}
