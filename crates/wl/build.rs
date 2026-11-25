//! Wayland scanner: parses protocols/*.xml with a tiny hand-written XML reader
//! and generates typed request/event enums plus send functions for both sides.
use std::fmt::Write as _;
use std::{env, fs, path::Path};

struct Arg {
    name: String,
    ty: String,
    iface: Option<String>,
    nullable: bool,
}
struct Msg {
    name: String,
    since: u32,
    args: Vec<Arg>,
}
struct Entry {
    name: String,
    value: u32,
}
struct Enum {
    name: String,
    entries: Vec<Entry>,
}
struct Iface {
    name: String,
    version: u32,
    requests: Vec<Msg>,
    events: Vec<Msg>,
    enums: Vec<Enum>,
}

fn main() {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../protocols");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("protocols dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |x| x == "xml"))
        .collect();
    files.sort();
    let mut ifaces = Vec::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let text = fs::read_to_string(f).unwrap();
        parse(&text, &mut ifaces);
    }
    let code = generate(&ifaces);
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("protocol.rs"), code).unwrap();
}

// ------------------------------------------------------------------ XML

enum Tok {
    Open(String, Vec<(String, String)>, bool),
    Close(String),
}

fn find(s: &str, from: usize, pat: &str) -> usize {
    s[from..].find(pat).map(|i| i + from).expect("unterminated xml construct")
}

fn unescape(v: &str) -> String {
    v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn tokenize(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            i = find(s, i, "-->") + 3;
            continue;
        }
        if rest.starts_with("<?") {
            i = find(s, i, "?>") + 2;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = find(s, i, "]]>") + 3;
            continue;
        }
        if rest.starts_with("<!") {
            i = find(s, i, ">") + 1;
            continue;
        }
        // Tag end, honouring quotes.
        let mut j = i + 1;
        let mut q = 0u8;
        while j < b.len() {
            let c = b[j];
            if q != 0 {
                if c == q {
                    q = 0;
                }
            } else if c == b'"' || c == b'\'' {
                q = c;
            } else if c == b'>' {
                break;
            }
            j += 1;
        }
        let inner = &s[i + 1..j];
        i = j + 1;
        if let Some(n) = inner.strip_prefix('/') {
            out.push(Tok::Close(n.trim().to_string()));
            continue;
        }
        let self_close = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let ib = inner.as_bytes();
        let mut k = 0;
        while k < ib.len() && !ib[k].is_ascii_whitespace() {
            k += 1;
        }
        let name = inner[..k].to_string();
        let mut attrs = Vec::new();
        loop {
            while k < ib.len() && ib[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= ib.len() {
                break;
            }
            let ks = k;
            while k < ib.len() && ib[k] != b'=' && !ib[k].is_ascii_whitespace() {
                k += 1;
            }
            let key = inner[ks..k].to_string();
            while k < ib.len() && (ib[k] == b'=' || ib[k].is_ascii_whitespace()) {
                k += 1;
            }
            if k >= ib.len() {
                attrs.push((key, String::new()));
                break;
            }
            let qc = ib[k];
            k += 1;
            let vs = k;
            while k < ib.len() && ib[k] != qc {
                k += 1;
            }
            attrs.push((key, unescape(&inner[vs..k])));
            k += 1;
        }
        out.push(Tok::Open(name, attrs, self_close));
    }
    out
}

fn attr<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn parse_num(v: &str) -> u32 {
    let v = v.trim();
    if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).unwrap()
    } else {
        v.parse::<i64>().unwrap() as u32
    }
}

fn parse(text: &str, out: &mut Vec<Iface>) {
    #[derive(PartialEq)]
    enum In {
        None,
        Req,
        Evt,
        Enum,
    }
    let mut cur: Option<Iface> = None;
    let mut msg: Option<Msg> = None;
    let mut en: Option<Enum> = None;
    let mut ctx = In::None;
    for t in tokenize(text) {
        match t {
            Tok::Open(name, a, sc) => match name.as_str() {
                "interface" => {
                    cur = Some(Iface {
                        name: attr(&a, "name").unwrap().to_string(),
                        version: attr(&a, "version").map_or(1, parse_num),
                        requests: vec![],
                        events: vec![],
                        enums: vec![],
                    })
                }
                "request" | "event" => {
                    ctx = if name == "request" { In::Req } else { In::Evt };
                    msg = Some(Msg {
                        name: attr(&a, "name").unwrap().to_string(),
                        since: attr(&a, "since").map_or(1, parse_num),
                        args: vec![],
                    });
                    if sc {
                        let m = msg.take().unwrap();
                        let i = cur.as_mut().unwrap();
                        if ctx == In::Req { i.requests.push(m) } else { i.events.push(m) }
                        ctx = In::None;
                    }
                }
                "arg" => {
                    if let Some(m) = msg.as_mut() {
                        m.args.push(Arg {
                            name: attr(&a, "name").unwrap().to_string(),
                            ty: attr(&a, "type").unwrap().to_string(),
                            iface: attr(&a, "interface").map(|s| s.to_string()),
                            nullable: attr(&a, "allow-null") == Some("true"),
                        });
                    }
                }
                "enum" => {
                    ctx = In::Enum;
                    en = Some(Enum { name: attr(&a, "name").unwrap().to_string(), entries: vec![] });
                    if sc {
                        cur.as_mut().unwrap().enums.push(en.take().unwrap());
                        ctx = In::None;
                    }
                }
                "entry" => {
                    if let Some(e) = en.as_mut() {
                        e.entries.push(Entry {
                            name: attr(&a, "name").unwrap().to_string(),
                            value: parse_num(attr(&a, "value").unwrap()),
                        });
                    }
                }
                _ => {}
            },
            Tok::Close(name) => match name.as_str() {
                "interface" => out.push(cur.take().unwrap()),
                "request" | "event" => {
                    if let Some(m) = msg.take() {
                        let i = cur.as_mut().unwrap();
                        if ctx == In::Req { i.requests.push(m) } else { i.events.push(m) }
                    }
                    ctx = In::None;
                }
                "enum" => {
                    if let Some(e) = en.take() {
                        cur.as_mut().unwrap().enums.push(e);
                    }
                    ctx = In::None;
                }
                _ => {}
            },
        }
    }
}

