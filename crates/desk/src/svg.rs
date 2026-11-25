//! A small SVG renderer, enough for application icons (hicolor, Papirus, Breeze,
//! Adwaita): paths with every command, basic shapes, <g>/<use>/<symbol>, transforms,
//! solid and gradient fills (linear, radial), strokes with round joins, opacity,
//! nonzero/evenodd, `currentColor` and simple class rules from <style>.
//! Not supported (drawn without): clip paths, masks, filters, patterns, text.
use std::collections::HashMap;

// ------------------------------------------------------------------ XML

struct Node {
    tag: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
    fn num(&self, k: &str, def: f32) -> f32 {
        self.attr(k).and_then(|v| Nums::new(v).next()).unwrap_or(def)
    }
    fn href(&self) -> Option<&str> {
        self.attr("xlink:href").or_else(|| self.attr("href")).and_then(|h| h.strip_prefix('#'))
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn parse_xml(s: &str) -> Option<Node> {
    let b = s.as_bytes();
    let new = |tag: &str| Node { tag: tag.to_string(), attrs: Vec::new(), kids: Vec::new(), text: String::new() };
    let mut stack = vec![new("#root")];
    let mut i = 0;
    let find = |from: usize, pat: &str| s[from..].find(pat).map(|p| from + p);
    while i < b.len() {
        if b[i] != b'<' {
            let end = find(i, "<").unwrap_or(b.len());
            stack.last_mut()?.text.push_str(&decode_entities(&s[i..end]));
            i = end;
            continue;
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            i = find(i, "-->")? + 3;
        } else if rest.starts_with("<![CDATA[") {
            let end = find(i, "]]>")?;
            stack.last_mut()?.text.push_str(&s[i + 9..end]);
            i = end + 3;
        } else if rest.starts_with("<!DOCTYPE") {
            // May carry an internal subset in [...].
            let close = find(i, ">")?;
            i = match (find(i, "["), find(i, "]>")) {
                (Some(o), Some(e)) if o < close => e + 2,
                _ => close + 1,
            };
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            i = find(i, ">")? + 1;
        } else if rest.starts_with("</") {
            i = find(i, ">")? + 1;
            if stack.len() > 1 {
                let n = stack.pop()?;
                stack.last_mut()?.kids.push(n);
            }
        } else {
            i += 1;
            let st = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
                i += 1;
            }
            let name = &s[st..i];
            let mut n = new(name.rsplit(':').next().unwrap_or(name));
            let mut closed = false;
            loop {
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= b.len() {
                    break;
                }
                if b[i] == b'/' {
                    closed = true;
                    i = find(i, ">")? + 1;
                    break;
                }
                if b[i] == b'>' {
                    i += 1;
                    break;
                }
                let st = i;
                while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
                    i += 1;
                }
                let key = s[st..i].to_string();
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= b.len() || b[i] != b'=' {
                    continue; // attribute without a value
                }
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let q = *b.get(i)?;
                if q != b'"' && q != b'\'' {
                    return None;
                }
                let end = s[i + 1..].find(q as char)? + i + 1;
                n.attrs.push((key, decode_entities(&s[i + 1..end])));
                i = end + 1;
            }
            if closed {
                stack.last_mut()?.kids.push(n);
            } else {
                stack.push(n);
            }
        }
    }
    while stack.len() > 1 {
        let n = stack.pop()?;
        stack.last_mut()?.kids.push(n);
    }
    stack.pop()?.kids.into_iter().find(|n| n.tag == "svg")
}

// ------------------------------------------------------------------ numbers, colors, transforms

/// Numbers in SVG syntax: "1.5.5-2e1,3" → 1.5 0.5 -20 3.
struct Nums<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Nums<'a> {
    fn new(s: &'a str) -> Self {
        Nums { b: s.as_bytes(), i: 0 }
    }
    fn skip(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.b.get(self.i)?;
        if c == b'0' || c == b'1' {
            self.i += 1;
            Some(c == b'1')
        } else {
            None
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.b.get(self.i).copied()
    }
}

impl Iterator for Nums<'_> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.skip();
        let b = self.b;
        let st = self.i;
        let mut i = st;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut digits = false;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&b[st..i]).ok()?.parse().ok()
    }
}

/// A length; percentages relative to `rel`.
fn length(v: &str, rel: f32) -> Option<f32> {
    let v = v.trim();
    let n = Nums::new(v).next()?;
    Some(if v.ends_with('%') { n / 100.0 * rel } else { n })
}

