//! Checks for «Проводник» without a window: the app gets the same pointer and key events
//! the window would give it, in a sandbox folder, and after every event the invariants
//! are checked (counts agree, focus and rename point at real items, the address shows
//! where we are, each visible item is what the pointer hits at its centre…).
//!
//! * scenarios: hand-written Windows behaviours;
//! * `history_model`: random back / forward / up / clicks / tabs / menus, compared step by
//!   step with a reference model of browser-style history;
//! * `monkey`: thousands of random clicks, drags, wheels and keys (seeded; a failure
//!   prints the seed and the last steps to replay).
//!
//! Nothing is started (open::DRY) and no operation may leave the sandbox (ops::SANDBOX).
use super::input::k;
use super::*;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());
static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const LEFT: u32 = 0x110;
const RIGHT: u32 = 0x111;
const MIDDLE: u32 = 0x112;
const SIDE: u32 = 0x113;
const EXTRA: u32 = 0x114;

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

/// A fresh home: user folders, nested folders, files of a few types.
fn sandbox() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("resc-files-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    for d in ["Desktop", "Downloads", "Documents/Проекты/a", "Documents/Проекты/b/c", "Documents/Счета", "Pictures", "Music", "Videos"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    write(&home.join(".config/user-dirs.dirs"), "XDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\nXDG_DOCUMENTS_DIR=\"$HOME/Documents\"\nXDG_PICTURES_DIR=\"$HOME/Pictures\"\nXDG_MUSIC_DIR=\"$HOME/Music\"\nXDG_VIDEOS_DIR=\"$HOME/Videos\"\n");
    for (i, n) in ["Отчёт 1.txt", "Отчёт 2.txt", "Отчёт 10.txt", "notes.md", "data.bin", "config.json"].iter().enumerate() {
        write(&home.join("Documents").join(n), &"x".repeat(i * 300));
    }
    write(&home.join("Documents/Проекты/a/main.rs"), "fn main() {}");
    write(&home.join("Downloads/file.pdf"), "%PDF");
    write(&home.join("Documents/.hidden"), "");
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    std::env::remove_var("RESC_DESK_CTL");
    std::fs::create_dir_all(root.join("run")).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", root.join("run"));
    *model::MOUNTS_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(vec![Mount { path: root.clone(), dev: "/dev/test".into(), fs: "ext4".into(), label: "Тест".into(), removable: false, size: 1 << 30, free: 1 << 29 }]);
    *ops::SANDBOX.lock().unwrap_or_else(|e| e.into_inner()) = Some(root.clone());
    open::DRY.store(true, Ordering::Relaxed);
    root
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

struct Sim {
    app: App,
    root: PathBuf,
    log: Vec<String>,
    _g: std::sync::MutexGuard<'static, ()>,
}

impl Sim {
    fn new() -> Sim {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = sandbox();
        open::DRY_LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
        crate::font::init();
        let app = App::new(Loc::Dir(model::home()), 1180, 720);
        let mut s = Sim { app, root, log: vec![], _g: g };
        s.settle();
        s
    }

    /// The desktop's icons instead of a window.
    fn desk() -> Sim {
        let mut s = Sim::new();
        let d = s.home().join("Desktop");
        for n in ["Список дел.txt", "Отчёт.pdf", "фото.png"] {
            write(&d.join(n), "x");
        }
        std::fs::create_dir_all(d.join("Проекты 2026")).unwrap();
        s.app = App::new(Loc::Dir(desktop::desktop_dir()), 1600, 836);
        s.app.desk = true;
        s.app.view = s.app.view_for(&s.app.loc().clone());
        s.app.lay();
        s.settle();
        s
    }

    fn home(&self) -> PathBuf {
        model::home()
    }

