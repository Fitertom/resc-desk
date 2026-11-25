//! Sound: a client of the PulseAudio native protocol (PulseAudio itself, PipeWire's
//! pipewire-pulse, WSLg's server), no libpulse. Only what the shell needs: the outputs
//! (sinks) and inputs (sources) with their volume and mute, the defaults, and change
//! events.
//!
//! Wire format: packets of a 20-byte descriptor (length, channel, offset hi/lo, flags; big
//! endian) and a "tagstruct" payload, where every value carries a one-byte type tag. Commands
//! go on channel -1 as (command, tag, argumentsâ€¦); the server answers REPLY or ERROR with the
//! same tag, and sends SUBSCRIBE_EVENT packets on its own.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const VERSION: u32 = 32;
const CH_COMMAND: u32 = 0xffff_ffff;

const CMD_ERROR: u32 = 0;
const CMD_REPLY: u32 = 2;
const CMD_AUTH: u32 = 8;
const CMD_SET_CLIENT_NAME: u32 = 9;
const CMD_GET_SERVER_INFO: u32 = 20;
const CMD_GET_SINK_INFO_LIST: u32 = 22;
const CMD_GET_SOURCE_INFO_LIST: u32 = 24;
const CMD_SUBSCRIBE: u32 = 35;
const CMD_SET_SINK_VOLUME: u32 = 36;
const CMD_SET_SOURCE_VOLUME: u32 = 38;
const CMD_SET_SINK_MUTE: u32 = 39;
const CMD_SET_SOURCE_MUTE: u32 = 40;
const CMD_SET_DEFAULT_SINK: u32 = 44;
const CMD_SET_DEFAULT_SOURCE: u32 = 45;
const CMD_SUBSCRIBE_EVENT: u32 = 66;

/// Subscription mask: sinks, sources, the server (defaults), cards (ports).
const MASK: u32 = 0x0001 | 0x0002 | 0x0080 | 0x0200;

/// PA_VOLUME_NORM: 100 %.
pub const NORM: u32 = 0x10000;
/// The UI goes up to 100 % (louder distorts); keys and the wheel too.
pub const MAX_PERCENT: u32 = 100;

/// One sink or source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub index: u32,
    pub name: String,
    pub desc: String,
    /// Per channel, PA_VOLUME_NORM = 100 %.
    pub volume: Vec<u32>,
    pub mute: bool,
}

impl Device {
    /// The loudest channel, in percent.
    pub fn percent(&self) -> u32 {
        let v = self.volume.iter().copied().max().unwrap_or(0) as u64;
        ((v * 100 + NORM as u64 / 2) / NORM as u64) as u32
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    pub sinks: Vec<Device>,
    /// Real inputs (monitors of outputs left out).
    pub sources: Vec<Device>,
    pub default_sink: String,
    pub default_source: String,
}

impl Audio {
    pub fn sink(&self) -> Option<&Device> {
        self.sinks.iter().find(|s| s.name == self.default_sink).or(self.sinks.first())
    }
    pub fn source(&self) -> Option<&Device> {
        self.sources.iter().find(|s| s.name == self.default_source).or(self.sources.first())
    }
}

// ------------------------------------------------------------------ tagstruct

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.push(b'L');
        self.0.extend(v.to_be_bytes());
        self
    }
    fn str(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            Some(s) => {
                self.0.push(b't');
                self.0.extend(s.as_bytes());
                self.0.push(0);
            }
            None => self.0.push(b'N'),
        }
        self
    }
    fn bool(&mut self, b: bool) -> &mut Self {
        self.0.push(if b { b'1' } else { b'0' });
        self
    }
    fn arbitrary(&mut self, data: &[u8]) -> &mut Self {
        self.0.push(b'x');
        self.0.extend((data.len() as u32).to_be_bytes());
        self.0.extend(data);
        self
    }
    fn cvolume(&mut self, v: &[u32]) -> &mut Self {
        self.0.push(b'v');
        self.0.push(v.len() as u8);
        for c in v {
            self.0.extend(c.to_be_bytes());
        }
        self
    }
    fn proplist(&mut self, props: &[(&str, &str)]) -> &mut Self {
        self.0.push(b'P');
        for (k, v) in props {
            let mut val = v.as_bytes().to_vec();
            val.push(0);
            self.str(Some(k));
            self.u32(val.len() as u32);
            self.arbitrary(&val);
        }
        self.str(None)
    }
}

/// A tagged value, as read.
#[derive(Clone, Debug)]
enum V {
    U32(u32),
    U8(u8),
    U64(u64),
    Str(Option<String>),
    Bool(bool),
    CVol(Vec<u32>),
    /// Values the shell does not look at (sample spec, channel map, proplist, â€¦).
    Other,
}

