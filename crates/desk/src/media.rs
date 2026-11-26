//! The media keys (play/pause, next, previous, stop, rewind, fast forward) go to the
//! players over MPRIS (org.mpris.MediaPlayer2.* on the session bus), as desktops do: the
//! one playing now, else the one last told, else the first. On a thread of its own (the bus
//! may be slow); nothing to wait for.
use crate::dbus::{Bus, Val};
use std::sync::Mutex;

/// The player last told (it gets the next key when none is playing).
static LAST: Mutex<String> = Mutex::new(String::new());

#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    PlayPause,
    Next,
    Previous,
    Stop,
    /// Seek by seconds.
    Seek(i64),
}

/// The evdev key's command, if it is a media key.
pub fn of_key(code: u32) -> Option<Cmd> {
    Some(match code {
        164 | 200 | 201 => Cmd::PlayPause,
        163 => Cmd::Next,
        165 => Cmd::Previous,
        166 | 128 => Cmd::Stop,
        168 => Cmd::Seek(-10),
        208 => Cmd::Seek(10),
        _ => return None,
    })
}

pub fn send(cmd: Cmd) {
    std::thread::spawn(move || {
        match run(cmd) {
            Ok(p) => {
                if std::env::var_os("RESC_MEDIA_LOG").is_some() {
                    eprintln!("media: {cmd:?} → {p}");
                }
            }
            Err(e) => eprintln!("media: {cmd:?}: {e}"),
        }
    });
}

fn run(cmd: Cmd) -> std::io::Result<String> {
    let mut b = Bus::session()?;
    let names = b.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", &[])?;
    let players: Vec<String> = names.first().map(|v| v.array().iter().filter_map(|n| n.str()).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")).map(str::to_string).collect()).unwrap_or_default();
    if players.is_empty() {
        return Ok("no player".into());
    }
    const PATH: &str = "/org/mpris/MediaPlayer2";
    const IFACE: &str = "org.mpris.MediaPlayer2.Player";
    let playing = players.iter().find(|p| b.prop(p, PATH, IFACE, "PlaybackStatus").ok().is_some_and(|v| v.inner().str() == Some("Playing"))).cloned();
    let last = LAST.lock().map(|l| l.clone()).unwrap_or_default();
    let p = playing.or_else(|| players.iter().find(|p| **p == last).cloned()).unwrap_or_else(|| players[0].clone());
    if let Ok(mut l) = LAST.lock() {
        *l = p.clone();
    }
    match cmd {
        Cmd::PlayPause => b.call(&p, PATH, IFACE, "PlayPause", &[]),
        Cmd::Next => b.call(&p, PATH, IFACE, "Next", &[]),
        Cmd::Previous => b.call(&p, PATH, IFACE, "Previous", &[]),
        Cmd::Stop => b.call(&p, PATH, IFACE, "Stop", &[]),
        Cmd::Seek(s) => b.call(&p, PATH, IFACE, "Seek", &[Val::I64(s * 1_000_000)]),
    }
    .map(|_| p)
}
