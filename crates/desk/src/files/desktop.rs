//! The desktop's icons: «Проводник» in desktop mode (`resc-desk files --desktop`). The
//! compositor keeps this window under all others over the whole work area, with no frame,
//! taskbar button or Alt+Tab card (app id resc.desktop). It shows ~/Desktop as icons on
//! the wallpaper on a grid, as in Windows: an icon dragged onto the wallpaper stays in the
//! cell it is dropped in (the nearest free one if that is taken), and the places are kept
//! (~/.config/resc-desk/desktop-icons.conf); new icons take the first free cell column by
//! column from the top left; «Сортировка» lines them all up again. Folders open in a
//! «Проводник» window. Selecting, renaming, menus, the clipboard and drag and drop are the
//! explorer's own.
use super::*;

/// ~/Desktop (XDG_DESKTOP_DIR), made if missing.
pub fn desktop_dir() -> PathBuf {
    let p = model::user_dirs().into_iter().find(|(k, _)| *k == "DESKTOP").map(|(_, p)| p).unwrap_or_else(|| model::home().join("Desktop"));
    let _ = std::fs::create_dir_all(&p);
    p
}

/// The icon cell for icon size `s` (Windows: 75×91 at the normal 48).
pub fn cell(s: i32) -> (i32, i32) {
    ((s + 36).max(80), s + 54)
}

fn cells_path() -> PathBuf {
    let cfg = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| model::home().join(".config"));
    cfg.join("resc-desk").join("desktop-icons.conf")
}

/// The icons' kept cells: `column row name` a line.
pub fn load_cells() -> HashMap<String, (i32, i32)> {
    let mut m = HashMap::new();
    for l in std::fs::read_to_string(cells_path()).unwrap_or_default().lines() {
        let mut it = l.splitn(3, ' ');
        if let (Some(c), Some(r), Some(n)) = (it.next(), it.next(), it.next()) {
            if let (Ok(c), Ok(r)) = (c.parse(), r.parse()) {
                m.insert(n.to_string(), (c, r));
            }
        }
    }
    m
}

fn save_cells(m: &HashMap<String, (i32, i32)>) {
    let p = cells_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut v: Vec<_> = m.iter().collect();
    v.sort_by_key(|(n, c)| (c.0, c.1, n.to_string()));
    let s: String = v.iter().map(|(n, (c, r))| format!("{c} {r} {n}\n")).collect();
    let _ = std::fs::write(p, s);
}

/// The free cell nearest to `want` (by distance, then column by column), in a grid of
/// `cols`×`rows`.
fn nearest_free(want: (i32, i32), cols: i32, rows: i32, taken: &std::collections::HashSet<(i32, i32)>) -> (i32, i32) {
    let mut best = None;
    for c in 0..cols {
        for r in 0..rows {
            if taken.contains(&(c, r)) {
                continue;
            }
            let d = (c - want.0).pow(2) + (r - want.1).pow(2);
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, (c, r)));
            }
        }
    }
    // Every cell taken: past the last column.
    best.map_or((cols, want.1), |(_, c)| c)
}

impl App {
    /// On the desktop: where the icon of `p` is (window coordinates), for a launch to grow
    /// out of.
    pub fn desk_icon_rect(&self, p: &std::path::Path) -> Option<Rect> {
        if !self.desk {
            return None;
        }
        let i = self.items.iter().position(|x| x.e.path == p)?;
        let r = *self.rects.get(i)?;
        let vr = self.view_rect();
        let s = self.icon_size();
        Some(Rect::new(vr.x + r.x + (r.w - s) / 2, vr.y + r.y + 8, s, s))
    }

    fn icon_size(&self) -> i32 {
        match self.view.mode {
            Mode::Icons(s) => s,
            _ => 48,
        }
    }

    /// The grid: a cell's width and height, columns, rows.
    pub fn desk_grid(&self) -> (i32, i32, i32, i32) {
        let (cw, ch) = cell(self.icon_size());
        (cw, ch, ((self.w - 16) / (cw + 4)).max(1), ((self.h - 16) / (ch + 4)).max(1))
    }