/// Colour as non-premultiplied 0xAARRGGBB.
pub fn color(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(h) = v.strip_prefix('#') {
        let x = |s: &str| u32::from_str_radix(s, 16).ok();
        return match h.len() {
            3 | 4 => {
                let d: Vec<u32> = h.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
                let a = if h.len() == 4 { d[3] * 17 } else { 255 };
                Some(a << 24 | d[0] * 17 << 16 | d[1] * 17 << 8 | d[2] * 17)
            }
            6 => Some(0xff00_0000 | x(h)?),
            8 => {
                let v = x(h)?;
                Some((v & 0xff) << 24 | v >> 8)
            }
            _ => None,
        };
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.trim_end_matches(')');
        let parts: Vec<&str> = args.split(|c| c == ',' || c == '/' || c == ' ').filter(|s| !s.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        let ch = |s: &str| length(s, 255.0).map(|n| n.round().clamp(0.0, 255.0) as u32);
        let a = match parts.get(3) {
            Some(s) => (length(s, 1.0)? * 255.0).round().clamp(0.0, 255.0) as u32,
            None => 255,
        };
        return Some(a << 24 | ch(parts[0])? << 16 | ch(parts[1])? << 8 | ch(parts[2])?);
    }
    let rgb = match v.to_ascii_lowercase().as_str() {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "lime" => 0x00ff00,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "orange" => 0xffa500,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "transparent" => return Some(0),
        _ => return None,
    };
    Some(0xff00_0000 | rgb)
}

/// Affine matrix [a b c d e f]: x' = a x + c y + e, y' = b x + d y + f.
#[derive(Clone, Copy)]
struct M([f32; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn mul(self, n: M) -> M {
        let (m, n) = (self.0, n.0);
        M([
            m[0] * n[0] + m[2] * n[1],
            m[1] * n[0] + m[3] * n[1],
            m[0] * n[2] + m[2] * n[3],
            m[1] * n[2] + m[3] * n[3],
            m[0] * n[4] + m[2] * n[5] + m[4],
            m[1] * n[4] + m[3] * n[5] + m[5],
        ])
    }
    fn ap(self, p: (f32, f32)) -> (f32, f32) {
        let m = self.0;
        (m[0] * p.0 + m[2] * p.1 + m[4], m[1] * p.0 + m[3] * p.1 + m[5])
    }
    fn inv(self) -> Option<M> {
        let m = self.0;
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 1e-12 {
            return None;
        }
        let i = 1.0 / det;
        Some(M([
            m[3] * i,
            -m[1] * i,
            -m[2] * i,
            m[0] * i,
            (m[2] * m[5] - m[3] * m[4]) * i,
            (m[1] * m[4] - m[0] * m[5]) * i,
        ]))
    }
    fn scale(self) -> f32 {
        let m = self.0;
        (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
    }
    fn translate(x: f32, y: f32) -> M {
        M([1.0, 0.0, 0.0, 1.0, x, y])
    }
}

fn transform(v: &str) -> M {
    let mut m = M::ID;
    let mut rest = v;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let a: Vec<f32> = Nums::new(&rest[open + 1..open + close]).collect();
        let g = |i: usize, d: f32| a.get(i).copied().unwrap_or(d);
        let t = match name {
            "matrix" if a.len() >= 6 => M([a[0], a[1], a[2], a[3], a[4], a[5]]),
            "translate" => M::translate(g(0, 0.0), g(1, 0.0)),
            "scale" => M([g(0, 1.0), 0.0, 0.0, g(1, g(0, 1.0)), 0.0, 0.0]),
            "rotate" => {
                let r = g(0, 0.0).to_radians();
                let (s, c) = r.sin_cos();
                let (cx, cy) = (g(1, 0.0), g(2, 0.0));
                M::translate(cx, cy).mul(M([c, s, -s, c, 0.0, 0.0])).mul(M::translate(-cx, -cy))
            }
            "skewX" => M([1.0, 0.0, g(0, 0.0).to_radians().tan(), 1.0, 0.0, 0.0]),
            "skewY" => M([1.0, g(0, 0.0).to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => M::ID,
        };
        m = m.mul(t);
        rest = &rest[open + close + 1..];
    }
    m
}

// ------------------------------------------------------------------ style

#[derive(Clone)]
enum Paint {
    None,
    Color(u32),
    Current,
    Url(String, Box<Paint>),
}

fn paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    if v == "none" {
        return Some(Paint::None);
    }
    if v == "currentColor" {
        return Some(Paint::Current);
    }
    if let Some(r) = v.strip_prefix("url(") {
        let close = r.find(')')?;
        let id = r[..close].trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#').to_string();
        let fb = paint(&r[close + 1..]).unwrap_or(Paint::None);
        return Some(Paint::Url(id, Box::new(fb)));
    }
    color(v).map(Paint::Color)
}

#[derive(Clone)]
struct Style {
    fill: Paint,
    fill_op: f32,
    evenodd: bool,
    stroke: Paint,
    stroke_w: f32,
    stroke_op: f32,
    color: u32,
    opacity: f32,
    display: bool,
    visible: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fill: Paint::Color(0xff00_0000),
            fill_op: 1.0,
            evenodd: false,
            stroke: Paint::None,
            stroke_w: 1.0,
            stroke_op: 1.0,
            color: 0xff00_0000,
            opacity: 1.0,
            display: true,
            visible: true,
        }
    }
}

fn opacity(v: &str) -> Option<f32> {
    length(v, 1.0).map(|o| o.clamp(0.0, 1.0))
}

fn apply_prop(st: &mut Style, k: &str, v: &str) {
    let v = v.trim().trim_end_matches("!important").trim();
    if v == "inherit" {
        return;
    }
    match k.trim() {
        "fill" => {
            if let Some(p) = paint(v) {
                st.fill = p;
            }
        }
        "fill-opacity" => st.fill_op = opacity(v).unwrap_or(st.fill_op),
        "fill-rule" => st.evenodd = v == "evenodd",
        "stroke" => {
            if let Some(p) = paint(v) {
                st.stroke = p;
            }
        }
        "stroke-width" => st.stroke_w = length(v, 1.0).unwrap_or(st.stroke_w),
        "stroke-opacity" => st.stroke_op = opacity(v).unwrap_or(st.stroke_op),
        "color" => {
            if let Some(c) = color(v) {
                st.color = c;
            }
        }
        "opacity" => st.opacity *= opacity(v).unwrap_or(1.0),
        "display" => st.display = v != "none",
        "visibility" => st.visible = v == "visible",
        _ => {}
    }
}

/// Simple selectors only: `.cls`, `#id`, `tag`, `tag.cls`.
struct Rule {
    tag: Option<String>,
    class: Option<String>,
    id: Option<String>,
    decls: Vec<(String, String)>,
}

fn parse_css(css: &str, out: &mut Vec<Rule>) {
    let mut s = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        s.push_str(&rest[..i]);
        rest = rest[i..].find("*/").map_or("", |e| &rest[i + e + 2..]);
    }
    s.push_str(rest);
    for block in s.split('}') {
        let Some((sel, body)) = block.split_once('{') else { continue };
        let decls: Vec<(String, String)> = body
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        for sel in sel.split(',') {
            let sel = sel.trim();
            if sel.is_empty() || sel.contains(' ') || sel.contains('>') || sel.contains(':') {
                continue;
            }
            let (head, id) = match sel.split_once('#') {
                Some((h, id)) => (h, Some(id.to_string())),
                None => (sel, None),
            };
            let (tag, class) = match head.split_once('.') {
                Some((t, c)) => (t, Some(c.to_string())),
                None => (head, None),
            };
            let tag = (!tag.is_empty() && tag != "*").then(|| tag.to_string());
            out.push(Rule { tag, class, id, decls: decls.clone() });
        }
    }
}

