use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::os::{FileTimes, TimeChange};
use crate::prelude::*;

const HELP: &str = "Usage: touch [OPTION]... FILE...
Set the access and modification times of each FILE to the current time.
Create each FILE that does not exist as an empty file.

Options:
  -a                    change only the access time
  -m                    change only the modification time
  -c, --no-create       do not create a missing FILE
  -d, --date=DATE       use DATE instead of the current time
  -r, --reference=REF   use the times of REF instead of the current time
  -t STAMP              use [[CC]YY]MMDDhhmm[.ss] instead of the current time

DATE is ISO 8601: YYYY-MM-DD, then optionally T or a space and hh:mm[:ss[.frac]],
then optionally Z or +hh:mm. DATE can also be @SECONDS since the epoch.
A time without a zone is UTC, as in date.";

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// Where the new times come from, when not from the clock.
enum Source {
    /// One time for both the access and the modification time.
    Time(SystemTime),
    /// The times of this file.
    Reference(String),
}

#[command("touch")]
async fn cmd_touch(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut access = false;
    let mut modification = false;
    let mut no_create = false;
    let mut sources = Vec::new();
    let mut files = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('a') => access = true,
            Short('m') => modification = true,
            Short('c') | Long("no-create") => no_create = true,
            Short('d') | Long("date") => {
                let date = parser.value()?.string()?;
                sources.push(Source::Time(parse_date(&date)?));
            }
            Short('t') => {
                let stamp = parser.value()?.string()?;
                sources.push(Source::Time(parse_stamp(&stamp, os.now())?));
            }
            Short('r') | Long("reference") => {
                sources.push(Source::Reference(parser.value()?.string()?));
            }
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => files.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if sources.len() > 1 {
        return Err("cannot specify times from more than one source".into());
    }
    if files.is_empty() {
        return Err("missing file operand".into());
    }
    let explicit = !sources.is_empty();
    let (accessed, modified) = match sources.pop() {
        None => (TimeChange::Now, TimeChange::Now),
        Some(Source::Time(time)) => (TimeChange::To(time), TimeChange::To(time)),
        Some(Source::Reference(reference)) => reference_times(os, &reference).await?,
    };
    // Neither -a nor -m changes both times, as both together do.
    let both = access == modification;
    let times = FileTimes {
        accessed: if both || access {
            accessed
        } else {
            TimeChange::Keep
        },
        modified: if both || modification {
            modified
        } else {
            TimeChange::Keep
        },
    };
    let mut code = 0;
    for path in &files {
        if let Err(error) = touch_one(os, path, times, explicit, no_create).await {
            let mut ew = io::stderr()?;
            wprintln!(ew, "touch: {}: {}", path, error)?;
            code = 1;
        }
    }
    Ok(code)
}

/// Create `path` when it is missing, then set its times.
async fn touch_one(
    os: &dyn Kernel,
    path: &str,
    times: FileTimes,
    explicit: bool,
    no_create: bool,
) -> std::io::Result<()> {
    if !io::stat(os, path).await.exists {
        if no_create {
            return Ok(());
        }
        // Append does not truncate, so a file that another writer made since the check keeps its
        // content, and closing the descriptor writes nothing that changes the new times.
        let fd = io::open(os, path, OpenFlags::append()).await?;
        io::with_process(|p| p.close(fd));
        if !explicit {
            return Ok(());
        }
        // A host file is written once its writer closes, so wait for it before setting its times.
        let failures = io::settle_host_writes().await;
        if !failures.is_empty() {
            return Err(std::io::Error::other(failures.join("; ")));
        }
    }
    io::set_times(os, path, times).await
}

/// The access and modification times of `reference`.
async fn reference_times(
    os: &dyn Kernel,
    reference: &str,
) -> Result<(TimeChange, TimeChange), Failure> {
    let stat = io::stat(os, reference).await;
    if !stat.exists {
        return Err(format!("{reference}: No such file or directory").into());
    }
    let modified = stat
        .modified
        .ok_or_else(|| format!("{reference}: the modification time is not available"))?;
    // A store that records no access time reports the modification time in its place.
    let accessed = stat.accessed.unwrap_or(modified);
    Ok((TimeChange::To(accessed), TimeChange::To(modified)))
}

