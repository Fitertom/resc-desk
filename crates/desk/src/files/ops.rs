//! File operations on worker threads, as in Windows' copy dialog: progress in bytes,
//! speed, pause, cancel, and a question per name clash (replace / skip / keep both,
//! «для всех»). Archives go through 7-Zip (`7z`, the Arch package `7zip`) as a child
//! process whose percentage we read; pause stops it with SIGSTOP.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// Tests: the folder every operation must stay in.
pub static SANDBOX: Mutex<Option<PathBuf>> = Mutex::new(None);

pub enum Job {
    Copy { items: Vec<PathBuf>, dest: PathBuf },
    Move { items: Vec<PathBuf>, dest: PathBuf },
    /// Each back to its own place (undoing a move).
    MoveBack(Vec<(PathBuf, PathBuf)>),
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    /// Trashed files back to where they were.
    Restore(Vec<PathBuf>),
    /// 7-Zip with these arguments, in `cwd`; `made`: what it creates (for undo, selection).
    Zip { args: Vec<String>, cwd: PathBuf, made: Option<PathBuf>, password: Option<String> },
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub dir: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Answer {
    Replace,
    Skip,
    Both,
    Cancel,
}

/// What undoes an operation.
#[derive(Clone, Debug)]
pub enum Undo {
    /// (from, to): moved from → to.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// New things (copies, archives, folders): undo trashes them.
    Made(Vec<PathBuf>),
    /// Trash files: undo restores them.
    Trashed(Vec<PathBuf>),
    Renamed(PathBuf, PathBuf),
}

#[derive(Default)]
pub struct Prog {
    /// Bytes (or items, or percent for 7-Zip): done of total.
    pub total: u64,
    pub done: u64,
    pub items_total: u64,
    pub items_done: u64,
    pub bytes: bool,
    pub current: String,
    pub scanning: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub errors: Vec<String>,
    pub ask: Option<Conflict>,
    pub need_password: bool,
    /// Could not go to the trash (another disk without one, no rights): delete for good?
    pub no_trash: Vec<PathBuf>,
    pub undo: Option<Undo>,
    /// The new top-level things, to select when done.
    pub made: Vec<PathBuf>,
}

pub struct Op {
    pub id: u32,
    pub verb: &'static str,
    pub title: String,
    pub dest: Option<PathBuf>,
    pub prog: Arc<Mutex<Prog>>,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
    answer: mpsc::Sender<(Answer, bool)>,
    pub started: u64,
    /// Bytes per second, sampled 4 times a second (the graph).
    pub speed: Vec<f32>,
    last: (u64, u64),
    /// Kept for a retry with a password.
    pub zip: Option<(Vec<String>, PathBuf, Option<PathBuf>)>,
}

struct Ctx {
    prog: Arc<Mutex<Prog>>,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    answers: mpsc::Receiver<(Answer, bool)>,
    all: Option<Answer>,
}

impl Op {
    pub fn start(id: u32, verb: &'static str, title: String, dest: Option<PathBuf>, job: Job) -> Op {
        // Tests: never touch anything outside the sandbox.
        let sandbox = SANDBOX.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(root) = sandbox {
            let mut paths: Vec<&Path> = dest.iter().map(|d| d.as_path()).collect();
            match &job {
                Job::Copy { items, dest } | Job::Move { items, dest } => {
                    paths.extend(items.iter().map(|p| p.as_path()));
                    paths.push(dest);
                }
                Job::Trash(v) | Job::Delete(v) | Job::Restore(v) => paths.extend(v.iter().map(|p| p.as_path())),
                Job::MoveBack(v) => paths.extend(v.iter().flat_map(|(a, b)| [a.as_path(), b.as_path()])),
                Job::Zip { cwd, .. } => paths.push(cwd),
            }
            let tmp = std::env::temp_dir();
            if let Some(bad) = paths.iter().find(|p| !p.starts_with(&root) && !p.starts_with(&tmp)) {
                panic!("operation outside the sandbox: {verb} {}", bad.display());
            }
        }
        let prog = Arc::new(Mutex::new(Prog { scanning: true, ..Default::default() }));
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let zip = match &job {
            Job::Zip { args, cwd, made, .. } => Some((args.clone(), cwd.clone(), made.clone())),
            _ => None,
        };
        let mut ctx = Ctx { prog: prog.clone(), cancel: cancel.clone(), pause: pause.clone(), answers: rx, all: None };
        std::thread::spawn(move || {
            run(&mut ctx, job);
            let mut p = ctx.prog.lock().unwrap();
            p.cancelled = ctx.cancel.load(Ordering::Relaxed);
            p.finished = true;
        });
        Op { id, verb, title, dest, prog, cancel, pause, answer: tx, started: sys::now_ns(), speed: Vec::new(), last: (sys::now_ns(), 0), zip }
    }