// ------------------------------------------------------------------ geometry

/// Flattened sub-paths in user space; `closed` matters only for strokes.
struct Sub {
    pts: Vec<(f32, f32)>,
    closed: bool,
}

struct PathB {
    subs: Vec<Sub>,
    cur: Vec<(f32, f32)>,
    /// Segments per unit of user-space length (from the device scale).
    k: f32,
}

impl PathB {
    fn new(k: f32) -> Self {
        PathB { subs: Vec::new(), cur: Vec::new(), k }
    }
    fn last(&self) -> (f32, f32) {
        self.cur.last().copied().unwrap_or((0.0, 0.0))
    }
    fn end(&mut self, closed: bool) {
        if self.cur.len() > 1 {
            let pts = std::mem::take(&mut self.cur);
            self.subs.push(Sub { pts, closed });
        } else {
            self.cur.clear();
        }
    }
    fn move_to(&mut self, p: (f32, f32)) {
        self.end(false);
        self.cur.push(p);
    }
    fn line_to(&mut self, p: (f32, f32)) {
        if self.cur.is_empty() {
            self.cur.push((0.0, 0.0));
        }
        self.cur.push(p);
    }
    fn steps(&self, len: f32) -> usize {
        ((len * self.k).sqrt() * 1.6).ceil().clamp(1.0, 96.0) as usize
    }
    fn cubic(&mut self, p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) {
        let p0 = self.last();
        let d = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        let n = self.steps(d(p0, p1) + d(p1, p2) + d(p2, p3));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let (a, b, c, e) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            self.line_to((a * p0.0 + b * p1.0 + c * p2.0 + e * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + e * p3.1));
        }
    }
    fn quad(&mut self, p1: (f32, f32), p2: (f32, f32)) {
        let p0 = self.last();
        let d = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        let n = self.steps(d(p0, p1) + d(p1, p2));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let (a, b, c) = (mt * mt, 2.0 * mt * t, t * t);
            self.line_to((a * p0.0 + b * p1.0 + c * p2.0, a * p0.1 + b * p1.1 + c * p2.1));
        }
    }
    /// Elliptical arc to `p` (SVG endpoint parameterisation, spec F.6.5).
    fn arc(&mut self, rx: f32, ry: f32, rot: f32, large: bool, sweep: bool, p: (f32, f32)) {
        let p0 = self.last();
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx < 1e-6 || ry < 1e-6 || (p0.0 - p.0).abs() + (p0.1 - p.1).abs() < 1e-6 {
            self.line_to(p);
            return;
        }
        let (s, c) = rot.to_radians().sin_cos();
        let (dx, dy) = ((p0.0 - p.0) / 2.0, (p0.1 - p.1) / 2.0);
        let x1 = c * dx + s * dy;
        let y1 = -s * dx + c * dy;
        let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lam > 1.0 {
            rx *= lam.sqrt();
            ry *= lam.sqrt();
        }
        let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
        let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut co = (num / den).max(0.0).sqrt();
        if large == sweep {
            co = -co;
        }
        let cx1 = co * rx * y1 / ry;
        let cy1 = -co * ry * x1 / rx;
        let cx = c * cx1 - s * cy1 + (p0.0 + p.0) / 2.0;
        let cy = s * cx1 + c * cy1 + (p0.1 + p.1) / 2.0;
        let ang = |ux: f32, uy: f32| uy.atan2(ux);
        let t1 = ang((x1 - cx1) / rx, (y1 - cy1) / ry);
        let mut dt = ang((-x1 - cx1) / rx, (-y1 - cy1) / ry) - t1;
        let tau = std::f32::consts::TAU;
        if sweep && dt < 0.0 {
            dt += tau;
        } else if !sweep && dt > 0.0 {
            dt -= tau;
        }
        let n = self.steps(dt.abs() * rx.max(ry)).max(2);
        for i in 1..=n {
            let t = t1 + dt * i as f32 / n as f32;
            let (st, ct) = t.sin_cos();
            let (ex, ey) = (rx * ct, ry * st);
            self.line_to(if i == n { p } else { (c * ex - s * ey + cx, s * ex + c * ey + cy) });
        }
    }
    fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) {
        let n = self.steps(std::f32::consts::TAU * rx.max(ry)).max(8);
        self.end(false);
        for i in 0..n {
            let t = std::f32::consts::TAU * i as f32 / n as f32;
            self.cur.push((cx + rx * t.cos(), cy + ry * t.sin()));
        }
        self.end(true);
    }
}