    pub fn desk_cell_rect(&self, c: (i32, i32)) -> Rect {
        let (cw, ch, _, _) = self.desk_grid();
        Rect::new(8 + c.0 * (cw + 4), 8 + c.1 * (ch + 4), cw, ch)
    }

    /// The cell under (`x`, `y`) of the window (clamped to the grid).
    pub fn desk_cell_at(&self, x: f64, y: f64) -> (i32, i32) {
        let (cw, ch, cols, rows) = self.desk_grid();
        let vr = self.view_rect();
        let c = ((x - vr.x as f64 - 8.0) / (cw + 4) as f64).floor() as i32;
        let r = ((y - vr.y as f64 - 8.0) / (ch + 4) as f64).floor() as i32;
        (c.clamp(0, cols - 1), r.clamp(0, rows - 1))
    }

    /// The cell item `i` is in now.
    pub fn desk_cell_of(&self, i: usize) -> (i32, i32) {
        let (cw, ch, _, _) = self.desk_grid();
        let r = self.rects[i];
        ((r.x - 8) / (cw + 4), (r.y - 8) / (ch + 4))
    }

    /// Each icon in its kept cell (if it is on the grid and free), the others in the first
    /// free cells column by column from the top left.
    pub fn lay_desktop(&mut self) {
        if !self.desk_loaded {
            self.desk_cells = load_cells();
            self.desk_loaded = true;
        }
        let (_, _, cols, rows) = self.desk_grid();
        let mut taken = std::collections::HashSet::new();
        let mut cells: Vec<Option<(i32, i32)>> = vec![None; self.items.len()];
        for (i, it) in self.items.iter().enumerate() {
            if let Some(&c) = self.desk_cells.get(&it.e.name) {
                if c.0 >= 0 && c.0 < cols && c.1 >= 0 && c.1 < rows && taken.insert(c) {
                    cells[i] = Some(c);
                }
            }
        }
        let mut next = 0;
        for cell in cells.iter_mut().filter(|c| c.is_none()) {
            while taken.contains(&(next / rows, next % rows)) {
                next += 1;
            }
            let c = (next / rows, next % rows);
            taken.insert(c);
            *cell = Some(c);
        }
        self.rects = cells.into_iter().map(|c| self.desk_cell_rect(c.unwrap_or((0, 0)))).collect();
        self.heads.clear();
        self.content_h = self.h;
    }

    /// Where the selected icons land if dragged from `from` to `to` (window coordinates):
    /// each moves by the cells between, into the nearest free cell if that one is taken.
    pub fn desk_plan(&self, from: (f64, f64), to: (f64, f64)) -> Vec<(usize, (i32, i32))> {
        let (_, _, cols, rows) = self.desk_grid();
        let a = self.desk_cell_at(from.0, from.1);
        let b = self.desk_cell_at(to.0, to.1);
        let d = (b.0 - a.0, b.1 - a.1);
        let moving = self.selected();
        let mut taken: std::collections::HashSet<(i32, i32)> =
            (0..self.items.len()).filter(|i| !moving.contains(i)).map(|i| self.desk_cell_of(i)).collect();
        // The one under the pointer first, then the others nearest to it.
        let mut order = moving.clone();
        order.sort_by_key(|i| {
            let c = self.desk_cell_of(*i);
            ((c.0 - a.0).abs() + (c.1 - a.1).abs(), c.0, c.1)
        });
        let mut out = Vec::new();
        for i in order {
            let c = self.desk_cell_of(i);
            let want = ((c.0 + d.0).clamp(0, cols - 1), (c.1 + d.1).clamp(0, rows - 1));
            let got = if taken.contains(&want) { nearest_free(want, cols, rows, &taken) } else { want };
            taken.insert(got);
            out.push((i, got));
        }
        out
    }