/// Parse `-t [[CC]YY]MMDDhhmm[.ss]` as UTC; a missing year is the current year.
fn parse_stamp(stamp: &str, now: SystemTime) -> Result<SystemTime, Failure> {
    let invalid = || -> Failure { format!("invalid date format '{stamp}'").into() };
    let (digits, second) = match stamp.split_once('.') {
        Some((digits, second)) if second.len() == 2 => {
            (digits, number(second).ok_or_else(invalid)?)
        }
        Some(_) => return Err(invalid()),
        None => (stamp, 0),
    };
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let (year, rest) = match digits.len() {
        8 => (
            civil_from_days(seconds_since_epoch(now).div_euclid(86_400)).0,
            digits,
        ),
        10 => {
            let short = number(&digits[..2]).ok_or_else(invalid)?;
            // POSIX: 69 to 99 are 1969 to 1999, and 00 to 68 are 2000 to 2068.
            (
                if short >= 69 {
                    1900 + short
                } else {
                    2000 + short
                },
                &digits[2..],
            )
        }
        12 => (number(&digits[..4]).ok_or_else(invalid)?, &digits[4..]),
        _ => return Err(invalid()),
    };
    let field = |index: usize| number(&rest[index..index + 2]);
    let (month, day, hour, minute) = (field(0), field(2), field(4), field(6));
    let (Some(month), Some(day), Some(hour), Some(minute)) = (month, day, hour, minute) else {
        return Err(invalid());
    };
    civil_time(year, month, day, hour, minute, second, 0, 0).ok_or_else(invalid)
}

/// Parse `-d DATE`: ISO 8601 date and time, or `@SECONDS` since the epoch.
fn parse_date(date: &str) -> Result<SystemTime, Failure> {
    let invalid = || -> Failure { format!("invalid date format '{date}'").into() };
    if let Some(seconds) = date.strip_prefix('@') {
        let seconds: i64 = seconds.parse().map_err(|_| invalid())?;
        return from_epoch(seconds, 0).ok_or_else(invalid);
    }
    // Every valid DATE is ASCII, and an ASCII string slices at any byte index.
    if !date.is_ascii() {
        return Err(invalid());
    }
    let bytes = date.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid());
    }
    let year = number(&date[..4]).ok_or_else(invalid)?;
    let month = number(&date[5..7]).ok_or_else(invalid)?;
    let day = number(&date[8..10]).ok_or_else(invalid)?;
    let rest = &date[10..];
    if rest.is_empty() {
        return civil_time(year, month, day, 0, 0, 0, 0, 0).ok_or_else(invalid);
    }
    let rest = rest.strip_prefix(['T', 't', ' ']).ok_or_else(invalid)?;
    let (clock, offset) = split_zone(rest).ok_or_else(invalid)?;
    let mut parts = clock.splitn(3, ':');
    let hour = parts.next().and_then(two_digits).ok_or_else(invalid)?;
    let minute = parts.next().and_then(two_digits).ok_or_else(invalid)?;
    let (second, nanosecond) = match parts.next() {
        None => (0, 0),
        Some(seconds) => {
            let (whole, fraction) = match seconds.split_once(['.', ',']) {
                Some((whole, fraction)) => (whole, Some(fraction)),
                None => (seconds, None),
            };
            let second = two_digits(whole).ok_or_else(invalid)?;
            let nanosecond = match fraction {
                None => 0,
                Some(fraction) => nanoseconds(fraction).ok_or_else(invalid)?,
            };
            (second, nanosecond)
        }
    };
    civil_time(year, month, day, hour, minute, second, nanosecond, offset).ok_or_else(invalid)
}

/// Split a trailing `Z`, `+hh`, `+hhmm`, or `+hh:mm` zone from a clock time, as seconds east of UTC.
fn split_zone(clock: &str) -> Option<(&str, i64)> {
    if let Some(clock) = clock.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let Some(at) = clock.rfind(['+', '-']) else {
        return Some((clock, 0));
    };
    let (clock, zone) = clock.split_at(at);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let zone = &zone[1..];
    let (hours, minutes) = match zone.len() {
        2 => (two_digits(zone)?, 0),
        4 => (two_digits(&zone[..2])?, two_digits(&zone[2..])?),
        5 if zone.as_bytes()[2] == b':' => (two_digits(&zone[..2])?, two_digits(&zone[3..])?),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((clock, sign * (hours * 3600 + minutes * 60)))
}

/// The value of an all-digit string.
fn number(digits: &str) -> Option<i64> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The value of exactly two digits.
fn two_digits(digits: &str) -> Option<i64> {
    if digits.len() == 2 {
        number(digits)
    } else {
        None
    }
}

/// The nanoseconds a decimal fraction of a second names; digits past the ninth are dropped.
fn nanoseconds(fraction: &str) -> Option<u32> {
    number(fraction)?;
    let kept: String = fraction.chars().take(9).collect();
    let scale = 10u32.pow(9 - kept.len() as u32);
    kept.parse::<u32>().ok().map(|value| value * scale)
}

/// The time a calendar date and clock time name, at `offset` seconds east of UTC.
///
/// A second of 60 is a leap second, so it names the first second of the next minute.
#[allow(clippy::too_many_arguments)]
fn civil_time(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    nanosecond: u32,
    offset: i64,
) -> Option<SystemTime> {
    let valid = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=60).contains(&second);
    if !valid {
        return None;
    }
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    from_epoch(seconds, nanosecond)
}