    /// Lets springs land and background work finish; then checks everything.
    fn settle(&mut self) {
        if std::env::var("TRACE").is_ok() {
            let t = &self.app.tabs;
            eprintln!("  > {} | tab {} {:?}", self.log.last().cloned().unwrap_or_default(), self.app.cur, t.iter().map(|t| (t.loc.key(), t.back.iter().map(|l| l.key()).collect::<Vec<_>>())).collect::<Vec<_>>());
        }
        let mut idle = 0;
        for _ in 0..200 {
            self.app.tick();
            self.app.step(1.0);
            let busy = !self.app.ops.is_empty() && self.app.dialog.is_none();
            // Twice idle in a row, a moment apart: file system events have arrived.
            idle = if !busy && self.app.reload_at.is_none() { idle + 1 } else { 0 };
            if idle >= 2 {
                break;
            }
            if idle == 1 {
                std::thread::sleep(std::time::Duration::from_millis(3));
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            if std::env::var("TRACE").is_ok() {
                eprintln!("    waiting: ops {} reload {:?}", self.app.ops.len(), self.app.reload_at.map(|t| t as i64 - sys::now_ns() as i64));
            }
        }
        self.app.winreq.clear();
        if self.app.quit {
            self.log.push("(closed; reopened)".into());
            self.app.quit = false;
        }
        self.check();
    }

    fn fail(&self, what: &str) -> ! {
        let tail: Vec<&String> = self.log.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
        let tabs: Vec<String> = self.app.tabs.iter().map(|t| format!("{:?} back {:?} fwd {:?}", t.loc, t.back, t.fwd)).collect();
        panic!(
            "{what}\n{}\nloc: {:?} (tab {})\ntabs:\n  {}\nlast steps:\n  {}",
            self.log.first().cloned().unwrap_or_default(),
            self.app.loc(),
            self.app.cur,
            tabs.join("\n  "),
            tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
        );
    }

    fn check(&self) {
        let a = &self.app;
        let n = a.items.len();
        if a.sel.len() != n || a.rects.len() != n {
            self.fail(&format!("counts: items {n}, sel {}, rects {}", a.sel.len(), a.rects.len()));
        }
        if a.focus.map_or(false, |f| f >= n) {
            self.fail("focus past the end");
        }
        if a.rename.as_ref().map_or(false, |r| r.0 >= n) {
            self.fail("rename past the end");
        }
        if a.cur >= a.tabs.len() {
            self.fail("current tab past the end");
        }
        if a.tab_drop_at.is_some() && a.incoming.as_ref().map_or(true, |i| i.tab.is_none()) {
            self.fail("a tab drop mark with no tab coming");
        }
        if a.col_drag.is_some() && a.press.is_none() {
            self.fail("a header drag with no button held");
        }
        if a.columns().first() != Some(&Col::Name) {
            self.fail("«Имя» is not the first column");
        }
        if a.menu.is_some() && a.dialog.is_some() {
            self.fail("a menu and a dialog at once");
        }
        let max = a.max_scroll();
        if a.scroll.value < -0.5 || a.scroll.value > max + 0.5 {
            self.fail(&format!("scroll {} outside 0..{max}", a.scroll.value));
        }
        let t = &a.tabs[a.cur];
        if t.back.last() == Some(&t.loc) {
            self.fail("the back history ends with where we already are");
        }
        if t.fwd.last() == Some(&t.loc) {
            self.fail("the forward history starts with where we already are");
        }
        // Never outside the sandbox.
        let inside = |p: &Path| p.starts_with(&self.root);
        let ok = match a.loc() {
            Loc::Dir(p) | Loc::Search(p, _) => inside(p),
            Loc::Arc(f, _) => inside(f),
            Loc::Pc | Loc::Trash => true,
        };
        if !ok {
            self.fail("left the sandbox");
        }
        if let Loc::Dir(p) = a.loc() {
            if !p.is_dir() && a.load_err.is_none() {
                self.fail("showing a folder that is gone, without saying so");
            }
        }
        // The address bar ends with where we are.
        if a.crumbs().last().map(|c| &c.1) != Some(a.loc()) {
            self.fail(&format!("address shows {:?}", a.crumbs().last().map(|c| &c.1)));
        }
        // What the pointer hits at an item's centre is that item.
        if a.menu.is_none() && a.dialog.is_none() && a.ops.is_empty() {
            let vr = a.view_rect();
            for i in 0..n {
                let r = a.screen(a.rects[i]);
                if vr.contains_rect(&r) {
                    let (cx, cy) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
                    let h = a.hit(cx, cy);
                    if h != Hit::Item(i) {
                        self.fail(&format!("item {i} ({}) at its centre hits {h:?}", a.items[i].disp));
                    }
                }
            }
        }
    }

    // -------------------------------------------------------------- input

    fn mods(&mut self, ctrl: bool, shift: bool, alt: bool) {
        self.app.mods = Mods { ctrl, shift, alt, ..Default::default() };
    }

    fn press(&mut self, b: u32, x: f64, y: f64) {
        self.app.motion(x, y);
        self.app.button(b, true);
        self.app.motion(x, y);
        self.app.button(b, false);
    }

    fn click_b(&mut self, b: u32, x: f64, y: f64, what: &str) {
        self.log.push(format!("click {b:#x} {x:.0},{y:.0} {what}"));
        // Not a double click with whatever came before.
        self.app.last_click = (0, Hit::None);
        self.press(b, x, y);
        self.settle();
    }

    fn click(&mut self, r: Rect, what: &str) {
        let (x, y) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
        self.click_b(LEFT, x, y, what);
    }

    fn dbl(&mut self, r: Rect, what: &str) {
        let (x, y) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
        self.log.push(format!("double-click {x:.0},{y:.0} {what}"));
        self.app.last_click = (0, Hit::None);
        self.press(LEFT, x, y);
        self.press(LEFT, x, y);
        self.settle();
    }

    fn key_m(&mut self, code: u32, ctrl: bool, shift: bool, alt: bool) {
        self.log.push(format!("key {code}{}{}{}", if ctrl { " +ctrl" } else { "" }, if shift { " +shift" } else { "" }, if alt { " +alt" } else { "" }));
        self.mods(ctrl, shift, alt);
        self.app.key(code);
        self.mods(false, false, false);
        self.settle();
    }

    fn key(&mut self, code: u32) {
        self.key_m(code, false, false, false);
    }

    fn item(&self, name: &str) -> usize {
        self.app.items.iter().position(|i| i.disp == name || i.e.name == name).unwrap_or_else(|| self.fail(&format!("no item {name}")))
    }

    fn item_rect(&self, i: usize) -> Rect {
        self.app.screen(self.app.rects[i])
    }

    fn open(&mut self, name: &str) {
        let i = self.item(name);
        let r = self.item_rect(i);
        self.dbl(r, name);
    }

    fn nav(&mut self, label: &str) {
        let j = self.app.nav.iter().position(|n| n.label == label).unwrap();
        let r = self.app.nav_rects()[j];
        self.click(r, label);
    }

    fn at(&self) -> Loc {
        self.app.loc().clone()
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        *ops::SANDBOX.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *model::MOUNTS_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ------------------------------------------------------------------ scenarios

#[test]
fn back_forward_up() {
    let mut s = Sim::new();
    let home = s.home();
    s.open("Документы");
    assert_eq!(s.at(), Loc::Dir(home.join("Documents")));
    s.open("Проекты");
    s.open("b");
    assert_eq!(s.at(), Loc::Dir(home.join("Documents/Проекты/b")));
    // Back with each of Windows' ways.
    s.key_m(k::LEFT, false, false, true);
    assert_eq!(s.at(), Loc::Dir(home.join("Documents/Проекты")));
    // The folder we came out of is selected.
    assert!(s.app.sel[s.item("b")], "came back from b: b must be selected");
    s.click_b(SIDE, 600.0, 400.0, "mouse back");
    assert_eq!(s.at(), Loc::Dir(home.join("Documents")));
    s.key(k::BACKSPACE);
    assert_eq!(s.at(), Loc::Dir(home.clone()));
    // Forward twice.
    s.click_b(EXTRA, 600.0, 400.0, "mouse forward");
    s.key_m(k::RIGHT, false, false, true);
    assert_eq!(s.at(), Loc::Dir(home.join("Documents/Проекты")));
    // Up from a folder; up again; up from home goes to the disk, from there «Этот компьютер».
    s.key_m(k::UP, false, false, true);
    assert_eq!(s.at(), Loc::Dir(home.join("Documents")));
    assert!(s.app.tabs[0].fwd.is_empty(), "up is a new step: forward history is gone");
    let up = s.app.tool_btn(2);
    s.click(up, "toolbar up");
    s.click(up, "toolbar up");
    s.click(up, "toolbar up");
    assert_eq!(s.at(), Loc::Pc);
    assert!(s.app.parent().is_none());
}

#[test]
fn back_with_a_menu_open_only_closes_it() {
    let mut s = Sim::new();
    s.open("Документы");
    let i = s.item("notes.md");
    let r = s.item_rect(i);
    s.click_b(RIGHT, r.x as f64 + 40.0, r.y as f64 + 10.0, "right click notes.md");
    assert!(s.app.menu.is_some(), "context menu");
    let here = s.at();
    // The mouse's back button over the menu must not pick an item, nor go back.
    let (mx, my) = { let m = s.app.menu.as_ref().unwrap(); (m.x as f64 + 30.0, m.y as f64 + 20.0) };
    s.click_b(SIDE, mx, my, "mouse back over the menu");
    assert!(s.app.menu.is_none());
    assert_eq!(s.at(), here);
    assert!(s.app.dialog.is_none() && s.app.rename.is_none());
    // Alt+Left with a menu open: closes the menu only.
    s.click_b(RIGHT, r.x as f64 + 40.0, r.y as f64 + 10.0, "right click again");
    s.key_m(k::LEFT, false, false, true);
    assert!(s.app.menu.is_none());
    assert_eq!(s.at(), here);
    // Then back works.
    s.key_m(k::LEFT, false, false, true);
    assert_eq!(s.at(), Loc::Dir(s.home()));
}

#[test]
fn back_with_a_dialog_open_does_nothing() {
    let mut s = Sim::new();
    s.open("Документы");
    let i = s.item("notes.md");
    s.click(s.item_rect(i), "notes.md");
    s.key_m(k::DELETE, false, true, false);
    assert!(s.app.dialog.is_some(), "Shift+Del asks");
    let here = s.at();
    let b = draw::dialog_buttons(s.app.dialog.as_ref().unwrap(), draw::dialog_rect(s.app.dialog.as_ref().unwrap(), s.app.w, s.app.h))[0];
    s.click_b(SIDE, b.x as f64 + 5.0, b.y as f64 + 5.0, "mouse back over «Да»");
    assert!(s.app.dialog.is_some(), "the dialog stays");
    assert_eq!(s.at(), here);
    assert!(s.home().join("Documents/notes.md").exists(), "nothing was deleted");
    s.key(k::ESC);
    assert!(s.app.dialog.is_none());
    assert!(s.home().join("Documents/notes.md").exists());
}

#[test]
fn keyboard_selection() {
    let mut s = Sim::new();
    s.open("Документы");
    // Folders first, then files in natural order.
    let names: Vec<String> = s.app.items.iter().map(|i| i.disp.clone()).collect();
    let a = names.iter().position(|n| n == "Отчёт 2.txt").unwrap();
    let b = names.iter().position(|n| n == "Отчёт 10.txt").unwrap();
    assert!(a < b, "natural order: {names:?}");
    assert!(!names.iter().any(|n| n == ".hidden"), "hidden files are hidden");
    s.key(k::HOME);
    assert_eq!(s.app.selected(), vec![0]);
    s.key_m(k::DOWN, false, true, false);
    s.key_m(k::DOWN, false, true, false);
    assert_eq!(s.app.selected(), vec![0, 1, 2]);
    s.key(k::END);
    assert_eq!(s.app.selected(), vec![s.app.items.len() - 1]);
    s.key_m(k::A, true, false, false);
    assert_eq!(s.app.selected().len(), s.app.items.len());
    // Type-ahead.
    s.key(k::ESC);
    s.key(49); // n
    assert_eq!(s.app.items[s.app.selected()[0]].disp, "notes.md");
}

#[test]
fn new_folder_rename_undo() {
    let mut s = Sim::new();
    s.open("Документы");
    s.key_m(k::N, true, true, false);
    let i = s.app.rename.as_ref().map(|r| r.0).unwrap_or_else(|| s.fail("no rename after new folder"));
    assert_eq!(s.app.items[i].e.name, "Новая папка");
    assert_eq!(i, s.app.items.len() - 1, "a new folder stays at the end");
    for c in [20, 18, 31, 20] {
        s.key(c); // "test"
    }
    s.key(k::ENTER);
    assert!(s.home().join("Documents/test").is_dir());
    // Undo goes step by step: first the rename, then the new folder itself.
    s.key_m(k::Z, true, false, false);
    assert!(s.home().join("Documents/Новая папка").is_dir() && !s.home().join("Documents/test").exists());
    s.key_m(k::Z, true, false, false);
    assert!(!s.home().join("Documents/Новая папка").exists());
    assert!(!s.home().join("Documents/test").exists(), "undo of a new folder removes it");
}

#[test]
fn copy_paste_trash_restore() {
    let mut s = Sim::new();
    s.open("Документы");
    let i = s.item("notes.md");
    s.click(s.item_rect(i), "notes.md");
    s.key_m(k::C, true, false, false);
    s.key_m(k::V, true, false, false);
    assert!(s.home().join("Documents/notes — копия.md").exists());
    // The copy is selected; Del sends it to the trash; Ctrl+Z brings it back.
    assert_eq!(s.app.items[s.app.selected()[0]].e.name, "notes — копия.md");
    s.key(k::DELETE);
    assert!(!s.home().join("Documents/notes — копия.md").exists());
    s.key_m(k::Z, true, false, false);
    assert!(s.home().join("Documents/notes — копия.md").exists());
    // Paste into a subfolder twice: the second asks.
    s.open("Счета");
    s.key_m(k::V, true, false, false);
    s.key_m(k::V, true, false, false);
    assert!(matches!(s.app.dialog.as_ref().map(|d| &d.on), Some(On::Conflict(_))), "name clash asks");
    s.app.dialog_done(2); // keep both
    s.settle();
    assert!(s.home().join("Documents/Счета/notes (2).md").exists());
}

#[test]
fn tabs() {
    let mut s = Sim::new();
    s.open("Документы");
    s.key_m(k::T, true, false, false);
    assert_eq!(s.app.tabs.len(), 2);
    assert_eq!(s.at(), Loc::Dir(s.home()));
    s.open("Загрузки");
    s.key_m(k::TAB, true, false, false);
    assert_eq!(s.app.cur, 0);
    assert_eq!(s.at(), Loc::Dir(s.home().join("Documents")));
    // Back history belongs to its tab.
    s.key_m(k::LEFT, false, false, true);
    assert_eq!(s.at(), Loc::Dir(s.home()));
    s.key_m(k::TAB, true, false, false);
    assert_eq!(s.at(), Loc::Dir(s.home().join("Downloads")));
    s.key_m(k::W, true, false, false);
    assert_eq!(s.app.tabs.len(), 1);
    assert_eq!(s.at(), Loc::Dir(s.home()));
}

#[test]
fn nothing_outside_is_started() {
    let mut s = Sim::new();
    s.open("Документы");
    s.open("notes.md");
    let log = open::DRY_LOG.lock().unwrap().clone();
    assert!(log.iter().all(|l| l.starts_with("spawn") || l.starts_with("clip") || l.starts_with("ctl ")), "{log:?}");
}

#[test]
fn columns_hide_show_move() {
    let mut s = Sim::new();
    s.open("Документы");
    let names = |s: &Sim| s.app.columns().iter().map(|c| c.title()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Тип", "Размер"]);
    // Right click on «Тип»: the column list.
    let r = s.app.head_cols()[2].1;
    s.click_b(RIGHT, r.x as f64 + 20.0, r.y as f64 + 15.0, "right click «Тип»");
    let m = s.app.menu.as_ref().unwrap_or_else(|| s.fail("no header menu"));
    let i = m.items.iter().position(|it| it.label == "Тип").unwrap();
    assert_eq!(m.items[i].check, Some(true));
    let name = m.items.iter().position(|it| it.label == "Имя").unwrap();
    assert!(!m.items[name].on, "«Имя» can't be hidden");
    let r = s.app.menu.as_ref().unwrap().item_rect(i);
    s.click(r, "uncheck «Тип»");
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Размер"]);
    // Show «Права».
    let r = s.app.head_cols()[1].1;
    s.click_b(RIGHT, r.x as f64 + 20.0, r.y as f64 + 15.0, "right click header");
    let i = s.app.menu.as_ref().unwrap().items.iter().position(|it| it.label == "Права").unwrap();
    let r = s.app.menu.as_ref().unwrap().item_rect(i);
    s.click(r, "check «Права»");
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Размер", "Права"]);
    assert_eq!(draw::cell_text(&s.app, &s.app.items[s.item("notes.md")], Col::Perms), "rw-r--r--");
    // Drag «Размер» before «Дата изменения».
    let from = s.app.head_cols()[2].1;
    let to = s.app.head_cols()[1].1;
    let (y, x0, x1) = (from.y as f64 + 15.0, from.x as f64 + 30.0, to.x as f64 + 10.0);
    s.log.push("drag «Размер» left".into());
    s.app.motion(x0, y);
    s.app.button(LEFT, true);
    for t in 1..=6 {
        s.app.motion(x0 + (x1 - x0) * t as f64 / 6.0, y);
    }
    s.check();
    s.app.button(LEFT, false);
    s.settle();
    assert_eq!(names(&s), ["Имя", "Размер", "Дата изменения", "Права"]);
    assert_eq!(s.app.view.sort, Col::Name, "a drag is not a click: no sorting");
    // And to the very end.
    let from = s.app.head_cols()[1].1;
    let last = *s.app.head_cols().last().unwrap();
    s.app.motion(from.x as f64 + 30.0, y);
    s.app.button(LEFT, true);
    for t in 1..=6 {
        s.app.motion(from.x as f64 + 30.0 + (last.1.x1() as f64 + 40.0 - from.x as f64 - 30.0) * t as f64 / 6.0, y);
    }
    s.app.button(LEFT, false);
    s.settle();
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Права", "Размер"]);
    // «Имя» doesn't move.
    let n = s.app.head_cols()[0].1;
    s.app.motion(n.x as f64 + 60.0, y);
    s.app.button(LEFT, true);
    for t in 1..=6 {
        s.app.motion(n.x as f64 + 60.0 + 100.0 * t as f64, y);
    }
    s.app.button(LEFT, false);
    s.settle();
    assert_eq!(names(&s)[0], "Имя");
    // Esc while dragging puts it back.
    let from = s.app.head_cols()[2].1;
    s.app.motion(from.x as f64 + 30.0, y);
    s.app.button(LEFT, true);
    s.app.motion(from.x as f64 - 200.0, y);
    s.key(k::ESC);
    s.app.button(LEFT, false);
    s.settle();
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Права", "Размер"]);
    // Folders and the trash keep their own sets; defaults come back.
    s.nav("Корзина");
    assert_eq!(names(&s)[1], "Исходное расположение");
    s.key_m(k::LEFT, false, false, true);
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Права", "Размер"]);
    s.app.act(Act::ResetCols);
    assert_eq!(names(&s), ["Имя", "Дата изменения", "Тип", "Размер"]);
}

// ------------------------------------------------------------------ history model

#[derive(Clone, Debug, PartialEq)]
struct MTab {
    loc: Loc,
    back: Vec<Loc>,
    fwd: Vec<Loc>,
}

struct Model {
    tabs: Vec<MTab>,
    cur: usize,
}

impl Model {
    fn go(&mut self, l: Loc) {
        let t = &mut self.tabs[self.cur];
        if t.loc == l {
            return;
        }
        let old = std::mem::replace(&mut t.loc, l);
        t.back.push(old);
        t.fwd.clear();
    }
    fn back(&mut self) {
        let t = &mut self.tabs[self.cur];
        if let Some(l) = t.back.pop() {
            let c = std::mem::replace(&mut t.loc, l);
            t.fwd.push(c);
        }
    }
    fn fwd(&mut self) {
        let t = &mut self.tabs[self.cur];
        if let Some(l) = t.fwd.pop() {
            let c = std::mem::replace(&mut t.loc, l);
            t.back.push(c);
        }
    }
}

fn compare(s: &Sim, m: &Model, step: usize, seed: u64) {
    let a = &s.app;
    let bad = a.tabs.len() != m.tabs.len()
        || a.cur != m.cur
        || a.tabs.iter().zip(&m.tabs).enumerate().any(|(i, (t, mt))| {
            let loc = if i == a.cur { a.loc() } else { &t.loc };
            *loc != mt.loc || t.back != mt.back || t.fwd != mt.fwd
        });
    if bad {
        s.fail(&format!(
            "seed {seed} step {step}: history differs\napp: tab {} of {}, at {:?}, back {:?}, fwd {:?}\nmodel: tab {} of {}, at {:?}, back {:?}, fwd {:?}",
            a.cur,
            a.tabs.len(),
            a.loc(),
            a.tabs[a.cur].back,
            a.tabs[a.cur].fwd,
            m.cur,
            m.tabs.len(),
            m.tabs[m.cur].loc,
            m.tabs[m.cur].back,
            m.tabs[m.cur].fwd
        ));
    }
}

#[test]
fn history_model() {
    let seeds: u64 = std::env::var("HISTORY").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        let mut s = Sim::new();
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        s.log.push(format!("history seed {seed}"));
        let mut m = Model { tabs: vec![MTab { loc: s.at(), back: vec![], fwd: vec![] }], cur: 0 };
        for step in 0..120 {
            match rng.below(13) {
                0 | 1 => {
                    // Open a visible folder (double click, or click + Enter).
                    if matches!(s.at(), Loc::Trash | Loc::Arc(..)) {
                        continue;
                    }
                    let vr = s.app.view_rect();
                    let dirs: Vec<usize> = (0..s.app.items.len()).filter(|i| s.app.items[*i].e.dir && vr.contains_rect(&s.item_rect(*i))).collect();
                    if dirs.is_empty() {
                        continue;
                    }
                    let i = dirs[rng.below(dirs.len())];
                    let p = s.app.items[i].e.path.clone();
                    let r = s.item_rect(i);
                    if rng.below(2) == 0 {
                        s.dbl(r, &format!("open {}", p.display()));
                    } else {
                        s.click(r, "select");
                        s.key(k::ENTER);
                    }
                    m.go(Loc::Dir(p));
                }
                2 | 3 => {
                    match rng.below(4) {
                        0 => s.key_m(k::LEFT, false, false, true),
                        1 => s.click_b(SIDE, 700.0, 400.0, "mouse back"),
                        2 => {
                            let r = s.app.tool_btn(0);
                            s.click(r, "toolbar back");
                        }
                        _ => s.key(k::BACKSPACE),
                    }
                    m.back();
                }
                4 => {
                    match rng.below(3) {
                        0 => s.key_m(k::RIGHT, false, false, true),
                        1 => s.click_b(EXTRA, 700.0, 400.0, "mouse forward"),
                        _ => {
                            let r = s.app.tool_btn(1);
                            s.click(r, "toolbar forward");
                        }
                    }
                    m.fwd();
                }
                5 => {
                    let p = s.app.parent();
                    if rng.below(2) == 0 {
                        s.key_m(k::UP, false, false, true);
                    } else {
                        let r = s.app.tool_btn(2);
                        s.click(r, "toolbar up");
                    }
                    if let Some(p) = p {
                        m.go(p);
                    }
                }
                6 | 7 => {
                    let j = rng.below(s.app.nav.len());
                    let l = s.app.nav[j].loc.clone();
                    let r = s.app.nav_rects()[j];
                    s.click(r, &format!("nav {}", s.app.nav[j].label));
                    m.go(l);
                }
                8 => {
                    let (cr, _) = s.app.crumb_rects();
                    if cr.is_empty() {
                        continue;
                    }
                    let (r, i) = cr[rng.below(cr.len())];
                    let l = s.app.crumbs()[i].1.clone();
                    s.click(r, "crumb");
                    m.go(l);
                }
                9 => match rng.below(3) {
                    0 => {
                        s.key_m(k::T, true, false, false);
                        m.tabs.insert(m.cur + 1, MTab { loc: Loc::Dir(s.home()), back: vec![], fwd: vec![] });
                        m.cur += 1;
                    }
                    1 if m.tabs.len() > 1 => {
                        s.key_m(k::W, true, false, false);
                        let i = m.cur;
                        m.tabs.remove(i);
                        if m.cur >= m.tabs.len() {
                            m.cur = m.tabs.len() - 1;
                        }
                    }
                    _ => {
                        s.key_m(k::TAB, true, false, false);
                        m.cur = (m.cur + 1) % m.tabs.len();
                    }
                },
                10 => {
                    // A context menu, then a navigation key or button: it only closes the menu.
                    let vr = s.app.view_rect();
                    s.click_b(RIGHT, (vr.x + vr.w - 30) as f64, (vr.y1() - 20) as f64, "right click empty");
                    if s.app.menu.is_none() {
                        s.fail("no menu on right click");
                    }
                    match rng.below(3) {
                        0 => s.key_m(k::LEFT, false, false, true),
                        1 => s.click_b(SIDE, 700.0, 400.0, "mouse back with menu"),
                        _ => s.key(k::ESC),
                    }
                    if s.app.menu.is_some() {
                        s.fail("menu still open");
                    }
                }
                11 => {
                    // Type-ahead and arrows: never navigate.
                    s.key(30 + rng.below(10) as u32);
                    s.key(k::DOWN);
                }
                _ => {
                    // Clicking the current tab or empty space: nothing changes.
                    let vr = s.app.view_rect();
                    s.click_b(LEFT, (vr.x + vr.w - 30) as f64, (vr.y1() - 20) as f64, "empty space");
                    let (tabs, _) = s.app.tab_rects();
                    let r = tabs[s.app.cur];
                    s.click(Rect::new(r.x, r.y, r.w - 40, r.h), "current tab");
                }
            }
            if s.app.dialog.is_some() {
                s.key(k::ESC);
            }
            if std::env::var("TRACE").is_ok() {
                eprintln!("seed {seed} step {step}: {}", s.log.last().cloned().unwrap_or_default());
            }
            compare(&s, &m, step, seed);
        }
    }
}

// ------------------------------------------------------------------ drag and drop between windows

fn starts(s: &Sim) -> Vec<WinReq> {
    s.app.winreq.iter().filter(|r| matches!(r, WinReq::StartDrag { .. })).cloned().collect()
}

fn told(s: &Sim) -> Option<WinReq> {
    s.app.winreq.iter().rev().find(|r| matches!(r, WinReq::DndAccept { .. })).cloned()
}

/// A system drag from another program: enter at (x, y) offering `mime` with `data`.
fn enter(s: &mut Sim, x: f64, y: f64, mime: &str, data: String) {
    s.log.push(format!("dnd enter {x:.0},{y:.0} {mime}"));
    s.app.winreq.clear();
    let mut read = |m: &str| (m == mime).then(|| data.clone().into_bytes());
    s.app.dnd_enter(x, y, &[mime.to_string()], &mut read);
}

#[test]
fn uris_and_tabs_as_text() {
    let p = PathBuf::from("/tmp/a b/Отчёт #1.txt");
    let u = open::uri_of(&p);
    assert!(u.starts_with("file:///tmp/a%20b/"));
    std::fs::create_dir_all("/tmp/a b").unwrap();
    std::fs::write(&p, "x").unwrap();
    assert_eq!(dnd::parse_uris(format!("# comment\r\n{u}\r\nfile:///nope/gone\r\n").as_bytes()), vec![p.clone()]);
    let _ = std::fs::remove_dir_all("/tmp/a b");
    let t = Tab {
        loc: Loc::Search("/x".into(), "q w".into()),
        back: vec![Loc::Dir("/a".into()), Loc::Pc, Loc::Trash, Loc::Arc("/f.7z".into(), "in/side".into())],
        fwd: vec![Loc::Dir("/b".into())],
        sel: vec![],
        scroll: 0.0,
    };
    let back = dnd::tab_parse(&dnd::tab_str(&t)).unwrap();
    assert_eq!((back.loc, back.back, back.fwd), (t.loc, t.back, t.fwd));
}

#[test]
fn files_dropped_from_elsewhere() {
    let mut s = Sim::new();
    let home = s.home();
    write(&home.join("Downloads/a.txt"), "a");
    write(&home.join("Downloads/b.txt"), "b");
    s.open("Документы");
    let vr = s.app.view_rect();
    let (ex, ey) = ((vr.x + vr.w - 40) as f64, (vr.y1() - 20) as f64);
    // Onto empty space: into this folder; the window says it takes text/uri-list.
    enter(&mut s, ex, ey, dnd::URI_MIME, open::uri_of(&home.join("Downloads/a.txt")) + "\r\n");
    match told(&s) {
        Some(WinReq::DndAccept { mime: Some(m), actions, .. }) => assert!(m == dnd::URI_MIME && actions & dnd::COPY != 0),
        r => s.fail(&format!("no accept: {r:?}")),
    }
    s.app.dnd_drop(dnd::COPY);
    s.settle();
    assert!(home.join("Documents/a.txt").exists() && home.join("Downloads/a.txt").exists(), "copied");
    // Onto a folder, moving.
    let i = s.item("Счета");
    let r = s.item_rect(i);
    enter(&mut s, r.x as f64 + 30.0, r.y as f64 + 10.0, dnd::URI_MIME, open::uri_of(&home.join("Downloads/b.txt")));
    s.app.dnd_drop(dnd::MOVE);
    s.settle();
    assert!(home.join("Documents/Счета/b.txt").exists() && !home.join("Downloads/b.txt").exists(), "moved into the folder");
    // Things already here, onto empty space: nothing to do.
    enter(&mut s, ex, ey, dnd::URI_MIME, open::uri_of(&home.join("Documents/notes.md")));
    assert!(matches!(told(&s), Some(WinReq::DndAccept { mime: None, .. })), "not into its own folder");
    s.app.dnd_leave();
    s.settle();
    // A right drag asks.
    write(&home.join("Downloads/c.txt"), "c");
    enter(&mut s, ex, ey, dnd::URI_MIME, open::uri_of(&home.join("Downloads/c.txt")));
    s.app.dnd_drop(dnd::ASK);
    s.settle();
    let m = s.app.menu.as_ref().unwrap_or_else(|| s.fail("no drop menu"));
    let i = m.items.iter().position(|it| it.label == "Отмена").unwrap();
    let r = m.item_rect(i);
    s.click(r, "Отмена");
    assert!(!home.join("Documents/c.txt").exists() && s.app.drop_paths.is_none());
    // And the selection is not what a later drop moves.
    let i = s.item("notes.md");
    s.click(s.item_rect(i), "notes.md");
    assert!(home.join("Downloads/c.txt").exists());
}

#[test]
fn items_dragged_out_and_back() {
    let mut s = Sim::new();
    let home = s.home();
    s.open("Документы");
    let i = s.item("notes.md");
    let r = s.item_rect(i);
    let (x0, y0) = (r.x as f64 + 40.0, r.y as f64 + 10.0);
    s.app.motion(x0, y0);
    s.app.button(LEFT, true);
    s.app.motion(x0 + 30.0, y0 + 5.0);
    s.app.motion(-40.0, y0);
    let st = starts(&s);
    assert_eq!(st.len(), 1, "one system drag");
    if let WinReq::StartDrag { mimes, actions, icon } = &st[0] {
        assert!(mimes.contains(&dnd::URI_MIME) && *actions == dnd::COPY | dnd::MOVE && icon.is_some());
    }
    let data = String::from_utf8(s.app.drag_data(dnd::URI_MIME)).unwrap();
    assert_eq!(data, open::uri_of(&home.join("Documents/notes.md")) + "\r\n");
    s.app.motion(-60.0, y0);
    assert_eq!(starts(&s).len(), 1, "started once");
    // Back over our own window, onto «Счета», dropped (no button release comes: the
    // compositor holds the pointer during a system drag).
    let j = s.item("Счета");
    let r = s.item_rect(j);
    s.app.winreq.clear();
    let mut read = |_: &str| -> Option<Vec<u8>> { panic!("reading our own drag would hang") };
    s.app.dnd_enter(r.x as f64 + 30.0, r.y as f64 + 10.0, &[dnd::URI_MIME.to_string()], &mut read);
    assert!(matches!(told(&s), Some(WinReq::DndAccept { mime: Some(_), .. })));
    s.app.dnd_drop(dnd::MOVE);
    s.app.drag_end(true);
    s.settle();
    assert!(home.join("Documents/Счета/notes.md").exists());
    assert!(s.app.outgoing.is_none() && s.app.press.is_none() && s.app.incoming.is_none());
    // Dragged out and let go nowhere: nothing happens.
    let i = s.item("config.json");
    let r = s.item_rect(i);
    s.app.motion(r.x as f64 + 40.0, r.y as f64 + 10.0);
    s.app.button(LEFT, true);
    s.app.motion(r.x as f64 + 80.0, r.y as f64 + 30.0);
    s.app.motion(s.app.w as f64 + 50.0, 300.0);
    s.app.dnd_leave();
    s.app.drag_end(false);
    s.settle();
    assert!(home.join("Documents/config.json").exists() && s.app.press.is_none());
}

#[test]
fn tabs_torn_off_and_taken_in() {
    let mut s = Sim::new();
    s.open("Документы");
    s.key_m(k::T, true, false, false);
    s.open("Загрузки");
    assert_eq!(s.app.tabs.len(), 2);
    // Pull tab 2 down off the strip and let go outside any window: a window of its own.
    let (tabs, _) = s.app.tab_rects();
    let r = tabs[1];
    s.app.motion(r.x as f64 + 40.0, 20.0);
    s.app.button(LEFT, true);
    s.app.motion(r.x as f64 + 40.0, 60.0);
    assert!(starts(&s).is_empty(), "still on the strip");
    s.app.motion(r.x as f64 + 40.0, 200.0);
    let st = starts(&s);
    assert!(matches!(&st[..], [WinReq::StartDrag { mimes, .. }] if mimes == &vec![dnd::TAB_MIME]));
    let carried = dnd::tab_parse(&String::from_utf8(s.app.drag_data(dnd::TAB_MIME)).unwrap()).unwrap();
    assert_eq!(carried.loc, Loc::Dir(s.home().join("Downloads")));
    assert_eq!(carried.back, vec![Loc::Dir(s.home())], "its history goes with it");
    open::DRY_LOG.lock().unwrap().clear();
    s.app.dnd_leave();
    s.app.drag_end(false);
    s.settle();
    assert_eq!(s.app.tabs.len(), 1);
    let log = open::DRY_LOG.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("ctl place-next resc.files")), "{log:?}");
    assert!(log.iter().any(|l| l.starts_with("spawn") && l.contains("files --tab")), "{log:?}");
    // A tab from another window over our strip: taken in where it is let go.
    let t = Tab { loc: Loc::Dir(s.home().join("Music")), back: vec![Loc::Dir(s.home())], fwd: vec![], sel: vec![], scroll: 0.0 };
    let (tabs, _) = s.app.tab_rects();
    enter(&mut s, (tabs[0].x + 4) as f64, 20.0, dnd::TAB_MIME, dnd::tab_str(&t));
    assert!(matches!(told(&s), Some(WinReq::DndAccept { mime: Some(m), .. }) if m == dnd::TAB_MIME));
    assert_eq!(s.app.tab_drop_at, Some(0));
    // Not over the content.
    s.app.dnd_motion(500.0, 400.0);
    assert!(matches!(told(&s), Some(WinReq::DndAccept { mime: None, .. })));
    s.app.dnd_motion((tabs[0].x + 4) as f64, 20.0);
    s.app.dnd_drop(dnd::MOVE);
    s.settle();
    assert_eq!(s.app.tabs.len(), 2);
    assert_eq!(s.app.cur, 0);
    assert_eq!(s.at(), Loc::Dir(s.home().join("Music")));
    s.key_m(k::LEFT, false, false, true);
    assert_eq!(s.at(), Loc::Dir(s.home()), "with its history");
    // Our only tab dragged and let go away from any strip: the window moves there.
    s.key_m(k::W, true, false, false);
    assert_eq!(s.app.tabs.len(), 1);
    open::DRY_LOG.lock().unwrap().clear();
    let (tabs, _) = s.app.tab_rects();
    s.app.motion(tabs[0].x as f64 + 40.0, 20.0);
    s.app.button(LEFT, true);
    s.app.motion(tabs[0].x as f64 + 40.0, 300.0);
    s.app.drag_end(false);
    s.settle();
    let log = open::DRY_LOG.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("ctl move-active")), "{log:?}");
    assert!(!s.app.quit && s.app.tabs.len() == 1);
}