    pub fn reply(&self, a: Answer, all: bool) {
        if a == Answer::Cancel {
            self.cancel.store(true, Ordering::Relaxed);
        }
        self.prog.lock().unwrap().ask = None;
        let _ = self.answer.send((a, all));
    }

    /// Takes a speed sample when due; true when one was taken.
    pub fn sample(&mut self) -> bool {
        let now = sys::now_ns();
        if now - self.last.0 < 250_000_000 {
            return false;
        }
        let done = self.prog.lock().unwrap().done;
        let dt = (now - self.last.0) as f32 / 1e9;
        if !self.pause.load(Ordering::Relaxed) {
            self.speed.push(done.saturating_sub(self.last.1) as f32 / dt);
            if self.speed.len() > 240 {
                self.speed.remove(0);
            }
        }
        self.last = (now, done);
        true
    }
}

impl Ctx {
    /// Waits while paused; true when cancelled.
    fn stop(&self) -> bool {
        while self.pause.load(Ordering::Relaxed) && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.cancel.load(Ordering::Relaxed)
    }

    fn ask(&mut self, c: Conflict) -> Answer {
        if let Some(a) = self.all {
            return a;
        }
        self.prog.lock().unwrap().ask = Some(c);
        match self.answers.recv() {
            Ok((a, all)) => {
                if all {
                    self.all = Some(a);
                }
                a
            }
            Err(_) => Answer::Cancel,
        }
    }

    fn err(&self, what: &Path, e: &std::io::Error) {
        let name = what.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.prog.lock().unwrap().errors.push(format!("{name}: {}", explain(e)));
    }

    fn current(&self, p: &Path) {
        self.prog.lock().unwrap().current = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    }

    fn add(&self, bytes: u64, items: u64) {
        let mut p = self.prog.lock().unwrap();
        if p.bytes {
            p.done += bytes;
        } else {
            p.done += items;
        }
        p.items_done += items;
    }
}

/// An io error in Windows' words.
pub fn explain(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(13) | Some(1) => "Нет доступа".into(),
        Some(28) => "Недостаточно места на диске".into(),
        Some(30) => "Диск защищён от записи".into(),
        Some(2) => "Файл не найден".into(),
        Some(36) => "Слишком длинное имя".into(),
        Some(39) => "Папка не пуста".into(),
        Some(16) => "Файл занят".into(),
        _ => e.to_string(),
    }
}

/// (bytes, items) under `p` (links not followed).
fn measure(p: &Path, cancel: &AtomicBool) -> (u64, u64) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 1) };
    if !m.is_dir() {
        return (m.len(), 1);
    }
    let mut t = (0, 1);
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (b, n) = measure(&e.path(), cancel);
            t.0 += b;
            t.1 += n;
        }
    }
    t
}

fn dev(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).map_or(0, |m| m.dev())
}

/// A free name next to `p`: «Имя — копия.txt», «Имя — копия (2).txt» for copies into the
/// same folder; «Имя (2).txt» otherwise (Windows' "keep both").
pub fn free_name(p: &Path, copy: bool) -> PathBuf {
    let dir = p.parent().unwrap_or(Path::new("/"));
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let is_dir = p.is_dir();
    let sl = super::model::stem_len(&name, is_dir);
    let stem: String = name.chars().take(sl).collect();
    let ext: String = name.chars().skip(sl).collect();
    for n in 1.. {
        let cand = match (copy, n) {
            (true, 1) => format!("{stem} — копия{ext}"),
            (true, n) => format!("{stem} — копия ({n}){ext}"),
            (false, 1) => continue,
            (false, n) => format!("{stem} ({n}){ext}"),
        };
        let c = dir.join(cand);
        if std::fs::symlink_metadata(&c).is_err() {
            return c;
        }
    }
    unreachable!()
}

