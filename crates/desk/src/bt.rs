//! Bluetooth through BlueZ (org.bluez on the system bus, our own D-Bus client): the
//! adapter's power, looking for devices, pairing, connecting, forgetting. For the
//! «Bluetooth» page of «Параметры».
//!
//! Pairing and connecting take seconds, so everything runs on a thread of its own with its
//! own bus connection; the window sends commands and gets the whole state back after each
//! one and every second or two. Looking for devices is tied to that connection: when the
//! window closes, BlueZ stops on its own.
//!
//! Pairing registers no agent: headphones, speakers and mice pair as «just works»; a
//! keyboard or phone that wants a PIN or a confirmation is not handled yet.
use crate::dbus::{Bus, Val};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

const BLUEZ: &str = "org.bluez";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dev {
    pub path: String,
    pub name: String,
    pub addr: String,
    /// freedesktop icon name (audio-headset, input-mouse, phone, ...).
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    pub rssi: Option<i32>,
    pub battery: Option<u32>,
}

impl Dev {
    /// What kind of device, in words.
    pub fn kind(&self) -> &'static str {
        match self.icon.as_str() {
            "audio-headset" | "audio-headphones" => "Наушники",
            "audio-card" | "audio-speakers" => "Колонка",
            "input-mouse" => "Мышь",
            "input-keyboard" => "Клавиатура",
            "input-gaming" => "Геймпад",
            "input-tablet" => "Планшет",
            "phone" => "Телефон",
            "computer" => "Компьютер",
            "video-display" => "Экран",
            _ => "Устройство",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// No adapter (or no BlueZ): why, for the page.
    pub missing: Option<String>,
    pub adapter: String,
    pub powered: bool,
    pub discovering: bool,
    /// Paired first (connected on top), then the others by signal.
    pub devices: Vec<Dev>,
    /// A device being paired or connected, and what is going on.
    pub busy: Option<(String, &'static str)>,
    /// The last thing that went wrong.
    pub error: Option<String>,
}

pub enum Cmd {
    Power(bool),
    Scan(bool),
    Connect(String),
    Disconnect(String),
    /// Pair, trust, connect.
    Pair(String),
    Forget(String),
}

pub struct Bt {
    tx: Sender<Cmd>,
    rx: Receiver<State>,
    pub state: State,
}

impl Bt {
    pub fn start() -> Bt {
        let (tx, crx) = channel();
        let (stx, rx) = channel();
        std::thread::spawn(move || worker(crx, stx));
        Bt { tx, rx, state: State::default() }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    /// The newest state from the thread; true if it changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(s) = self.rx.try_recv() {
            if s != self.state {
                self.state = s;
                changed = true;
            }
        }
        changed
    }
}

fn worker(rx: Receiver<Cmd>, tx: Sender<State>) {
    let mut bus: Option<Bus> = None;
    let mut st = State::default();
    let mut scan = false;
    loop {
        if bus.is_none() {
            match Bus::system() {
                Ok(b) => bus = Some(b),
                Err(e) => st.missing = Some(format!("Нет связи с системной шиной D-Bus: {e}")),
            }
        }
        let mut lost = false;
        if let Some(b) = bus.as_mut() {
            match read(b, &mut st) {
                Err(e) => {
                    lost = e.kind() != std::io::ErrorKind::NotFound;
                    st.missing = Some(e.to_string());
                }
                // Keep looking while asked (BlueZ stops after a while on some adapters).
                Ok(()) if scan && st.powered && !st.discovering && st.missing.is_none() => {
                    let _ = b.call(BLUEZ, &st.adapter, "org.bluez.Adapter1", "StartDiscovery", &[]);
                }
                Ok(()) => {}
            }
        }
        if lost {
            bus = None;
        }
        if tx.send(st.clone()).is_err() {
            break;
        }
        let cmd = match rx.recv_timeout(Duration::from_millis(if scan { 1000 } else { 2000 })) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let Some(b) = bus.as_mut() else { continue };
        if let Cmd::Scan(on) = cmd {
            scan = on;
        }
        st.error = run(b, &mut st, cmd, &tx).err().map(|e| nice_error(&e.to_string()));
        st.busy = None;
    }
    if let Some(b) = bus.as_mut() {
        if scan {
            let _ = b.call(BLUEZ, &st.adapter, "org.bluez.Adapter1", "StopDiscovery", &[]);
        }
    }
}

fn run(b: &mut Bus, st: &mut State, cmd: Cmd, tx: &Sender<State>) -> std::io::Result<()> {
    let adapter = st.adapter.clone();
    let dev = "org.bluez.Device1";
    let busy = |st: &mut State, path: &str, what: &'static str| {
        st.busy = Some((path.to_string(), what));
        let _ = tx.send(st.clone());
    };
    match cmd {
        Cmd::Power(on) => b.set_prop(BLUEZ, &adapter, "org.bluez.Adapter1", "Powered", Val::B(on)),
        Cmd::Scan(on) => {
            let m = if on { "StartDiscovery" } else { "StopDiscovery" };
            match b.call(BLUEZ, &adapter, "org.bluez.Adapter1", m, &[]) {
                // Already looking / not looking: fine.
                Err(e) if e.to_string().contains("InProgress") || e.to_string().contains("NotReady") || !on => Ok(()),
                r => r.map(|_| ()),
            }
        }
        Cmd::Connect(p) => {
            busy(st, &p, "Подключение…");
            long(b, |b| b.call(BLUEZ, &p, dev, "Connect", &[]).map(|_| ()))
        }
        Cmd::Disconnect(p) => {
            busy(st, &p, "Отключение…");
            long(b, |b| b.call(BLUEZ, &p, dev, "Disconnect", &[]).map(|_| ()))
        }
        Cmd::Pair(p) => {
            busy(st, &p, "Сопряжение…");
            long(b, |b| match b.call(BLUEZ, &p, dev, "Pair", &[]) {
                Err(e) if e.to_string().contains("AlreadyExists") => Ok(()),
                r => r.map(|_| ()),
            })?;
            // Trusted: it may connect back by itself next time (headphones do).
            let _ = b.set_prop(BLUEZ, &p, dev, "Trusted", Val::B(true));
            busy(st, &p, "Подключение…");
            long(b, |b| b.call(BLUEZ, &p, dev, "Connect", &[]).map(|_| ()))
        }
        Cmd::Forget(p) => b.call(BLUEZ, &adapter, "org.bluez.Adapter1", "RemoveDevice", &[Val::O(p)]).map(|_| ()),
    }
}

