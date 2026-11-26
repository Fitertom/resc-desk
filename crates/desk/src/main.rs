//! resc-desk: a Wayland compositor + desktop shell written from scratch in Rust.
mod anim;
mod prof;
mod xcursor;
mod apps;
mod backend;
mod bt;
mod cal;
mod clipboard;
mod client;
mod clock;
mod dbus;
mod desk;
mod dmabuf;
mod dnd;
mod files;
mod font;
mod geom;
mod glass;
mod gpu;
mod icons;
mod input;
mod ipc;
mod jpeg;
mod layout;
mod look;
mod monitors;
mod update;
mod bench;
mod net;
mod png;
mod pulse;
mod svg;
mod taskview;
mod tray;
mod render;
mod server;
mod settings;
mod screencopy;
mod shell;
mod shm;
mod snip;
mod osd;
mod slab;
mod boot;
mod life;
mod alttab;
mod start;
mod startanim;
mod state;
mod textedit;
mod wallpaper;
mod winanim;
mod wm;
mod x11;
mod xwm;

use backend::Backend;
use desk::*;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixListener;

const USAGE: &str = "resc-desk [--backend auto|nested|kms|headless] [--outputs 1920x1080,1280x1024@1.25] \
[--socket NAME] [--spawn CMD]... [--terminal CMD]\n       resc-desk ctl <command>   (ping, outputs, windows, \
stats, spawn CMD, move X Y, click X Y [btn], drag X0 Y0 X1 Y1, key CODE [down|up|tap], chord A+B, scroll V, \
screenshot PATH [all|N] [step], quit)";

