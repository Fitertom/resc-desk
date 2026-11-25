//! Single-line text field for the shell's own inputs (start menu search, later the
//! settings), with the editing keys people expect from Windows and GTK:
//! arrows, Home/End, Ctrl+arrows by word, Shift+any of them to select, Backspace/Delete
//! (Ctrl: by word), Ctrl+A, Ctrl+C/X/V and Ctrl/Shift+Insert, Shift+Delete, Ctrl+Z and
//! Ctrl+Y / Ctrl+Shift+Z, and text in the current layout (us / ru) with Shift and Caps.
use crate::font;
use crate::geom::Rect;
use crate::render::{argb, Canvas, Xf};

/// Modifier state for one key press.
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps: bool,
    /// Layout group: 0 us, 1 ru.
    pub group: u32,
}

/// What the owner of the field has to do after a key.
#[derive(Debug, PartialEq)]
pub enum Act {
    /// Not an editing key: the owner may use it (Up/Down, Enter, Esc, Tab...).
    Pass,
    /// Caret or selection moved.
    Moved,
    /// The text changed.
    Changed,
    /// Put this on the clipboard.
    Copy(String),
    /// Put this on the clipboard; the text already changed.
    Cut(String),
    /// Insert the clipboard (the owner fetches it and calls `insert`).
    Paste,
}

#[derive(Default, Clone)]
pub struct TextField {
    text: Vec<char>,
    /// Caret, in chars.
    pub caret: usize,
    /// The other end of the selection, if any.
    pub anchor: Option<usize>,
    undo: Vec<(Vec<char>, usize)>,
    redo: Vec<(Vec<char>, usize)>,
}

mod k {
    pub const BACKSPACE: u32 = 14;
    pub const A: u32 = 30;
    pub const C: u32 = 46;
    pub const V: u32 = 47;
    pub const X: u32 = 45;
    pub const Y: u32 = 21;
    pub const Z: u32 = 44;
    pub const HOME: u32 = 102;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const INSERT: u32 = 110;
    pub const DELETE: u32 = 111;
}

