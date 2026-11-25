//! The desktop's icons: «Проводник» in desktop mode (`resc-desk files --desktop`). The
//! compositor keeps this window under all others over the whole work area, with no frame,
//! taskbar button or Alt+Tab card (app id resc.desktop). It shows ~/Desktop as icons on
//! the wallpaper, column by column from the top left as in Windows; folders open in a
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

impl App {
    /// Column by column from the top left, wrapping to the next column at the bottom.
    pub fn lay_desktop(&mut self) {
        let s = match self.view.mode {
            Mode::Icons(s) => s,
            _ => 48,
        };
        let (cw, ch) = cell(s);
        let rows = ((self.h - 16) / (ch + 4)).max(1);
        self.rects = (0..self.items.len())
            .map(|i| {
                let (c, r) = (i as i32 / rows, i as i32 % rows);
                Rect::new(8 + c * (cw + 4), 8 + r * (ch + 4), cw, ch)
            })
            .collect();
        self.heads.clear();
        self.content_h = self.h;
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
