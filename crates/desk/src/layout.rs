//! PHYSICAL MONITOR LAYOUT. Every output is a rectangle on one flat plane in
//! millimetres (left, top, width, height; top downward). The desktop's logical
//! coordinates ARE that plane, scaled by one factor `k` (logical px per mm):
//!
//!   logical = (mm - origin) * k,   output scale = device px / (plane width * k)
//!
//! so one logical unit is the same physical size on every monitor. A window keeps
//! its physical size and height when it crosses a joint, a window spanning two
//! monitors runs on straight across the bezel (the part behind the bezel is simply
//! not shown), and the pointer crosses at the physical height it left at.
//!
//! The plane comes from (in order): the saved calibration (~/.config/resc-desk/
//! displays.conf), LittleBigMouse's Current.xml (same idea as RESC's F11 cover
//! mode, crates/resc_window/src/physical.rs), the monitor's reported size (EDID),
//! or 96 dpi × the scale hint.
use crate::desk::Desk;
use std::collections::HashMap;

#[derive(Default)]
pub struct DisplayConfig {
    pub ui_scale: f64,
    pub planes: HashMap<String, [f64; 4]>,
    /// "Склеить по горизонтали": horizontal bezel gaps collapse (nothing hides behind
    /// a bezel); heights and physical scale stay as calibrated.
    pub glue: bool,
    pub wallpaper: Option<String>,
    /// Settings: frosted or clear glass, and the frost's sigma (CSS px).
    pub glass_frosted: bool,
    pub glass_blur: f32,
    /// The bezel's width (CSS px).
    pub glass_bezel: f32,
    pub glass_win_bezel: f32,
    /// How programs start and windows close (life.rs): 0 «из кнопки», 1 «из значка»;
    /// 0 into the button, 1 evaporates, 2 into the ✕.
    pub anim_launch: u8,
    pub anim_close: u8,
    /// Each monitor's own settings (monitors.rs), the main one and Win+P's way.
    pub mons: HashMap<String, crate::monitors::MonCfg>,
    pub primary: Option<String>,
    pub project: crate::monitors::Project,
}

/// Calibration mode state (Параметры → Мониторы → Калибровка).
pub struct Calib {
    pub sel: usize,
    pub backup: Vec<[f64; 4]>,
    pub note: String,
}

fn config_path() -> String {
    let base = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    format!("{base}/resc-desk/displays.conf")
}

pub fn load_config() -> DisplayConfig {
    let mut c = DisplayConfig {
        ui_scale: 1.0,
        planes: HashMap::new(),
        glue: false,
        wallpaper: None,
        glass_frosted: true,
        glass_blur: crate::glass::FROST,
        glass_bezel: crate::glass::BEZEL,
        glass_win_bezel: crate::glass::WIN_BEZEL,
        anim_launch: 0,
        anim_close: 0,
        mons: HashMap::new(),
        primary: None,
        project: Default::default(),
    };
    let Ok(text) = std::fs::read_to_string(config_path()) else { return c };
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("wallpaper ") {
            c.wallpaper = Some(p.trim().to_string());
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["ui_scale", v] => c.ui_scale = v.parse().unwrap_or(1.0f64).clamp(0.5, 4.0),
            ["glue", v] => c.glue = *v == "on",
            ["glass_frost", v] => c.glass_frosted = *v != "off",
            ["glass_win_bezel", v] => c.glass_win_bezel = v.parse().unwrap_or(crate::glass::WIN_BEZEL).clamp(0.0, crate::glass::MAX_BEZEL),
            ["glass_bezel", v] => c.glass_bezel = v.parse().unwrap_or(crate::glass::BEZEL).clamp(0.0, crate::glass::MAX_BEZEL),
            ["anim_launch", v] => c.anim_launch = v.parse::<u8>().unwrap_or(0).min(1),
            ["anim_close", v] => c.anim_close = v.parse::<u8>().unwrap_or(0).min(2),
            ["monitor", name, rest @ ..] => {
                c.mons.insert(name.to_string(), crate::monitors::MonCfg::parse(rest));
            }
            ["primary", name] => c.primary = Some(name.to_string()),
            ["project", p] => c.project = crate::monitors::Project::from_key(p).unwrap_or_default(),
            ["glass_blur", v] => c.glass_blur = v.parse().unwrap_or(crate::glass::FROST).clamp(0.0, crate::glass::MAX_BLUR),
            // A glass place's dim, blur and edge (look.rs).
            ["glass_place", k, dim, blur, bezel] => {
                if let (Some(p), Ok(dim), Ok(blur), Ok(bezel)) = (crate::look::Place::from_key(k), dim.parse(), blur.parse(), bezel.parse()) {
                    crate::look::set_glass(p, crate::look::Glass { dim, blur, bezel });
                }
            }
            ["output", name, x, y, w, h] => {
                let v: Vec<f64> = [x, y, w, h].iter().filter_map(|s| s.parse().ok()).collect();
                if v.len() == 4 && v[2] > 0.0 && v[3] > 0.0 {
                    c.planes.insert(name.to_string(), [v[0], v[1], v[2], v[3]]);
                }
            }
            _ => {}
        }
    }
    c
}

