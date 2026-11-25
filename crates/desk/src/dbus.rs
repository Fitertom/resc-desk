//! A small D-Bus client, no libdbus: the system bus over its unix socket, SASL EXTERNAL
//! (the kernel vouches for our uid), method calls with their replies. Enough for
//! NetworkManager (net.rs); signals are not subscribed to (the shell polls).
//!
//! Wire format (little endian only): a 16-byte fixed header (endianness, type, flags,
//! version, body length, serial), the header fields as `a(yv)`, padding to 8, the body.
//! Every value is aligned to its own size from the start of the message.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Y(u8),
    B(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    D(f64),
    S(String),
    O(String),
    G(String),
    /// Element signature and the elements.
    A(String, Vec<Val>),
    St(Vec<Val>),
    E(Box<Val>, Box<Val>),
    V(Box<Val>),
}

impl Val {
    pub fn sig(&self) -> String {
        match self {
            Val::Y(_) => "y".into(),
            Val::B(_) => "b".into(),
            Val::I16(_) => "n".into(),
            Val::U16(_) => "q".into(),
            Val::I32(_) => "i".into(),
            Val::U32(_) => "u".into(),
            Val::I64(_) => "x".into(),
            Val::U64(_) => "t".into(),
            Val::D(_) => "d".into(),
            Val::S(_) => "s".into(),
            Val::O(_) => "o".into(),
            Val::G(_) => "g".into(),
            Val::A(e, _) => format!("a{e}"),
            Val::St(v) => format!("({})", v.iter().map(|x| x.sig()).collect::<String>()),
            Val::E(k, v) => format!("{{{}{}}}", k.sig(), v.sig()),
            Val::V(_) => "v".into(),
        }
    }

    // Accessors (a variant is looked through).
    pub fn inner(&self) -> &Val {
        match self {
            Val::V(v) => v.inner(),
            v => v,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self.inner() {
            Val::S(s) | Val::O(s) | Val::G(s) => Some(s),
            _ => None,
        }
    }
    pub fn u32(&self) -> Option<u32> {
        match *self.inner() {
            Val::Y(v) => Some(v as u32),
            Val::U16(v) => Some(v as u32),
            Val::U32(v) => Some(v),
            Val::I32(v) => Some(v as u32),
            _ => None,
        }
    }
    pub fn bool(&self) -> Option<bool> {
        match *self.inner() {
            Val::B(b) => Some(b),
            _ => None,
        }
    }
    pub fn array(&self) -> &[Val] {
        match self.inner() {
            Val::A(_, v) => v,
            _ => &[],
        }
    }
    /// An `ay` as bytes.
    pub fn bytes(&self) -> Vec<u8> {
        self.array().iter().filter_map(|v| v.u32().map(|b| b as u8)).collect()
    }
    /// In an `a{s…}`: the value under `key`.
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.array().iter().find_map(|e| match e {
            Val::E(k, v) if k.str() == Some(key) => Some(&**v),
            _ => None,
        })
    }
}

/// `a{sv}` from pairs.
pub fn dict(pairs: Vec<(&str, Val)>) -> Val {
    Val::A("{sv}".into(), pairs.into_iter().map(|(k, v)| Val::E(Box::new(Val::S(k.into())), Box::new(Val::V(Box::new(v))))).collect())
}