fn parse_path(d: &str, pb: &mut PathB) {
    let mut ns = Nums::new(d);
    let mut cmd = b'M';
    let mut start = (0.0, 0.0);
    // Last control point for S/T reflection, and which kind it was.
    let mut last_c: Option<((f32, f32), bool)> = None;
    loop {
        let Some(c) = ns.peek() else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            ns.i += 1;
            if cmd == b'z' || cmd == b'Z' {
                if !pb.cur.is_empty() {
                    pb.end(true);
                    // The pen goes back to the sub-path start; what follows continues from it.
                    pb.cur.push(start);
                }
                last_c = None;
                continue;
            }
        } else if cmd == b'z' || cmd == b'Z' {
            break; // numbers after Z: malformed
        }
        let rel = cmd.is_ascii_lowercase();
        let o = if rel { pb.last() } else { (0.0, 0.0) };
        let pt = |ns: &mut Nums| -> Option<(f32, f32)> {
            let x = ns.next()?;
            let y = ns.next()?;
            Some((x + o.0, y + o.1))
        };
        let ok: Option<()> = (|| {
            match cmd.to_ascii_uppercase() {
                b'M' => {
                    let p = pt(&mut ns)?;
                    if pb.cur.len() == 1 {
                        pb.cur.clear();
                    }
                    pb.move_to(p);
                    start = p;
                    cmd = if rel { b'l' } else { b'L' };
                    last_c = None;
                }
                b'L' => {
                    let p = pt(&mut ns)?;
                    pb.line_to(p);
                    last_c = None;
                }
                b'H' => {
                    let x = ns.next()? + o.0;
                    let y = pb.last().1;
                    pb.line_to((x, y));
                    last_c = None;
                }
                b'V' => {
                    let y = ns.next()? + o.1;
                    let x = pb.last().0;
                    pb.line_to((x, y));
                    last_c = None;
                }
                b'C' => {
                    let (p1, p2, p3) = (pt(&mut ns)?, pt(&mut ns)?, pt(&mut ns)?);
                    pb.cubic(p1, p2, p3);
                    last_c = Some((p2, true));
                }
                b'S' => {
                    let cur = pb.last();
                    let p1 = match last_c {
                        Some((c, true)) => (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1),
                        _ => cur,
                    };
                    let (p2, p3) = (pt(&mut ns)?, pt(&mut ns)?);
                    pb.cubic(p1, p2, p3);
                    last_c = Some((p2, true));
                }
                b'Q' => {
                    let (p1, p2) = (pt(&mut ns)?, pt(&mut ns)?);
                    pb.quad(p1, p2);
                    last_c = Some((p1, false));
                }
                b'T' => {
                    let cur = pb.last();
                    let p1 = match last_c {
                        Some((c, false)) => (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1),
                        _ => cur,
                    };
                    let p2 = pt(&mut ns)?;
                    pb.quad(p1, p2);
                    last_c = Some((p1, false));
                }
                b'A' => {
                    let (rx, ry, rot) = (ns.next()?, ns.next()?, ns.next()?);
                    let (large, sweep) = (ns.flag()?, ns.flag()?);
                    let p = pt(&mut ns)?;
                    pb.arc(rx, ry, rot, large, sweep, p);
                    last_c = None;
                }
                _ => return None,
            }
            Some(())
        })();
        if ok.is_none() {
            break;
        }
    }
    pb.end(false);
}

// ------------------------------------------------------------------ raster

const SUB: usize = 5;