/// A call that may take a while (pairing, connecting): a longer reply timeout for it.
fn long(b: &mut Bus, f: impl FnOnce(&mut Bus) -> std::io::Result<()>) -> std::io::Result<()> {
    b.set_timeout(Duration::from_secs(40));
    let r = f(b);
    b.set_timeout(Duration::from_millis(1500));
    r
}

fn nice_error(e: &str) -> String {
    let what = if e.contains("AuthenticationFailed") || e.contains("AuthenticationRejected") {
        "Устройство не приняло сопряжение. Переведите его в режим сопряжения и попробуйте снова"
    } else if e.contains("AuthenticationCanceled") || e.contains("ConnectionAttemptFailed") || e.contains("page-timeout") || e.contains("Page Timeout") {
        "Устройство не ответило. Оно включено и рядом?"
    } else if e.contains("br-connection-profile-unavailable") || e.contains("profile-unavailable") {
        "Нет подходящего профиля. Для наушников нужен Bluetooth-звук PipeWire (пакет pipewire-audio)"
    } else if e.contains("timed out") || e.contains("WouldBlock") || e.contains("Resource temporarily") {
        "Устройство слишком долго не отвечает"
    } else if e.contains("NotReady") || e.contains("rfkill") || e.contains("Blocked") {
        "Bluetooth выключен или заблокирован (rfkill)"
    } else {
        return e.to_string();
    };
    what.to_string()
}

/// The adapter and every device BlueZ knows (ObjectManager.GetManagedObjects).
fn read(b: &mut Bus, st: &mut State) -> std::io::Result<()> {
    let v = b.call(BLUEZ, "/", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", &[]).map_err(|e| {
        let s = e.to_string();
        if s.contains("ServiceUnknown") || s.contains("NameHasNoOwner") {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Служба Bluetooth не запущена: sudo systemctl enable --now bluetooth")
        } else {
            e
        }
    })?;
    let objs = v.into_iter().next().unwrap_or(Val::A(String::new(), vec![]));
    let mut adapter: Option<(String, &Val)> = None;
    let mut devs = Vec::new();
    for o in objs.array() {
        let Val::E(path, ifaces) = o else { continue };
        let path = path.str().unwrap_or("").to_string();
        if let Some(a) = ifaces.get("org.bluez.Adapter1") {
            // The first adapter (hci0) is the one.
            if adapter.as_ref().map_or(true, |(p, _)| path < *p) {
                adapter = Some((path.clone(), a));
            }
        }
        if let Some(d) = ifaces.get("org.bluez.Device1") {
            let s = |k: &str| d.get(k).and_then(|v| v.str()).unwrap_or("").to_string();
            let flag = |k: &str| d.get(k).and_then(|v| v.bool()).unwrap_or(false);
            let rssi = d.get("RSSI").and_then(|v| match v.inner() {
                Val::I16(r) => Some(*r as i32),
                _ => None,
            });
            let battery = ifaces.get("org.bluez.Battery1").and_then(|bat| bat.get("Percentage")).and_then(|p| p.u32());
            let alias = s("Alias");
            let name = if s("Name").is_empty() && alias.replace('-', ":") == s("Address") { String::new() } else { alias };
            devs.push((s("Adapter"), Dev { path, name, addr: s("Address"), icon: s("Icon"), paired: flag("Paired"), connected: flag("Connected"), rssi, battery }));
        }
    }
    let Some((apath, a)) = adapter else {
        st.missing = Some("Нет адаптера Bluetooth".into());
        st.devices.clear();
        return Ok(());
    };
    st.missing = None;
    st.powered = a.get("Powered").and_then(|v| v.bool()).unwrap_or(false);
    st.discovering = a.get("Discovering").and_then(|v| v.bool()).unwrap_or(false);
    st.adapter = apath.clone();
    // Nameless devices nearby are just addresses: not worth a row unless paired.
    let mut list: Vec<Dev> = devs.into_iter().filter(|(ad, d)| *ad == apath && (d.paired || !d.name.is_empty())).map(|(_, d)| d).collect();
    list.sort_by(|x, y| {
        (!x.paired, !x.connected, -x.rssi.unwrap_or(-200), x.name.to_lowercase()).cmp(&(!y.paired, !y.connected, -y.rssi.unwrap_or(-200), y.name.to_lowercase()))
    });
    st.devices = list;
    Ok(())
}
