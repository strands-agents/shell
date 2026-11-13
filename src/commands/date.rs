use crate::prelude::*;

const HELP: &str = "Usage: date [-u] [-d STRING] [+FORMAT]
Display the current date and time, or the time described by STRING.

Format specifiers:
  %Y  year (4 digits)
  %y  year (2 digits)
  %m  month (01-12)
  %d  day (01-31)
  %e  day, space padded ( 1-31)
  %j  day of year (001-366)
  %H  hour (00-23)
  %I  hour (01-12)
  %p  AM or PM
  %M  minute (00-59)
  %S  second (00-59)
  %N  nanoseconds (000000000-999999999)
  %s  seconds since 1970-01-01 00:00:00 UTC
  %a  weekday name (Sun-Sat)
  %A  full weekday name (Sunday-Saturday)
  %u  day of week (1-7, Monday is 1)
  %w  day of week (0-6, Sunday is 0)
  %b  month name (Jan-Dec)
  %B  full month name (January-December)
  %F  same as %Y-%m-%d
  %D  same as %m/%d/%y
  %T  same as %H:%M:%S
  %R  same as %H:%M
  %z  numeric time zone (+0000)
  %Z  time zone name (UTC)
  %c  default format
  %n  newline
  %t  tab
  %%  a literal %

Date strings for -d:
  @SECONDS                     seconds since the epoch, e.g. @0 or @1.5
  YYYY-MM-DD                   midnight on that day
  YYYY-MM-DD HH:MM[:SS[.N]]    also with T between date and time; a
                               time may end in Z, UTC, +HH, +HHMM or
                               +HH:MM, otherwise it is UTC
  HH:MM[:SS]                   that time today
  now, today, yesterday, tomorrow
  [+-][N] UNIT [ago]           relative offset; UNIT is year, month,
                               fortnight, week, day, hour, min[ute] or
                               sec[ond], optionally plural

Options:
  -d, --date=STRING  display the time described by STRING, not now
  -u                 use UTC time
  -h                 display this help";

#[command("date")]
async fn cmd_date(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut _utc = false;
    let mut format = String::new();
    let mut date = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('d') | Long("date") => date = Some(parser.value()?.string()?),
            Short('u') => _utc = true,
            Value(val) => {
                let s = val.string()?;
                if let Some(fmt) = s.strip_prefix('+') {
                    format = fmt.to_string();
                } else {
                    return Err(format!("date: invalid argument: {}", s).into());
                }
            }
            _ => return Err(arg.unexpected().into()),
        }
    }

    let since_epoch = os
        .now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let now = (since_epoch.as_secs() as i64, since_epoch.subsec_nanos());
    let (secs, nanos) = match date {
        Some(s) => parse_date(&s, now).ok_or_else(|| format!("invalid date '{}'", s))?,
        None => now,
    };
    let fmt = if format.is_empty() { "%c" } else { &format };
    let result = format_date(fmt, secs, nanos);

    let mut w = io::stdout()?;
    wprintln!(w, "{}", result)?;
    Ok(0)
}

/// Expands the conversion specifiers in `fmt` for the UTC instant `ts`
/// seconds plus `nanos` nanoseconds after the Unix epoch, in a single pass
/// so expanded text is never re-scanned. Like GNU `date`, an unknown
/// specifier or a trailing `%` is copied through literally.
fn format_date(fmt: &str, ts: i64, nanos: u32) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let hour = secs / 3600;
    let min = (secs % 3600) / 60;
    let sec = secs % 60;
    let (year, month, day) = civil_from_days(days);
    let yday = days - days_from_civil(year, 1, 1) + 1;

    let wday = (days + 4).rem_euclid(7) as usize;
    let wday_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let wday_full_names = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let month_names = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_full_names = [
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
    let wday_name = wday_names[wday];
    let month_name = month_names[(month - 1) as usize];
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };

    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'a' => out.push_str(wday_name),
            'A' => out.push_str(wday_full_names[wday]),
            'b' => out.push_str(month_name),
            'B' => out.push_str(month_full_names[(month - 1) as usize]),
            'c' => out.push_str(&format!(
                "{} {} {:2} {:02}:{:02}:{:02} UTC {}",
                wday_name, month_name, day, hour, min, sec, year
            )),
            'd' => out.push_str(&format!("{:02}", day)),
            'D' => out.push_str(&format!("{:02}/{:02}/{:02}", month, day, year % 100)),
            'e' => out.push_str(&format!("{:2}", day)),
            'F' => out.push_str(&format!("{:04}-{:02}-{:02}", year, month, day)),
            'H' => out.push_str(&format!("{:02}", hour)),
            'I' => out.push_str(&format!("{:02}", hour12)),
            'j' => out.push_str(&format!("{:03}", yday)),
            'm' => out.push_str(&format!("{:02}", month)),
            'M' => out.push_str(&format!("{:02}", min)),
            'n' => out.push('\n'),
            'N' => out.push_str(&format!("{:09}", nanos)),
            'p' => out.push_str(if hour < 12 { "AM" } else { "PM" }),
            'R' => out.push_str(&format!("{:02}:{:02}", hour, min)),
            's' => out.push_str(&ts.to_string()),
            'S' => out.push_str(&format!("{:02}", sec)),
            't' => out.push('\t'),
            'T' => out.push_str(&format!("{:02}:{:02}:{:02}", hour, min, sec)),
            'u' => out.push_str(&(if wday == 0 { 7 } else { wday }).to_string()),
            'w' => out.push_str(&wday.to_string()),
            'y' => out.push_str(&format!("{:02}", year % 100)),
            'Y' => out.push_str(&format!("{:04}", year)),
            'z' => out.push_str("+0000"),
            'Z' => out.push_str("UTC"),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

