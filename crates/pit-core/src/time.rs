//! Time as Pitlane writes it: Unix seconds, UTC timestamps and the local day, without a date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, in Unix seconds (0 if the clock is before 1970).
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// For `skip_serializing_if`: counters left out of a file while they are zero.
pub(crate) fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The day `secs` falls on where you are (`2026-11-20`): a decision taken at 22:00 in São
/// Paulo is dated that day, not the next one as in UTC.
pub fn local_day(secs: u64) -> String {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed tm we own and reads only `t`.
    let offset = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff }
    };
    let local = (secs as i64 + offset).max(0) as u64;
    timestamp(local)[..10].to_owned()
}

/// Unix seconds to `2026-11-20T14:03:00Z` (UTC), without a date crate.
pub fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