fn run(c: &mut Ctx, job: Job) {
    match job {
        Job::Copy { items, dest } => copy_items(c, &items, &dest, false),
        Job::Move { items, dest } => copy_items(c, &items, &dest, true),
        Job::MoveBack(pairs) => {
            start_items(c, pairs.len() as u64);
            let mut done = Vec::new();
            for (from, to) in pairs {
                if c.stop() {
                    break;
                }
                c.current(&to);
                match move_one(c, &to, &from) {
                    Ok(true) => done.push((to, from)),
                    Ok(false) => {}
                    Err(e) => c.err(&to, &e),
                }
                c.add(0, 1);
            }
            c.prog.lock().unwrap().undo = Some(Undo::Moved(done));
        }
        Job::Trash(items) => {
            start_items(c, items.len() as u64);
            let mut done = Vec::new();
            for p in items {
                if c.stop() {
                    break;
                }
                c.current(&p);
                match super::trash::put(&p) {
                    Ok(t) => done.push(t),
                    Err(_) => c.prog.lock().unwrap().no_trash.push(p.clone()),
                }
                c.add(0, 1);
            }
            c.prog.lock().unwrap().undo = Some(Undo::Trashed(done));
        }
        Job::Delete(items) => {
            let mut total = 0;
            for p in &items {
                total += measure(p, &c.cancel).1;
            }
            start_items(c, total);
            for p in items {
                if c.stop() {
                    break;
                }
                delete_tree(c, &p);
                // A trashed file also loses its .trashinfo.
                if let Some(i) = super::trash::info_of(&p).filter(|i| i.is_file() && p.starts_with(i.parent().unwrap().parent().unwrap())) {
                    let _ = std::fs::remove_file(i);
                }
            }
        }
        Job::Restore(items) => {
            start_items(c, items.len() as u64);
            let mut made = Vec::new();
            for p in items {
                if c.stop() {
                    break;
                }
                c.current(&p);
                match super::trash::restore(&p) {
                    Ok(d) => made.push(d),
                    Err(e) => c.err(&p, &e),
                }
                c.add(0, 1);
            }
            c.prog.lock().unwrap().made = made;
        }
        Job::Zip { args, cwd, made, password } => seven_zip(c, args, &cwd, made, password),
    }
}

fn start_items(c: &Ctx, n: u64) {
    let mut p = c.prog.lock().unwrap();
    p.scanning = false;
    p.bytes = false;
    p.total = n;
    p.items_total = n;
}

fn delete_tree(c: &Ctx, p: &Path) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return };
    if m.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                if c.stop() {
                    return;
                }
                delete_tree(c, &e.path());
            }
        }
        c.current(p);
        if let Err(e) = std::fs::remove_dir(p) {
            c.err(p, &e);
        }
    } else {
        c.current(p);
        if let Err(e) = std::fs::remove_file(p) {
            c.err(p, &e);
        }
    }
    c.add(0, 1);
}