// ------------------------------------------------------------------ codegen

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
    "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box",
    "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

fn ident(n: &str) -> String {
    if KEYWORDS.contains(&n) { format!("r#{n}") } else { n.to_string() }
}

fn camel(n: &str) -> String {
    let mut s = String::new();
    let mut up = true;
    for c in n.chars() {
        if c == '_' {
            up = true;
        } else if up {
            s.extend(c.to_uppercase());
            up = false;
        } else {
            s.push(c);
        }
    }
    s
}

fn upper(n: &str) -> String {
    let u = n.to_uppercase();
    if u.chars().next().map_or(false, |c| c.is_ascii_digit()) { format!("_{u}") } else { u }
}

/// Field type stored in parsed enums.
fn owned_ty(a: &Arg) -> &'static str {
    match a.ty.as_str() {
        "int" => "i32",
        "uint" => "u32",
        "fixed" => "Fixed",
        "string" => {
            if a.nullable { "Option<String>" } else { "String" }
        }
        "object" => "ObjId",
        "new_id" => {
            if a.iface.is_some() { "ObjId" } else { "NewIdDyn" }
        }
        "array" => "Vec<u8>",
        "fd" => "OwnedFd",
        t => panic!("unknown arg type {t}"),
    }
}

fn read_expr(a: &Arg) -> &'static str {
    match a.ty.as_str() {
        "int" => "a.int()?",
        "uint" => "a.uint()?",
        "fixed" => "a.fixed()?",
        "string" => {
            if a.nullable { "a.opt_string()?" } else { "a.string()?" }
        }
        "object" => "a.uint()?",
        "new_id" => {
            if a.iface.is_some() { "a.uint()?" } else { "a.new_id_dyn()?" }
        }
        "array" => "a.array()?",
        "fd" => "a.fd()?",
        _ => unreachable!(),
    }
}

fn param(a: &Arg) -> String {
    let t = match a.ty.as_str() {
        "int" => "i32".to_string(),
        "uint" => "u32".into(),
        "fixed" => "Fixed".into(),
        "string" => {
            if a.nullable { "Option<&str>".into() } else { "&str".into() }
        }
        "object" => "ObjId".into(),
        "new_id" => {
            if a.iface.is_some() { "ObjId".into() } else { "(&str, u32, ObjId)".into() }
        }
        "array" => "&[u8]".into(),
        "fd" => "BorrowedFd<'_>".into(),
        _ => unreachable!(),
    };
    format!("{}: {}", ident(&a.name), t)
}

fn write_stmt(a: &Arg) -> String {
    let n = ident(&a.name);
    match a.ty.as_str() {
        "int" => format!("c.put_i32({n});"),
        "uint" | "object" => format!("c.put_u32({n});"),
        "fixed" => format!("c.put_i32({n}.0);"),
        "string" => {
            if a.nullable { format!("c.put_opt_str({n});") } else { format!("c.put_str({n});") }
        }
        "new_id" => {
            if a.iface.is_some() {
                format!("c.put_u32({n});")
            } else {
                format!("c.put_str({n}.0); c.put_u32({n}.1); c.put_u32({n}.2);")
            }
        }
        "array" => format!("c.put_array({n});"),
        "fd" => format!("c.put_fd({n});"),
        _ => unreachable!(),
    }
}

