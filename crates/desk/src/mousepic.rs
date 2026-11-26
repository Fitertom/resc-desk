//! «Параметры» → «Мышь»: the Logitech G502 HERO from above (our own drawing), its buttons
//! named by leader lines on either side with what each does, the chosen one lit. A click on
//! a button or its label chooses it. The layout of buttons follows libratbag's numbering
//! for the G502 (0 left … 10 tilt left); G6–G9 work inside the mouse (DPI) until the desk
//! talks to it over HID++.

/// A part of the picture: its id in the SVG, the binding's name (mousebind.rs BUTTONS; None:
/// not seen by the desk), its label, on the left (true) or right, where its leader ends
/// (picture units, 300 × 360) and where it can be clicked.
pub struct Part {
    pub id: &'static str,
    pub bind: Option<&'static str>,
    pub label: &'static str,
    pub left: bool,
    pub anchor: (f32, f32),
    pub hit: [f32; 4],
}

/// Small parts first: a click is the first one it falls in.
pub const PARTS: [Part; 11] = [
    Part { id: "tilt_left", bind: Some("tilt_left"), label: "Наклон колеса влево", left: true, anchor: (131.0, 68.0), hit: [124.0, 58.0, 138.0, 78.0] },
    Part { id: "tilt_right", bind: Some("tilt_right"), label: "Наклон колеса вправо", left: false, anchor: (169.0, 68.0), hit: [162.0, 58.0, 176.0, 78.0] },
    Part { id: "middle", bind: Some("middle"), label: "Нажатие колеса (G3)", left: false, anchor: (150.0, 52.0), hit: [140.0, 44.0, 160.0, 92.0] },
    Part { id: "g9", bind: None, label: "G9", left: false, anchor: (150.0, 125.0), hit: [140.0, 116.0, 160.0, 135.0] },
    Part { id: "g8", bind: None, label: "G8 (DPI +)", left: true, anchor: (104.0, 62.0), hit: [97.0, 38.0, 111.0, 98.0] },
    Part { id: "g7", bind: None, label: "G7 (DPI −)", left: true, anchor: (100.0, 125.0), hit: [94.0, 99.0, 106.0, 153.0] },
    Part { id: "sniper", bind: None, label: "G6 · снайпер", left: true, anchor: (78.0, 162.0), hit: [66.0, 143.0, 90.0, 181.0] },
    Part { id: "extra", bind: Some("extra"), label: "G5 · вперёд", left: true, anchor: (70.0, 205.0), hit: [58.0, 181.0, 82.0, 227.0] },
    Part { id: "side", bind: Some("side"), label: "G4 · назад", left: true, anchor: (70.0, 250.0), hit: [57.0, 227.0, 82.0, 275.0] },
    Part { id: "left", bind: Some("left"), label: "Левая (G1)", left: true, anchor: (122.0, 140.0), hit: [96.0, 20.0, 145.0, 170.0] },
    Part { id: "right", bind: Some("right"), label: "Правая (G2)", left: false, anchor: (186.0, 110.0), hit: [155.0, 20.0, 216.0, 168.0] },
];

pub const W: f32 = 300.0;
pub const H: f32 = 360.0;

