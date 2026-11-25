//! Local wall clock without libc: UTC from SystemTime + offset from /etc/localtime (TZif).
use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static OFFSET: Cell<(i64, i64)> = const { Cell::new((i64::MIN, 0)) }; // (valid_until, utoff)
}

fn tz_offset(now: i64) -> i64 {
    OFFSET.with(|c| {
        let (until, off) = c.get();
        if now < until {
            return off;
        }
        let off = read_tzif(now).unwrap_or(0);
        c.set((now + 3600, off));
        off
    })
}

fn read_tzif(now: i64) -> Option<i64> {
    let d = std::fs::read("/etc/localtime").ok()?;
    if d.get(0..4)? != b"TZif" {
        return None;
    }
    let be = |o: usize| -> Option<u32> { Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?)) };
    let (isut, isstd, leap, timecnt, typecnt, charcnt) =
        (be(20)? as usize, be(24)? as usize, be(28)? as usize, be(32)? as usize, be(36)? as usize, be(40)? as usize);
    let version = d[4];
    // Prefer the 64-bit v2 block when present.
    let (base, tsize, timecnt, typecnt) = if version >= b'2' {
        let v1len = timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut;
        let h2 = 44 + v1len;
        if d.get(h2..h2 + 4)? != b"TZif" {
            return None;
        }
        let be2 = |o: usize| -> Option<u32> { Some(u32::from_be_bytes(d.get(h2 + o..h2 + o + 4)?.try_into().ok()?)) };
        (h2 + 44, 8usize, be2(32)? as usize, be2(36)? as usize)
    } else {
        (44, 4usize, timecnt, typecnt)
    };
    let times = base;
    let idxs = times + timecnt * tsize;
    let types = idxs + timecnt;
    let mut ti = 0usize;
    // First non-DST type as default.
    for t in 0..typecnt {
        if *d.get(types + t * 6 + 4)? == 0 {
            ti = t;
            break;
        }
    }
    for i in 0..timecnt {
        let o = times + i * tsize;
        let t = if tsize == 8 {
            i64::from_be_bytes(d.get(o..o + 8)?.try_into().ok()?)
        } else {
            i32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?) as i64
        };
        if t > now {
            break;
        }
        ti = *d.get(idxs + i)? as usize;
    }
    let o = types + ti * 6;
    Some(i32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?) as i64)
}

/// (hour, minute, day, month, year) in local time.
pub fn local_now() -> (u32, u32, u32, u32, i32) {
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let t = utc + tz_offset(utc);
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (if month <= 2 { y + 1 } else { y }) as i32;
    ((secs / 3600) as u32, ((secs % 3600) / 60) as u32, day, month, year)
}

/// Seconds until the next minute boundary (taskbar clock refresh).
pub fn secs_to_next_minute() -> u64 {
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    60 - utc % 60
}