fn gen_parse_enum(o: &mut String, ename: &str, msgs: &[Msg]) {
    writeln!(o, "    #[derive(Debug)]\n    pub enum {ename} {{").unwrap();
    for m in msgs {
        if m.args.is_empty() {
            writeln!(o, "        {},", camel(&m.name)).unwrap();
        } else {
            let fields: Vec<String> = m.args.iter().map(|a| format!("{}: {}", ident(&a.name), owned_ty(a))).collect();
            writeln!(o, "        {} {{ {} }},", camel(&m.name), fields.join(", ")).unwrap();
        }
    }
    if msgs.is_empty() {
        writeln!(o, "        #[doc(hidden)] _Never,").unwrap();
    }
    writeln!(o, "    }}\n    impl {ename} {{").unwrap();
    writeln!(o, "        pub fn parse(op: u16, a: &mut Args<'_>) -> Result<Self, WireError> {{").unwrap();
    writeln!(o, "            Ok(match op {{").unwrap();
    for (i, m) in msgs.iter().enumerate() {
        if m.args.is_empty() {
            writeln!(o, "                {i} => Self::{},", camel(&m.name)).unwrap();
        } else {
            let fields: Vec<String> = m.args.iter().map(|a| format!("{}: {}", ident(&a.name), read_expr(a))).collect();
            writeln!(o, "                {i} => Self::{} {{ {} }},", camel(&m.name), fields.join(", ")).unwrap();
        }
    }
    writeln!(o, "                _ => return Err(WireError::BadOpcode(op)),\n            }})\n        }}").unwrap();
    let names: Vec<String> = msgs.iter().map(|m| format!("{:?}", m.name)).collect();
    writeln!(o, "        pub const NAMES: &'static [&'static str] = &[{}];", names.join(", ")).unwrap();
    let since: Vec<String> = msgs.iter().map(|m| m.since.to_string()).collect();
    writeln!(o, "        pub const SINCE: &'static [u32] = &[{}];", since.join(", ")).unwrap();
    writeln!(o, "    }}").unwrap();
}

fn gen_send_mod(o: &mut String, modname: &str, msgs: &[Msg]) {
    writeln!(o, "    pub mod {modname} {{\n        use super::*;").unwrap();
    for (i, m) in msgs.iter().enumerate() {
        writeln!(o, "        pub const {}: u16 = {i};", upper(&m.name)).unwrap();
        let mut ps = vec!["c: &mut Conn".to_string(), "self_id: ObjId".to_string()];
        ps.extend(m.args.iter().map(param));
        writeln!(o, "        pub fn {}({}) {{", ident(&m.name), ps.join(", ")).unwrap();
        writeln!(o, "            let s = c.begin(self_id, {i});").unwrap();
        for a in &m.args {
            writeln!(o, "            {}", write_stmt(a)).unwrap();
        }
        writeln!(o, "            c.end(s);\n        }}").unwrap();
    }
    writeln!(o, "    }}").unwrap();
}

fn generate(ifaces: &[Iface]) -> String {
    let mut o = String::new();
    o.push_str("// @generated by crates/wl/build.rs from protocols/*.xml\n");
    // Interface registry.
    o.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\npub enum Iface {\n");
    for i in ifaces {
        writeln!(o, "    {},", camel(&i.name)).unwrap();
    }
    o.push_str("}\nimpl Iface {\n    pub fn name(self) -> &'static str {\n        match self {\n");
    for i in ifaces {
        writeln!(o, "            Iface::{} => {:?},", camel(&i.name), i.name).unwrap();
    }
    o.push_str("        }\n    }\n    pub fn version(self) -> u32 {\n        match self {\n");
    for i in ifaces {
        writeln!(o, "            Iface::{} => {},", camel(&i.name), i.version).unwrap();
    }
    o.push_str("        }\n    }\n    pub fn from_name(n: &str) -> Option<Self> {\n        Some(match n {\n");
    for i in ifaces {
        writeln!(o, "            {:?} => Iface::{},", i.name, camel(&i.name)).unwrap();
    }
    o.push_str("            _ => return None,\n        })\n    }\n}\n\n");

    for i in ifaces {
        writeln!(o, "#[allow(non_upper_case_globals, dead_code, unused_imports, unused_variables, unreachable_code, clippy::all)]").unwrap();
        writeln!(o, "pub mod {} {{", i.name).unwrap();
        o.push_str("    use crate::wire::*;\n    use std::os::fd::{BorrowedFd, OwnedFd};\n");
        writeln!(o, "    pub const NAME: &str = {:?};\n    pub const VERSION: u32 = {};", i.name, i.version).unwrap();
        gen_parse_enum(&mut o, "Request", &i.requests);
        gen_parse_enum(&mut o, "Event", &i.events);
        gen_send_mod(&mut o, "req", &i.requests);
        gen_send_mod(&mut o, "evt", &i.events);
        for e in &i.enums {
            let mname = if e.name == "req" || e.name == "evt" { format!("{}_enum", e.name) } else { e.name.clone() };
            writeln!(o, "    pub mod {} {{", ident(&mname)).unwrap();
            for en in &e.entries {
                writeln!(o, "        pub const {}: u32 = {};", upper(&en.name), en.value).unwrap();
            }
            writeln!(o, "    }}").unwrap();
        }
        o.push_str("}\n\n");
    }
    o
}