// ------------------------------------------------------------------ the desktop

#[test]
fn desktop_icons() {
    let mut s = Sim::desk();
    let desk = s.home().join("Desktop");
    assert_eq!(s.at(), Loc::Dir(desk.clone()));
    assert_eq!(s.app.view_rect(), Rect::new(0, 0, 1600, 836));
    assert!(matches!(s.app.view.mode, Mode::Icons(48)));
    // Column by column from the top left.
    assert_eq!(s.app.rects[0].x, s.app.rects[1].x);
    assert!(s.app.rects[1].y > s.app.rects[0].y);
    // A folder opens in a window; the desktop stays where it is.
    open::DRY_LOG.lock().unwrap().clear();
    s.open("Проекты 2026");
    assert_eq!(s.at(), Loc::Dir(desk.clone()));
    let log = open::DRY_LOG.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("spawn") && l.contains(" files ") && l.contains("Проекты 2026")), "{log:?}");
    // Window and tab keys do nothing here; the desktop can't be closed.
    for (c, ctrl, alt) in [(k::W, true, false), (k::T, true, false), (k::LEFT, false, true), (k::UP, false, true), (k::BACKSPACE, false, false)] {
        s.key_m(c, ctrl, false, alt);
        assert_eq!(s.at(), Loc::Dir(desk.clone()));
        assert!(!s.app.quit && s.app.tabs.len() == 1);
    }
    // Its own menu on the wallpaper.
    s.click_b(RIGHT, 900.0, 700.0, "right click the wallpaper");
    let m = s.app.menu.as_ref().unwrap_or_else(|| s.fail("no menu"));
    assert!(m.items.iter().any(|i| i.label == "Персонализация"));
    assert!(!m.items.iter().any(|i| i.label == "Обновить"), "the desktop needs no «Обновить»");
    s.key(k::ESC);
    // New folder, named in place.
    s.key_m(k::N, true, true, false);
    for c in [20, 18, 31, 20] {
        s.key(c);
    }
    s.key(k::ENTER);
    assert!(desk.join("test").is_dir());
    // Files dropped from a window onto the wallpaper land here.
    write(&s.home().join("Downloads/z.txt"), "z");
    let z = open::uri_of(&s.home().join("Downloads/z.txt"));
    enter(&mut s, 900.0, 600.0, dnd::URI_MIME, z);
    s.app.dnd_drop(dnd::COPY);
    s.settle();
    assert!(desk.join("z.txt").exists());
}