fn align_of(c: u8) -> usize {
    match c {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b's' | b'o' | b'a' | b'h' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

/// Index just past the complete type starting at `i`.
fn type_end(sig: &[u8], i: usize) -> Option<usize> {
    match *sig.get(i)? {
        b'a' => type_end(sig, i + 1),
        open @ (b'(' | b'{') => {
            let close = if open == b'(' { b')' } else { b'}' };
            let mut j = i + 1;
            while *sig.get(j)? != close {
                j = type_end(sig, j)?;
            }
            Some(j + 1)
        }
        _ => Some(i + 1),
    }
}

// ------------------------------------------------------------------ writing

struct W {
    b: Vec<u8>,
}

impl W {
    fn pad(&mut self, a: usize) {
        while self.b.len() % a != 0 {
            self.b.push(0);
        }
    }
    fn u32(&mut self, v: u32) {
        self.pad(4);
        self.b.extend(v.to_le_bytes());
    }
    fn put(&mut self, v: &Val) {
        match v {
            Val::Y(x) => self.b.push(*x),
            Val::B(x) => self.u32(*x as u32),
            Val::I16(x) => {
                self.pad(2);
                self.b.extend(x.to_le_bytes())
            }
            Val::U16(x) => {
                self.pad(2);
                self.b.extend(x.to_le_bytes())
            }
            Val::I32(x) => self.u32(*x as u32),
            Val::U32(x) => self.u32(*x),
            Val::I64(x) => {
                self.pad(8);
                self.b.extend(x.to_le_bytes())
            }
            Val::U64(x) => {
                self.pad(8);
                self.b.extend(x.to_le_bytes())
            }
            Val::D(x) => {
                self.pad(8);
                self.b.extend(x.to_le_bytes())
            }
            Val::S(s) | Val::O(s) => {
                self.u32(s.len() as u32);
                self.b.extend(s.as_bytes());
                self.b.push(0);
            }
            Val::G(s) => {
                self.b.push(s.len() as u8);
                self.b.extend(s.as_bytes());
                self.b.push(0);
            }
            Val::A(e, items) => {
                self.u32(0);
                let at = self.b.len() - 4;
                self.pad(align_of(e.as_bytes()[0]));
                let start = self.b.len();
                for it in items {
                    self.put(it);
                }
                let n = (self.b.len() - start) as u32;
                self.b[at..at + 4].copy_from_slice(&n.to_le_bytes());
            }
            Val::St(items) => {
                self.pad(8);
                for it in items {
                    self.put(it);
                }
            }
            Val::E(k, v) => {
                self.pad(8);
                self.put(k);
                self.put(v);
            }
            Val::V(inner) => {
                self.put(&Val::G(inner.sig()));
                self.put(inner);
            }
        }
    }
}

// ------------------------------------------------------------------ reading

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> R<'a> {
    fn pad(&mut self, a: usize) {
        self.at = self.at.div_ceil(a) * a;
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.pad(4);
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        self.pad(8);
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// One value of the complete type at `sig[i..]`.
    fn get(&mut self, sig: &[u8], i: usize) -> Option<Val> {
        Some(match sig[i] {
            b'y' => Val::Y(self.take(1)?[0]),
            b'b' => Val::B(self.u32()? != 0),
            b'n' | b'q' => {
                self.pad(2);
                let v = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
                if sig[i] == b'n' { Val::I16(v as i16) } else { Val::U16(v) }
            }
            b'i' => Val::I32(self.u32()? as i32),
            b'u' | b'h' => Val::U32(self.u32()?),
            b'x' => Val::I64(self.u64()? as i64),
            b't' => Val::U64(self.u64()?),
            b'd' => Val::D(f64::from_bits(self.u64()?)),
            b's' | b'o' => {
                let n = self.u32()? as usize;
                let s = String::from_utf8_lossy(self.take(n)?).into_owned();
                self.take(1)?;
                if sig[i] == b's' { Val::S(s) } else { Val::O(s) }
            }
            b'g' => {
                let n = self.take(1)?[0] as usize;
                let s = String::from_utf8_lossy(self.take(n)?).into_owned();
                self.take(1)?;
                Val::G(s)
            }
            b'v' => {
                let n = self.take(1)?[0] as usize;
                let s = self.take(n)?.to_vec();
                self.take(1)?;
                if s.is_empty() || type_end(&s, 0)? != s.len() {
                    return None;
                }
                Val::V(Box::new(self.get(&s, 0)?))
            }
            b'a' => {
                let len = self.u32()? as usize;
                let e = i + 1;
                let ee = type_end(sig, e)?;
                self.pad(align_of(sig[e]));
                let end = self.at + len;
                if end > self.b.len() {
                    return None;
                }
                let mut v = Vec::new();
                while self.at < end {
                    v.push(self.get(sig, e)?);
                }
                Val::A(String::from_utf8_lossy(&sig[e..ee]).into_owned(), v)
            }
            b'(' => {
                self.pad(8);
                let mut j = i + 1;
                let mut v = Vec::new();
                while sig[j] != b')' {
                    v.push(self.get(sig, j)?);
                    j = type_end(sig, j)?;
                }
                Val::St(v)
            }
            b'{' => {
                self.pad(8);
                let k = self.get(sig, i + 1)?;
                let j = type_end(sig, i + 1)?;
                let v = self.get(sig, j)?;
                Val::E(Box::new(k), Box::new(v))
            }
            _ => return None,
        })
    }

    /// The values of a whole signature.
    fn all(&mut self, sig: &[u8]) -> Option<Vec<Val>> {
        let mut v = Vec::new();
        let mut i = 0;
        while i < sig.len() {
            v.push(self.get(sig, i)?);
            i = type_end(sig, i)?;
        }
        Some(v)
    }
}

// ------------------------------------------------------------------ connection

pub struct Bus {
    s: UnixStream,
    serial: u32,
    rbuf: Vec<u8>,
}

fn err(s: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, s)
}

const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;

impl Bus {
    /// The system bus ($DBUS_SYSTEM_BUS_ADDRESS, else its usual socket).
    pub fn system() -> std::io::Result<Bus> {
        let path = std::env::var("DBUS_SYSTEM_BUS_ADDRESS")
            .ok()
            .and_then(|a| a.split(';').find_map(|e| e.strip_prefix("unix:path=").map(|p| p.split(',').next().unwrap_or(p).to_string())))
            .unwrap_or_else(|| "/run/dbus/system_bus_socket".into());
        let mut s = UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_millis(1500)))?;
        s.set_write_timeout(Some(Duration::from_millis(1500)))?;
        // SASL: our uid as hex-encoded ASCII.
        let uid: String = sys::uid().to_string().bytes().map(|b| format!("{b:02x}")).collect();
        s.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())?;
        let mut line = Vec::new();
        let mut c = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            if s.read(&mut c)? == 0 || line.len() > 512 {
                return Err(err("dbus: auth".into()));
            }
            line.push(c[0]);
        }
        if !line.starts_with(b"OK ") {
            return Err(err(format!("dbus: auth refused: {}", String::from_utf8_lossy(&line).trim())));
        }
        s.write_all(b"BEGIN\r\n")?;
        let mut bus = Bus { s, serial: 0, rbuf: Vec::new() };
        bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[])?;
        Ok(bus)
    }

    fn message(&mut self, dest: &str, path: &str, iface: &str, member: &str, args: &[Val]) -> Vec<u8> {
        self.serial = self.serial.wrapping_add(1).max(1);
        let mut body = W { b: Vec::new() };
        for a in args {
            body.put(a);
        }
        let sig: String = args.iter().map(|a| a.sig()).collect();
        let field = |code: u8, v: Val| Val::St(vec![Val::Y(code), Val::V(Box::new(v))]);
        let mut fields = vec![field(1, Val::O(path.into())), field(2, Val::S(iface.into())), field(3, Val::S(member.into())), field(6, Val::S(dest.into()))];
        if !sig.is_empty() {
            fields.push(field(8, Val::G(sig)));
        }
        let mut m = W { b: vec![b'l', METHOD_CALL, 0, 1] };
        m.b.extend((body.b.len() as u32).to_le_bytes());
        m.b.extend(self.serial.to_le_bytes());
        m.put(&Val::A("(yv)".into(), fields));
        m.pad(8);
        m.b.extend(body.b);
        m.b
    }

    /// One whole message: its type, reply serial, signature and body.
    fn read_message(&mut self) -> std::io::Result<(u8, u32, String, Vec<u8>)> {
        loop {
            if self.rbuf.len() >= 16 {
                if self.rbuf[0] != b'l' {
                    return Err(err("dbus: big-endian message".into()));
                }
                let body_len = u32::from_le_bytes(self.rbuf[4..8].try_into().unwrap()) as usize;
                let fields_len = u32::from_le_bytes(self.rbuf[12..16].try_into().unwrap()) as usize;
                let head = (16 + fields_len).div_ceil(8) * 8;
                let total = head + body_len;
                if total > 1 << 26 {
                    return Err(err("dbus: message too large".into()));
                }
                if self.rbuf.len() >= total {
                    let msg: Vec<u8> = self.rbuf.drain(..total).collect();
                    let mut r = R { b: &msg[..head], at: 12 };
                    let fields = r.get(b"a(yv)", 0).ok_or_else(|| err("dbus: bad header".into()))?;
                    let (mut reply, mut sig) = (0, String::new());
                    for f in fields.array() {
                        if let Val::St(v) = f {
                            match (v.first().and_then(|c| c.u32()), v.get(1)) {
                                (Some(5), Some(x)) => reply = x.u32().unwrap_or(0),
                                (Some(8), Some(x)) => sig = x.str().unwrap_or("").to_string(),
                                _ => {}
                            }
                        }
                    }
                    return Ok((msg[1], reply, sig, msg[head..].to_vec()));
                }
            }
            let mut buf = [0u8; 16384];
            let n = self.s.read(&mut buf)?;
            if n == 0 {
                return Err(err("dbus: the bus went away".into()));
            }
            self.rbuf.extend_from_slice(&buf[..n]);
        }
    }

    /// Calls a method and waits for its reply (or error).
    pub fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, args: &[Val]) -> std::io::Result<Vec<Val>> {
        let m = self.message(dest, path, iface, member, args);
        let serial = self.serial;
        self.s.write_all(&m)?;
        loop {
            let (ty, reply, sig, body) = self.read_message()?;
            if reply != serial || (ty != METHOD_RETURN && ty != ERROR) {
                continue;
            }
            let vals = R { b: &body, at: 0 }.all(sig.as_bytes()).ok_or_else(|| err(format!("dbus: bad reply to {member}")))?;
            if ty == ERROR {
                let msg = vals.first().and_then(|v| v.str()).unwrap_or("").to_string();
                return Err(err(format!("{member}: {msg}")));
            }
            return Ok(vals);
        }
    }

    /// How long a call may wait for its reply (Bluetooth pairing takes seconds).
    pub fn set_timeout(&mut self, t: Duration) {
        let _ = self.s.set_read_timeout(Some(t));
    }

    /// org.freedesktop.DBus.Properties.Get.
    pub fn prop(&mut self, dest: &str, path: &str, iface: &str, name: &str) -> std::io::Result<Val> {
        let v = self.call(dest, path, "org.freedesktop.DBus.Properties", "Get", &[Val::S(iface.into()), Val::S(name.into())])?;
        v.into_iter().next().ok_or_else(|| err("dbus: empty Get".into()))
    }

    /// org.freedesktop.DBus.Properties.GetAll: an `a{sv}`.
    pub fn props(&mut self, dest: &str, path: &str, iface: &str) -> std::io::Result<Val> {
        let v = self.call(dest, path, "org.freedesktop.DBus.Properties", "GetAll", &[Val::S(iface.into())])?;
        v.into_iter().next().ok_or_else(|| err("dbus: empty GetAll".into()))
    }

    pub fn set_prop(&mut self, dest: &str, path: &str, iface: &str, name: &str, v: Val) -> std::io::Result<()> {
        self.call(dest, path, "org.freedesktop.DBus.Properties", "Set", &[Val::S(iface.into()), Val::S(name.into()), Val::V(Box::new(v))]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = vec![
            Val::S("org.example".into()),
            Val::U32(7),
            dict(vec![("Ssid", Val::A("y".into(), b"home".iter().map(|b| Val::Y(*b)).collect())), ("Strength", Val::Y(80)), ("On", Val::B(true))]),
            Val::A("{sa{sv}}".into(), vec![Val::E(Box::new(Val::S("802-11-wireless-security".into())), Box::new(dict(vec![("psk", Val::S("secret".into()))])))]),
            Val::U64(1 << 40),
        ];
        let mut w = W { b: Vec::new() };
        for x in &v {
            w.put(x);
        }
        let sig: String = v.iter().map(|x| x.sig()).collect();
        assert_eq!(sig, "sua{sv}a{sa{sv}}t");
        let back = R { b: &w.b, at: 0 }.all(sig.as_bytes()).unwrap();
        assert_eq!(back, v);
        assert_eq!(back[2].get("Ssid").unwrap().bytes(), b"home");
        assert_eq!(back[2].get("Strength").unwrap().u32(), Some(80));
    }
}
