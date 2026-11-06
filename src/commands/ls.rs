use crate::os::FileStat;
use crate::prelude::*;

const HELP: &str = "Usage: ls [-laR1hrSt] [FILE]...
List directory contents.

Options:
  -l    long listing format
  -a    include entries starting with .
  -R    list subdirectories recursively
  -1    one entry per line (default when not a terminal)
  -h, --human-readable  with -l, print sizes like 1K 234M 2G
  -r, --reverse         reverse order while sorting
  -S    sort by file size, largest first
  -t    sort by modification time, newest first";

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Name,
    Time,
    Size,
}

struct Opts {
    long: bool,
    show_all: bool,
    recursive: bool,
    human: bool,
    sort: SortBy,
    reverse: bool,
}

/// Format a size like GNU `ls -lh`: powers of 1024, rounded up, with one
/// decimal place when the scaled value is below 10.
fn human_size(len: u64) -> String {
    const BASE: u64 = 1024;
    const SUFFIXES: &[char] = &['K', 'M', 'G', 'T', 'P', 'E', 'Z', 'Y'];
    if len < BASE {
        return len.to_string();
    }
    let mut amt = len;
    let mut tenths = 0;
    // Set when the value lost bits below the tenths digit.
    let mut inexact = false;
    let mut exp = 0;
    while amt >= BASE && exp < SUFFIXES.len() {
        let r10 = (amt % BASE) * 10 + tenths;
        inexact |= !r10.is_multiple_of(BASE);
        amt /= BASE;
        tenths = r10 / BASE;
        exp += 1;
    }
    let mut frac = String::new();
    if amt < 10 {
        if inexact {
            tenths += 1;
            inexact = false;
            if tenths == 10 {
                amt += 1;
                tenths = 0;
            }
        }
        if amt < 10 {
            frac = format!(".{}", tenths);
            tenths = 0;
        }
    }
    if tenths > 0 || inexact {
        amt += 1;
        if amt == BASE && exp < SUFFIXES.len() {
            exp += 1;
            amt = 1;
            frac = ".0".into();
        }
    }
    format!("{}{}{}", amt, frac, SUFFIXES[exp - 1])
}

fn format_size(len: u64, human: bool) -> String {
    if human {
        human_size(len)
    } else {
        len.to_string()
    }
}

fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(match mode & 0o170000 {
        0o120000 => 'l',
        0o040000 => 'd',
        0o010000 => 'p',
        0o140000 => 's',
        0o060000 => 'b',
        0o020000 => 'c',
        _ => '-',
    });
    for (shift, x_char) in [(6, 's'), (3, 's'), (0, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let set_bit = mode
            & (if shift == 0 {
                0o1000
            } else {
                0o4000 >> (2 - shift / 3)
            });
        s.push(if set_bit != 0 {
            if bits & 1 != 0 {
                x_char
            } else {
                x_char.to_ascii_uppercase()
            }
        } else if bits & 1 != 0 {
            'x'
        } else {
            '-'
        });
    }
    s
}

fn format_time(st: &FileStat, now: &std::time::SystemTime) -> String {
    if let Some(t) = st.modified {
        let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs() as i64;
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let mins = (time_of_day % 3600) / 60;
        let (y, m, d) = epoch_days_to_date(days);
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let now_secs = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if (now_secs - secs).abs() > 180 * 86400 {
            format!("{} {:2}  {:4}", months[(m - 1) as usize], d, y)
        } else {
            format!(
                "{} {:2} {:02}:{:02}",
                months[(m - 1) as usize],
                d,
                hours,
                mins
            )
        }
    } else {
        "            ".into()
    }
}