#[test]
fn desktop_monkey() {
    let seeds: u64 = std::env::var("MONKEY").ok().and_then(|v| v.parse().ok()).map(|n: u64| (n / 4).max(1)).unwrap_or(6);
    for seed in 1..=seeds {
        let mut s = Sim::desk();
        let desk = s.home().join("Desktop");
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 77);
        s.log.push(format!("desktop monkey seed {seed}"));
        for _ in 0..300 {
            let (w, h) = (s.app.w as f64, s.app.h as f64);
            let (x, y) = ((rng.below(1000) as f64 / 1000.0) * w, (rng.below(1000) as f64 / 1000.0) * h);
            match rng.below(9) {
                0..=2 => s.click_b(LEFT, x, y, "click"),
                3 => s.click_b(RIGHT, x, y, "right"),
                4 => {
                    s.app.last_click = (0, Hit::None);
                    s.log.push(format!("double {x:.0},{y:.0}"));
                    s.press(LEFT, x, y);
                    s.press(LEFT, x, y);
                    s.settle();
                }
                5 => s.key_m([k::A, k::C, k::V, k::Z, k::W, k::T, k::N][rng.below(7)], true, rng.below(3) == 0, false),
                6 => s.key_m([k::LEFT, k::UP, k::RIGHT][rng.below(3)], false, false, true),
                _ => s.key([k::UP, k::DOWN, k::LEFT, k::RIGHT, k::ENTER, k::ESC, k::F2, k::F5, k::DELETE, k::BACKSPACE, 30, 49][rng.below(12)]),
            }
            if s.app.dialog.is_some() && rng.below(2) == 0 {
                s.key(k::ESC);
            }
            if s.at() != Loc::Dir(desk.clone()) {
                s.fail("the desktop went somewhere else");
            }
            if s.app.quit || s.app.tabs.len() != 1 {
                s.fail("the desktop closed or grew tabs");
            }
        }
    }
}