pub fn save_config(d: &Desk) -> std::io::Result<String> {
    let path = config_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut s = String::from("# resc-desk physical monitor layout, millimetres: output NAME left top width height\n");
    s.push_str(&format!("ui_scale {}\n", d.display_cfg.ui_scale));
    s.push_str(&format!("glue {}\n", if d.display_cfg.glue { "on" } else { "off" }));
    if let Some(w) = &d.display_cfg.wallpaper {
        s.push_str(&format!("wallpaper {w}\n"));
    }
    s.push_str(&format!("glass_frost {}\n", if d.display_cfg.glass_frosted { "on" } else { "off" }));
    s.push_str(&format!("glass_blur {}\n", d.display_cfg.glass_blur));
    s.push_str(&format!("glass_bezel {}\n", d.display_cfg.glass_bezel));
    s.push_str(&format!("glass_win_bezel {}\n", d.display_cfg.glass_win_bezel));
    for p in crate::look::PLACES {
        let g = crate::look::glass(p);
        s.push_str(&format!("glass_place {} {} {} {}\n", p.key(), g.dim, g.blur, g.bezel));
    }
    s.push_str(&format!("anim_launch {}\n", d.display_cfg.anim_launch));
    s.push_str(&format!("anim_close {}\n", d.display_cfg.anim_close));
    if let Some(p) = &d.display_cfg.primary {
        s.push_str(&format!("primary {p}\n"));
    }
    s.push_str(&format!("project {}\n", d.display_cfg.project.key()));
    let mut mons: Vec<_> = d.display_cfg.mons.iter().collect();
    mons.sort_by(|a, b| a.0.cmp(b.0));
    for (n, c) in mons {
        s.push_str(&c.to_line(n));
        s.push('\n');
    }
    // Keep entries of monitors that are not connected right now.
    let mut all = d.display_cfg.planes.clone();
    for o in &d.outputs {
        all.insert(o.name.clone(), o.plane);
    }
    let mut names: Vec<_> = all.keys().cloned().collect();
    names.sort();
    for n in names {
        let p = all[&n];
        s.push_str(&format!("output {n} {:.2} {:.2} {:.2} {:.2}\n", p[0], p[1], p[2], p[3]));
    }
    std::fs::write(&path, s)?;
    Ok(path)
}

/// Initial plane for a new output: saved → reported mm → 96 dpi × hint; placed
/// to the right of the others, top-aligned with the first one.
pub fn initial_plane(d: &Desk, name: &str, pw: i32, ph: i32, mm: Option<(f64, f64)>, scale_hint: f64, pos: Option<(f64, f64)>) -> [f64; 4] {
    if let Some(p) = d.display_cfg.planes.get(name) {
        return *p;
    }
    let (w, h) = mm.filter(|m| m.0 > 1.0 && m.1 > 1.0).unwrap_or_else(|| {
        let mmpp = 25.4 / 96.0 * scale_hint.max(0.25);
        (pw as f64 * mmpp, ph as f64 * mmpp)
    });
    let (x, y) = pos.unwrap_or_else(|| {
        let right = d.outputs.iter().map(|o| o.plane[0] + o.plane[2]).fold(f64::MIN, f64::max);
        let top = d.outputs.first().map_or(0.0, |o| o.plane[1]);
        (if right == f64::MIN { 0.0 } else { right }, top)
    });
    [x, y, w, h]
}