/// Coverage (0..1) of the polygons, nonzero or evenodd, in a w×h mask.
fn fill_mask(polys: &[Vec<(f32, f32)>], w: usize, h: usize, evenodd: bool) -> Vec<f32> {
    let mut mask = vec![0.0f32; w * h];
    // Edges (y0 < y1) with direction.
    let mut edges: Vec<(f32, f32, f32, f32, i32)> = Vec::new();
    for p in polys {
        let n = p.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (p[i], p[(i + 1) % n]);
            if a.1 == b.1 || !(a.0.is_finite() && a.1.is_finite() && b.0.is_finite() && b.1.is_finite()) {
                continue;
            }
            if a.1 < b.1 {
                edges.push((a.0, a.1, b.0, b.1, 1));
            } else {
                edges.push((b.0, b.1, a.0, a.1, -1));
            }
        }
    }
    if edges.is_empty() {
        return mask;
    }
    edges.sort_by(|a, b| a.1.total_cmp(&b.1));
    let ymin = edges[0].1.max(0.0) as usize;
    let ymax = edges.iter().map(|e| e.3).fold(0.0f32, f32::max).ceil().min(h as f32) as usize;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let v = 1.0 / SUB as f32;
    for y in ymin..ymax {
        let row = &mut mask[y * w..(y + 1) * w];
        for s in 0..SUB {
            let sy = y as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for e in &edges {
                if e.1 > sy {
                    break;
                }
                if sy < e.3 {
                    xs.push((e.0 + (sy - e.1) * (e.2 - e.0) / (e.3 - e.1), e.4));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for i in 0..xs.len() {
                wind += xs[i].1;
                let inside = if evenodd { wind & 1 != 0 } else { wind != 0 };
                if inside && i + 1 < xs.len() {
                    span(row, xs[i].0, xs[i + 1].0, v);
                }
            }
        }
    }
    mask
}

fn span(row: &mut [f32], xa: f32, xb: f32, v: f32) {
    let w = row.len() as f32;
    let (xa, xb) = (xa.clamp(0.0, w), xb.clamp(0.0, w));
    if xb <= xa {
        return;
    }
    let (ia, ib) = (xa as usize, xb as usize);
    if ia == ib {
        row[ia] += (xb - xa) * v;
        return;
    }
    row[ia] += (ia as f32 + 1.0 - xa) * v;
    for p in &mut row[ia + 1..ib] {
        *p += v;
    }
    if ib < row.len() {
        row[ib] += (xb - ib as f32) * v;
    }
}

fn signed_area(p: &[(f32, f32)]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f32>() / 2.0
}

/// A stroke as polygons of one orientation (their nonzero union is the stroke):
/// a quad per segment and a round join at every vertex.
fn stroke_polys(subs: &[Vec<(f32, f32)>], closed: &[bool], hw: f32) -> Vec<Vec<(f32, f32)>> {
    let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
    let jn = ((hw * 2.0).sqrt() * 3.0).ceil().clamp(6.0, 24.0) as usize;
    for (pts, &cl) in subs.iter().zip(closed) {
        let n = pts.len();
        let segs = if cl { n } else { n.saturating_sub(1) };
        for i in 0..segs {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let (nx, ny) = (-dy / len * hw, dx / len * hw);
            out.push(vec![(p.0 + nx, p.1 + ny), (q.0 + nx, q.1 + ny), (q.0 - nx, q.1 - ny), (p.0 - nx, p.1 - ny)]);
        }
        let joins = if cl { 0..n } else { 1..n.saturating_sub(1) };
        for i in joins {
            let c = pts[i];
            out.push((0..jn).map(|k| {
                let t = std::f32::consts::TAU * k as f32 / jn as f32;
                (c.0 + hw * t.cos(), c.1 + hw * t.sin())
            }).collect());
        }
    }
    for p in &mut out {
        if signed_area(p) > 0.0 {
            p.reverse();
        }
    }
    out
}

// ------------------------------------------------------------------ paint servers

struct Grad {
    radial: bool,
    /// Linear: x1 y1 x2 y2. Radial: cx cy r.
    c: [f32; 4],
    /// Gradient space → device.
    inv: M,
    stops: Vec<(f32, [f32; 4])>,
}

impl Grad {
    fn at(&self, x: f32, y: f32) -> [f32; 4] {
        let p = self.inv.ap((x, y));
        let t = if self.radial {
            ((p.0 - self.c[0]).powi(2) + (p.1 - self.c[1]).powi(2)).sqrt() / self.c[2].max(1e-6)
        } else {
            let (dx, dy) = (self.c[2] - self.c[0], self.c[3] - self.c[1]);
            let l = dx * dx + dy * dy;
            if l < 1e-12 { 1.0 } else { ((p.0 - self.c[0]) * dx + (p.1 - self.c[1]) * dy) / l }
        }
        .clamp(0.0, 1.0);
        let s = &self.stops;
        if t <= s[0].0 {
            return s[0].1;
        }
        for i in 1..s.len() {
            if t <= s[i].0 {
                let (a, b) = (&s[i - 1], &s[i]);
                let k = if b.0 - a.0 < 1e-6 { 1.0 } else { (t - a.0) / (b.0 - a.0) };
                return [0, 1, 2, 3].map(|j| a.1[j] + (b.1[j] - a.1[j]) * k);
            }
        }
        s[s.len() - 1].1
    }
}

/// Premultiplied RGBA floats of a colour with an extra opacity.
fn premul(c: u32, op: f32) -> [f32; 4] {
    let a = ((c >> 24) & 0xff) as f32 / 255.0 * op;
    [((c >> 16) & 0xff) as f32 / 255.0 * a, ((c >> 8) & 0xff) as f32 / 255.0 * a, (c & 0xff) as f32 / 255.0 * a, a]
}

enum Src {
    Solid([f32; 4]),
    Grad(Grad),
}

struct Doc<'a> {
    ids: HashMap<&'a str, &'a Node>,
    rules: Vec<Rule>,
    w: usize,
    h: usize,
    /// Premultiplied RGBA.
    buf: Vec<[f32; 4]>,
}