/// Relative offsets accumulate in three buckets because GNU `date` applies
/// years and months to the calendar fields, days to the day count, and
/// everything shorter as plain seconds.
#[derive(Clone, Copy)]
enum Rel {
    Months,
    Days,
    Secs,
}

/// Maps a relative-offset unit word to its bucket and multiplier.
fn unit(word: &str) -> Option<(Rel, i64)> {
    let word = word.strip_suffix('s').unwrap_or(word);
    Some(match word {
        "year" => (Rel::Months, 12),
        "month" => (Rel::Months, 1),
        "fortnight" => (Rel::Days, 14),
        "week" => (Rel::Days, 7),
        "day" => (Rel::Days, 1),
        "hour" => (Rel::Secs, 3600),
        "minute" | "min" => (Rel::Secs, 60),
        "second" | "sec" => (Rel::Secs, 1),
        _ => return None,
    })
}

/// A byte cursor over a lowercased date string.
struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Cursor {
            b: s.as_bytes(),
            i: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.b.get(self.i + ahead).copied()
    }

    /// The first byte after the run of digits at the cursor.
    fn after_digits(&self) -> Option<u8> {
        self.b[self.i..]
            .iter()
            .copied()
            .find(|c| !c.is_ascii_digit())
    }

    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == Some(c);
        if hit {
            self.i += 1;
        }
        hit
    }

    /// Consumes an optional sign and returns it as 1 or -1.
    fn sign(&mut self) -> i64 {
        if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn take_while(&mut self, f: impl Fn(u8) -> bool) -> &'a str {
        let start = self.i;
        while self.peek().is_some_and(&f) {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i]).unwrap_or_default()
    }

    /// Reads one to `max` digits as a number.
    fn number(&mut self, max: usize) -> Option<i64> {
        let digits = self.take_while(|c| c.is_ascii_digit());
        if digits.is_empty() || digits.len() > max {
            return None;
        }
        digits.parse().ok()
    }

    /// Reads a run of digits as nanoseconds, ignoring digits past the ninth.
    fn fraction(&mut self) -> u32 {
        let digits = self.take_while(|c| c.is_ascii_digit()).as_bytes();
        (0..9).fold(0, |nanos, i| {
            nanos * 10 + digits.get(i).map_or(0, |c| u32::from(c - b'0'))
        })
    }

    /// Reads `YYYY-MM-DD`, rejecting days the month does not have. Like GNU,
    /// a two-digit year is 1969-2068.
    fn date(&mut self) -> Option<(i64, i64, i64)> {
        let start = self.i;
        let mut year = self.number(9)?;
        if self.i - start == 2 {
            year += if year < 69 { 2000 } else { 1900 };
        }
        self.eat(b'-');
        let month = self.number(2)?;
        if !self.eat(b'-') {
            return None;
        }
        let day = self.number(2)?;
        let valid = (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
        valid.then_some((year, month, day))
    }

    /// Reads `H:MM[:SS[.N]]` as seconds into the day plus nanoseconds.
    fn time(&mut self) -> Option<(i64, u32)> {
        let hour = self.number(2)?;
        if !self.eat(b':') {
            return None;
        }
        let min = self.number(2)?;
        let (mut sec, mut nanos) = (0, 0);
        if self.eat(b':') {
            sec = self.number(2)?;
            if self.eat(b'.') {
                nanos = self.fraction();
            }
        }
        let valid = hour < 24 && min < 60 && sec < 60;
        valid.then_some((hour * 3600 + min * 60 + sec, nanos))
    }

    /// Reads a numeric zone `+H`, `+HH`, `+HHMM` or `+HH:MM` as seconds
    /// east of UTC.
    fn zone(&mut self) -> Option<i64> {
        let sign = self.sign();
        let digits = self.take_while(|c| c.is_ascii_digit());
        let n: i64 = digits.parse().ok()?;
        let (hours, mins) = match digits.len() {
            1 | 2 if self.eat(b':') => (n, self.number(2)?),
            1 | 2 => (n, 0),
            3 | 4 => (n / 100, n % 100),
            _ => return None,
        };
        let valid = hours <= 24 && mins < 60;
        valid.then_some(sign * (hours * 3600 + mins * 60))
    }
}

