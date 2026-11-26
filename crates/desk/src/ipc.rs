//! Control socket: scripted input, screenshots, introspection. One command per
//! connection, text protocol, answer ends with "ok" or "err ...".
use crate::backend::Backend;
use crate::desk::Desk;
use crate::input;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

pub fn accept(d: &mut Desk, b: &mut dyn Backend, l: &UnixListener) {
    loop {
        match l.accept() {
            Ok((s, _)) => handle(d, b, s),
            Err(_) => break,
        }
    }
}

fn handle(d: &mut Desk, b: &mut dyn Backend, s: UnixStream) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let mut line = String::new();
    let mut r = BufReader::new(&s);
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut out = String::new();
    let res = run(d, b, line.trim(), &mut out);
    match res {
        Ok(()) => out.push_str("ok\n"),
        Err(e) => out.push_str(&format!("err {e}\n")),
    }
    let mut w = &s;
    let _ = w.write_all(out.as_bytes());
}

fn btn(name: &str) -> Result<u32, String> {
    Ok(match name {
        "left" => input::BTN_LEFT,
        "right" => input::BTN_RIGHT,
        "middle" => input::BTN_MIDDLE,
        _ => match crate::mousebind::BUTTONS.iter().find(|b| b.0 == name && b.1 != 0) {
            Some(b) => b.1 as u32,
            None => return Err(format!("unknown button {name}")),
        },
    })
}

