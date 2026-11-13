use crate::prelude::*;

const HELP: &str = "Usage: date [+FORMAT]
Display the current date and time.

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

Options:
  -u  use UTC time
  -h  display this help";

#[command("date")]
async fn cmd_date(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut _utc = false;
    let mut format = String::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
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
    let fmt = if format.is_empty() { "%c" } else { &format };
    let result = format_date(
        fmt,
        since_epoch.as_secs() as i64,
        since_epoch.subsec_nanos(),
    );

    let mut w = io::stdout()?;
    wprintln!(w, "{}", result)?;
    Ok(0)
}

/// Expands the conversion specifiers in `fmt` for the UTC instant `ts`
/// seconds plus `nanos` nanoseconds after the Unix epoch, in a single pass
/// so expanded text is never re-scanned. Like GNU `date`, an unknown
/// specifier or a trailing `%` is copied through literally.
pub(super) fn format_date(fmt: &str, ts: i64, nanos: u32) -> String {
    // Convert to date components (UTC)
    let days = ts / 86400;
    let secs = ts % 86400;
    let hour = secs / 3600;
    let min = (secs % 3600) / 60;
    let sec = secs % 60;

    let (year, month, day, yday) = {
        let mut y = 1970i64;
        let mut d = days;
        loop {
            let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
            let days_in_year = if leap { 366 } else { 365 };
            if d < days_in_year {
                break;
            }
            d -= days_in_year;
            y += 1;
        }
        let yday = d + 1;
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let mdays = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut m = 0;
        for md in mdays {
            if d < md {
                break;
            }
            d -= md;
            m += 1;
        }
        (y, m + 1, d + 1, yday)
    };

    let wday = ((days + 4) % 7) as usize;
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

#[cfg(test)]
mod tests {
    use super::format_date;

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
}