fn epoch_days_to_date(mut days: i64) -> (i64, i64, i64) {
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

async fn list_one(
    os: &dyn Kernel,
    w: &mut crate::os::FdWriter,
    path: &str,
    name_prefix: &str,
    opts: &Opts,
    multi: bool,
    now: &std::time::SystemTime,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let Opts {
        long,
        show_all,
        recursive,
        human,
        sort,
        reverse,
    } = *opts;
    let mut entries = io::list_dir(os, path).await?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    if sort != SortBy::Name {
        let mut keyed = Vec::with_capacity(entries.len());
        for entry in entries {
            let full = if path == "." {
                entry.name.clone()
            } else {
                format!("{}/{}", path, entry.name)
            };
            let st = io::lstat(os, &full).await;
            keyed.push((st.modified, st.len, entry));
        }
        // Stable sort keeps name order for ties.
        keyed.sort_by(|a, b| match sort {
            SortBy::Time => b.0.cmp(&a.0),
            _ => b.1.cmp(&a.1),
        });
        entries = keyed.into_iter().map(|(_, _, e)| e).collect();
    }
    if reverse {
        entries.reverse();
    }

    if multi {
        wprintln!(w, "{}:", name_prefix)?;
    }

    let mut subdirs = Vec::new();
    for entry in &entries {
        if !show_all && entry.name.starts_with('.') {
            continue;
        }
        let full = if path == "." {
            entry.name.clone()
        } else {
            format!("{}/{}", path, entry.name)
        };
        if long {
            let st = io::lstat(os, &full).await;
            let link = if st.is_symlink {
                io::read_link(os, &full)
                    .await
                    .map(|t| format!(" -> {}", t))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            wprintln!(
                w,
                "{} {:>8} {} {}{}",
                format_mode(st.mode),
                format_size(st.len, human),
                format_time(&st, now),
                entry.name,
                link
            )?;
        } else {
            wprintln!(w, "{}", entry.name)?;
        }
        if recursive && entry.is_dir {
            let sub_name = if name_prefix == "." {
                entry.name.clone()
            } else {
                format!("{}/{}", name_prefix, entry.name)
            };
            subdirs.push((full, sub_name));
        }
    }
    for (sub_path, sub_name) in subdirs {
        wprintln!(w)?;
        Box::pin(list_one(os, w, &sub_path, &sub_name, opts, true, now)).await?;
    }
    Ok(0)
}

#[command("ls")]
async fn cmd_ls(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut long = false;
    let mut show_all = false;
    let mut recursive = false;
    let mut human = false;
    let mut sort = SortBy::Name;
    let mut reverse = false;
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('l') => long = true,
            Short('a') => show_all = true,
            Short('R') => recursive = true,
            Short('1') => {} // already one-per-line
            Short('h') | Long("human-readable") => human = true,
            Short('r') | Long("reverse") => reverse = true,
            Short('S') => sort = SortBy::Size,
            Short('t') => sort = SortBy::Time,
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => paths.push(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    if paths.is_empty() {
        paths.push(".".into());
    }
    let multi = paths.len() > 1 || recursive;
    let opts = Opts {
        long,
        show_all,
        recursive,
        human,
        sort,
        reverse,
    };
    let now = os.now();
    let mut w = io::stdout()?;
    let mut code = 0;
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            wprintln!(w)?;
        }
        let lst = io::lstat(os, path).await;
        if !lst.exists {
            let st = io::stat(os, path).await;
            if !st.exists {
                let mut e = io::stderr()?;
                wprintln!(e, "ls: cannot access '{}': No such file or directory", path)?;
                code = 2;
                continue;
            }
        }
        // For symlinks to dirs: ls shows contents, ls -l shows the link itself
        let is_dir_target = if lst.is_symlink {
            io::stat(os, path).await.is_dir
        } else {
            lst.is_dir
        };
        if is_dir_target && !(long && lst.is_symlink) {
            if let Err(err) = list_one(os, &mut w, path, path, &opts, multi, &now).await {
                let mut e = io::stderr()?;
                wprintln!(e, "ls: {}: {}", path, err)?;
                code = 2;
            }
        } else if long {
            let link = if lst.is_symlink {
                io::read_link(os, path)
                    .await
                    .map(|t| format!(" -> {}", t))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            wprintln!(
                w,
                "{} {:>8} {} {}{}",
                format_mode(lst.mode),
                format_size(lst.len, human),
                format_time(&lst, &now),
                path,
                link
            )?;
        } else {
            wprintln!(w, "{}", path)?;
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn human_size_matches_gnu() {
        // Expected values from GNU coreutils `ls -lh`.
        let cases: &[(u64, &str)] = &[
            (0, "0"),
            (999, "999"),
            (1000, "1000"),
            (1023, "1023"),
            (1024, "1.0K"),
            (1025, "1.1K"),
            (1536, "1.5K"),
            (10239, "10K"),
            (10240, "10K"),
            (10241, "11K"),
            (104857, "103K"),
            (1022976, "999K"),
            (1022977, "1000K"),
            (1047552, "1023K"),
            (1047553, "1.0M"),
            (1048575, "1.0M"),
            (1048576, "1.0M"),
            (5000000, "4.8M"),
            (10485759, "10M"),
            (10485760, "10M"),
            (1073741824, "1.0G"),
            (1610612736, "1.5G"),
            (1099511627776, "1.0T"),
        ];
        for &(len, want) in cases {
            assert_eq!(human_size(len), want, "size {}", len);
        }
    }
}
