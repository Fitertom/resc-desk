//! Integer rectangles and small rect-list "regions".

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_edges(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    pub fn x1(&self) -> i32 {
        self.x + self.w
    }
    pub fn y1(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x as f64 && py >= self.y as f64 && px < self.x1() as f64 && py < self.y1() as f64
    }
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.x1() <= self.x1() && o.y1() <= self.y1()
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.x1().min(o.x1());
        let y1 = self.y1().min(o.y1());
        if x1 <= x0 || y1 <= y0 { Rect::default() } else { Rect::from_edges(x0, y0, x1, y1) }
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        !self.intersect(o).is_empty()
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_edges(self.x.min(o.x), self.y.min(o.y), self.x1().max(o.x1()), self.y1().max(o.y1()))
    }
    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn inflate(&self, d: i32) -> Rect {
        Rect { x: self.x - d, y: self.y - d, w: self.w + 2 * d, h: self.h + 2 * d }
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.w as i64 * self.h as i64 }
    }
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
    /// Logical → physical pixels for an output at (ox, oy) with `scale`; rounds outward.
    pub fn to_physical(&self, ox: i32, oy: i32, scale: f64) -> Rect {
        let x0 = ((self.x - ox) as f64 * scale).floor() as i32;
        let y0 = ((self.y - oy) as f64 * scale).floor() as i32;
        let x1 = ((self.x1() - ox) as f64 * scale).ceil() as i32;
        let y1 = ((self.y1() - oy) as f64 * scale).ceil() as i32;
        Rect::from_edges(x0, y0, x1, y1)
    }
}

/// A damage accumulator: a handful of rects; collapses to the bounding box
/// once it gets fragmented (cheaper than exact region math for a CPU blitter).
#[derive(Clone, Debug, Default)]
pub struct Damage {
    pub rects: Vec<Rect>,
}

impl Damage {
    const MAX: usize = 12;
    pub fn add(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        // Absorb into an existing rect if contained / heavily overlapping.
        for e in self.rects.iter_mut() {
            if e.contains_rect(&r) {
                return;
            }
            if r.contains_rect(e) {
                *e = r;
                self.normalize();
                return;
            }
            let u = e.union(&r);
            if u.area() <= e.area() + r.area() + r.area() / 4 {
                *e = u;
                self.normalize();
                return;
            }
        }
        self.rects.push(r);
        if self.rects.len() > Self::MAX {
            let b = self.bounds();
            self.rects.clear();
            self.rects.push(b);
        }
    }
    fn normalize(&mut self) {
        // One pass of merging contained rects.
        let mut i = 0;
        while i < self.rects.len() {
            let mut merged = false;
            for j in 0..self.rects.len() {
                if i != j && self.rects[j].contains_rect(&self.rects[i]) {
                    self.rects.swap_remove(i);
                    merged = true;
                    break;
                }
            }
            if !merged {
                i += 1;
            }
        }
    }
    pub fn bounds(&self) -> Rect {
        self.rects.iter().fold(Rect::default(), |a, r| a.union(r))
    }
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
    pub fn clear(&mut self) {
        self.rects.clear();
    }
    pub fn extend(&mut self, o: &Damage) {
        for r in &o.rects {
            self.add(*r);
        }
    }
}

/// wl_region: ordered add/subtract ops, evaluated lazily.
#[derive(Clone, Debug, Default)]
pub struct Region {
    pub ops: Vec<(bool, Rect)>,
}

impl Region {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let mut inside = false;
        for (add, r) in &self.ops {
            if r.contains(x, y) {
                inside = *add;
            }
        }
        inside
    }
    /// True if `r` is fully covered (conservative: only exact single add-rects without later subtracts).
    pub fn covers(&self, r: &Rect) -> bool {
        let mut covered = false;
        for (add, o) in &self.ops {
            if *add && o.contains_rect(r) {
                covered = true;
            } else if !*add && o.overlaps(r) {
                covered = false;
            }
        }
        covered
    }
}