impl TextField {
    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        *self = TextField::default();
    }

    /// New text, all of it selected (an address bar, a name to rename).
    pub fn set(&mut self, s: &str) {
        *self = TextField::default();
        self.text = s.chars().collect();
        self.caret = self.text.len();
        self.anchor = Some(0);
    }

    /// Selects chars `a..b` (the caret at `b`).
    pub fn select(&mut self, a: usize, b: usize) {
        let n = self.text.len();
        self.anchor = Some(a.min(n));
        self.caret = b.min(n);
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    fn sel(&self) -> Option<(usize, usize)> {
        self.anchor.filter(|a| *a != self.caret).map(|a| (a.min(self.caret), a.max(self.caret)))
    }

    pub fn selected(&self) -> Option<String> {
        self.sel().map(|(a, b)| self.text[a..b].iter().collect())
    }

    fn snapshot(&mut self) {
        self.undo.push((self.text.clone(), self.caret));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn delete_sel(&mut self) -> bool {
        let Some((a, b)) = self.sel() else { return false };
        self.text.drain(a..b);
        self.caret = a;
        self.anchor = None;
        true
    }

    /// Inserts text at the caret (replacing the selection). Newlines become spaces.
    pub fn insert(&mut self, s: &str) {
        let chars: Vec<char> = s.chars().map(|c| if c == '\n' || c == '\t' || c == '\r' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        if chars.is_empty() && self.sel().is_none() {
            return;
        }
        self.snapshot();
        self.delete_sel();
        let n = chars.len();
        self.text.splice(self.caret..self.caret, chars);
        self.caret += n;
        self.anchor = None;
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// Word boundary left of `i` (Ctrl+Left): skip spaces/punctuation, then the word.
    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !Self::is_word(self.text[i - 1]) {
            i -= 1;
        }
        while i > 0 && Self::is_word(self.text[i - 1]) {
            i -= 1;
        }
        i
    }

    /// Word boundary right of `i` (Ctrl+Right, like GTK: to the end of the next word).
    fn word_right(&self, mut i: usize) -> usize {
        let n = self.text.len();
        while i < n && !Self::is_word(self.text[i]) {
            i += 1;
        }
        while i < n && Self::is_word(self.text[i]) {
            i += 1;
        }
        i
    }

    fn move_to(&mut self, to: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = to.min(self.text.len());
    }

    pub fn key(&mut self, code: u32, m: Mods) -> Act {
        let n = self.text.len();
        match code {
            k::LEFT | k::RIGHT | k::HOME | k::END => {
                // Without Shift, Left/Right on a selection collapse it to that side.
                if !m.shift && !m.ctrl && (code == k::LEFT || code == k::RIGHT) {
                    if let Some((a, b)) = self.sel() {
                        self.caret = if code == k::LEFT { a } else { b };
                        self.anchor = None;
                        return Act::Moved;
                    }
                }
                let to = match code {
                    k::LEFT if m.ctrl => self.word_left(self.caret),
                    k::LEFT => self.caret.saturating_sub(1),
                    k::RIGHT if m.ctrl => self.word_right(self.caret),
                    k::RIGHT => (self.caret + 1).min(n),
                    k::HOME => 0,
                    _ => n,
                };
                self.move_to(to, m.shift);
                Act::Moved
            }
            k::BACKSPACE | k::DELETE if m.shift && code == k::DELETE => {
                // Shift+Delete: cut.
                match self.selected() {
                    Some(s) => {
                        self.snapshot();
                        self.delete_sel();
                        Act::Cut(s)
                    }
                    None => Act::Moved,
                }
            }
            k::BACKSPACE | k::DELETE => {
                if self.sel().is_some() {
                    self.snapshot();
                    self.delete_sel();
                    return Act::Changed;
                }
                let (a, b) = if code == k::BACKSPACE {
                    (if m.ctrl { self.word_left(self.caret) } else { self.caret.saturating_sub(1) }, self.caret)
                } else {
                    (self.caret, if m.ctrl { self.word_right(self.caret) } else { (self.caret + 1).min(n) })
                };
                if a == b {
                    return Act::Moved;
                }
                self.snapshot();
                self.text.drain(a..b);
                self.caret = a;
                Act::Changed
            }
            k::INSERT if m.ctrl => self.selected().map_or(Act::Moved, Act::Copy),
            k::INSERT if m.shift => Act::Paste,
            k::A if m.ctrl => {
                self.anchor = Some(0);
                self.caret = n;
                Act::Moved
            }
            k::C if m.ctrl => self.selected().map_or(Act::Moved, Act::Copy),
            k::X if m.ctrl => match self.selected() {
                Some(s) => {
                    self.snapshot();
                    self.delete_sel();
                    Act::Cut(s)
                }
                None => Act::Moved,
            },
            k::V if m.ctrl => Act::Paste,
            k::Z if m.ctrl && !m.shift => match self.undo.pop() {
                Some((t, c)) => {
                    self.redo.push((std::mem::replace(&mut self.text, t), self.caret));
                    self.caret = c.min(self.text.len());
                    self.anchor = None;
                    Act::Changed
                }
                None => Act::Moved,
            },
            k::Y | k::Z if m.ctrl => match self.redo.pop() {
                Some((t, c)) => {
                    self.undo.push((std::mem::replace(&mut self.text, t), self.caret));
                    self.caret = c.min(self.text.len());
                    self.anchor = None;
                    Act::Changed
                }
                None => Act::Moved,
            },
            _ if m.ctrl || m.alt => Act::Pass,
            _ => match keycode_char(code, m.shift, m.caps, m.group) {
                Some(c) => {
                    self.insert(&c.to_string());
                    Act::Changed
                }
                None => Act::Pass,
            },
        }
    }

    /// Draws the text with its selection and caret; scrolls so the caret stays visible.
    pub fn draw(&self, cv: &mut Canvas, clip: Rect, xf: Xf, x: i32, baseline: i32, w: i32, size: f32, color: u32, weight: font::W) {
        let width = |n: usize| font::text_width_w(&self.text[..n].iter().collect::<String>(), size, weight);
        let caret_x = width(self.caret);
        let scroll = (caret_x - w + 4).max(0);
        let area = xf.r(Rect::new(x, baseline - size as i32 - 4, w, size as i32 + 10)).intersect(&clip);
        if let Some((a, b)) = self.sel() {
            let (xa, xb) = (width(a) - scroll, width(b) - scroll);
            let r = Rect::from_edges(x + xa.max(0), baseline - size as i32 - 1, x + xb.min(w), baseline + 5);
            cv.rrect(area, xf.r(r), xf.len(3), argb(110, 167, 139, 250));
        }
        // Text clipped to the field: draw from the first visible char.
        let mut first = 0;
        while first < self.text.len() && width(first + 1) <= scroll {
            first += 1;
        }
        let off = width(first) - scroll;
        let s: String = self.text[first..].iter().collect();
        font::draw_text_w(cv, area, xf, x + off, baseline, &s, size, color, w - off, weight);
        let cx = x + caret_x - scroll;
        cv.fill(area, xf.r(Rect::new(cx, baseline - size as i32, 2, size as i32 + 4)), 0xffa7_8bfa);
    }
}

/// Character of an evdev key in layout `group` (0: us, 1: ru, as in the keymap
/// "pc+us+ru:2"), with Shift and Caps Lock (Caps affects letters only).
pub fn keycode_char(code: u32, shift: bool, caps: bool, group: u32) -> Option<char> {
    const US: [&str; 2] = ["`1234567890-=qwertyuiop[]asdfghjkl;'\\zxcvbnm,./", "~!@#$%^&*()_+QWERTYUIOP{}ASDFGHJKL:\"|ZXCVBNM<>?"];
    const RU: [&str; 2] = ["ё1234567890-=йцукенгшщзхъфывапролджэ\\ячсмитьбю.", "Ё!\"№;%:?*()_+ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭ/ЯЧСМИТЬБЮ,"];
    // Index of each key in the strings above.
    let idx = match code {
        41 => 0,
        2..=13 => (code - 2 + 1) as usize,
        16..=27 => (code - 16 + 13) as usize,
        30..=40 => (code - 30 + 25) as usize,
        43 => 36,
        44..=53 => (code - 44 + 37) as usize,
        57 => return Some(' '),
        _ => return None,
    };
    let table = if group == 1 { RU } else { US };
    let base = table[0].chars().nth(idx)?;
    let letter = base.is_alphabetic();
    let upper = if letter { shift != caps } else { shift };
    table[upper as usize].chars().nth(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(f: &mut TextField, s: &str) {
        f.insert(s);
    }

    #[test]
    fn layouts_shift_and_caps() {
        assert_eq!(keycode_char(16, false, false, 0), Some('q'));
        assert_eq!(keycode_char(16, false, false, 1), Some('й'));
        assert_eq!(keycode_char(16, true, false, 1), Some('Й'));
        assert_eq!(keycode_char(16, true, true, 1), Some('й'));
        assert_eq!(keycode_char(41, false, false, 1), Some('ё'));
        assert_eq!(keycode_char(53, false, false, 1), Some('.'));
        assert_eq!(keycode_char(53, true, false, 1), Some(','));
        assert_eq!(keycode_char(4, true, false, 1), Some('№'));
        assert_eq!(keycode_char(4, true, true, 0), Some('#')); // Caps does not touch digits
        assert_eq!(keycode_char(40, true, false, 0), Some('"'));
        assert_eq!(keycode_char(43, false, false, 0), Some('\\'));
    }

    #[test]
    fn word_moves_selection_and_delete() {
        let mut f = TextField::default();
        typed(&mut f, "foo bar.baz qux");
        let ctrl = Mods { ctrl: true, ..Default::default() };
        let cs = Mods { ctrl: true, shift: true, ..Default::default() };
        assert_eq!(f.key(k::LEFT, ctrl), Act::Moved);
        assert_eq!(f.caret, 12);
        f.key(k::LEFT, cs);
        assert_eq!(f.selected().as_deref(), Some("baz "));
        assert_eq!(f.key(k::X, ctrl), Act::Cut("baz ".into()));
        assert_eq!(f.text(), "foo bar.qux");
        f.key(k::END, Mods::default());
        f.key(k::BACKSPACE, ctrl);
        assert_eq!(f.text(), "foo bar.");
        f.key(k::Z, ctrl);
        assert_eq!(f.text(), "foo bar.qux");
        f.key(k::Z, cs);
        assert_eq!(f.text(), "foo bar.");
        f.key(k::A, ctrl);
        assert_eq!(f.selected().as_deref(), Some("foo bar."));
        f.key(k::LEFT, Mods::default());
        assert_eq!((f.caret, f.anchor), (0, None));
        f.key(k::HOME, Mods::default());
        f.key(k::END, Mods { shift: true, ..Default::default() });
        assert_eq!(f.key(k::DELETE, Mods { shift: true, ..Default::default() }), Act::Cut("foo bar.".into()));
        assert!(f.is_empty());
        f.insert("a\nb");
        assert_eq!(f.text(), "a b");
    }
}