struct In<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn be32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn cstr(&mut self) -> Option<String> {
        let rest = self.b.get(self.at..)?;
        let n = rest.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.at += n + 1;
        Some(s)
    }
    fn done(&self) -> bool {
        self.at >= self.b.len()
    }

    fn value(&mut self) -> Option<V> {
        let tag = *self.take(1)?.first()?;
        Some(match tag {
            b'L' => V::U32(self.be32()?),
            b'B' => V::U8(self.take(1)?[0]),
            b'R' | b'r' | b'U' => V::U64(u64::from_be_bytes(self.take(8)?.try_into().ok()?)),
            b'T' => {
                self.take(8)?;
                V::Other
            }
            b'V' => V::U32(self.be32()?),
            b't' => V::Str(Some(self.cstr()?)),
            b'N' => V::Str(None),
            b'1' => V::Bool(true),
            b'0' => V::Bool(false),
            b'a' => {
                self.take(6)?;
                V::Other
            }
            b'm' => {
                let n = self.take(1)?[0] as usize;
                self.take(n)?;
                V::Other
            }
            b'v' => {
                let n = self.take(1)?[0] as usize;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.be32()?);
                }
                V::CVol(v)
            }
            b'x' => {
                let n = self.be32()? as usize;
                self.take(n)?;
                V::Other
            }
            b'P' => {
                // key, length, data â€¦ up to a null string.
                loop {
                    match self.value()? {
                        V::Str(None) => break,
                        V::Str(Some(_)) => {
                            self.value()?;
                            self.value()?;
                        }
                        _ => return None,
                    }
                }
                V::Other
            }
            // A format info: its encoding and proplist follow as values of their own.
            b'f' => V::Other,
            _ => return None,
        })
    }

    fn u32(&mut self) -> Option<u32> {
        match self.value()? {
            V::U32(v) => Some(v),
            V::U8(v) => Some(v as u32),
            _ => None,
        }
    }
    fn str(&mut self) -> Option<String> {
        match self.value()? {
            V::Str(s) => Some(s.unwrap_or_default()),
            _ => None,
        }
    }
    fn bool(&mut self) -> Option<bool> {
        match self.value()? {
            V::Bool(b) => Some(b),
            _ => None,
        }
    }
    fn cvol(&mut self) -> Option<Vec<u32>> {
        match self.value()? {
            V::CVol(v) => Some(v),
            _ => None,
        }
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        for _ in 0..n {
            self.value()?;
        }
        Some(())
    }
}

/// One sink or source info (protocol version `ver`); `monitor`: for a source, whether it is
/// the monitor of a sink.
fn read_device(r: &mut In, ver: u32, source: bool) -> Option<(Device, bool)> {
    let index = r.u32()?;
    let name = r.str()?;
    let desc = r.str()?;
    r.skip(2)?; // sample spec, channel map
    r.u32()?; // owner module
    let volume = r.cvol()?;
    let mute = r.bool()?;
    let other = r.u32()?; // sink: its monitor source; source: the sink it monitors
    r.str()?;
    r.skip(1)?; // latency
    r.str()?; // driver
    r.u32()?; // flags
    if ver >= 13 {
        r.skip(2)?; // proplist, configured latency
    }
    if ver >= 15 {
        r.skip(4)?; // base volume, state, volume steps, card
    }
    if ver >= 16 {
        let ports = r.u32()?;
        for _ in 0..ports {
            r.skip(3)?; // name, description, priority
            if ver >= 24 {
                r.skip(1)?; // available
            }
            if ver >= 34 {
                r.skip(2)?; // availability group, type
            }
        }
        r.str()?; // active port
    }
    if ver >= if source { 22 } else { 21 } {
        let n = r.u32()?;
        for _ in 0..n {
            r.skip(3)?; // 'f', encoding, proplist
        }
    }
    let monitor = source && other != 0xffff_ffff;
    Some((Device { index, name, desc, volume, mute }, monitor))
}

// ------------------------------------------------------------------ connection

pub struct Pulse {
    s: UnixStream,
    tag: u32,
    ver: u32,
    /// A change event came in: the state should be read again.
    pub dirty: bool,
    rbuf: Vec<u8>,
}

/// Where the server listens: $PULSE_SERVER (its unix: entries), else the runtime dir.
fn socket_paths() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(s) = std::env::var("PULSE_SERVER") {
        for e in s.split_whitespace() {
            let e = e.strip_prefix('{').and_then(|e| e.split_once('}')).map_or(e, |(_, r)| r);
            if let Some(p) = e.strip_prefix("unix:") {
                v.push(p.to_string());
            } else if e.starts_with('/') {
                v.push(e.to_string());
            }
        }
    }
    if let Ok(r) = std::env::var("XDG_RUNTIME_DIR") {
        v.push(format!("{r}/pulse/native"));
    }
    v.push(format!("/run/user/{}/pulse/native", sys::uid()));
    v
}