/// Parses `SECONDS[.FRACTION]` after the `@`, borrowing a second for a
/// negative fraction so the nanoseconds stay non-negative.
fn parse_epoch(s: &str) -> Option<(i64, u32)> {
    let mut cur = Cursor::new(s);
    let sign = cur.sign();
    let mut secs = cur.number(18)? * sign;
    let mut nanos = if cur.eat(b'.') { cur.fraction() } else { 0 };
    if cur.peek().is_some() {
        return None;
    }
    if sign < 0 && nanos > 0 {
        secs -= 1;
        nanos = 1_000_000_000 - nanos;
    }
    Some((secs, nanos))
}

/// Parses a `-d` date string into seconds and nanoseconds since the epoch,
/// relative to `now`. Accepts the GNU `date` forms listed in the help text;
/// a time without an explicit zone is UTC.
fn parse_date(input: &str, now: (i64, u32)) -> Option<(i64, u32)> {
    let s = input.trim().to_ascii_lowercase();
    if let Some(epoch) = s.strip_prefix('@') {
        return parse_epoch(epoch.trim_start());
    }
    let mut cur = Cursor::new(&s);
    let mut date = None;
    let mut time = None;
    let mut zone = None;
    let mut rel = [0i64; 3];
    // The last relative item, which a following `ago` negates.
    let mut last: Option<(Rel, i64)> = None;
    let mut after_time = false;
    loop {
        cur.skip_ws();
        let Some(c) = cur.peek() else { break };
        let follows_time = std::mem::take(&mut after_time);
        let mut item = None;
        if c.is_ascii_alphabetic() {
            match cur.take_while(|c| c.is_ascii_alphabetic()) {
                // Like GNU, these are not relative items `ago` can negate.
                "now" | "today" => {}
                "yesterday" => rel[Rel::Days as usize] = rel[Rel::Days as usize].checked_sub(1)?,
                "tomorrow" => rel[Rel::Days as usize] = rel[Rel::Days as usize].checked_add(1)?,
                "ago" => {
                    let (k, n) = last.take()?;
                    rel[k as usize] = rel[k as usize].checked_sub(n.checked_mul(2)?)?;
                    continue;
                }
                "z" | "utc" | "gmt" => {
                    if zone.replace(0).is_some() {
                        return None;
                    }
                }
                word => item = Some(unit(word)?),
            }
        } else if (c == b'+' || c == b'-') && follows_time {
            // Like GNU, a signed number right after a time is its zone.
            if zone.replace(cur.zone()?).is_some() {
                return None;
            }
        } else if c.is_ascii_digit() && cur.after_digits() == Some(b'-') {
            if date.replace(cur.date()?).is_some() {
                return None;
            }
            if cur.peek() == Some(b't') && cur.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                cur.i += 1;
                if time.replace(cur.time()?).is_some() {
                    return None;
                }
                after_time = true;
            }
        } else if c.is_ascii_digit() && cur.after_digits() == Some(b':') {
            if time.replace(cur.time()?).is_some() {
                return None;
            }
            after_time = true;
        } else if c.is_ascii_digit() || c == b'+' || c == b'-' {
            let sign = cur.sign();
            let n = cur.number(18)? * sign;
            cur.skip_ws();
            let (k, f) = unit(cur.take_while(|c| c.is_ascii_alphabetic()))?;
            item = Some((k, n.checked_mul(f)?));
        } else {
            return None;
        }
        if let Some((k, n)) = item {
            rel[k as usize] = rel[k as usize].checked_add(n)?;
        }
        last = item;
    }

    let (now_secs, now_nanos) = now;
    let (year, month, day) = date.unwrap_or_else(|| civil_from_days(now_secs.div_euclid(86400)));
    let (sod, nanos) = match time {
        Some(t) => t,
        None if date.is_some() || s.is_empty() => (0, 0),
        None => (now_secs.rem_euclid(86400), now_nanos),
    };
    let months = (year * 12 + month - 1).checked_add(rel[Rel::Months as usize])?;
    let year = months.div_euclid(12);
    if year.abs() > 1_000_000_000 {
        return None;
    }
    let days = days_from_civil(year, months.rem_euclid(12) + 1, 1)
        .checked_add(day - 1)?
        .checked_add(rel[Rel::Days as usize])?;
    let secs = days
        .checked_mul(86400)?
        .checked_add(sod - zone.unwrap_or(0))?
        .checked_add(rel[Rel::Secs as usize])?;
    Some((secs, nanos))
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date, after Howard
/// Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The proleptic Gregorian `(year, month, day)` for a day count since
/// 1970-01-01; the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::{format_date, parse_date};

    // 2025-11-06 11:34:56.123456789 UTC, a Thursday.
    const TS: i64 = 1762428896;
    const NANOS: u32 = 123456789;

    #[test]
    fn expands_every_specifier_like_gnu() {
        // Expected text is GNU coreutils output for
        // `TZ=UTC LC_ALL=C date -d @1762428896.123456789 '+<fmt>'`.
        let fmt =
            "%a|%A|%b|%B|%d|%D|%e|%F|%H|%I|%j|%m|%M|%n|%N|%p|%R|%s|%S|%t|%T|%u|%w|%y|%Y|%z|%Z|%%";
        assert_eq!(
            format_date(fmt, TS, NANOS),
            "Thu|Thursday|Nov|November|06|11/06/25| 6|2025-11-06|11|11|310|11|34|\n\
             |123456789|AM|11:34|1762428896|56|\t|11:34:56|4|4|25|2025|+0000|UTC|%"
        );
    }

    #[test]
    fn default_format_is_unchanged() {
        assert_eq!(format_date("%c", TS, NANOS), "Thu Nov  6 11:34:56 UTC 2025");
        assert_eq!(format_date("%c", 0, 0), "Thu Jan  1 00:00:00 UTC 1970");
    }

    #[test]
    fn escaped_percent_is_not_reexpanded() {
        assert_eq!(format_date("%%Y %%%Y", TS, 0), "%Y %2025");
    }

    #[test]
    fn unknown_specifier_and_trailing_percent_are_literal() {
        assert_eq!(format_date("%Q %-d end%", TS, 0), "%Q %-d end%");
    }

    #[test]
    fn literal_text_passes_through() {
        assert_eq!(format_date("é %Y ü", TS, 0), "é 2025 ü");
    }

    #[test]
    fn twelve_hour_clock_weekday_and_day_of_year() {
        // Checked against GNU date: the epoch, 2024-12-31 13:00 (day 366 of a
        // leap year) and Sunday 2025-11-09.
        assert_eq!(format_date("%I %p %u %w %j", 0, 0), "12 AM 4 4 001");
        assert_eq!(
            format_date("%I %p %u %w %j", 1735650000, 0),
            "01 PM 2 2 366"
        );
        assert_eq!(format_date("%a %u %w", 1762646400, 0), "Sun 7 0");
    }

    #[test]
    fn formats_instants_before_the_epoch() {
        // GNU: `TZ=UTC date -d @-1 '+%F %T %j %a'` and `-d 1600-02-29`.
        assert_eq!(
            format_date("%F %T %j %a", -1, 0),
            "1969-12-31 23:59:59 365 Wed"
        );
        assert_eq!(
            format_date("%F %j %a", -11670998400, 0),
            "1600-02-29 060 Tue"
        );
    }

    /// Formats `parse_date(s)` relative to `TS` like `+%F %T.%N`.
    fn parsed(s: &str) -> Option<String> {
        parse_date(s, (TS, NANOS)).map(|(secs, nanos)| format_date("%F %T.%N", secs, nanos))
    }

    #[test]
    fn parses_epoch_seconds() {
        // Expected values are GNU `TZ=UTC date -d STRING '+%F %T.%N'`.
        assert_eq!(parsed("@0").unwrap(), "1970-01-01 00:00:00.000000000");
        assert_eq!(parsed("@ 5").unwrap(), "1970-01-01 00:00:05.000000000");
        assert_eq!(
            parsed("@1762428896.123456789").unwrap(),
            "2025-11-06 11:34:56.123456789"
        );
        assert_eq!(parsed("@-1.5").unwrap(), "1969-12-31 23:59:58.500000000");
        for bad in ["@", "@abc", "@1.5.5", "@0 +1 day"] {
            assert_eq!(parsed(bad), None, "{bad}");
        }
    }

    #[test]
    fn parses_iso_dates_and_times() {
        for (s, want) in [
            ("2024-01-15", "2024-01-15 00:00:00.000000000"),
            ("2024-1-5", "2024-01-05 00:00:00.000000000"),
            ("24-01-15", "2024-01-15 00:00:00.000000000"),
            ("68-12-31", "2068-12-31 00:00:00.000000000"),
            ("69-01-01", "1969-01-01 00:00:00.000000000"),
            ("024-01-15", "0024-01-15 00:00:00.000000000"),
            ("2024-02-29", "2024-02-29 00:00:00.000000000"),
            ("1960-06-01", "1960-06-01 00:00:00.000000000"),
            ("2024-01-15 10:30", "2024-01-15 10:30:00.000000000"),
            ("2024-01-15 10:30:45", "2024-01-15 10:30:45.000000000"),
            ("2024-01-15T10:30:45", "2024-01-15 10:30:45.000000000"),
            ("2024-01-15T10:30:45Z", "2024-01-15 10:30:45.000000000"),
            ("2024-01-15 10:30 UTC", "2024-01-15 10:30:00.000000000"),
            ("2024-01-15T10:30:45.123Z", "2024-01-15 10:30:45.123000000"),
            ("2024-01-15T10:30:45+05:30", "2024-01-15 05:00:45.000000000"),
            ("2024-01-15T10:30:45-0800", "2024-01-15 18:30:45.000000000"),
            ("2024-01-15T10:30:45 +02", "2024-01-15 08:30:45.000000000"),
            ("10:30", "2025-11-06 10:30:00.000000000"),
            ("", "2025-11-06 00:00:00.000000000"),
        ] {
            assert_eq!(parsed(s).as_deref(), Some(want), "{s}");
        }
    }

    #[test]
    fn rejects_impossible_dates_and_times() {
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-01-15T25:00",
            "2024-01-15 23:59:60",
            "2024-01-15 2024-01-16",
            "10:00 11:00",
            "xyz",
            "ago",
            "tomorrow ago",
            "1 day ago ago",
            "5 bogus",
        ] {
            assert_eq!(parsed(bad), None, "{bad}");
        }
    }

    #[test]
    fn parses_keywords_and_relative_offsets() {
        for (s, want) in [
            ("now", "2025-11-06 11:34:56.123456789"),
            ("Today", "2025-11-06 11:34:56.123456789"),
            ("yesterday", "2025-11-05 11:34:56.123456789"),
            ("tomorrow", "2025-11-07 11:34:56.123456789"),
            ("today 10:00", "2025-11-06 10:00:00.000000000"),
            ("+1 day", "2025-11-07 11:34:56.123456789"),
            ("+1day", "2025-11-07 11:34:56.123456789"),
            ("-2 hours", "2025-11-06 09:34:56.123456789"),
            ("1 week ago", "2025-10-30 11:34:56.123456789"),
            ("-1 days ago", "2025-11-07 11:34:56.123456789"),
            ("1 day 2 hours ago", "2025-11-07 09:34:56.123456789"),
            ("3 fortnights", "2025-12-18 11:34:56.123456789"),
            ("10 min", "2025-11-06 11:44:56.123456789"),
            ("1 year ago", "2024-11-06 11:34:56.123456789"),
            ("2024-01-31 +1 month", "2024-03-02 00:00:00.000000000"),
            ("2024-02-29 +1 year", "2025-03-01 00:00:00.000000000"),
            ("2024-12-31 +1 day", "2025-01-01 00:00:00.000000000"),
            ("1 hour ago 2024-01-15", "2024-01-14 23:00:00.000000000"),
            // GNU reads a signed number right after a time as its zone.
            ("2024-01-15 10:00 +1 day", "2024-01-16 09:00:00.000000000"),
        ] {
            assert_eq!(parsed(s).as_deref(), Some(want), "{s}");
        }
    }
}