/// Recomputes every output's logical position and scale from the plane.
/// The main monitor is the reference: its scale is `ui_scale` and it sits at (0,0).
pub fn relayout(d: &mut Desk) {
    let main = crate::monitors::primary_index(d);
    let Some(p0) = d.outputs.get(main).map(|o| (o.plane, o.pw)) else { return };
    let (plane0, pw0) = p0;
    // Glue: shift every monitor left by the horizontal gaps before it.
    let shifts = glue_shifts(&d.outputs.iter().map(|o| o.plane).collect::<Vec<_>>(), d.display_cfg.glue);
    for (o, s) in d.outputs.iter_mut().zip(&shifts) {
        o.shift_mm = *s;
    }
    let k = (pw0 as f64 / plane0[2]) / d.display_cfg.ui_scale.max(0.25);
    d.mm_k = k;
    d.mm_origin = (plane0[0] - shifts[main], plane0[1]);
    let ox = d.mm_origin.0;
    let mut changed = Vec::new();
    for (i, o) in d.outputs.iter_mut().enumerate() {
        let x = ((o.plane[0] - o.shift_mm - ox) * k).round() as i32;
        let y = ((o.plane[1] - plane0[1]) * k).round() as i32;
        let scale = o.pw as f64 / (o.plane[2] * k);
        if x != o.x || y != o.y || (scale - o.scale).abs() > 1e-9 {
            o.x = x;
            o.y = y;
            o.scale = scale;
            changed.push(i);
        }
    }
    if d.display_cfg.glue {
        let rects: Vec<_> = d.outputs.iter().map(|o| o.rect()).collect();
        for (i, o) in d.outputs.iter_mut().enumerate() {
            if let Some(r) = rects.iter().enumerate().find(|(j, r)| *j != i && (r.x1() - o.x).abs() <= 2 && r.x1() != o.x).map(|(_, r)| r) {
                o.x = r.x1();
                if !changed.contains(&i) {
                    changed.push(i);
                }
            }
        }
    }
    d.wall.invalidate();
    if changed.is_empty() {
        return;
    }
    for i in changed {
        broadcast_output(d, i);
    }
    crate::wm::outputs_changed(d);
}