    /// The selected icons dropped on the wallpaper: they stay where they were put, and every
    /// icon's place is kept from now on.
    pub fn desk_move(&mut self, from: (f64, f64), to: (f64, f64)) {
        let plan = self.desk_plan(from, to);
        if plan.iter().all(|(i, c)| self.desk_cell_of(*i) == *c) {
            return;
        }
        let mut cells: HashMap<String, (i32, i32)> = (0..self.items.len()).map(|i| (self.items[i].e.name.clone(), self.desk_cell_of(i))).collect();
        for (i, c) in plan {
            cells.insert(self.items[i].e.name.clone(), c);
        }
        self.desk_cells = cells;
        save_cells(&self.desk_cells);
        self.lay_desktop();
        // The focus ring and the hover follow the icons to their new cells.
        if let Some(f) = self.focus.filter(|f| *f < self.rects.len()) {
            self.fdrop.go(self.rects[f]);
        }
        match self.ptr.map(|(x, y)| self.hit(x, y)) {
            Some(Hit::Item(i)) => self.hdrop.go(self.rects[i]),
            _ => self.hdrop.hide(),
        }
        self.dirty = true;
    }

    /// A renamed icon keeps its place.
    pub fn desk_renamed(&mut self, old: &str, new: &str) {
        if let Some(c) = self.desk_cells.remove(old) {
            self.desk_cells.insert(new.to_string(), c);
            save_cells(&self.desk_cells);
        }
    }

    /// «Сортировка»: all lined up again from the top left.
    pub fn desk_line_up(&mut self) {
        self.desk_cells.clear();
        save_cells(&self.desk_cells);
        self.lay_desktop();
    }

    /// A place to show from the desktop: in a «Проводник» window.
    pub fn open_window(&mut self, l: &Loc) {
        let arg = match l {
            Loc::Dir(p) | Loc::Arc(p, _) | Loc::Search(p, _) => p.to_string_lossy().to_string(),
            Loc::Pc => "computer:".into(),
            Loc::Trash => "trash:".into(),
        };
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "resc-desk".into());
        open::spawn(&format!("{} files {}", open::quote(&exe), open::quote(&arg)), None);
    }

    /// Right click on the wallpaper.
    pub fn desk_menu(&self) -> Vec<MItem> {
        let size = match self.view.mode {
            Mode::Icons(s) => s,
            _ => 48,
        };
        let view = vec![
            MItem::new("Крупные значки", Act::Mode(Mode::Icons(96))).check(size == 96),
            MItem::new("Обычные значки", Act::Mode(Mode::Icons(48))).check(size == 48),
            MItem::new("Мелкие значки", Act::Mode(Mode::Icons(32))).check(size == 32),
        ];
        let sort = vec![
            MItem::new("Имя", Act::Sort(Col::Name)).check(self.view.sort == Col::Name),
            MItem::new("Размер", Act::Sort(Col::Size)).check(self.view.sort == Col::Size),
            MItem::new("Тип", Act::Sort(Col::Type)).check(self.view.sort == Col::Type),
            MItem::new("Дата изменения", Act::Sort(Col::Date)).check(self.view.sort == Col::Date),
        ];
        vec![
            MItem::new("Вид", Act::None).g(G::View).sub(view),
            MItem::new("Сортировка", Act::None).g(G::Sort).sub(sort),
            MItem::sep(),
            MItem::new("Вставить", Act::Paste).hint("Ctrl+V").g(G::Paste).on(self.clip.is_some()),
            MItem::new("Отменить", Act::Undo).hint("Ctrl+Z").on(!self.undo.is_empty()),
            MItem::sep(),
            MItem::new("Создать", Act::None).g(G::Plus).sub(self.new_menu()),
            MItem::sep(),
            MItem::new("Открыть в терминале", Act::Terminal),
            MItem::new("Параметры экрана", Act::Settings("displays")),
            MItem::new("Персонализация", Act::Settings("wallpaper")),
        ]
    }
}
