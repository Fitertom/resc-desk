//! Updates from the project's GitHub releases (public: no login).
//!
//! Every build knows its version: `r<commit time>-<commit>` (build.rs). A release is tagged
//! the same way and carries the program (`resc-desk-x86_64`) and its SHA-256
//! (`resc-desk-x86_64.sha256`). Checking asks GitHub for the latest release; a later commit
//! time is an update. Installing downloads it next to the running program, checks the
//! SHA-256, and puts it in its place (a rename: the running one keeps its old file until it
//! exits). `ctl restart` then starts the new one in place of the running desk.
//!
//! The network goes through `curl` (on every Arch: pacman needs it): no TLS of our own.

pub const REPO: &str = "Fitertom/resc-desk";
pub const ASSET: &str = "resc-desk-x86_64";

/// This build's version: `r<unix time of its commit>-<short commit>` ("dev" outside git).
pub const BUILD: &str = env!("RESC_BUILD");

/// The commit time a version stands for.
pub fn build_time(tag: &str) -> Option<u64> {
    tag.strip_prefix('r')?.split('-').next()?.parse().ok()
}

/// The version shown to people: `25.11.2026 17:40 · 1a2b3c4`.
pub fn pretty(tag: &str) -> String {
    let Some(t) = build_time(tag) else { return tag.to_string() };
    let sha = tag.split('-').nth(1).unwrap_or("");
    let days = (t / 86400) as i64;
    let (y, m, d) = civil(days);
    let s = t % 86400;
    format!("{d:02}.{m:02}.{y} {:02}:{:02} UTC · {sha}", s / 3600, s / 60 % 60)
}

fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub sha_url: String,
}

/// A JSON string's value after `"key":` from `at` on (escapes undone); and where it ended.
fn json_str_from(text: &str, key: &str, at: usize) -> Option<(String, usize)> {
    let pat = format!("\"{key}\"");
    let i = at + text[at..].find(&pat)? + pat.len();
    let rest = &text[i..];
    let colon = rest.find(':')?;
    let rest2 = rest[colon + 1..].trim_start();
    let start = i + colon + 1 + (rest[colon + 1..].len() - rest2.len());
    let mut chars = rest2.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some((j, c)) = chars.next() {
        match c {
            '"' => return Some((out, start + j + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|x| x.1)).collect();
                    let cp = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(cp).unwrap_or('?'));
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    None
}

/// GitHub's "latest release" answer: its tag, notes and our two files' links.
pub fn parse_release(json: &str) -> Option<Release> {
    let tag = json_str_from(json, "tag_name", 0)?.0;
    let notes = json_str_from(json, "body", 0).map(|x| x.0).unwrap_or_default();
    let mut url = None;
    let mut sha_url = None;
    let mut at = 0;
    while let Some((u, end)) = json_str_from(json, "browser_download_url", at) {
        let name = u.rsplit('/').next().unwrap_or("");
        if name == ASSET {
            url = Some(u.clone());
        } else if name == format!("{ASSET}.sha256") {
            sha_url = Some(u.clone());
        }
        at = end;
    }
    Some(Release { tag, notes, url: url?, sha_url: sha_url? })
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "15", "-A", "resc-desk-updater"])
        .args(args)
        .output()
        .map_err(|e| format!("нет curl ({e})"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "сеть недоступна".into() } else { err });
    }
    Ok(out.stdout)
}

/// The latest release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["-H", "Accept: application/vnd.github+json", &api])?;
    let text = String::from_utf8_lossy(&body);
    let rel = parse_release(&text).ok_or("в последнем релизе нет программы")?;
    let newer = match (build_time(&rel.tag), build_time(BUILD)) {
        (Some(a), Some(b)) => a > b,
        // A build from outside git: any release is newer.
        (Some(_), None) => true,
        _ => false,
    };
    Ok(newer.then_some(rel))
}

/// The running program's file (symlinks followed).
pub fn exe_path() -> Result<std::path::PathBuf, String> {
    let p = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

/// Downloads release `rel`, checks it and puts it in place of the running program.
pub fn install(rel: &Release) -> Result<std::path::PathBuf, String> {
    let exe = exe_path()?;
    let dir = exe.parent().ok_or("нет папки программы")?.to_path_buf();
    let tmp = dir.join(".resc-desk.update");
    let tmp_s = tmp.to_string_lossy().to_string();
    if let Err(e) = std::fs::write(&tmp, b"") {
        return Err(format!("нет прав на запись в {} ({e})", dir.display()));
    }
    let res = (|| {
        curl(&["-o", &tmp_s, &rel.url])?;
        let want = String::from_utf8_lossy(&curl(&[&rel.sha_url])?).split_whitespace().next().unwrap_or("").to_lowercase();
        let data = std::fs::read(&tmp).map_err(|e| e.to_string())?;
        let got = hex(&sha256(&data));
        if want.len() != 64 || got != want {
            return Err("файл повреждён: SHA-256 не совпал".to_string());
        }
        // An ELF at all (a page of HTML would not be).
        if !data.starts_with(b"\x7fELF") {
            return Err("скачалась не программа".into());
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &exe).map_err(|e| format!("не удалось заменить {}: {e}", exe.display()))?;
        Ok(exe.clone())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// SHA-256 (FIPS 180-4).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    let mut w = [0u32; 64];
    for chunk in msg.chunks(64) {
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

/// `resc-desk update [check]`: the same from a terminal.
pub fn cli(args: &[String]) -> i32 {
    println!("установлена версия {} ({})", BUILD, pretty(BUILD));
    let rel = match check() {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("обновлений нет");
            return 0;
        }
        Err(e) => {
            eprintln!("проверка не удалась: {e}");
            return 1;
        }
    };
    println!("есть новая версия {} ({})", rel.tag, pretty(&rel.tag));
    if !rel.notes.trim().is_empty() {
        println!("{}", rel.notes.trim());
    }
    if args.first().map(String::as_str) == Some("check") {
        return 0;
    }
    match install(&rel) {
        Ok(p) => {
            println!("установлено в {}; перезапуск: resc-desk ctl restart", p.display());
            0
        }
        Err(e) => {
            eprintln!("не установилось: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answers() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha256(long)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        assert_eq!(hex(&sha256(&[b'a'; 1000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    #[test]
    fn github_answer_is_read() {
        let json = r#"{"url":"x","tag_name": "r1764090000-1a2b3c4","name":"r1764090000-1a2b3c4",
            "body":"- Мониторы\n- \"стекло\" é","assets":[
            {"name":"resc-desk-x86_64.sha256","browser_download_url":"https://github.com/Fitertom/resc-desk/releases/download/r1/resc-desk-x86_64.sha256"},
            {"name":"resc-desk-x86_64","browser_download_url":"https://github.com/Fitertom/resc-desk/releases/download/r1/resc-desk-x86_64"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.tag, "r1764090000-1a2b3c4");
        assert_eq!(r.notes, "- Мониторы\n- \"стекло\" é");
        assert!(r.url.ends_with("/resc-desk-x86_64") && r.sha_url.ends_with(".sha256"));
        assert_eq!(build_time(&r.tag), Some(1764090000));
        assert_eq!(build_time("dev"), None);
        // No program in it: not a release for us.
        assert_eq!(parse_release(r#"{"tag_name":"r1-a","assets":[]}"#), None);
    }

    #[test]
    fn versions_read_as_dates() {
        assert_eq!(pretty("r1764090000-1a2b3c4"), "25.11.2025 17:00 UTC · 1a2b3c4");
        assert_eq!(pretty("dev"), "dev");
    }
}