/// Horizontal shift (mm) per monitor that closes the horizontal gaps between the
/// monitors' x-extents (sweep left to right). Vertical placement is untouched.
pub fn glue_shifts(planes: &[[f64; 4]], glue: bool) -> Vec<f64> {
    let mut out = vec![0.0; planes.len()];
    if !glue {
        return out;
    }
    let mut order: Vec<usize> = (0..planes.len()).collect();
    order.sort_by(|a, b| planes[*a][0].partial_cmp(&planes[*b][0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut right = f64::MIN;
    let mut shift = 0.0;
    for i in order {
        let p = planes[i];
        if right != f64::MIN && p[0] > right {
            shift += p[0] - right;
        }
        out[i] = shift;
        right = right.max(p[0] + p[2]);
    }
    out
}

/// Re-sends wl_output / xdg_output state of output `idx` to every client.
pub fn broadcast_output(d: &mut Desk, idx: usize) {
    let id = d.outputs[idx].id;
    for ck in d.clients.keys() {
        let objs: Vec<_> = d.clients[ck].outputs.iter().filter(|(i, _)| *i == id).map(|(_, o)| *o).collect();
        for obj in objs {
            crate::server::send_output_info(d, ck, obj, idx, false);
        }
    }
}

// ------------------------------------------------------------------ calibration

pub fn toggle_calibration(d: &mut Desk) {
    if d.calib.is_some() {
        finish_calibration(d, false);
        return;
    }
    crate::shell::close_menus(d);
    let sel = d.output_at(d.seat.x, d.seat.y).unwrap_or(0);
    d.calib = Some(Calib { sel, backup: d.outputs.iter().map(|o| o.plane).collect(), note: String::new() });
    d.damage_all();
}

pub fn finish_calibration(d: &mut Desk, save: bool) {
    let Some(c) = d.calib.take() else { return };
    if save {
        match save_config(d) {
            Ok(p) => eprintln!("displays: saved {p}"),
            Err(e) => eprintln!("displays: save failed: {e}"),
        }
        for o in &d.outputs {
            d.display_cfg.planes.insert(o.name.clone(), o.plane);
        }
    } else {
        for (o, p) in d.outputs.iter_mut().zip(c.backup) {
            o.plane = p;
        }
        relayout(d);
    }
    d.damage_all();
}

/// Keyboard while calibrating. Arrows move the selected monitor (0.5 mm, Shift: 5 mm),
/// +/- resize it about its centre keeping the pixel aspect, Tab selects, Enter saves.
pub fn calib_key(d: &mut Desk, code: u32) {
    use crate::input::key;
    let shift = d.seat.held & 1 != 0;
    let step = if shift { 5.0 } else { 0.5 };
    let Some(c) = d.calib.as_mut() else { return };
    let n = d.outputs.len();
    let sel = c.sel.min(n.saturating_sub(1));
    match code {
        key::ESC => return finish_calibration(d, false),
        key::ENTER => return finish_calibration(d, true),
        key::TAB => {
            c.sel = (sel + 1) % n.max(1);
        }
        key::LEFT | key::RIGHT | key::UP | key::DOWN => {
            let p = &mut d.outputs[sel].plane;
            match code {
                key::LEFT => p[0] -= step,
                key::RIGHT => p[0] += step,
                key::UP => p[1] -= step,
                _ => p[1] += step,
            }
        }
        12 | 13 | 74 | 78 => {
            // minus, equal(+), keypad minus, keypad plus
            let grow = code == 13 || code == 78;
            let o = &mut d.outputs[sel];
            let aspect = o.ph as f64 / o.pw as f64;
            let p = &mut o.plane;
            let nw = (p[2] + if grow { step } else { -step }).max(20.0);
            let nh = nw * aspect;
            p[0] -= (nw - p[2]) / 2.0;
            p[1] -= (nh - p[3]) / 2.0;
            p[2] = nw;
            p[3] = nh;
        }
        34 => {
            // G: glue horizontally on/off
            d.display_cfg.glue = !d.display_cfg.glue;
        }
        38 => {
            // L: import LittleBigMouse
            let path = "/mnt/c/ProgramData/Mgth/LittleBigMouse/Current.xml";
            let note = match std::fs::read_to_string(path) {
                Ok(xml) => match import_lbm(d, &xml) {
                    Ok(n) => format!("LittleBigMouse: {n} мониторов"),
                    Err(e) => format!("LittleBigMouse: {e}"),
                },
                Err(_) => "LittleBigMouse: файл не найден".into(),
            };
            if let Some(c) = d.calib.as_mut() {
                c.note = note;
            }
        }
        _ => return,
    }
    relayout(d);
    d.damage_all();
}

// ------------------------------------------------------------------ LittleBigMouse

/// Applies LittleBigMouse's measured layout. Every output must be matched (by its
/// pixel size, in order), otherwise nothing changes: half mm / half pixels would
/// be a plane of two scales.
pub fn import_lbm(d: &mut Desk, xml: &str) -> Result<usize, String> {
    let zones = lbm_zones(xml);
    if zones.is_empty() {
        return Err("в файле нет зон".into());
    }
    let mut used = vec![false; zones.len()];
    let mut pick = Vec::new();
    for o in &d.outputs {
        let i = zones
            .iter()
            .enumerate()
            .position(|(i, z)| !used[i] && z.0[2] == o.pw as i64 && z.0[3] == o.ph as i64)
            .ok_or_else(|| format!("нет зоны {}x{} для {}", o.pw, o.ph, o.name))?;
        used[i] = true;
        pick.push(i);
    }
    let pixels: Vec<[i64; 4]> = pick.iter().map(|i| zones[*i].0).collect();
    let mut planes: Vec<[f64; 4]> = pick.iter().map(|i| zones[*i].1).collect();
    square_planes(&pixels, &mut planes);
    for (o, p) in d.outputs.iter_mut().zip(planes) {
        o.plane = p;
    }
    relayout(d);
    d.damage_all();
    Ok(pick.len())
}

/// Each zone's pixel rectangle and physical one (mm), read by hand.
fn lbm_zones(xml: &str) -> Vec<([i64; 4], [f64; 4])> {
    xml.split("<Zone ")
        .skip(1)
        .filter_map(|zone| {
            let px = lbm_rect(zone, "<PixelsBounds>")?;
            let mm = lbm_rect(zone, "<PhysicalBounds>")?;
            Some((px.map(|v| v.round() as i64), mm))
        })
        .filter(|(px, mm)| px[2] > 0 && px[3] > 0 && mm[2] > 0.0 && mm[3] > 0.0)
        .collect()
}

fn lbm_rect(zone: &str, tag: &str) -> Option<[f64; 4]> {
    let at = zone.find(tag)? + tag.len();
    let start = at + zone[at..].find("<Rect ")?;
    let end = start + zone[start..].find('>')?;
    let r = &zone[start..end];
    let attr = |name: &str| -> Option<f64> {
        let key = format!(" {name}=\"");
        let from = r.find(&key)? + key.len();
        r[from..from + r[from..].find('"')?].parse().ok()
    };
    Some([attr("Left")?, attr("Top")?, attr("Width")?, attr("Height")?])
}

/// LittleBigMouse's two sizes are not both trustworthy; the size ALONG the joint
/// with the primary is (the mouse crossing proves it), the other comes from the
/// pixel aspect, the edge facing the primary held. Same rule as RESC physical.rs.
fn square_planes(pixels: &[[i64; 4]], planes: &mut [[f64; 4]]) {
    let Some(first) = planes.first().copied() else { return };
    let primary = pixels.iter().position(|r| r[0] == 0 && r[1] == 0).map_or(first, |i| planes[i]);
    let (cx, cy) = (primary[0] + primary[2] * 0.5, primary[1] + primary[3] * 0.5);
    for (r, plane) in pixels.iter().zip(planes.iter_mut()) {
        let aspect = r[2] as f64 / r[3].max(1) as f64;
        let [l, t, w, h] = *plane;
        let (dx, dy) = (l + w * 0.5 - cx, t + h * 0.5 - cy);
        *plane = if dx.abs() >= dy.abs() {
            let nw = h * aspect;
            [if dx < 0.0 { l + w - nw } else if dx > 0.0 { l } else { l + (w - nw) * 0.5 }, t, nw, h]
        } else {
            let nh = w / aspect;
            [l, if dy < 0.0 { t + h - nh } else { t }, w, nh]
        };
    }
}

// ------------------------------------------------------------------ pointer across joints

/// The pointer left every output into a bezel gap: carry it across the gap along
/// the direction of motion to the monitor on the other side, at the same physical
/// height (LittleBigMouse-style). None when nothing is there (then it is clamped).
pub fn cross_gap(d: &Desk, from: (f64, f64), to: (f64, f64)) -> Option<(f64, f64)> {
    let cur = d.output_at(from.0, from.1)?;
    let c = d.outputs[cur].rect();
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let mut best: Option<(f64, (f64, f64))> = None;
    for (i, o) in d.outputs.iter().enumerate() {
        if i == cur {
            continue;
        }
        let r = o.rect();
        let cand = if dx > 0.0 && r.x >= c.x1() && to.1 >= r.y as f64 && to.1 < r.y1() as f64 {
            Some(((r.x - c.x1()) as f64, (r.x as f64 + (to.0 - c.x1() as f64).max(0.0), to.1)))
        } else if dx < 0.0 && r.x1() <= c.x && to.1 >= r.y as f64 && to.1 < r.y1() as f64 {
            Some(((c.x - r.x1()) as f64, (r.x1() as f64 - 1.0 - (c.x as f64 - to.0).max(0.0), to.1)))
        } else if dy > 0.0 && r.y >= c.y1() && to.0 >= r.x as f64 && to.0 < r.x1() as f64 {
            Some(((r.y - c.y1()) as f64, (to.0, r.y as f64 + (to.1 - c.y1() as f64).max(0.0))))
        } else if dy < 0.0 && r.y1() <= c.y && to.0 >= r.x as f64 && to.0 < r.x1() as f64 {
            Some(((c.y - r.y1()) as f64, (to.0, r.y1() as f64 - 1.0 - (c.y as f64 - to.1).max(0.0))))
        } else {
            None
        };
        if let Some((dist, p)) = cand {
            if best.map_or(true, |b| dist < b.0) {
                best = Some((dist, p));
            }
        }
    }
    let (_, (x, y)) = best?;
    // Land inside the target monitor.
    let o = d.output_at(x, y).or_else(|| d.outputs.iter().position(|o| o.rect().inflate(1).contains(x, y)))?;
    let r = d.outputs[o].rect();
    Some((x.clamp(r.x as f64, r.x1() as f64 - 1.0), y.clamp(r.y as f64, r.y1() as f64 - 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lbm_three_monitors_are_squared() {
        let pixels = [[0, 0, 3440, 1440], [3440, 364, 2560, 1080], [-1080, -154, 1080, 1920]];
        let mut planes = [[0.0, 0.31, 1048.0, 441.0], [1088.0, 201.32, 673.0, 241.4], [-336.0, -18.72, 296.0, 532.27]];
        square_planes(&pixels, &mut planes);
        assert!((planes[1][2] - 241.4 * 2560.0 / 1080.0).abs() < 1e-9);
        assert!((planes[2][0] + planes[2][2] - -40.0).abs() < 1e-9);
    }

    #[test]
    fn glue_closes_horizontal_gaps_only() {
        // portrait | 40 mm | primary | 37 mm | 25"
        let planes = [[0.0, 0.0, 1050.0, 441.0], [1088.0, 201.0, 572.0, 241.0], [-339.0, -18.0, 299.0, 532.0]];
        let s = glue_shifts(&planes, true);
        assert_eq!(s[2], 0.0);
        assert!((s[0] - 40.0).abs() < 1e-9); // primary moves left onto the portrait's edge
        assert!((s[1] - 78.0).abs() < 1e-9); // 25" closes both gaps
        assert_eq!(glue_shifts(&planes, false), vec![0.0; 3]);
    }
}