fn run(d: &mut Desk, b: &mut dyn Backend, cmd: &str, out: &mut String) -> Result<(), String> {
    let mut it = cmd.split_whitespace();
    let op = it.next().unwrap_or("");
    let rest: Vec<&str> = it.collect();
    let arg = |i: usize| rest.get(i).copied().ok_or_else(|| format!("missing argument {i}"));
    let t = d.now_ms();
    match op {
        "ping" => out.push_str("pong\n"),
        "quit" => d.running = false,
        "restart" => {
            d.restart = true;
            d.running = false;
        }
        "version" => out.push_str(&format!("{}
", crate::update::BUILD)),
        "spawn" => {
            let c = cmd.splitn(2, ' ').nth(1).ok_or("missing command")?;
            crate::life::launch_cmd(d, c);
        }
        "outputs" => {
            for (i, o) in d.outputs.iter().enumerate() {
                out.push_str(&format!(
                    "{i} {} {}x{} scale {} at {},{} logical {}x{} frames {}\n",
                    o.name, o.pw, o.ph, o.scale, o.x, o.y, o.lw(), o.lh(), o.frames
                ));
            }
        }
        "windows" => {
            for wk in d.stack.clone() {
                let w = &d.windows[wk];
                let g = d.surfaces[w.surf].geom();
                out.push_str(&format!(
                    "{} title={:?} app={:?} at {},{} size {}x{} tile={:?} min={} ws={} active={} ssd={}{}\n",
                    wk.idx,
                    w.title,
                    w.app_id,
                    w.x,
                    w.y,
                    g.w,
                    g.h,
                    w.tile,
                    w.minimized,
                    w.workspace,
                    w.activated,
                    w.ssd,
                    w.morph.map_or(String::new(), |m| format!(
                        " morph={:?}@{:.2} mapped={} buffer={}",
                        m.kind,
                        m.progress(),
                        d.surfaces[w.surf].mapped,
                        d.surfaces[w.surf].buffer.is_some()
                    ))
                ));
            }
        }
        // bench start SECONDS [move|idle|still]; bench: the result (bench.rs)
        "bench" => match rest.first().copied() {
            Some("start") => {
                let secs: f64 = arg(1)?.parse().map_err(|_| "bad seconds")?;
                let mode = match rest.get(2).copied().unwrap_or("move") {
                    "move" => crate::bench::Mode::Move,
                    "idle" => crate::bench::Mode::Idle,
                    "still" => crate::bench::Mode::Still,
                    _ => return Err("bench start SECONDS [move|idle|still]".into()),
                };
                crate::bench::start(d, secs.clamp(0.1, 600.0), mode);
            }
            _ => match (&d.bench.run, &d.bench.report) {
                (Some(_), _) => out.push_str("running
"),
                (None, Some(r)) => out.push_str(r),
                (None, None) => out.push_str("no bench yet
"),
            },
        },
        "stats" => {
            out.push_str(&format!(
                "clients {} surfaces {} windows {} workspace {}/{} pointer {:.0},{:.0}\n",
                d.clients.len(),
                d.surfaces.len(),
                d.windows.len(),
                d.workspace + 1,
                d.workspaces,
                d.seat.x,
                d.seat.y
            ));
            use std::sync::atomic::Ordering::Relaxed;
            out.push_str(&format!("glass: blurred {} kept {}
", crate::gpu::BLURS.load(Relaxed), crate::gpu::KEPT.load(Relaxed)));
        }
        // place-next APP X Y: the next window of APP opens with its point (X, Y) under the pointer.
        "place-next" => {
            let app = arg(0)?.to_string();
            let x: i32 = arg(1)?.parse().map_err(|_| "bad x")?;
            let y: i32 = arg(2)?.parse().map_err(|_| "bad y")?;
            d.place_next = Some((app, x, y, d.now_ms() + 5000));
        }
        // move-active X Y: the active window moves so its point (X, Y) is under the pointer.
        "move-active" => {
            let x: i32 = arg(0)?.parse().map_err(|_| "bad x")?;
            let y: i32 = arg(1)?.parse().map_err(|_| "bad y")?;
            crate::wm::move_active_to_pointer(d, x, y);
        }
        "move" => {
            let x: f64 = arg(0)?.parse().map_err(|_| "bad x")?;
            let y: f64 = arg(1)?.parse().map_err(|_| "bad y")?;
            input::pointer_motion(d, x, y, t);
        }
        "button" => {
            let bt = btn(arg(0)?)?;
            // As from the mouse: a bound button does its binding (mousebind.rs).
            let mut press = |d: &mut Desk, down: bool, t: u32| {
                if !crate::mousebind::button(d, bt as u16, down, t) {
                    input::pointer_button(d, bt, down, t);
                }
            };
            match rest.get(1).copied().unwrap_or("click") {
                "down" => press(d, true, t),
                "up" => press(d, false, t),
                _ => {
                    press(d, true, t);
                    press(d, false, t + 1);
                }
            }
        }
        "click" => {
            let x: f64 = arg(0)?.parse().map_err(|_| "bad x")?;
            let y: f64 = arg(1)?.parse().map_err(|_| "bad y")?;
            input::pointer_motion(d, x, y, t);
            let bt = btn(rest.get(2).copied().unwrap_or("left"))?;
            input::pointer_button(d, bt, true, t);
            input::pointer_button(d, bt, false, t + 1);
        }
        "drag" => {
            // drag x0 y0 x1 y1 [steps]
            let p: Vec<f64> = (0..4).map(|i| arg(i).and_then(|s| s.parse().map_err(|_| "bad coord".into()))).collect::<Result<_, String>>()?;
            let steps: usize = rest.get(4).and_then(|s| s.parse().ok()).unwrap_or(10);
            input::pointer_motion(d, p[0], p[1], t);
            input::pointer_button(d, input::BTN_LEFT, true, t);
            for i in 1..=steps {
                let k = i as f64 / steps as f64;
                input::pointer_motion(d, p[0] + (p[2] - p[0]) * k, p[1] + (p[3] - p[1]) * k, t + i as u32);
            }
            input::pointer_button(d, input::BTN_LEFT, false, t + steps as u32 + 1);
        }
        "scroll" => {
            let v: f64 = arg(0)?.parse().map_err(|_| "bad value")?;
            input::pointer_axis(d, t, 0, v * 10.0, Some((v * 120.0) as i32), Some(0));
        }
        "key" => {
            let code: u32 = arg(0)?.parse().map_err(|_| "bad keycode")?;
            match rest.get(1).copied().unwrap_or("tap") {
                "down" => input::key(d, code, true, t),
                "up" => input::key(d, code, false, t),
                _ => {
                    input::key(d, code, true, t);
                    input::key(d, code, false, t + 1);
                }
            }
        }
        "chord" => {
            let codes: Vec<u32> = arg(0)?.split('+').map(|s| s.parse().map_err(|_| format!("bad key {s}"))).collect::<Result<_, _>>()?;
            for c in &codes {
                input::key(d, *c, true, t);
            }
            for c in codes.iter().rev() {
                input::key(d, *c, false, t + 1);
            }
        }
        "calibrate" => crate::layout::toggle_calibration(d),
        "planes" => {
            out.push_str(&format!("k {:.4} px/mm, ui_scale {}\n", d.mm_k, d.display_cfg.ui_scale));
            for o in &d.outputs {
                let p = o.plane;
                out.push_str(&format!(
                    "{} plane {:.2},{:.2} {:.2}x{:.2} mm -> logical {},{} {}x{} scale {:.4}\n",
                    o.name, p[0], p[1], p[2], p[3], o.x, o.y, o.lw(), o.lh(), o.scale
                ));
            }
        }
        "set-plane" => {
            let name = arg(0)?;
            let v: Vec<f64> = (1..5).map(|i| arg(i).and_then(|s| s.parse().map_err(|_| "bad number".into()))).collect::<Result<_, String>>()?;
            let o = d.outputs.iter_mut().find(|o| o.name == name).ok_or("no such output")?;
            o.plane = [v[0], v[1], v[2], v[3]];
            crate::layout::relayout(d);
            d.damage_all();
        }
        "ui-scale" => {
            d.display_cfg.ui_scale = arg(0)?.parse::<f64>().map_err(|_| "bad scale")?.clamp(0.5, 4.0);
            crate::layout::relayout(d);
            d.damage_all();
        }
        "clip" => {
            for (i, e) in d.clip.items.iter().enumerate() {
                let t = String::from_utf8_lossy(&e.text).replace('\n', "\\n");
                out.push_str(&format!("{i}: {t}\n"));
            }
            if let Some(k) = d.selection {
                let own = d.sources.get(k).is_some_and(|s| s.own.is_some());
                out.push_str(&format!("selection: {}\n", if own { "compositor (history)" } else { "client" }));
            }
        }
        "glue" => {
            d.display_cfg.glue = match rest.first().copied() {
                Some("on") => true,
                Some("off") => false,
                _ => !d.display_cfg.glue,
            };
            crate::layout::relayout(d);
            d.damage_all();
            out.push_str(&format!("glue {}
", if d.display_cfg.glue { "on" } else { "off" }));
        }
        // livewall on|picture|off: the live wallpaper «Частицы» (on: on black, picture: over
        // the wallpaper's picture).
        "livewall" => {
            match rest.first().copied() {
                Some("on") | Some("particles") => crate::livewall::set(d, true, false),
                Some("picture") => crate::livewall::set(d, true, true),
                Some("off") => crate::livewall::set(d, false, false),
                _ => {}
            }
            let _ = crate::layout::save_config(d);
            out.push_str(match &d.live { Some(l) if l.on_picture => "livewall: picture\n", Some(_) => "livewall: on\n", None => "livewall: off\n" });
        }
        // launch-desktop X Y W H ICON NAME,NAME… -- CMD: a program started from an icon on the
        // desktop (X Y W H: the icon in the desktop window), growing out of it.
        "launch-desktop" => {
            let n = |i: usize| -> Result<i32, String> { arg(i)?.parse().map_err(|_| "bad rect".to_string()) };
            let r = crate::geom::Rect::new(n(0)?, n(1)?, n(2)?, n(3)?);
            let icon = arg(4)?.to_string();
            let names: Vec<String> = arg(5)?.split(',').map(str::to_string).collect();
            let sep = rest.iter().position(|a| *a == "--").ok_or("no --")?;
            let cmd = rest[sep + 1..].join(" ");
            let desk = d.windows.iter().find(|(_, w)| w.desktop()).map(|(_, w)| (w.x, w.y));
            let nm: Vec<&str> = names.iter().map(String::as_str).collect();
            match desk {
                Some((x, y)) => crate::life::launch_from(d, &cmd, &icon, &nm, r.translate(x, y)),
                None => crate::life::launch(d, &cmd, &icon, &nm, None),
            }
        }
        // mouse-bind NAME LAYER ACTION…: a mouse button's binding (mousebind.rs).
        "mouse-bind" => {
            let name = arg(0)?.to_string();
            let layer: u8 = arg(1)?.parse().map_err(|_| "bad layer")?;
            let act = crate::mousebind::Act::parse(&rest[2..].join(" "));
            crate::mousebind::set(d, &name, layer, act);
        }
        // media playpause|next|prev|stop|seek SECONDS: the player's (MPRIS, media.rs).
        "media" => {
            use crate::media::Cmd;
            let cmd = match arg(0)? {
                "playpause" => Cmd::PlayPause,
                "next" => Cmd::Next,
                "prev" | "previous" => Cmd::Previous,
                "stop" => Cmd::Stop,
                "seek" => Cmd::Seek(arg(1)?.parse().map_err(|_| "bad seconds")?),
                _ => return Err("media playpause|next|prev|stop|seek SECONDS".into()),
            };
            crate::media::send(cmd);
        }
        // fullscreen: the focused window on or off fullscreen (tests).
        "fullscreen" => {
            if let Some(wk) = crate::wm::focused(d) {
                let on = d.windows[wk].tile != crate::state::Tile::Fullscreen;
                crate::wm::fullscreen(d, wk, on, None);
            }
        }
        // scanout: per output, whether its window goes to the monitor as it is, or why not.
        "scanout" => {
            match rest.first().copied() {
                Some("off") => crate::render::SCANOUT.store(false, std::sync::atomic::Ordering::Relaxed),
                Some("on") => crate::render::SCANOUT.store(true, std::sync::atomic::Ordering::Relaxed),
                _ => {}
            }
            d.damage_all();
            out.push_str(if crate::render::SCANOUT.load(std::sync::atomic::Ordering::Relaxed) { "direct scanout: on\n" } else { "direct scanout: off\n" });
            for oi in 0..d.outputs.len() {
                let r = crate::render::scanout_check(d, oi, (256, 256));
                out.push_str(&format!("{}: {}\n", d.outputs[oi].name, match r { Ok(_) => "yes", Err(e) => e }));
            }
        }
        // profile on|off: frame stages timed; profile: the table since the last one.
        "profile" => match rest.first().copied() {
            Some("on") => crate::prof::set(true),
            Some("off") => crate::prof::set(false),
            _ => out.push_str(&crate::prof::report()),
        },
        "slowmo" => {
            let k: f32 = arg(0)?.parse().map_err(|_| "slowmo RATE (1: normal)".to_string())?;
            d.overlay.slowmo = if k > 0.0 && k != 1.0 { k.clamp(0.02, 4.0) } else { 0.0 };
            out.push_str("ok
");
        }
        "boot" => {
            crate::boot::start(d, 0);
            out.push_str("ok\n");
        }
        "glass" => {
            let frosted = match rest.first().copied() {
                Some("on") => true,
                Some("off") => false,
                _ => !crate::glass::frosted(),
            };
            let blur = rest.get(1).and_then(|v| v.parse::<f32>().ok()).unwrap_or(crate::glass::blur()).clamp(0.0, crate::glass::MAX_BLUR);
            crate::glass::set(frosted, blur);
            d.display_cfg.glass_frosted = frosted;
            d.display_cfg.glass_blur = blur;
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // monitor NAME on|off | mode WxH@HZ|auto | rot 0|90|180|270 | vrr on|off | hdr on|off
        "monitor" => {
            let usage = "monitor NAME on|off | mode WxH@HZ|auto | rot 0|90|180|270 | vrr on|off | hdr on|off";
            let name = arg(0)?.to_string();
            let mut c = d.display_cfg.mons.get(&name).cloned().unwrap_or_default();
            let on = |v: Option<&&str>| matches!(v.copied(), Some("on") | Some("1"));
            match rest.get(1).copied() {
                Some("on") => c.off = false,
                Some("off") => c.off = true,
                Some("mode") => c.mode = match rest.get(2).copied() {
                    Some("auto") => None,
                    Some(m) => Some(crate::monitors::parse_mode(m).ok_or(usage)?),
                    None => return Err(usage.into()),
                },
                Some("rot") => c.rot = rest.get(2).and_then(|r| r.parse::<u16>().ok()).filter(|r| [0, 90, 180, 270].contains(r)).ok_or(usage)?,
                Some("vrr") => c.vrr = on(rest.get(2)),
                Some("hdr") => c.hdr = on(rest.get(2)),
                _ => return Err(usage.into()),
            }
            d.display_cfg.mons.insert(name, c);
            b.reconfigure(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        "primary" => {
            d.display_cfg.primary = Some(arg(0)?.to_string());
            crate::layout::relayout(d);
            b.reconfigure(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // taskbar-autohide on|off: the taskbar hides until the pointer reaches the bottom edge
        "taskbar-autohide" => {
            d.display_cfg.taskbar_autohide = match arg(0)? {
                "on" => true,
                "off" => false,
                _ => return Err("taskbar-autohide on|off".into()),
            };
            crate::shell::taskbar_poke(d);
            crate::wm::outputs_changed(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // taskbar-desktop on|off: with autohide, it stays out on an empty desktop
        "taskbar-desktop" => {
            d.display_cfg.taskbar_keep_desktop = match arg(0)? {
                "on" => true,
                "off" => false,
                _ => return Err("taskbar-desktop on|off".into()),
            };
            crate::shell::taskbar_poke(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // taskbar all|main: on every monitor or the main one only
        "taskbar" => {
            d.display_cfg.taskbar_all = match arg(0)? {
                "all" => true,
                "main" => false,
                _ => return Err("taskbar all|main".into()),
            };
            crate::shell::close_menus(d);
            crate::tray::close(d);
            crate::wm::outputs_changed(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // project primary-only|duplicate|extend|second-only (Win+P)
        "project" => {
            let p = crate::monitors::Project::from_key(arg(0)?).ok_or("project primary-only|duplicate|extend|second-only")?;
            d.display_cfg.project = p;
            b.reconfigure(d);
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        "monitors" => {
            for m in b.monitors(d) {
                out.push_str(&format!(
                    "{} {} {}x{}@{:.2} rot {} vrr {}/{} hdr {}/{}{}\n",
                    m.name,
                    if m.on { "on" } else { "off" },
                    m.cur.0,
                    m.cur.1,
                    m.cur.2 as f64 / 1000.0,
                    m.rot,
                    m.vrr as u8,
                    m.vrr_cap as u8,
                    m.hdr as u8,
                    m.hdr_cap as u8,
                    m.mirror.map_or(String::new(), |s| format!(" mirror of {s}"))
                ));
            }
        }
        // glass-place PLACE DIM BLUR BEZEL: one glass place's look (look.rs).
        "glass-place" => {
            let usage = "glass-place files|settings|calendar|timer|windows|app:ID DIM(0..1) BLUR(px) BEZEL(px) | glass-place app:ID reset";
            let key = arg(0)?;
            if rest.get(1).copied() == Some("reset") {
                crate::look::set_app_glass(key.strip_prefix("app:").ok_or(usage)?, None);
            } else {
                let num = |i: usize| rest.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or(usage.to_string());
                let g = crate::look::Glass { dim: num(1)?, blur: num(2)?, bezel: num(3)? };
                if !crate::look::set_glass_key(key, g) {
                    return Err(usage.into());
                }
            }
            crate::look::publish();
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
            out.push_str("ok\n");
        }
        // bezel PANELS [WINDOWS]: the glass's bezel width, CSS px.
        "bezel" => {
            let num = |i: usize| rest.get(i).and_then(|v| v.parse::<f32>().ok());
            let p = num(0).ok_or("bezel: panels [windows], px")?.clamp(0.0, crate::glass::MAX_BEZEL);
            let w = num(1).unwrap_or(crate::glass::win_bezel()).clamp(0.0, crate::glass::MAX_BEZEL);
            crate::glass::set_bezel(p, w);
            d.display_cfg.glass_bezel = p;
            d.display_cfg.glass_win_bezel = w;
            d.damage_all();
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // anim launch 0|1 / anim close 0|1|2: how programs start and windows close (life.rs).
        "anim" => {
            let v = rest.get(1).and_then(|v| v.parse::<u8>().ok()).ok_or("anim launch|close N")?;
            match rest.first().copied() {
                Some("launch") => d.display_cfg.anim_launch = v.min(1),
                Some("close") => d.display_cfg.anim_close = v.min(2),
                _ => return Err("anim launch|close N".into()),
            }
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        "save" => {
            crate::layout::save_config(d).map_err(|e| e.to_string())?;
        }
        // Everything the settings app shows, one fact per line.
        "state" => {
            out.push_str(&format!("glass {} {}
", crate::glass::frosted() as u8, crate::glass::blur()));
            out.push_str(&format!("max_blur {}
", crate::glass::MAX_BLUR));
            out.push_str(&format!("bezel {} {} {}
", crate::glass::bezel(), crate::glass::win_bezel(), crate::glass::MAX_BEZEL));
            out.push_str(&format!("anim {} {}\n", d.display_cfg.anim_launch, d.display_cfg.anim_close));
            out.push_str(&format!("accent {:06x}\n", crate::look::accent() & 0xff_ffff));
            // Monitors: «mon NAME on W H MHZ ROT VRRCAP VRR HDRCAP HDR PRIMARY MIRROR», their modes.
            let main = d.outputs.get(crate::monitors::primary_index(d)).map(|o| o.name.clone());
            out.push_str(&format!("project {}\n", d.display_cfg.project.key()));
            out.push_str(&format!("taskbar {}\n", if d.display_cfg.taskbar_all { "all" } else { "main" }));
            out.push_str(&format!("taskbar_autohide {}\n", if d.display_cfg.taskbar_autohide { "on" } else { "off" }));
            out.push_str(&format!("taskbar_keep_desktop {}\n", if d.display_cfg.taskbar_keep_desktop { "on" } else { "off" }));
            out.push_str(match &d.live { Some(l) if l.on_picture => "livewall picture\n", Some(_) => "livewall on\n", None => "livewall off\n" });
            out.push_str(&crate::mousebind::state(d));
            for m in b.monitors(d) {
                out.push_str(&format!(
                    "mon {} {} {} {} {} {} {} {} {} {} {} {}\n",
                    m.name,
                    m.on as u8,
                    m.cur.0,
                    m.cur.1,
                    m.cur.2,
                    m.rot,
                    m.vrr_cap as u8,
                    m.vrr as u8,
                    m.hdr_cap as u8,
                    m.hdr as u8,
                    (main.as_deref() == Some(m.name.as_str())) as u8,
                    m.mirror.as_deref().unwrap_or("-")
                ));
                for (w, h, r) in &m.modes {
                    out.push_str(&format!("mode {} {w} {h} {r}\n", m.name));
                }
            }
            for p in crate::look::PLACES {
                let g = crate::look::glass(p);
                out.push_str(&format!("place {} {} {} {}\n", p.key(), g.dim, g.blur, g.bezel));
            }
            // Programs with glass of their own: the ones set apart, and the ones running now
            // (resc_glass_v1, Liquify), as «Окна» until set: place app:ID DIM BLUR BEZEL NAME.
            let mut apps: Vec<String> = crate::look::app_glasses().into_iter().map(|(a, _)| a).collect();
            for (wk, w) in d.windows.iter() {
                if !w.app_id.is_empty() && !apps.contains(&w.app_id) && crate::look::Place::of_app(&w.app_id) == crate::look::Place::Windows && (crate::look::glass_under(&w.app_id) || crate::render::window_surfaces(d, wk).iter().any(|(sk, _)| d.surfaces.get(*sk).is_some_and(|s| s.glass_obj != 0))) {
                    apps.push(w.app_id.clone());
                }
            }
            for a in apps {
                let g = crate::look::glass_of_app(&a);
                // Its name in the start menu (.desktop), else its id with a capital.
                let name = d
                    .apps
                    .iter()
                    .find(|x| x.id.eq_ignore_ascii_case(&a) || x.wmclass.eq_ignore_ascii_case(&a))
                    .map(|x| x.name.clone())
                    .unwrap_or_else(|| a.chars().take(1).flat_map(char::to_uppercase).chain(a.chars().skip(1)).collect());
                out.push_str(&format!("place app:{a} {} {} {} {}\n", g.dim, g.blur, g.bezel, name.replace('\n', " ")));
            }
            out.push_str(&format!("ui_scale {}
", d.display_cfg.ui_scale));
            out.push_str(&format!("wallpaper {}
", d.display_cfg.wallpaper.as_deref().unwrap_or("default")));
            for (o, w) in &d.display_cfg.wallpapers {
                out.push_str(&format!("wallpaper_on {o} {w}\n"));
            }
            out.push_str(&format!("glue {}
", d.display_cfg.glue as u8));
            out.push_str(&format!("renderer {}
", crate::gpu::renderer()));
            let x = d.xwm.as_ref().map_or("-".to_string(), |x| format!(":{}", x.display));
            out.push_str(&format!("session {} {}
", d.socket_name, x));
            for o in &d.outputs {
                out.push_str(&format!(
                    "output {} {} {} {} {} {} {} {} {}
",
                    o.name, o.pw, o.ph, o.refresh_mhz, o.plane[0], o.plane[1], o.plane[2], o.plane[3], o.scale
                ));
            }
        }
        // wallpaper PATH (the rest of the line, spaces allowed) | default
        "wallpaper" => {
            let p = rest.join(" ");
            if p.is_empty() {
                return Err("wallpaper: a path or default".into());
            }
            let p = if p == "default" { None } else { Some(p) };
            if let Some(f) = &p {
                std::fs::metadata(f).map_err(|e| format!("{f}: {e}"))?;
            }
            // Decoded on a thread; shown (and saved) when ready. Every monitor's: their own
            // pictures go.
            crate::wallpaper::set_all(d, p);
        }
        // wallpaper-on OUTPUT PATH|all: a monitor's own picture, or the common one again.
        "wallpaper-on" => {
            let usage = "wallpaper-on OUTPUT PATH|all";
            let o = arg(0)?.to_string();
            if !d.outputs.iter().any(|x| x.name == o) && !d.display_cfg.wallpapers.contains_key(&o) {
                return Err(format!("{o}: no such monitor"));
            }
            let p = rest[1..].join(" ");
            if p.is_empty() {
                return Err(usage.into());
            }
            if p == "all" {
                crate::wallpaper::set_for(d, &o, None);
            } else {
                std::fs::metadata(&p).map_err(|e| format!("{p}: {e}"))?;
                crate::wallpaper::set_for(d, &o, Some(p));
            }
            out.push_str("ok\n");
        }
        "import-lbm" => {
            let xml = std::fs::read_to_string(arg(0)?).map_err(|e| e.to_string())?;
            let n = crate::layout::import_lbm(d, &xml)?;
            out.push_str(&format!("matched {n} monitors\n"));
        }
        "screenshot" => {
            let path = arg(0)?;
            let which = rest.get(1).copied().unwrap_or("all");
            let step: usize = rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
            // Optional crop (logical): screenshot PATH all STEP X Y W H
            let crop: Vec<i32> = rest.iter().skip(3).filter_map(|s| s.parse().ok()).collect();
            if crop.len() == 4 {
                d.shot_crop = Some(crate::geom::Rect::new(crop[0], crop[1], crop[2], crop[3]));
            }
            let png = screenshot(d, b, which, step);
            d.shot_crop = None;
            let png = png?;
            std::fs::write(path, png).map_err(|e| e.to_string())?;
            out.push_str(&format!("wrote {path}\n"));
        }
        _ => return Err(format!("unknown command {op:?}")),
    }
    Ok(())
}

/// PNG of one output, or of the whole layout ("all", logical placement, scale 1).
pub fn screenshot(d: &mut Desk, b: &mut dyn Backend, which: &str, step: usize) -> Result<Vec<u8>, String> {
    let grab = |d: &mut Desk, b: &mut dyn Backend, oi: usize| -> (Vec<u32>, i32, i32) {
        // A host that stopped sending frames (a hidden nested window) leaves the last
        // shown frame stale: draw the current state instead.
        let o = &d.outputs[oi];
        let stale = o.frame_pending && sys::now_ns().saturating_sub(o.last_frame_ns) > 300_000_000;
        if stale {
            return crate::render::render_offscreen(d, oi);
        }
        b.read_pixels(d, oi).unwrap_or_else(|| crate::render::render_offscreen(d, oi))
    };
    if which != "all" {
        let oi: usize = which.parse().map_err(|_| "bad output index")?;
        if oi >= d.outputs.len() {
            return Err("no such output".into());
        }
        let (px, w, h) = grab(d, b, oi);
        return Ok(crate::png::encode(&px, w, h, w as usize, step));
    }
    // Whole layout as it stands on the physical plane: every output resampled to
    // logical units (so mixed densities line up), gaps between monitors dark grey.
    let bounds = d.shot_crop.unwrap_or_else(|| d.layout_bounds());
    let step = step.max(1) as i32;
    let (bw, bh) = ((bounds.w.max(1) + step - 1) / step, (bounds.h.max(1) + step - 1) / step);
    let mut img = vec![0xff30_3030u32; (bw * bh) as usize];
    for oi in 0..d.outputs.len() {
        let r = d.outputs[oi].rect();
        let s = d.outputs[oi].scale;
        let (px, w, h) = grab(d, b, oi);
        for iy in 0..bh {
            let ly = bounds.y + iy * step;
            if ly < r.y || ly >= r.y1() {
                continue;
            }
            let sy = (((ly - r.y) as f64 + 0.5) * s) as i32;
            if sy >= h {
                continue;
            }
            for ix in 0..bw {
                let lx = bounds.x + ix * step;
                if lx < r.x || lx >= r.x1() {
                    continue;
                }
                let sx = (((lx - r.x) as f64 + 0.5) * s) as i32;
                if sx < w {
                    img[(iy * bw + ix) as usize] = px[(sy * w + sx) as usize];
                }
            }
        }
    }
    Ok(crate::png::encode(&img, bw, bh, bw as usize, 1))
}

/// `resc-desk ctl <command...>`: send one command, print the answer.
pub fn ctl(args: &[String]) -> i32 {
    let path = std::env::var("RESC_DESK_CTL").ok().or_else(find_ctl);
    let Some(path) = path else {
        eprintln!("no resc-desk control socket found");
        return 1;
    };
    let mut s = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("connect {path}: {e}");
            return 1;
        }
    };
    let _ = s.write_all(format!("{}\n", args.join(" ")).as_bytes());
    let mut r = BufReader::new(s);
    let mut code = 0;
    let mut line = String::new();
    while r.read_line(&mut line).map_or(false, |n| n > 0) {
        print!("{line}");
        if line.starts_with("err") {
            code = 1;
        }
        line.clear();
    }
    code
}

/// One command to the running compositor; its answer (without the final "ok").
pub fn request(cmd: &str) -> Result<String, String> {
    let path = std::env::var("RESC_DESK_CTL").ok().or_else(find_ctl).ok_or("no resc-desk control socket")?;
    let mut s = UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    s.write_all(format!("{cmd}
").as_bytes()).map_err(|e| e.to_string())?;
    let mut text = String::new();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    while r.read_line(&mut line).map_or(false, |n| n > 0) {
        if line.starts_with("err") {
            return Err(line.trim().to_string());
        }
        if line.trim() != "ok" {
            text.push_str(&line);
        }
        line.clear();
    }
    Ok(text)
}

fn find_ctl() -> Option<String> {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path().to_string_lossy().to_string())
        .filter(|p| p.ends_with(".ctl") && p.contains("resc-desk"))
        .collect();
    v.sort();
    v.pop()
}
