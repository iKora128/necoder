//! date — ローカルの日付。フォルダ名（`2026-09-18 …`）とプロンプトの「今日」に使う。
//!
//! 依存を足さないために OS の API を直接引く。UTC で済ませると、日本の朝 9 時までに作った
//! チャットが前日のフォルダに入る（Finder で日付順に探す時に 1 日ずれる）。

/// 年月日。`Display` は `YYYY-MM-DD`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl std::fmt::Display for Date {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

impl Date {
    /// 今日（ローカル時刻）。
    pub fn today() -> Date {
        local_now().0
    }

    /// UTC の Unix 秒から（ローカル時刻を引けなかった時の退避と、テスト用）。
    pub fn from_unix_utc(seconds: i64) -> Date {
        civil_from_days(seconds.div_euclid(86_400))
    }
}

/// 一覧の日付グループ（`docs/CHAT.md` §4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Recency {
    Today,
    Yesterday,
    /// 2〜7 日前。
    ThisWeek,
    Older,
}

impl Date {
    /// 1970-01-01 からの日数（`civil_from_days` の逆）。
    pub fn days_from_epoch(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month = i64::from(self.month);
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5
            + i64::from(self.day)
            - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// Unix ミリ秒をローカルの日付へ。`offset_seconds` は [`local_offset_seconds`]。
    pub fn from_unix_ms_local(milliseconds: i64, offset_seconds: i64) -> Date {
        Date::from_unix_utc(milliseconds.div_euclid(1000) + offset_seconds)
    }

    /// `today` から見た `self` の新しさ。未来の日付（時計のずれ）は今日に寄せる。
    pub fn recency(self, today: Date) -> Recency {
        match today.days_from_epoch() - self.days_from_epoch() {
            age if age <= 0 => Recency::Today,
            1 => Recency::Yesterday,
            2..=7 => Recency::ThisWeek,
            _ => Recency::Older,
        }
    }
}

/// ローカル時刻と UTC の差（秒）。夏時間の切り替わりをまたぐ過去の時刻では 1 時間ずれうるが、
/// 用途は一覧の日付グループなので許容する（依存を足さない方を取る）。
pub fn local_offset_seconds() -> i64 {
    let utc_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let (date, hour, minute) = local_now();
    let local_minutes = date.days_from_epoch() * 1440 + i64::from(hour) * 60 + i64::from(minute);
    (local_minutes - utc_seconds.div_euclid(60)) * 60
}

/// 今のローカル時刻の `(日付, 時, 分)`。
pub fn local_now() -> (Date, u32, u32) {
    platform_local_now().unwrap_or_else(|| {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0);
        let day_seconds = seconds.rem_euclid(86_400) as u32;
        (
            Date::from_unix_utc(seconds),
            day_seconds / 3600,
            day_seconds % 3600 / 60,
        )
    })
}

#[cfg(unix)]
fn platform_local_now() -> Option<(Date, u32, u32)> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as libc::time_t;
    // SAFETY: `localtime_r` は渡した `tm` にだけ書く再入可能版。`tm` は全フィールドが整数と
    // ポインタなのでゼロ初期化で有効な値になる。失敗時は null を返すのでそれを見る。
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some((
        Date {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
        },
        tm.tm_hour as u32,
        tm.tm_min as u32,
    ))
}

#[cfg(windows)]
fn platform_local_now() -> Option<(Date, u32, u32)> {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: `GetLocalTime` は渡した `SYSTEMTIME` に書くだけで失敗しない。全フィールドが u16。
    let now = unsafe {
        let mut now = std::mem::zeroed();
        GetLocalTime(&mut now);
        now
    };
    Some((
        Date {
            year: now.wYear as i32,
            month: now.wMonth as u32,
            day: now.wDay as u32,
        },
        now.wHour as u32,
        now.wMinute as u32,
    ))
}

#[cfg(not(any(unix, windows)))]
fn platform_local_now() -> Option<(Date, u32, u32)> {
    None
}

/// 1970-01-01 からの日数 → 年月日（Howard Hinnant の `civil_from_days`。閏年を表なしで扱える）。
fn civil_from_days(days: i64) -> Date {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Date {
        year: year as i32,
        month: month as u32,
        day: day as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_seconds_become_calendar_dates() {
        assert_eq!(Date::from_unix_utc(0).to_string(), "1970-01-01");
        // 2026-09-18 00:00:00 UTC
        assert_eq!(Date::from_unix_utc(1_789_689_600).to_string(), "2026-09-18");
        // 閏日と、その翌日
        assert_eq!(Date::from_unix_utc(1_709_164_800).to_string(), "2024-02-29");
        assert_eq!(Date::from_unix_utc(1_709_251_200).to_string(), "2024-03-01");
        // 1970 より前でも壊れない
        assert_eq!(Date::from_unix_utc(-1).to_string(), "1969-12-31");
    }

    #[test]
    fn days_from_epoch_inverts_the_calendar_conversion() {
        for seconds in [0_i64, 1_789_689_600, 1_709_164_800, -86_400, 4_102_444_800] {
            let date = Date::from_unix_utc(seconds);
            assert_eq!(date.days_from_epoch(), seconds.div_euclid(86_400), "{date}");
        }
    }

    #[test]
    fn recency_groups_the_list_by_whole_days() {
        let today = Date {
            year: 2026,
            month: 9,
            day: 18,
        };
        let day = |day| Date {
            year: 2026,
            month: 9,
            day,
        };
        assert_eq!(day(18).recency(today), Recency::Today);
        assert_eq!(day(17).recency(today), Recency::Yesterday);
        assert_eq!(day(16).recency(today), Recency::ThisWeek);
        assert_eq!(day(11).recency(today), Recency::ThisWeek);
        assert_eq!(day(10).recency(today), Recency::Older);
        // 月をまたいでも日数で数える。時計がずれて未来になっていても今日に寄せる。
        assert_eq!(
            Date {
                year: 2026,
                month: 8,
                day: 31
            }
            .recency(Date {
                year: 2026,
                month: 9,
                day: 1
            }),
            Recency::Yesterday
        );
        assert_eq!(day(19).recency(today), Recency::Today);
    }

    #[test]
    fn the_local_offset_is_a_whole_number_of_minutes_within_a_day() {
        let offset = local_offset_seconds();
        assert_eq!(offset % 60, 0);
        assert!(offset.abs() <= 14 * 3600 + 60, "{offset}");
    }

    #[test]
    fn today_is_a_plausible_date() {
        let (today, hour, minute) = local_now();
        assert!(today.year >= 2026, "{today}");
        assert!((1..=12).contains(&today.month) && (1..=31).contains(&today.day));
        assert!(hour < 24 && minute < 60);
    }
}
