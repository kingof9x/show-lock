//! Time and date text for the lock screen, in the user's own Windows format
//! (12/24-hour, language, date order).

/// (time, date), e.g. ("14:05", "Saturday, 21 November 2026").
#[cfg(windows)]
pub fn now_strings() -> (String, String) {
    use windows_sys::Win32::Globalization::{
        GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE, TIME_NOSECONDS,
    };
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut st = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    let mut buf = [0u16; 128];
    let n = unsafe {
        GetTimeFormatEx(
            std::ptr::null(),
            TIME_NOSECONDS,
            &st,
            std::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    let time = if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        format!("{:02}:{:02}", st.wHour, st.wMinute)
    };
    let n = unsafe {
        GetDateFormatEx(
            std::ptr::null(),
            DATE_LONGDATE,
            &st,
            std::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            std::ptr::null(),
        )
    };
    let date = if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        String::new()
    };
    (time, date)
}

/// Preview builds on other systems show UTC; only used during development.
#[cfg(not(windows))]
pub fn now_strings() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const DAYS: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    (
        format!("{h:02}:{m:02}"),
        format!(
            "{}, {d} {} {y}",
            DAYS[days.rem_euclid(7) as usize],
            MONTHS[mo as usize - 1]
        ),
    )
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(20_778), (2026, 11, 21));
    }
}