/// The picture with part `sel` lit in `accent` (0xRRGGBB).
pub fn svg(sel: Option<&str>, accent: u32) -> String {
    let lit = |id: &str| {
        if Some(id) == sel {
            format!(" fill=\"#{accent:06x}\" fill-opacity=\"0.55\" stroke=\"#{accent:06x}\" stroke-opacity=\"1\"")
        } else {
            String::new()
        }
    };
    let parts = [
        ("left", "path", "d=\"M113 32 C123 24 136 21 144 21 L144 152 L101 168 C97 122 101 74 113 32 Z\""),
        ("right", "path", "d=\"M156 21 C172 21 183 25 190 33 L207 73 C215 110 215 144 211 166 L156 152 Z\""),
        ("middle", "rect", "x=\"141\" y=\"46\" width=\"18\" height=\"44\" rx=\"9\""),
        ("tilt_left", "path", "d=\"M134 62 L128 68 L134 74 Z\""),
        ("tilt_right", "path", "d=\"M166 62 L172 68 L166 74 Z\""),
        ("g9", "rect", "x=\"141\" y=\"118\" width=\"18\" height=\"15\" rx=\"3\""),
        ("g8", "path", "d=\"M103 48 L109 40 L106 92 L99 98 Z\""),
        ("g7", "path", "d=\"M98 104 L105 99 L103 146 L96 152 Z\""),
        ("sniper", "path", "d=\"M70 156 L83 146 L86 170 L73 178 Z\""),
        ("extra", "path", "d=\"M62 190 L77 184 L79 220 L63 226 Z\""),
        ("side", "path", "d=\"M61 234 L77 228 L79 266 L63 272 Z\""),
    ];
    let mut s = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 300 360\" width=\"300\" height=\"360\">\
         <g fill=\"#ffffff\" fill-opacity=\"0.07\" stroke=\"#ffffff\" stroke-opacity=\"0.5\" stroke-width=\"2\" stroke-linejoin=\"round\">\
         <path d=\"M108 26 C122 14 178 14 192 26 L214 68 C226 112 230 172 224 232 C219 284 204 330 162 346 C136 355 108 346 93 326 C76 304 56 286 48 254 C41 226 44 200 60 184 L76 172 C80 118 88 66 108 26 Z\"/>\
         <rect x=\"143\" y=\"100\" width=\"14\" height=\"9\" rx=\"2\" fill-opacity=\"0.03\" stroke-opacity=\"0.25\"/>\
         <circle cx=\"176\" cy=\"252\" r=\"15\" fill=\"none\" stroke-opacity=\"0.25\" stroke-width=\"5\"/>",
    );
    for (id, tag, geo) in parts {
        let tilt = if id.starts_with("tilt") && Some(id) != sel { " fill-opacity=\"0.5\"" } else { "" };
        s += &format!("<{tag} {geo}{tilt}{}/>", lit(id));
    }
    s += "</g></svg>";
    s
}

/// The part under (x, y) in picture units.
pub fn hit(x: f32, y: f32) -> Option<&'static Part> {
    PARTS.iter().find(|p| x >= p.hit[0] && x <= p.hit[2] && y >= p.hit[1] && y <= p.hit[3])
}

/// The labels' heights on one side (picture units): by their anchors, at least `gap` apart,
/// within the picture.
pub fn label_ys(left: bool, gap: f32) -> Vec<(usize, f32)> {
    let mut v: Vec<(usize, f32)> = PARTS.iter().enumerate().filter(|(_, p)| p.left == left).map(|(i, p)| (i, p.anchor.1)).collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut y = f32::MIN;
    for e in v.iter_mut() {
        e.1 = e.1.max(y + gap).max(gap * 0.5);
        y = e.1;
    }
    // Pushed past the bottom: back up.
    let mut lim = H - gap * 0.5;
    for e in v.iter_mut().rev() {
        e.1 = e.1.min(lim);
        lim = e.1 - gap;
    }
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn picture_renders_and_parts_are_found() {
        let img = crate::svg::render(&super::svg(Some("side"), 0xa78bfa), 120).unwrap();
        assert!(img.iter().any(|p| p >> 24 > 0));
        assert_eq!(super::hit(70.0, 250.0).map(|p| p.id), Some("side"));
        assert_eq!(super::hit(150.0, 60.0).map(|p| p.id), Some("middle"));
        assert_eq!(super::hit(120.0, 100.0).map(|p| p.id), Some("left"));
        let l = super::label_ys(true, 40.0);
        assert!(l.windows(2).all(|w| w[1].1 - w[0].1 >= 39.9));
    }
}
