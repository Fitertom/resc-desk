//! Frame profile (`ctl profile`): how long each stage of a frame takes on each output, on
//! the machine itself (the KMS copies included). Off until `ctl profile on`; `ctl profile`
//! prints the stages since the last print (count, mean, p95-ish max) and starts over.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub static ON: AtomicBool = AtomicBool::new(false);

#[derive(Default, Clone)]
struct Stat {
    n: u64,
    sum: f64,
    max: f64,
    /// Durations seen (ms), for a percentile.
    all: Vec<f32>,
}

static STATS: Mutex<Vec<(String, Stat)>> = Mutex::new(Vec::new());
static SINCE: Mutex<Option<Instant>> = Mutex::new(None);

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Adds `ms` to stage `name` (e.g. "DP-3 gpu").
pub fn add(name: &str, ms: f64) {
    if !on() {
        return;
    }
    let Ok(mut v) = STATS.lock() else { return };
    let i = match v.iter().position(|(n, _)| n == name) {
        Some(i) => i,
        None => {
            v.push((name.to_string(), Stat::default()));
            v.len() - 1
        }
    };
    let s = &mut v[i].1;
    s.n += 1;
    s.sum += ms;
    s.max = s.max.max(ms);
    if s.all.len() < 100_000 {
        s.all.push(ms as f32);
    }
}

/// Times from `t` to now into stage `name`.
pub fn since(name: &str, t: Instant) {
    if on() {
        add(name, t.elapsed().as_secs_f64() * 1000.0);
    }
}

pub fn set(enable: bool) {
    ON.store(enable, Ordering::Relaxed);
    reset();
}

fn reset() {
    if let Ok(mut v) = STATS.lock() {
        v.clear();
    }
    if let Ok(mut s) = SINCE.lock() {
        *s = Some(Instant::now());
    }
}

/// The table since the last one, then starts over.
pub fn report() -> String {
    let secs = SINCE.lock().ok().and_then(|s| *s).map_or(0.0, |t| t.elapsed().as_secs_f64()).max(1e-6);
    let mut out = format!("{:.1} s{}\n", secs, if on() { "" } else { " (профиль выключен: ctl profile on)" });
    out.push_str(&format!("{:<28} {:>7} {:>8} {:>8} {:>8} {:>8}\n", "stage", "per s", "mean ms", "p95 ms", "max ms", "ms/s"));
    let mut v = STATS.lock().map(|v| v.clone()).unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, s) in v {
        let mut all = s.all.clone();
        all.sort_by(|a, b| a.total_cmp(b));
        let p95 = all.get(((all.len() as f64) * 0.95) as usize).or(all.last()).copied().unwrap_or(0.0);
        out.push_str(&format!(
            "{:<28} {:>7.1} {:>8.2} {:>8.2} {:>8.2} {:>8.1}\n",
            name,
            s.n as f64 / secs,
            s.sum / s.n.max(1) as f64,
            p95,
            s.max,
            s.sum / secs
        ));
    }
    reset();
    out
}
