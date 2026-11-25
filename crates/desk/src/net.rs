//! Networking through NetworkManager over our D-Bus client (dbus.rs): Wi-Fi on/off, the
//! networks around with their signal, connect (a saved profile, an open network, or a
//! password for WPA/WPA3-Personal), disconnect; whether a cable is plugged in.

use crate::dbus::{dict, Bus, Val};
use std::io::Result;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEV: &str = "org.freedesktop.NetworkManager.Device";
const WIFI: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const CONN: &str = "org.freedesktop.NetworkManager.Settings.Connection";

/// NM_DEVICE_STATE_*.
pub const DEV_ACTIVATED: u32 = 100;
pub const DEV_FAILED: u32 = 120;
pub const DEV_DISCONNECTED: u32 = 30;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ap {
    pub path: String,
    pub ssid: String,
    /// 0–100.
    pub strength: u8,
    pub secure: bool,
    /// WPA3 only (SAE, no PSK).
    pub sae: bool,
    /// 802.1X (enterprise): not ours to connect.
    pub enterprise: bool,
    pub active: bool,
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    /// NetworkManager answers.
    pub available: bool,
    pub wifi_enabled: bool,
    /// The Wi-Fi device (D-Bus path), its state.
    pub wifi_dev: Option<String>,
    pub wifi_state: u32,
    /// An Ethernet device is connected.
    pub wired: bool,
    pub aps: Vec<Ap>,
}

impl Net {
    pub fn active(&self) -> Option<&Ap> {
        self.aps.iter().find(|a| a.active)
    }
    /// The Wi-Fi device is connecting (prepare … secondaries).
    pub fn connecting(&self) -> bool {
        (40..DEV_ACTIVATED).contains(&self.wifi_state)
    }
}

fn ssid_of(v: &Val) -> String {
    String::from_utf8_lossy(&v.bytes()).into_owned()
}

/// Saved Wi-Fi profiles: (SSID, connection path).
pub fn saved(bus: &mut Bus) -> Result<Vec<(String, String)>> {
    let list = bus.call(NM, SETTINGS_PATH, SETTINGS, "ListConnections", &[])?;
    let mut out = Vec::new();
    for c in list.first().map(|v| v.array().to_vec()).unwrap_or_default() {
        let Some(path) = c.str() else { continue };
        let Ok(s) = bus.call(NM, path, CONN, "GetSettings", &[]) else { continue };
        let Some(wifi) = s.first().and_then(|s| s.get("802-11-wireless")) else { continue };
        if let Some(ssid) = wifi.get("ssid") {
            out.push((ssid_of(ssid), path.to_string()));
        }
    }
    Ok(out)
}

/// Everything the tray shows. `with_saved`: also which networks have a profile (a call
/// per profile: only while the flyout is open).
pub fn state(bus: &mut Bus, with_saved: bool) -> Result<Net> {
    let mut n = Net { available: true, ..Default::default() };
    n.wifi_enabled = bus.prop(NM, NM_PATH, NM, "WirelessEnabled")?.bool().unwrap_or(false);
    let devs = bus.call(NM, NM_PATH, NM, "GetDevices", &[])?;
    for d in devs.first().map(|v| v.array().to_vec()).unwrap_or_default() {
        let Some(path) = d.str() else { continue };
        let p = bus.props(NM, path, DEV)?;
        let ty = p.get("DeviceType").and_then(|v| v.u32()).unwrap_or(0);
        let st = p.get("State").and_then(|v| v.u32()).unwrap_or(0);
        match ty {
            1 if st == DEV_ACTIVATED => n.wired = true,
            2 if n.wifi_dev.is_none() => {
                n.wifi_dev = Some(path.to_string());
                n.wifi_state = st;
            }
            _ => {}
        }
    }
    let Some(dev) = n.wifi_dev.clone() else { return Ok(n) };
    let active = bus.prop(NM, &dev, WIFI, "ActiveAccessPoint")?.str().unwrap_or("/").to_string();
    let aps = bus.call(NM, &dev, WIFI, "GetAllAccessPoints", &[])?;
    let saved = if with_saved { saved(bus).unwrap_or_default() } else { Vec::new() };
    for a in aps.first().map(|v| v.array().to_vec()).unwrap_or_default() {
        let Some(path) = a.str() else { continue };
        let Ok(p) = bus.props(NM, path, AP) else { continue };
        let ssid = p.get("Ssid").map(ssid_of).unwrap_or_default();
        if ssid.is_empty() {
            continue;
        }
        let flags = p.get("Flags").and_then(|v| v.u32()).unwrap_or(0);
        let wpa = p.get("WpaFlags").and_then(|v| v.u32()).unwrap_or(0);
        let rsn = p.get("RsnFlags").and_then(|v| v.u32()).unwrap_or(0);
        let key = wpa | rsn;
        let ap = Ap {
            path: path.to_string(),
            strength: p.get("Strength").and_then(|v| v.u32()).unwrap_or(0) as u8,
            secure: flags & 1 != 0 || key != 0,
            sae: key & 0x400 != 0 && key & 0x100 == 0,
            enterprise: key & 0x200 != 0,
            active: path == active,
            saved: saved.iter().any(|(s, _)| *s == ssid),
            ssid,
        };
        // One row per network: the active access point, else the strongest.
        match n.aps.iter_mut().find(|x| x.ssid == ap.ssid) {
            Some(x) => {
                if ap.active || (!x.active && ap.strength > x.strength) {
                    let saved = x.saved;
                    *x = ap;
                    x.saved |= saved;
                }
            }
            None => n.aps.push(ap),
        }
    }
    n.aps.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)));
    Ok(n)
}

pub fn set_wifi(bus: &mut Bus, on: bool) -> Result<()> {
    bus.set_prop(NM, NM_PATH, NM, "WirelessEnabled", Val::B(on))
}

pub fn scan(bus: &mut Bus, dev: &str) -> Result<()> {
    bus.call(NM, dev, WIFI, "RequestScan", &[dict(vec![])]).map(|_| ())
}

pub fn disconnect(bus: &mut Bus, dev: &str) -> Result<()> {
    bus.call(NM, dev, DEV, "Disconnect", &[]).map(|_| ())
}

/// Connects to `ap`: its saved profile, or a new one (with `password` for a secured
/// network; a wrong old profile is replaced when a password is given).
pub fn connect(bus: &mut Bus, dev: &str, ap: &Ap, password: Option<&str>) -> Result<()> {
    let profiles = saved(bus).unwrap_or_default();
    let mine: Vec<&String> = profiles.iter().filter(|(s, _)| *s == ap.ssid).map(|(_, p)| p).collect();
    if password.is_none() {
        if let Some(p) = mine.first() {
            let args = [Val::O((*p).clone()), Val::O(dev.into()), Val::O(ap.path.clone())];
            return bus.call(NM, NM_PATH, NM, "ActivateConnection", &args).map(|_| ());
        }
    } else {
        for p in mine {
            let _ = bus.call(NM, p, CONN, "Delete", &[]);
        }
    }
    let mut groups = Vec::new();
    if let Some(pw) = password {
        let key = if ap.sae { "sae" } else { "wpa-psk" };
        groups.push(Val::E(Box::new(Val::S("802-11-wireless-security".into())), Box::new(dict(vec![("key-mgmt", Val::S(key.into())), ("psk", Val::S(pw.into()))]))));
    }
    let settings = Val::A("{sa{sv}}".into(), groups);
    bus.call(NM, NM_PATH, NM, "AddAndActivateConnection", &[settings, Val::O(dev.into()), Val::O(ap.path.clone())]).map(|_| ())
}
