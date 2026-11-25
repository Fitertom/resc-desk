//! Liquify's motion (`Liquify_last/src/anim.rs`, the spring as it is): every animated value
//! is a damped spring pulled toward its target. There are no durations: a new target mid-way
//! keeps the current velocity, so any animation can be redirected, reversed or grabbed at any
//! moment and it stays continuous.

/// A damped spring. `response`: the period of the undamped oscillation, seconds (how quick it feels);
/// `damping`: 1 arrives without overshoot, below 1 overshoots and wobbles (0.7: one visible bounce).
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub response: f32,
    pub damping: f32,
    /// Settled when nearer than this to the target (and slower than 10 x this per second).
    pub rest: f32,
}

/// Integration step: stable for the stiffest springs we use, and cheap (a few dozen springs).
const STEP: f32 = 1.0 / 480.0;

impl Spring {
    pub const fn new(value: f32, response: f32, damping: f32) -> Self {
        Spring { value, velocity: 0.0, target: value, response, damping, rest: 0.001 }
    }

    pub fn to(&mut self, target: f32) {
        self.target = target;
    }

    /// Jumps there, at rest.
    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.velocity = 0.0;
    }

    pub fn moving(&self) -> bool {
        self.value != self.target || self.velocity != 0.0
    }

    /// Advances by `dt` seconds; true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if !self.moving() {
            return false;
        }
        if dt >= 1.0 {
            self.snap(self.target); // animations off: straight there
            return false;
        }
        let w = std::f32::consts::TAU / self.response.max(0.01);
        let (k, c) = (w * w, 2.0 * self.damping * w);
        let mut left = dt.min(0.1);
        while left > 0.0 {
            let h = left.min(STEP);
            // Semi-implicit Euler: velocity first, then position with the new velocity.
            self.velocity += (k * (self.target - self.value) - c * self.velocity) * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if (self.target - self.value).abs() < self.rest && self.velocity.abs() < self.rest * 10.0 {
            self.snap(self.target);
            return false;
        }
        true
    }
}

impl Default for Spring {
    fn default() -> Self {
        Spring::new(0.0, 0.4, 1.0)
    }
}

/// Liquify's `Drop`: a selection that flows between items like a drop of liquid. Its two edges
/// are springs in item units (item `i` spans `i .. i + 1`). The edge in front moves first and
/// quicker, the one behind follows: the drop stretches toward the new item, then pulls
/// together there with a little wobble.
#[derive(Clone, Copy, Debug)]
pub struct Drop {
    pub left: Spring,
    pub right: Spring,
}

const LEAD: (f32, f32) = (0.30, 0.68);
const TRAIL: (f32, f32) = (0.46, 0.80);

impl Drop {
    pub const fn at(i: f32) -> Self {
        Drop { left: Spring::new(i, LEAD.0, LEAD.1), right: Spring::new(i + 1.0, LEAD.0, LEAD.1) }
    }

    pub fn go(&mut self, i: f32) {
        let (front, back) = if i > self.left.target { (&mut self.right, &mut self.left) } else { (&mut self.left, &mut self.right) };
        (front.response, front.damping) = LEAD;
        (back.response, back.damping) = TRAIL;
        self.left.to(i);
        self.right.to(i + 1.0);
    }

    pub fn step(&mut self, dt: f32) -> bool {
        self.left.step(dt) | self.right.step(dt)
    }

    pub fn moving(&self) -> bool {
        self.left.moving() || self.right.moving()
    }

    /// How much of item `i` the drop covers, 0..1 (its colors follow the drop).
    pub fn cover(&self, i: usize) -> f32 {
        let (a, b) = (i as f32, i as f32 + 1.0);
        (self.right.value.min(b) - self.left.value.max(a)).clamp(0.0, 1.0)
    }
}