// ------------------------------------------------------------------ monkey

#[test]
fn monkey() {
    let keys = [
        k::UP, k::DOWN, k::LEFT, k::RIGHT, k::HOME, k::END, k::PGUP, k::PGDN, k::ENTER, k::ESC, k::BACKSPACE, k::DELETE, k::F2, k::F5, k::TAB, k::SPACE, k::MENU, 30, 31, 32, 49, 18, 20,
    ];
    let ctrl_keys = [k::A, k::C, k::X, k::V, k::Z, k::Y, k::T, k::W, k::H, k::L, k::TAB, k::N];
    // MONKEY=300 cargo test … for a long run.
    let seeds: u64 = std::env::var("MONKEY").ok().and_then(|v| v.parse().ok()).unwrap_or(25);
    let from: u64 = std::env::var("MONKEY_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    for seed in from..=seeds.max(from) {
        let mut s = Sim::new();
        let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
        s.log.push(format!("monkey seed {seed}"));
        for _ in 0..400 {
            let (w, h) = (s.app.w as f64, s.app.h as f64);
            let x = (rng.below(1000) as f64 / 1000.0) * w;
            let y = (rng.below(1000) as f64 / 1000.0) * h;
            match rng.below(19) {
                16 => {
                    // Something dragged out of the window; the system drag ends somehow.
                    s.log.push(format!("monkey drag out from {x:.0},{y:.0}"));
                    s.app.winreq.clear();
                    s.app.motion(x, y);
                    s.app.button([LEFT, RIGHT][rng.below(2)], true);
                    s.app.motion(x + 20.0, y + 10.0);
                    s.app.motion(-30.0, y);
                    if starts(&s).is_empty() {
                        let b = s.app.press.as_ref().map_or(LEFT, |p| p.button);
                        s.app.button(b, false);
                    } else {
                        if rng.below(2) == 0 {
                            let (x2, y2) = ((rng.below(1000) as f64 / 1000.0) * w, (rng.below(1000) as f64 / 1000.0) * h);
                            let mut read = |_: &str| -> Option<Vec<u8>> { None };
                            s.app.dnd_enter(x2, y2, &[dnd::URI_MIME.to_string()], &mut read);
                            s.check();
                            if rng.below(2) == 0 {
                                s.app.dnd_drop([dnd::COPY, dnd::MOVE, dnd::ASK][rng.below(3)]);
                            } else {
                                s.app.dnd_leave();
                            }
                        }
                        s.app.drag_end(rng.below(2) == 0);
                    }
                    s.settle();
                }
                17 => {
                    // Files from elsewhere (or a tab) dropped somewhere on the window.
                    let tab = rng.below(3) == 0;
                    let (mime, data) = if tab {
                        (dnd::TAB_MIME, dnd::tab_str(&Tab { loc: Loc::Dir(s.home().join("Music")), back: vec![], fwd: vec![], sel: vec![], scroll: 0.0 }))
                    } else {
                        let n = format!("in{}.txt", rng.below(1000));
                        let p = s.root.join("outside").join(&n);
                        write(&p, "x");
                        (dnd::URI_MIME, open::uri_of(&p))
                    };
                    enter(&mut s, x, y, mime, data);
                    s.check();
                    let (x2, y2) = ((rng.below(1000) as f64 / 1000.0) * w, (rng.below(1000) as f64 / 1000.0) * h);
                    s.app.dnd_motion(x2, y2);
                    s.check();
                    if rng.below(3) == 0 {
                        s.app.dnd_leave();
                    } else {
                        s.app.dnd_drop([dnd::COPY, dnd::MOVE, dnd::ASK][rng.below(3)]);
                    }
                    s.settle();
                }
                18 => {
                    // A tab pulled off the strip.
                    let (tabs, _) = s.app.tab_rects();
                    let t = tabs[rng.below(tabs.len())];
                    s.log.push("monkey tab out".into());
                    s.app.winreq.clear();
                    s.app.motion(t.x as f64 + 30.0, 20.0);
                    s.app.button(LEFT, true);
                    s.app.motion(t.x as f64 + 30.0, 250.0);
                    if starts(&s).is_empty() {
                        s.app.button(LEFT, false);
                    } else {
                        s.app.drag_end(rng.below(2) == 0);
                    }
                    s.settle();
                }
                0..=3 => s.click_b(LEFT, x, y, "monkey click"),
                4 => {
                    s.log.push(format!("monkey double {x:.0},{y:.0}"));
                    s.app.last_click = (0, Hit::None);
                    s.press(LEFT, x, y);
                    s.press(LEFT, x, y);
                    s.settle();
                }
                5 => s.click_b(RIGHT, x, y, "monkey right"),
                6 => s.click_b([MIDDLE, SIDE, EXTRA][rng.below(3)], x, y, "monkey other button"),
                7 => {
                    // A drag.
                    let (x2, y2) = ((rng.below(1000) as f64 / 1000.0) * w, (rng.below(1000) as f64 / 1000.0) * h);
                    s.log.push(format!("monkey drag {x:.0},{y:.0} → {x2:.0},{y2:.0}"));
                    s.app.motion(x, y);
                    s.app.button(LEFT, true);
                    for t in 1..=5 {
                        let f = t as f64 / 5.0;
                        s.app.motion(x + (x2 - x) * f, y + (y2 - y) * f);
                    }
                    s.app.button(LEFT, false);
                    s.settle();
                }
                8 => {
                    s.log.push(format!("monkey wheel {x:.0},{y:.0}"));
                    s.app.motion(x, y);
                    s.app.wheel(if rng.below(2) == 0 { 30.0 } else { -30.0 });
                    s.settle();
                }
                9 => {
                    let c = ctrl_keys[rng.below(ctrl_keys.len())];
                    s.key_m(c, true, rng.below(4) == 0, false);
                }
                10 => s.key_m([k::LEFT, k::RIGHT, k::UP][rng.below(3)], false, false, true),
                _ => {
                    let c = keys[rng.below(keys.len())];
                    s.key_m(c, false, rng.below(5) == 0, false);
                }
            }
            if s.app.dialog.is_some() && rng.below(3) == 0 {
                s.key(k::ESC);
            }
        }
    }
}