fn default_keymap() -> Option<state::Keymap> {
    let text = "xkb_keymap {\n\
        xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
        xkb_types { include \"complete\" };\n\
        xkb_compat { include \"complete\" };\n\
        xkb_symbols { include \"pc+us+ru:2+inet(evdev)\" };\n\
        xkb_geometry { include \"pc(pc105)\" };\n\
        };\n";
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    let fd = sys::memfd("resc-desk-keymap", 0).ok()?;
    sys::write_all(fd.as_fd(), &bytes).ok()?;
    sys::seal_readonly(fd.as_fd());
    Some(state::Keymap { fd, size: bytes.len() as u32 })
}

fn bind_socket(dir: &str, wanted: Option<&str>) -> std::io::Result<(UnixListener, String, std::fs::File)> {
    let names: Vec<String> = match wanted {
        Some(n) => vec![n.to_string()],
        None => (1..32).map(|i| format!("wayland-{i}")).collect(),
    };
    for name in names {
        let path = format!("{dir}/{name}");
        let lock = std::fs::OpenOptions::new().create(true).write(true).open(format!("{path}.lock"))?;
        if !sys::try_lock(lock.as_fd()) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path)?;
        l.set_nonblocking(true)?;
        return Ok((l, name, lock));
    }
    Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "no free wayland socket name"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("ctl") {
        std::process::exit(ipc::ctl(&args[2..]));
    }
    // resc-desk decode IN OUT.png: our JPEG/PNG decoder on a file (checks).
    if args.get(1).map(String::as_str) == Some("decode") && args.len() == 4 {
        match wallpaper::decode_file(&args[2]) {
            Ok(img) => {
                let png = png::encode(&img.px, img.w as i32, img.h as i32, img.w, 1);
                std::process::exit(if std::fs::write(&args[3], png).is_ok() { 0 } else { 1 });
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    if args.get(1).map(String::as_str) == Some("update") {
        std::process::exit(update::cli(&args[2..]));
    }
    if matches!(args.get(1).map(String::as_str), Some("version") | Some("--version")) {
        println!("{}", update::BUILD);
        std::process::exit(0);
    }
    if args.get(1).map(String::as_str) == Some("net") {
        std::process::exit(net_cli());
    }
    if args.get(1).map(String::as_str) == Some("sound") {
        std::process::exit(sound_cli(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("files") {
        std::process::exit(files::run(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("settings") {
        std::process::exit(settings::run(&args[2..]));
    }
    let mut backend_kind = "auto".to_string();
    let mut outputs = String::from("1920x1080");
    let mut socket = None;
    let mut spawn = Vec::new();
    let mut terminal = std::env::var("RESC_TERMINAL").unwrap_or_else(|_| "foot".into());
    let mut i = 1;
    while i < args.len() {
        let val = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--backend" => backend_kind = val,
            "--outputs" => outputs = val,
            "--socket" => socket = Some(val),
            "--spawn" => spawn.push(val),
            "--terminal" => terminal = val,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    sys::ignore_sigpipe();
    font::init();

    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let d = format!("/tmp/resc-desk-{}", sys::uid());
        let _ = std::fs::create_dir_all(&d);
        d
    });
    // auto: inside another compositor when there is one, else the console itself (KMS).
    let nested = match backend_kind.as_str() {
        "nested" => true,
        "headless" | "kms" => false,
        _ => std::env::var("WAYLAND_DISPLAY").is_ok(),
    };
    let want_kms = backend_kind == "kms" || (backend_kind == "auto" && !nested);
    let (listener, socket_name, _lock) = match bind_socket(&runtime, socket.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cannot create wayland socket in {runtime}: {e}");
            std::process::exit(1);
        }
    };
    let socket_path = format!("{runtime}/{socket_name}");
    let ep = sys::Epoll::new().expect("epoll");
    ep.add(listener.as_raw_fd(), TOK_LISTENER, sys::EPOLLIN).expect("epoll add");

    let mut d = Desk {
        ep,
        listener,
        socket_name: socket_name.clone(),
        socket_path: socket_path.clone(),
        clients: Default::default(),
        surfaces: Default::default(),
        windows: Default::default(),
        sources: Default::default(),
        outputs: vec![],
        globals: vec![],
        stack: vec![],
        mru: vec![],
        workspace: 0,
        workspaces: 1,
        seat: Default::default(),
        serial: 0,
        keymap: None,
        repeat: (30, 400),
        selection: None,
        dnd: None,
        place_next: None,
        primary: None,
        children: vec![],
        running: true,
        restart: false,
        next_output_id: 1,
        start_ns: sys::now_ns(),
        cfg: Config {
            terminal: terminal.clone(),
            launcher: std::env::var("RESC_LAUNCHER").unwrap_or_else(|_| "fuzzel || wofi --show drun".into()),
            files: std::env::var("RESC_FILES").unwrap_or_else(|_| {
                let exe = update::exe_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "resc-desk".into());
                format!("'{}' files", exe.replace('\'', r"'\''"))
            }),
            nested,
        },
        overlay: Default::default(),
        cursor_drawn: Default::default(),
        show_desktop: vec![],
        overlay_hover: None,
        start_hover: None,
        start_field: Default::default(),
        start_sel: 0,
        start: Default::default(),
        tray: Default::default(),
        apps: vec![],
        pending_screenshot: None,
        screencopy: Vec::new(),
        display_cfg: layout::load_config(),
        mm_k: 1.0,
        mm_origin: (0.0, 0.0),
        calib: None,
        snip: Default::default(),
        osd: Default::default(),
        shot_crop: None,
        wall: Default::default(),
        clip: Default::default(),
        mons: Default::default(),
        gpu: None,
        bench: Default::default(),
        xwm: None,
        x_receives: vec![],
        dmabuf: None,
    };
    glass::set(d.display_cfg.glass_frosted, d.display_cfg.glass_blur);
    glass::set_bezel(d.display_cfg.glass_bezel, d.display_cfg.glass_win_bezel);
    // The built-in wallpaper until the picture is decoded (on a thread).
    wallpaper::update_accent(&mut d);
    if let Some(p) = d.display_cfg.wallpaper.clone() {
        wallpaper::set(&mut d, Some(p));
    }
    server::advertised_globals(&mut d);

    let mut be: Box<dyn Backend> = if nested {
        match backend::nested::Nested::new(&mut d, &outputs) {
            Ok(b) => Box::new(b),
            Err(e) => {
                eprintln!("nested backend failed ({e}); falling back to headless");
                d.cfg.nested = false;
                Box::new(backend::headless::Headless::new(&mut d, &outputs).expect("headless"))
            }
        }
    } else if want_kms {
        match backend::kms::Kms::new(&mut d) {
            Ok(b) => Box::new(b),
            Err(e) if backend_kind == "kms" || e.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("kms backend failed: {e}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("kms backend unavailable ({e}); running headless");
                Box::new(backend::headless::Headless::new(&mut d, &outputs).expect("headless"))
            }
        }
    } else {
        Box::new(backend::headless::Headless::new(&mut d, &outputs).expect("headless"))
    };
    if d.keymap.is_none() {
        d.keymap = default_keymap();
    }
    d.gpu = gpu::init();
    dmabuf::init(&mut d);
    if let Some((x, y)) = d.outputs.get(monitors::primary_index(&d)).map(|o| o.rect().center()) {
        d.seat.x = x as f64;
        d.seat.y = y as f64;
    }

    let ctl_path = format!("{runtime}/resc-desk-{socket_name}.ctl");
    let _ = std::fs::remove_file(&ctl_path);
    let ctl = UnixListener::bind(&ctl_path).ok();
    if let Some(c) = &ctl {
        let _ = c.set_nonblocking(true);
        let _ = d.ep.add(c.as_raw_fd(), TOK_IPC, sys::EPOLLIN);
    }
    let house = sys::TimerFd::new().expect("timerfd");
    house.arm(1_000_000_000, 1_000_000_000);
    let _ = d.ep.add(house.fd.as_raw_fd(), TOK_TIMER, sys::EPOLLIN);

    xwm::init(&mut d);
    tray::init(&mut d);
    if std::env::var("RESC_BOOT").map_or(true, |v| v != "0") {
        boot::start(&mut d, spawn.len());
    }
    let x = d.xwm.as_ref().map(|x| format!(", X programs on DISPLAY=:{}", x.display)).unwrap_or_default();
    println!("resc-desk running on WAYLAND_DISPLAY={socket_name} ({} backend){x}, control: {ctl_path}", be.name());
    // The desktop's icons («Проводник» in desktop mode); RESC_DESKTOP=0 for none.
    if std::env::var("RESC_DESKTOP").map_or(true, |v| v != "0") {
        if let Ok(exe) = update::exe_path() {
            d.spawn(&format!("'{}' files --desktop", exe.to_string_lossy().replace('\'', r"'\''")));
        }
    }
    // Screen sharing: the portal (started by D-Bus) must find this desktop.
    screencopy::setup_portal(&mut d);
    if be.name() != "headless" {
        bt::autoconnect();
    }
    for c in spawn {
        d.spawn(&c);
    }

    let mut evs = Vec::with_capacity(64);
    let mut last_minute = clock::local_now().1;
    while d.running {
        let needs_paint = d.outputs.iter().any(|o| !o.frame_pending && (!o.damage.is_empty() || o.wants_frame));
        let timeout = if needs_paint { 0 } else if d.overlay.animating { 8 } else { -1 };
        if let Err(e) = d.ep.wait(timeout, &mut evs) {
            eprintln!("epoll: {e}");
            break;
        }
        let t_ev = std::time::Instant::now();
        for &(tok, ev) in evs.iter() {
            match tok & TOK_KIND_MASK {
                0 => match tok {
                    TOK_LISTENER => d.accept_clients(),
                    TOK_IPC => {
                        if let Some(c) = &ctl {
                            ipc::accept(&mut d, be.as_mut(), c);
                        }
                    }
                    TOK_TIMER => {
                        house.read();
                        housekeeping(&mut d, &mut last_minute);
                        be.housekeeping(&mut d);
                    }
                    _ => {}
                },
                TOK_BACKEND => be.dispatch(&mut d, tok),
                TOK_CLIP => clipboard::readable(&mut d, tok),
                TOK_XWM => xwm::dispatch(&mut d, tok, ev),
                TOK_PULSE => tray::readable(&mut d),
                TOK_CLIENT => {
                    let ck = token_client(tok);
                    if ev & sys::EPOLLIN != 0 {
                        server::client_readable(&mut d, ck);
                    }
                    if ev & (sys::EPOLLHUP | sys::EPOLLERR) != 0 {
                        if let Some(c) = d.clients.get_mut(ck) {
                            c.kill("eof".into());
                        }
                    }
                }
                _ => {}
            }
        }
        prof::since("loop: events", t_ev);
        let t_tick = std::time::Instant::now();
        d.overlay.animating = shell::tick(&mut d);
        prof::since("loop: tick", t_tick);
        // X windows follow where they were moved, resized, focused (before drawing them).
        xwm::sync(&mut d);
        bench::step(&mut d);
        // Repaint outputs that are ready and have something to show.
        for oi in 0..d.outputs.len() {
            let o = &d.outputs[oi];
            if !o.frame_pending && (!o.damage.is_empty() || o.wants_frame) {
                let t0 = sys::now_ns();
                let tp = std::time::Instant::now();
                be.present(&mut d, oi);
                if prof::on() {
                    prof::since(&format!("{} present", d.outputs[oi].name), tp);
                }
                if d.bench.run.is_some() {
                    bench::presented(&mut d, oi, t0);
                }
            }
        }
        screencopy::process(&mut d, be.as_mut());
        snip::process(&mut d, be.as_mut());
        if let Some(path) = d.pending_screenshot.take() {
            if let Ok(png) = ipc::screenshot(&mut d, be.as_mut(), "all", 1) {
                let _ = std::fs::write(&path, png);
                eprintln!("screenshot saved to {path}");
            }
        }
        d.flush_clients();
        // Monitor settings changed (Win+P, the settings app): the backend applies them.
        if d.mons.reconfigure {
            d.mons.reconfigure = false;
            be.reconfigure(&mut d);
            d.damage_all();
            let _ = layout::save_config(&d);
        }
        be.flush();
    }
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(format!("{socket_path}.lock"));
    let _ = std::fs::remove_file(&ctl_path);
    // `ctl restart` (after an update): the screen and the devices let go, then the program
    // file as it is now takes this process's place, with the same arguments.
    if d.restart {
        drop(be);
        drop(d);
        use std::os::unix::process::CommandExt;
        let exe = update::exe_path().unwrap_or_else(|_| "resc-desk".into());
        eprintln!("resc-desk: restarting as {}", exe.display());
        let err = std::process::Command::new(&exe).args(&args[1..]).exec();
        eprintln!("resc-desk: restart failed: {err}; trying {}", args[0]);
        let err = std::process::Command::new(&args[0]).args(&args[1..]).exec();
        eprintln!("resc-desk: restart failed: {err}");
    }
}

fn housekeeping(d: &mut Desk, last_minute: &mut u32) {
    d.reap_children();
    wallpaper::poll(d);
    tray::housekeeping(d);
    // Throttle hidden surfaces to 1 fps instead of starving them.
    let now = d.now_ms();
    let hidden: Vec<state::SKey> = d
        .surfaces
        .iter()
        .filter(|(_, s)| !s.frame_cbs.is_empty())
        .map(|(k, _)| k)
        .filter(|k| !d.surface_visible(*k))
        .collect();
    for sk in hidden {
        render::send_frame_done(d, sk, now);
    }
    let m = clock::local_now().1;
    if m != *last_minute {
        *last_minute = m;
        shell::damage_clocks(d);
    }
}

/// `resc-desk sound [volume N | mute | unmute | default NAME]`: the PulseAudio client from the
/// command line (prints the outputs and inputs).
fn sound_cli(args: &[String]) -> i32 {
    let mut p = match pulse::Pulse::connect("resc-desk sound") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sound: {e}");
            return 1;
        }
    };
    let run = |p: &mut pulse::Pulse| -> std::io::Result<()> {
        let a = p.state()?;
        let sink = a.sink().cloned();
        match (args.first().map(String::as_str), args.get(1)) {
            (Some("volume"), Some(v)) => p.set_volume(&sink.ok_or_else(|| std::io::Error::other("no output"))?, false, v.parse().unwrap_or(50))?,
            (Some("mute"), _) => p.set_mute(&sink.ok_or_else(|| std::io::Error::other("no output"))?, false, true)?,
            (Some("unmute"), _) => p.set_mute(&sink.ok_or_else(|| std::io::Error::other("no output"))?, false, false)?,
            (Some("default"), Some(n)) => p.set_default(n, false)?,
            _ => {}
        }
        let a = p.state()?;
        for s in &a.sinks {
            println!("{} out {} \"{}\" {}%{}", if s.name == a.default_sink { "*" } else { " " }, s.index, s.desc, s.percent(), if s.mute { " muted" } else { "" });
        }
        for s in &a.sources {
            println!("{} in  {} \"{}\" {}%{}", if s.name == a.default_source { "*" } else { " " }, s.index, s.desc, s.percent(), if s.mute { " muted" } else { "" });
        }
        Ok(())
    };
    match run(&mut p) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("sound: {e}");
            1
        }
    }
}

/// `resc-desk net`: what the tray's network flyout sees (NetworkManager over D-Bus).
fn net_cli() -> i32 {
    let res = dbus::Bus::system().and_then(|mut b| net::state(&mut b, true));
    match res {
        Ok(n) => {
            println!("wifi {} dev {:?} state {} wired {}", if n.wifi_enabled { "on" } else { "off" }, n.wifi_dev, n.wifi_state, n.wired);
            for a in &n.aps {
                println!("{} {:3}% {}{}{} {}", if a.active { "*" } else { " " }, a.strength, if a.secure { "secured" } else { "open" }, if a.saved { " saved" } else { "" }, if a.sae { " wpa3" } else { "" }, a.ssid);
            }
            0
        }
        Err(e) => {
            eprintln!("net: {e}");
            1
        }
    }
}