fn copy_items(c: &mut Ctx, items: &[PathBuf], dest: &Path, mv: bool) {
    // Moves within one disk are renames: count items. Anything else: bytes.
    let dd = dev(dest);
    let renames = mv && items.iter().all(|p| dev(p) == dd);
    if renames {
        start_items(c, items.len() as u64);
    } else {
        let (mut b, mut n) = (0, 0);
        for p in items {
            let (x, y) = measure(p, &c.cancel);
            b += x;
            n += y;
        }
        let mut p = c.prog.lock().unwrap();
        p.scanning = false;
        p.bytes = true;
        p.total = b;
        p.items_total = n;
    }
    let mut made = Vec::new();
    let mut moved = Vec::new();
    for src in items {
        if c.stop() {
            break;
        }
        let Some(name) = src.file_name() else { continue };
        let mut dst = dest.join(name);
        if dest.starts_with(src) {
            c.prog.lock().unwrap().errors.push(format!("{}: конечная папка находится внутри исходной", name.to_string_lossy()));
            continue;
        }
        if mv {
            if dst == *src {
                c.add(0, 1);
                continue;
            }
            match move_one(c, src, &dst) {
                Ok(true) => moved.push((src.clone(), dst)),
                Ok(false) => {}
                Err(e) => c.err(src, &e),
            }
            if renames {
                c.add(0, 1);
            }
        } else {
            if src.parent() == Some(dest) {
                dst = free_name(&dst, true);
            }
            match copy_entry(c, src, &dst) {
                Ok(Some(d)) => made.push(d),
                Ok(None) => {}
                Err(e) => c.err(src, &e),
            }
        }
    }
    let mut p = c.prog.lock().unwrap();
    if mv {
        p.made = moved.iter().map(|m| m.1.clone()).collect();
        p.undo = Some(Undo::Moved(moved));
    } else {
        p.made = made.clone();
        p.undo = Some(Undo::Made(made));
    }
}