impl Sim {
    /// A left-button drag from (x0, y0) to (x1, y1), in steps.
    fn drag(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, what: &str) {
        self.log.push(format!("drag {x0:.0},{y0:.0} → {x1:.0},{y1:.0} {what}"));
        self.app.last_click = (0, Hit::None);
        self.app.motion(x0, y0);
        self.app.button(LEFT, true);
        for k in 1..=8 {
            let t = k as f64 / 8.0;
            self.app.motion(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        }
        self.app.button(LEFT, false);
        self.settle();
    }

    fn cell_of(&self, name: &str) -> (i32, i32) {
        let i = self.item(name);
        self.app.desk_cell_of(i)
    }

    fn cell_center(&self, c: (i32, i32)) -> (f64, f64) {
        let r = self.app.screen(self.app.desk_cell_rect(c));
        (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0)
    }
}

#[test]
fn desktop_icons_stay_where_put() {
    let mut s = Sim::desk();
    let cells = |s: &Sim| -> Vec<(i32, i32)> { (0..s.app.items.len()).map(|i| s.app.desk_cell_of(i)).collect() };
    let distinct = |s: &Sim| {
        let v = cells(s);
        let set: std::collections::HashSet<_> = v.iter().collect();
        assert_eq!(set.len(), v.len(), "two icons in one cell: {v:?}");
    };
    // Onto an empty cell far away: it is there, the others did not move.
    let before: Vec<((i32, i32), String)> = s.app.items.iter().enumerate().map(|(i, it)| (s.app.desk_cell_of(i), it.e.name.clone())).collect();
    let from = s.cell_center(s.cell_of("Отчёт.pdf"));
    let to = s.cell_center((10, 3));
    s.drag(from.0, from.1, to.0, to.1, "Отчёт.pdf to cell 10,3");
    assert_eq!(s.cell_of("Отчёт.pdf"), (10, 3));
    for (c, n) in &before {
        if n != "Отчёт.pdf" {
            assert_eq!(s.cell_of(n), *c, "{n} moved");
        }
    }
    distinct(&s);
    // Kept: the desktop started anew has them in the same places.
    let now = cells(&s);
    s.app = App::new(Loc::Dir(desktop::desktop_dir()), 1600, 836);
    s.app.desk = true;
    s.app.view = s.app.view_for(&s.app.loc().clone());
    s.app.lay();
    s.settle();
    assert_eq!(cells(&s), now, "the places were not kept");
    // Onto another icon: the nearest free cell, nobody shares one.
    let from = s.cell_center(s.cell_of("фото.png"));
    let to = s.cell_center((10, 3));
    s.drag(from.0, from.1, to.0, to.1, "фото.png onto Отчёт.pdf");
    let c = s.cell_of("фото.png");
    assert_eq!(s.cell_of("Отчёт.pdf"), (10, 3));
    assert!((c.0 - 10).abs() + (c.1 - 3).abs() == 1, "фото.png went to {c:?}");
    distinct(&s);
    // A selection moves together, keeping its shape.
    let (a, b) = (s.cell_of("фото.png"), s.cell_of("Отчёт.pdf"));
    let (ax, ay) = s.cell_center(a);
    s.click_b(LEFT, ax, ay, "select фото.png");
    let (bx, by) = s.cell_center(b);
    s.mods(true, false, false);
    s.click_b(LEFT, bx, by, "ctrl-select Отчёт.pdf");
    s.mods(false, false, false);
    let to = s.cell_center((a.0 + 2, a.1 + 2));
    s.drag(ax, ay, to.0, to.1, "both by 2,2");
    assert_eq!(s.cell_of("фото.png"), (a.0 + 2, a.1 + 2));
    assert_eq!(s.cell_of("Отчёт.pdf"), (b.0 + 2, b.1 + 2));
    distinct(&s);
    // Dragged onto a folder it still goes into the folder (a move, not a place).
    let folder = s.cell_center(s.cell_of("Проекты 2026"));
    let from = s.cell_center(s.cell_of("Список дел.txt"));
    s.click_b(LEFT, from.0, from.1, "select Список дел.txt");
    s.drag(from.0, from.1, folder.0, folder.1, "Список дел.txt into the folder");
    assert!(s.home().join("Desktop/Проекты 2026/Список дел.txt").exists(), "not moved into the folder");
    // «Сортировка» lines them all up from the top left again.
    s.app.act(Act::Sort(Col::Name));
    s.settle();
    let mut v = cells(&s);
    v.sort();
    assert_eq!(v, (0..v.len() as i32).map(|r| (0, r)).collect::<Vec<_>>());
}