/// The time `seconds` and `nanosecond` after the epoch; `seconds` may be negative.
fn from_epoch(seconds: i64, nanosecond: u32) -> Option<SystemTime> {
    let base = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds.unsigned_abs()))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }?;
    base.checked_add(Duration::from_nanos(u64::from(nanosecond)))
}

/// Whole seconds since the epoch, negative before it.
fn seconds_since_epoch(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian year, month, and day of a day count from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> SystemTime {
        from_epoch(seconds, 0).expect("in range")
    }

    #[test]
    fn a_stamp_with_every_field_is_utc() {
        // 2020-01-01T00:00:00Z.
        let now = at(0);
        assert_eq!(parse_stamp("202001010000", now).unwrap(), at(1_577_836_800));
        assert_eq!(
            parse_stamp("202001010000.30", now).unwrap(),
            at(1_577_836_830)
        );
        assert_eq!(parse_stamp("2001010000", now).unwrap(), at(1_577_836_800));
        // 1969-01-01T00:00:00Z.
        assert_eq!(parse_stamp("6901010000", now).unwrap(), at(-31_536_000));
    }

    #[test]
    fn a_stamp_without_a_year_takes_the_current_year() {
        // now is 2024-06-15; the stamp names March 1 of that year.
        let now = at(1_718_409_600);
        assert_eq!(parse_stamp("03010000", now).unwrap(), at(1_709_251_200));
    }

    #[test]
    fn an_invalid_stamp_is_refused() {
        let now = at(0);
        for stamp in [
            "2020010100",
            "20201301",
            "202013010000",
            "202002300000",
            "202001010000.3",
            "2020o1010000",
            "",
        ] {
            assert!(parse_stamp(stamp, now).is_err(), "{stamp:?}");
        }
    }

    #[test]
    fn an_iso_date_is_parsed_with_its_zone() {
        assert_eq!(parse_date("2020-01-01").unwrap(), at(1_577_836_800));
        assert_eq!(
            parse_date("2020-01-01T00:00:30").unwrap(),
            at(1_577_836_830)
        );
        assert_eq!(parse_date("2020-01-01 01:00Z").unwrap(), at(1_577_840_400));
        assert_eq!(
            parse_date("2020-01-01T01:00:00+01:00").unwrap(),
            at(1_577_836_800)
        );
        assert_eq!(
            parse_date("2019-12-31T23:00:00-0100").unwrap(),
            at(1_577_836_800)
        );
        assert_eq!(
            parse_date("2020-01-01T00:00:00.5").unwrap(),
            from_epoch(1_577_836_800, 500_000_000).unwrap()
        );
        assert_eq!(parse_date("@1577836800").unwrap(), at(1_577_836_800));
        assert_eq!(parse_date("@-1").unwrap(), at(-1));
    }

    #[test]
    fn an_invalid_date_is_refused() {
        for date in [
            "2020-1-01",
            "2020-02-30",
            "2020-01-01T24:00",
            "2020-01-01T00",
            "2020-01-01X00:00",
            "@x",
            "yesterday",
            "2020-01-1é",
            "2020-01-01T00:00+1é2",
            "2020-01-01T00:00:0é",
        ] {
            assert!(parse_date(date).is_err(), "{date:?}");
        }
    }

    #[test]
    fn civil_days_round_trip() {
        for days in [-719_468, -1, 0, 59, 10_957, 18_262, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(
                days_from_civil(year, month, day),
                days,
                "{year}-{month}-{day}"
            );
        }
    }
}