impl<'a> Doc<'a> {
    fn index(&mut self, n: &'a Node) {
        if let Some(id) = n.attr("id") {
            self.ids.insert(id, n);
        }
        if n.tag == "style" {
            parse_css(&n.text, &mut self.rules);
        }
        for k in &n.kids {
            self.index(k);
        }
    }

    fn style_of(&self, n: &Node, parent: &Style) -> Style {
        let mut st = parent.clone();
        st.opacity = parent.opacity; // multiplied below if set
        st.display = true;
        for (k, v) in &n.attrs {
            apply_prop(&mut st, k, v);
        }
        let classes: Vec<&str> = n.attr("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        let id = n.attr("id");
        for r in &self.rules {
            let ok = r.tag.as_deref().map_or(true, |t| t == n.tag)
                && r.class.as_deref().map_or(true, |c| classes.contains(&c))
                && r.id.as_deref().map_or(true, |i| Some(i) == id)
                && (r.tag.is_some() || r.class.is_some() || r.id.is_some());
            if ok {
                for (k, v) in &r.decls {
                    apply_prop(&mut st, k, v);
                }
            }
        }
        if let Some(s) = n.attr("style") {
            for d in s.split(';') {
                if let Some((k, v)) = d.split_once(':') {
                    apply_prop(&mut st, k, v);
                }
            }
        }
        st
    }

    /// Follows xlink:href chains for a gradient attribute / its stops.
    fn grad_attr(&self, n: &'a Node, k: &str) -> Option<&'a str> {
        let mut cur = n;
        for _ in 0..8 {
            if let Some(v) = cur.attr(k) {
                return Some(v);
            }
            cur = *self.ids.get(cur.href()?)?;
        }
        None
    }

    fn grad_stops(&self, n: &'a Node) -> Vec<(f32, [f32; 4])> {
        let mut cur = n;
        for _ in 0..8 {
            let stops: Vec<&Node> = cur.kids.iter().filter(|k| k.tag == "stop").collect();
            if !stops.is_empty() {
                let mut last = 0.0f32;
                return stops
                    .iter()
                    .map(|s| {
                        let mut c = s.attr("stop-color").and_then(color).unwrap_or(0xff00_0000);
                        let mut op = s.attr("stop-opacity").and_then(opacity).unwrap_or(1.0);
                        if let Some(style) = s.attr("style") {
                            for d in style.split(';') {
                                if let Some((k, v)) = d.split_once(':') {
                                    match k.trim() {
                                        "stop-color" => c = color(v).unwrap_or(c),
                                        "stop-opacity" => op = opacity(v).unwrap_or(op),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        let off = s.attr("offset").and_then(|o| length(o, 1.0)).unwrap_or(0.0).clamp(0.0, 1.0).max(last);
                        last = off;
                        (off, premul(c, op))
                    })
                    .collect();
            }
            match cur.href().and_then(|h| self.ids.get(h)) {
                Some(next) => cur = next,
                None => break,
            }
        }
        Vec::new()
    }

    fn source(&self, p: &Paint, st: &Style, op: f32, ctm: M, bbox: (f32, f32, f32, f32)) -> Option<Src> {
        match p {
            Paint::None => None,
            Paint::Color(c) => Some(Src::Solid(premul(*c, op))),
            Paint::Current => Some(Src::Solid(premul(st.color, op))),
            Paint::Url(id, fb) => {
                let Some(n) = self.ids.get(id.as_str()).copied() else {
                    return self.source(fb, st, op, ctm, bbox);
                };
                let radial = match n.tag.as_str() {
                    "linearGradient" => false,
                    "radialGradient" => true,
                    _ => return self.source(fb, st, op, ctm, bbox),
                };
                let mut stops = self.grad_stops(n);
                if stops.is_empty() {
                    return None;
                }
                for s in &mut stops {
                    s.1 = s.1.map(|v| v * op);
                }
                if stops.len() == 1 {
                    return Some(Src::Solid(stops[0].1));
                }
                let user = self.grad_attr(n, "gradientUnits") == Some("userSpaceOnUse");
                let (bx, by, bw, bh) = bbox;
                let g = |k: &str, def: f32, rel: f32| self.grad_attr(n, k).and_then(|v| length(v, rel)).unwrap_or(def);
                let (rw, rh) = if user { (self.w as f32, self.h as f32) } else { (1.0, 1.0) };
                let c = if radial {
                    let r = g("r", 0.5 * rw.max(rh), rw.max(rh));
                    [g("cx", 0.5 * rw, rw), g("cy", 0.5 * rh, rh), r, 0.0]
                } else {
                    [g("x1", 0.0, rw), g("y1", 0.0, rh), g("x2", rw, rw), g("y2", 0.0, rh)]
                };
                let mut m = ctm;
                if !user {
                    if bw <= 0.0 || bh <= 0.0 {
                        return None;
                    }
                    m = m.mul(M([bw, 0.0, 0.0, bh, bx, by]));
                }
                if let Some(t) = self.grad_attr(n, "gradientTransform") {
                    m = m.mul(transform(t));
                }
                Some(Src::Grad(Grad { radial, c, inv: m.inv()?, stops }))
            }
        }
    }

    fn composite(&mut self, mask: &[f32], src: &Src, op: f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let m = mask[i].min(1.0) * op;
                if m <= 0.002 {
                    continue;
                }
                let s = match src {
                    Src::Solid(c) => *c,
                    Src::Grad(g) => g.at(x as f32 + 0.5, y as f32 + 0.5),
                };
                let d = &mut self.buf[i];
                let k = 1.0 - s[3] * m;
                for j in 0..4 {
                    d[j] = s[j] * m + d[j] * k;
                }
            }
        }
    }

    fn draw_shape(&mut self, subs: Vec<Sub>, st: &Style, ctm: M) {
        if subs.is_empty() || !st.visible {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for s in &subs {
            for p in &s.pts {
                x0 = x0.min(p.0);
                y0 = y0.min(p.1);
                x1 = x1.max(p.0);
                y1 = y1.max(p.1);
            }
        }
        let bbox = (x0, y0, x1 - x0, y1 - y0);
        let dev: Vec<Vec<(f32, f32)>> = subs.iter().map(|s| s.pts.iter().map(|p| ctm.ap(*p)).collect()).collect();
        if let Some(src) = self.source(&st.fill, st, st.fill_op, ctm, bbox) {
            let mask = fill_mask(&dev, self.w, self.h, st.evenodd);
            self.composite(&mask, &src, st.opacity);
        }
        if st.stroke_w > 0.0 {
            if let Some(src) = self.source(&st.stroke, st, st.stroke_op, ctm, bbox) {
                let closed: Vec<bool> = subs.iter().map(|s| s.closed).collect();
                let hw = (st.stroke_w * ctm.scale() / 2.0).max(0.35);
                let polys = stroke_polys(&dev, &closed, hw);
                let mask = fill_mask(&polys, self.w, self.h, false);
                self.composite(&mask, &src, st.opacity);
            }
        }
    }

    fn render(&mut self, n: &'a Node, parent: &Style, ctm: M, depth: u32) {
        if depth > 24 {
            return;
        }
        let st = self.style_of(n, parent);
        if !st.display {
            return;
        }
        let ctm = match n.attr("transform") {
            Some(t) => ctm.mul(transform(t)),
            None => ctm,
        };
        let k = ctm.scale().max(0.01);
        let mut pb = PathB::new(k);
        match n.tag.as_str() {
            "g" | "a" | "svg" => {
                let ctm = if n.tag == "svg" && depth > 0 {
                    ctm.mul(M::translate(n.num("x", 0.0), n.num("y", 0.0)))
                } else {
                    ctm
                };
                for kid in &n.kids {
                    self.render(kid, &st, ctm, depth + 1);
                }
                return;
            }
            "switch" => {
                if let Some(kid) = n.kids.iter().find(|k| k.tag != "foreignObject") {
                    self.render(kid, &st, ctm, depth + 1);
                }
                return;
            }
            "use" => {
                let Some(t) = n.href().and_then(|h| self.ids.get(h)).copied() else { return };
                let ctm = ctm.mul(M::translate(n.num("x", 0.0), n.num("y", 0.0)));
                if t.tag == "symbol" {
                    for kid in &t.kids {
                        self.render(kid, &st, ctm, depth + 1);
                    }
                } else {
                    self.render(t, &st, ctm, depth + 1);
                }
                return;
            }
            "path" => parse_path(n.attr("d").unwrap_or(""), &mut pb),
            "rect" => {
                let (x, y) = (n.num("x", 0.0), n.num("y", 0.0));
                let (w, h) = (n.num("width", 0.0), n.num("height", 0.0));
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let (mut rx, mut ry) = (n.attr("rx").and_then(|v| length(v, w)), n.attr("ry").and_then(|v| length(v, h)));
                if rx.is_none() {
                    rx = ry;
                }
                if ry.is_none() {
                    ry = rx;
                }
                let (rx, ry) = (rx.unwrap_or(0.0).clamp(0.0, w / 2.0), ry.unwrap_or(0.0).clamp(0.0, h / 2.0));
                if rx > 0.0 && ry > 0.0 {
                    pb.move_to((x + rx, y));
                    pb.line_to((x + w - rx, y));
                    pb.arc(rx, ry, 0.0, false, true, (x + w, y + ry));
                    pb.line_to((x + w, y + h - ry));
                    pb.arc(rx, ry, 0.0, false, true, (x + w - rx, y + h));
                    pb.line_to((x + rx, y + h));
                    pb.arc(rx, ry, 0.0, false, true, (x, y + h - ry));
                    pb.line_to((x, y + ry));
                    pb.arc(rx, ry, 0.0, false, true, (x + rx, y));
                } else {
                    pb.move_to((x, y));
                    pb.line_to((x + w, y));
                    pb.line_to((x + w, y + h));
                    pb.line_to((x, y + h));
                }
                pb.end(true);
            }
            "circle" => {
                let r = n.num("r", 0.0);
                if r <= 0.0 {
                    return;
                }
                pb.ellipse(n.num("cx", 0.0), n.num("cy", 0.0), r, r);
            }
            "ellipse" => {
                let (rx, ry) = (n.num("rx", 0.0), n.num("ry", 0.0));
                if rx <= 0.0 || ry <= 0.0 {
                    return;
                }
                pb.ellipse(n.num("cx", 0.0), n.num("cy", 0.0), rx, ry);
            }
            "line" => {
                pb.move_to((n.num("x1", 0.0), n.num("y1", 0.0)));
                pb.line_to((n.num("x2", 0.0), n.num("y2", 0.0)));
                pb.end(false);
            }
            "polyline" | "polygon" => {
                let v: Vec<f32> = Nums::new(n.attr("points").unwrap_or("")).collect();
                for (i, c) in v.chunks_exact(2).enumerate() {
                    if i == 0 {
                        pb.move_to((c[0], c[1]));
                    } else {
                        pb.line_to((c[0], c[1]));
                    }
                }
                pb.end(n.tag == "polygon");
            }
            _ => return, // defs, gradients, clipPath, mask, style, metadata, text, ...
        }
        self.draw_shape(pb.subs, &st, ctm);
    }
}

/// Renders an SVG document into a `size`×`size` premultiplied ARGB image (aspect kept,
/// centred).
pub fn render(text: &str, size: usize) -> Option<Vec<u32>> {
    let root = parse_xml(text)?;
    let size = size.clamp(1, 1024);
    let (vw, vh) = (
        root.attr("width").and_then(|v| length(v, 100.0)).unwrap_or(0.0),
        root.attr("height").and_then(|v| length(v, 100.0)).unwrap_or(0.0),
    );
    let vb: Vec<f32> = root.attr("viewBox").map(|v| Nums::new(v).collect()).unwrap_or_default();
    let (bx, by, bw, bh) = if vb.len() == 4 && vb[2] > 0.0 && vb[3] > 0.0 {
        (vb[0], vb[1], vb[2], vb[3])
    } else if vw > 0.0 && vh > 0.0 {
        (0.0, 0.0, vw, vh)
    } else {
        (0.0, 0.0, 100.0, 100.0)
    };
    let k = size as f32 / bw.max(bh);
    let (ox, oy) = ((size as f32 - bw * k) / 2.0, (size as f32 - bh * k) / 2.0);
    let ctm = M([k, 0.0, 0.0, k, ox - bx * k, oy - by * k]);
    let mut doc = Doc { ids: HashMap::new(), rules: Vec::new(), w: size, h: size, buf: vec![[0.0; 4]; size * size] };
    doc.index(&root);
    // The root's own width/height/x/y are not a translation.
    let st = doc.style_of(&root, &Style::default());
    for kid in &root.kids {
        doc.render(kid, &st, ctm, 1);
    }
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    Some(doc.buf.iter().map(|p| to8(p[3]) << 24 | to8(p[0]) << 16 | to8(p[1]) << 8 | to8(p[2])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u32], size: usize, x: usize, y: usize) -> u32 {
        img[y * size + x]
    }

    #[test]
    fn rect_and_circle() {
        let s = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="0" y="0" width="8" height="16" fill="#ff0000"/><circle cx="12" cy="8" r="3" style="fill:#0000ff"/></svg>"##;
        let img = render(s, 16).unwrap();
        assert_eq!(px(&img, 16, 2, 8), 0xffff0000);
        assert_eq!(px(&img, 16, 12, 8), 0xff0000ff);
        assert_eq!(px(&img, 16, 15, 0) >> 24, 0);
    }

    #[test]
    fn evenodd_hole_and_current_color() {
        let s = r##"<svg viewBox="0 0 10 10"><style>.c{color:#00ff00}</style><path class="c" fill="currentColor" fill-rule="evenodd" d="M0 0h10v10H0zM3 3h4v4H3z"/></svg>"##;
        let img = render(s, 10).unwrap();
        assert_eq!(px(&img, 10, 1, 1), 0xff00ff00);
        assert_eq!(px(&img, 10, 5, 5) >> 24, 0);
    }

    #[test]
    fn gradient_and_arc() {
        let s = r##"<svg viewBox="0 0 20 20"><defs><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient></defs><path d="M0 0H20V20H0Z" fill="url(#g)"/><path d="M10 2a8 8 0 1 1 0 16a8 8 0 1 1 0-16z" fill="none" stroke="red" stroke-width="2"/></svg>"##;
        let img = render(s, 20).unwrap();
        let l = px(&img, 20, 1, 1) & 0xff;
        let r = px(&img, 20, 18, 1) & 0xff;
        assert!(l < 40 && r > 200, "{l} {r}");
        // On the circle's stroke (top): red.
        assert_eq!(px(&img, 20, 10, 2) & 0x00ff_0000, 0x00ff_0000);
    }
}