/// Moves `src` to `dst` (rename, else copy and delete); false when skipped.
fn move_one(c: &mut Ctx, src: &Path, dst: &Path) -> std::io::Result<bool> {
    let mut dst = dst.to_path_buf();
    if let Ok(dm) = std::fs::symlink_metadata(&dst) {
        let sdir = std::fs::symlink_metadata(src).map_or(false, |m| m.is_dir());
        if sdir && dm.is_dir() {
            // Folders merge: move what is inside, then drop the emptied source.
            let Ok(rd) = std::fs::read_dir(src) else { return Ok(false) };
            for e in rd.flatten() {
                if c.stop() {
                    return Ok(false);
                }
                let _ = move_one(c, &e.path(), &dst.join(e.file_name()));
            }
            let _ = std::fs::remove_dir(src);
            return Ok(true);
        }
        match c.ask(Conflict { src: src.to_path_buf(), dst: dst.clone(), dir: dm.is_dir() }) {
            Answer::Replace => {
                if dm.is_dir() {
                    std::fs::remove_dir_all(&dst)?;
                }
            }
            Answer::Both => dst = free_name(&dst, false),
            Answer::Skip | Answer::Cancel => return Ok(false),
        }
    }
    match std::fs::rename(src, &dst) {
        Ok(()) => Ok(true),
        Err(e) if e.raw_os_error() == Some(18) => {
            // Another disk: copy, then delete the source if all of it arrived.
            let errs = c.prog.lock().unwrap().errors.len();
            if copy_entry(c, src, &dst)?.is_some() && c.prog.lock().unwrap().errors.len() == errs && !c.cancel.load(Ordering::Relaxed) {
                if std::fs::symlink_metadata(src)?.is_dir() {
                    std::fs::remove_dir_all(src)?;
                } else {
                    std::fs::remove_file(src)?;
                }
                return Ok(true);
            }
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Copies `src` to `dst`; Some(where) when something was written.
fn copy_entry(c: &mut Ctx, src: &Path, dst: &Path) -> std::io::Result<Option<PathBuf>> {
    let m = std::fs::symlink_metadata(src)?;
    let mut dst = dst.to_path_buf();
    c.current(src);
    if m.is_dir() {
        match std::fs::symlink_metadata(&dst) {
            Ok(d) if d.is_dir() => {} // merge into it
            Ok(_) => match c.ask(Conflict { src: src.to_path_buf(), dst: dst.clone(), dir: false }) {
                Answer::Replace => std::fs::remove_file(&dst)?,
                Answer::Both => dst = free_name(&dst, false),
                _ => {
                    c.add(measure(src, &c.cancel).0, measure(src, &c.cancel).1);
                    return Ok(None);
                }
            },
            Err(_) => {}
        }
        if !dst.exists() {
            std::fs::create_dir(&dst)?;
        }
        c.add(0, 1);
        for e in std::fs::read_dir(src)?.flatten() {
            if c.stop() {
                break;
            }
            if let Err(err) = copy_entry(c, &e.path(), &dst.join(e.file_name())) {
                c.err(&e.path(), &err);
            }
        }
        let _ = std::fs::set_permissions(&dst, m.permissions());
        return Ok(Some(dst));
    }
    if let Ok(d) = std::fs::symlink_metadata(&dst) {
        match c.ask(Conflict { src: src.to_path_buf(), dst: dst.clone(), dir: d.is_dir() }) {
            Answer::Replace => {
                if d.is_dir() {
                    std::fs::remove_dir_all(&dst)?;
                }
            }
            Answer::Both => dst = free_name(&dst, false),
            _ => {
                c.add(m.len(), 1);
                return Ok(None);
            }
        }
    }
    if m.file_type().is_symlink() {
        let t = std::fs::read_link(src)?;
        let _ = std::fs::remove_file(&dst);
        std::os::unix::fs::symlink(t, &dst)?;
        c.add(0, 1);
        return Ok(Some(dst));
    }
    let mut from = std::fs::File::open(src)?;
    let mut to = std::fs::File::create(&dst)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if c.stop() {
            drop(to);
            let _ = std::fs::remove_file(&dst);
            return Ok(None);
        }
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        c.add(n as u64, 0);
    }
    let _ = to.set_permissions(m.permissions());
    if let Ok(t) = m.modified() {
        let _ = to.set_modified(t);
    }
    c.add(0, 1);
    Ok(Some(dst))
}

// ------------------------------------------------------------------ 7-Zip

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn seven_zip(c: &mut Ctx, mut args: Vec<String>, cwd: &Path, made: Option<PathBuf>, password: Option<String>) {
    {
        let mut p = c.prog.lock().unwrap();
        p.scanning = false;
        p.total = 100;
        p.bytes = false;
    }
    // "__sh <command>": a shell pipeline (tar | xz) instead of 7-Zip.
    let (prog, args) = if args.first().map(String::as_str) == Some("__sh") {
        ("/bin/sh", vec!["-c".to_string(), args.get(1).cloned().unwrap_or_default()])
    } else {
        // Progress to stdout (-bsp1), messages off, errors to stderr; switches right
        // after the command letter.
        let mut extra = vec!["-bsp1".to_string(), "-bso0".to_string(), "-bse2".to_string(), "-y".to_string()];
        if let Some(pw) = &password {
            extra.push(format!("-p{pw}"));
        }
        let at = 1.min(args.len());
        for (i, e) in extra.into_iter().enumerate() {
            args.insert(at + i, e);
        }
        ("7z", args)
    };
    let child = std::process::Command::new(prog)
        .args(&args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(ch) => ch,
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::NotFound { "7-Zip не установлен (пакет 7zip)".to_string() } else { e.to_string() };
            c.prog.lock().unwrap().errors.push(msg);
            return;
        }
    };
    let pid = child.id() as i32;
    let mut err_text = String::new();
    let mut stderr = child.stderr.take().unwrap();
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut out = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut stopped = false;
    let mut line = Vec::new();
    loop {
        let paused = c.pause.load(Ordering::Relaxed);
        if paused != stopped {
            unsafe { kill(pid, if paused { 19 } else { 18 }) };
            stopped = paused;
        }
        if c.cancel.load(Ordering::Relaxed) {
            unsafe { kill(pid, 18) };
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(60)) {
            Ok(chunk) => {
                for b in chunk {
                    if b == 8 || b == b'\r' || b == b'\n' {
                        if !line.is_empty() {
                            progress_line(c, &String::from_utf8_lossy(&line));
                            line.clear();
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait();
    err_text.push_str(&errs.join().unwrap_or_default());
    let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
    let mut p = c.prog.lock().unwrap();
    if c.cancel.load(Ordering::Relaxed) {
        if let Some(m) = &made {
            if m.is_file() {
                let _ = std::fs::remove_file(m);
            }
        }
        return;
    }
    let low = err_text.to_lowercase();
    if code >= 2 && (low.contains("password") || low.contains("encrypted")) {
        p.need_password = true;
        return;
    }
    if code >= 2 || code < 0 {
        let msg = err_text.lines().map(str::trim).filter(|l| !l.is_empty()).filter(|l| l.starts_with("ERROR") || l.contains("rror")).collect::<Vec<_>>().join("; ");
        p.errors.push(if msg.is_empty() { format!("7-Zip завершился с кодом {code}") } else { msg });
        return;
    }
    p.done = 100;
    if let Some(m) = made {
        if m.exists() {
            p.made = vec![m.clone()];
            p.undo = Some(Undo::Made(vec![m]));
        }
    }
}

/// «  42% 13 + docs/file.txt» → 42 and the name.
fn progress_line(c: &Ctx, l: &str) {
    let l = l.trim();
    let Some(i) = l.find('%') else { return };
    let num: String = l[..i].chars().rev().take_while(|ch| ch.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    let Ok(v) = num.parse::<u64>() else { return };
    let mut p = c.prog.lock().unwrap();
    p.done = v.min(100);
    let rest = l[i + 1..].trim();
    if let Some(j) = rest.find(|ch| ch == '+' || ch == '-' || ch == 'U' || ch == 'T') {
        let name = rest[j + 1..].trim();
        if !name.is_empty() {
            p.current = name.rsplit('/').next().unwrap_or(name).to_string();
        }
    }
}

// ------------------------------------------------------------------ archives as folders

#[derive(Clone, Debug)]
pub struct ArcEntry {
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub packed: u64,
    pub mtime: i64,
    pub encrypted: bool,
}

/// `7z l -slt`: every entry of an archive. Err("password") when its names are encrypted.
pub fn list_archive(file: &Path, password: Option<&str>) -> Result<Vec<ArcEntry>, String> {
    let mut cmd = std::process::Command::new("7z");
    cmd.arg("l").arg("-slt").arg("-ba").arg("-sccUTF-8");
    cmd.arg(format!("-p{}", password.unwrap_or("")));
    cmd.arg("--").arg(file).stdin(std::process::Stdio::null());
    let out = cmd.output().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "7-Zip не установлен (пакет 7zip)".to_string() } else { e.to_string() })?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if !out.status.success() && (err.contains("password") || err.contains("encrypted")) {
        return Err("password".into());
    }
    if !out.status.success() && text.trim().is_empty() {
        return Err("Не удалось открыть архив".into());
    }
    let mut v = Vec::new();
    let mut cur: Option<ArcEntry> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("Path = ") {
            if let Some(e) = cur.take() {
                v.push(e);
            }
            cur = Some(ArcEntry { path: p.replace('\\', "/"), dir: false, size: 0, packed: 0, mtime: 0, encrypted: false });
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        if let Some(x) = line.strip_prefix("Size = ") {
            e.size = x.trim().parse().unwrap_or(0);
        } else if let Some(x) = line.strip_prefix("Packed Size = ") {
            e.packed = x.trim().parse().unwrap_or(0);
        } else if let Some(x) = line.strip_prefix("Folder = ") {
            e.dir |= x.trim() == "+";
        } else if let Some(x) = line.strip_prefix("Attributes = ") {
            e.dir |= x.starts_with('D');
        } else if let Some(x) = line.strip_prefix("Encrypted = ") {
            e.encrypted = x.trim() == "+";
        } else if let Some(x) = line.strip_prefix("Modified = ") {
            e.mtime = parse_7z_date(x.trim());
        }
    }
    if let Some(e) = cur.take() {
        v.push(e);
    }
    Ok(v)
}

/// "2024-05-01 12:30:00(.ffff)" in local time.
fn parse_7z_date(s: &str) -> i64 {
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d) = (num(0, 4), num(5, 7), num(8, 10));
    if y == 0 {
        return 0;
    }
    let (h, mi, se) = (num(11, 13), num(14, 16), num(17, 19));
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let local = (era * 146_097 + doe - 719_468) * 86400 + h * 3600 + mi * 60 + se;
    local - super::model::tz_offset(local)
}

/// Archive types 7-Zip opens (by our extension).
pub fn is_archive(ext: &str) -> bool {
    matches!(ext, "zip" | "7z" | "rar" | "tar" | "tar.gz" | "tar.xz" | "tar.zst" | "tar.bz2" | "gz" | "xz" | "zst" | "bz2" | "iso" | "cab" | "lzma" | "tgz" | "txz" | "jar" | "apk" | "deb" | "rpm" | "cpio" | "wim" | "arj" | "lzh" | "z")
}