fn cookie() -> Vec<u8> {
    let home = std::env::var("HOME").unwrap_or_default();
    let cfg = std::env::var("XDG_CONFIG_HOME").unwrap_or(format!("{home}/.config"));
    for p in [std::env::var("PULSE_COOKIE").unwrap_or_default(), format!("{cfg}/pulse/cookie"), format!("{home}/.pulse-cookie")] {
        if let Ok(c) = std::fs::read(&p) {
            if c.len() == 256 {
                return c;
            }
        }
    }
    vec![0; 256]
}

fn err(s: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, s.to_string())
}

impl Pulse {
    pub fn connect(app: &str) -> std::io::Result<Pulse> {
        let mut last = err("no PulseAudio server");
        for p in socket_paths() {
            match UnixStream::connect(&p) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_millis(800)))?;
                    s.set_write_timeout(Some(Duration::from_millis(800)))?;
                    let mut c = Pulse { s, tag: 0, ver: VERSION, dirty: false, rbuf: Vec::new() };
                    let mut a = Out::default();
                    a.u32(VERSION).arbitrary(&cookie());
                    let reply = c.request(CMD_AUTH, a)?;
                    let server = In { b: &reply, at: 0 }.u32().ok_or_else(|| err("auth reply"))? & 0xffff;
                    c.ver = VERSION.min(server);
                    let mut n = Out::default();
                    n.proplist(&[("application.name", app), ("application.id", "resc-desk")]);
                    c.request(CMD_SET_CLIENT_NAME, n)?;
                    return Ok(c);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.s.as_raw_fd()
    }

    fn send(&mut self, cmd: u32, args: Out) -> std::io::Result<u32> {
        self.tag = self.tag.wrapping_add(1);
        let mut body = Out::default();
        body.u32(cmd).u32(self.tag);
        body.0.extend(args.0);
        let mut pkt = Vec::with_capacity(20 + body.0.len());
        for v in [body.0.len() as u32, CH_COMMAND, 0, 0, 0] {
            pkt.extend(v.to_be_bytes());
        }
        pkt.extend(body.0);
        self.s.write_all(&pkt)?;
        Ok(self.tag)
    }

    /// One packet (blocking up to the read timeout): its command, tag and the rest.
    fn packet(&mut self) -> std::io::Result<(u32, u32, Vec<u8>)> {
        loop {
            if self.rbuf.len() >= 20 {
                let len = u32::from_be_bytes(self.rbuf[0..4].try_into().unwrap()) as usize;
                let ch = u32::from_be_bytes(self.rbuf[4..8].try_into().unwrap());
                if len > 1 << 24 {
                    return Err(err("packet too large"));
                }
                if self.rbuf.len() >= 20 + len {
                    let body: Vec<u8> = self.rbuf[20..20 + len].to_vec();
                    self.rbuf.drain(..20 + len);
                    if ch != CH_COMMAND {
                        continue; // stream data: not ours
                    }
                    let mut r = In { b: &body, at: 0 };
                    let (Some(cmd), Some(tag)) = (r.u32(), r.u32()) else { return Err(err("bad packet")) };
                    let at = r.at;
                    return Ok((cmd, tag, body[at..].to_vec()));
                }
            }
            let mut buf = [0u8; 16384];
            let n = self.s.read(&mut buf)?;
            if n == 0 {
                return Err(err("PulseAudio server went away"));
            }
            self.rbuf.extend_from_slice(&buf[..n]);
        }
    }

    fn request(&mut self, cmd: u32, args: Out) -> std::io::Result<Vec<u8>> {
        let tag = self.send(cmd, args)?;
        loop {
            let (c, t, body) = self.packet()?;
            match c {
                CMD_REPLY if t == tag => return Ok(body),
                CMD_ERROR if t == tag => {
                    let code = In { b: &body, at: 0 }.u32().unwrap_or(0);
                    return Err(err(&format!("PulseAudio error {code}")));
                }
                CMD_SUBSCRIBE_EVENT => self.dirty = true,
                _ => {}
            }
        }
    }

    /// Change events from now on (read with `drain`).
    pub fn subscribe(&mut self) -> std::io::Result<()> {
        let mut a = Out::default();
        a.u32(MASK);
        self.request(CMD_SUBSCRIBE, a).map(|_| ())
    }

    /// Reads what has come in without waiting; true if something changed.
    pub fn drain(&mut self) -> std::io::Result<bool> {
        self.s.set_nonblocking(true)?;
        let mut buf = [0u8; 16384];
        let res = loop {
            match self.s.read(&mut buf) {
                Ok(0) => break Err(err("PulseAudio server went away")),
                Ok(n) => self.rbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        self.s.set_nonblocking(false)?;
        res?;
        // Whole packets only; a partial one waits for the rest.
        while self.rbuf.len() >= 20 {
            let len = u32::from_be_bytes(self.rbuf[0..4].try_into().unwrap()) as usize;
            if self.rbuf.len() < 20 + len {
                break;
            }
            let (c, _, _) = self.packet()?;
            if c == CMD_SUBSCRIBE_EVENT {
                self.dirty = true;
            }
        }
        Ok(std::mem::take(&mut self.dirty))
    }

    /// Everything the shell shows.
    pub fn state(&mut self) -> std::io::Result<Audio> {
        let mut a = Audio::default();
        let info = self.request(CMD_GET_SERVER_INFO, Out::default())?;
        let mut r = In { b: &info, at: 0 };
        (|| {
            r.skip(5)?; // package, version, user, host, sample spec
            a.default_sink = r.str()?;
            a.default_source = r.str()?;
            Some(())
        })()
        .ok_or_else(|| err("server info"))?;
        let ver = self.ver;
        let sinks = self.request(CMD_GET_SINK_INFO_LIST, Out::default())?;
        let mut r = In { b: &sinks, at: 0 };
        while !r.done() {
            let (dev, _) = read_device(&mut r, ver, false).ok_or_else(|| err("sink info"))?;
            a.sinks.push(dev);
        }
        let sources = self.request(CMD_GET_SOURCE_INFO_LIST, Out::default())?;
        let mut r = In { b: &sources, at: 0 };
        while !r.done() {
            let (dev, monitor) = read_device(&mut r, ver, true).ok_or_else(|| err("source info"))?;
            if !monitor {
                a.sources.push(dev);
            }
        }
        Ok(a)
    }

    /// All channels to `percent`.
    pub fn set_volume(&mut self, dev: &Device, source: bool, percent: u32) -> std::io::Result<()> {
        let v = (percent.min(150) as u64 * NORM as u64 / 100) as u32;
        let mut a = Out::default();
        a.u32(dev.index).str(None).cvolume(&vec![v; dev.volume.len().max(1)]);
        self.request(if source { CMD_SET_SOURCE_VOLUME } else { CMD_SET_SINK_VOLUME }, a).map(|_| ())
    }

    pub fn set_mute(&mut self, dev: &Device, source: bool, mute: bool) -> std::io::Result<()> {
        let mut a = Out::default();
        a.u32(dev.index).str(None).bool(mute);
        self.request(if source { CMD_SET_SOURCE_MUTE } else { CMD_SET_SINK_MUTE }, a).map(|_| ())
    }

    pub fn set_default(&mut self, name: &str, source: bool) -> std::io::Result<()> {
        let mut a = Out::default();
        a.str(Some(name));
        self.request(if source { CMD_SET_DEFAULT_SOURCE } else { CMD_SET_DEFAULT_SINK }, a).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_info_parses() {
        // A sink as a v32 server sends it.
        let mut o = Out::default();
        o.u32(3).str(Some("alsa_output.pci")).str(Some("Built-in Audio"));
        o.0.extend([b'a', 3, 2, 0, 0, 0xac, 0x44]);
        o.0.extend([b'm', 2, 1, 2]);
        o.u32(7).cvolume(&[NORM / 2, NORM / 4]).bool(true).u32(4).str(Some("alsa_output.pci.monitor"));
        o.0.push(b'U');
        o.0.extend(0u64.to_be_bytes());
        o.str(Some("module-alsa-card.c")).u32(0x1f);
        o.proplist(&[("device.class", "sound")]);
        o.0.push(b'U');
        o.0.extend(0u64.to_be_bytes());
        o.0.push(b'V');
        o.0.extend(NORM.to_be_bytes());
        o.u32(0).u32(65537).u32(0);
        o.u32(1).str(Some("analog-output")).str(Some("Line Out")).u32(9900).u32(2);
        o.str(Some("analog-output"));
        o.0.extend([b'B', 1, b'f', b'B', 1]);
        o.proplist(&[]);
        let mut r = In { b: &o.0, at: 0 };
        let (d, mon) = read_device(&mut r, 32, false).unwrap();
        assert!(r.done());
        assert!(!mon);
        assert_eq!(d.index, 3);
        assert_eq!(d.desc, "Built-in Audio");
        assert!(d.mute);
        assert_eq!(d.percent(), 50);
    }
}
