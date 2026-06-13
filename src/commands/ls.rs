use crate::os::FileStat;
use crate::prelude::*;

const HELP: &str = "Usage: ls [-laR1] [FILE]...
List directory contents.

Options:
  -l    long listing format
  -a    include entries starting with .
  -R    list subdirectories recursively
  -1    one entry per line (default when not a terminal)";

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
    long: bool,
    show_all: bool,
    recursive: bool,
    multi: bool,
    now: &std::time::SystemTime,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let mut entries = io::list_dir(os, path).await?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));

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
                st.len,
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
        Box::pin(list_one(
            os, w, &sub_path, &sub_name, long, show_all, recursive, true, now,
        ))
        .await?;
    }
    Ok(0)
}

#[command("ls")]
async fn cmd_ls(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut long = false;
    let mut show_all = false;
    let mut recursive = false;
    let mut paths = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('l') => long = true,
            Short('a') => show_all = true,
            Short('R') => recursive = true,
            Short('1') => {} // already one-per-line
            Short('h') | Long("help") => {
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
            if let Err(err) = list_one(
                os, &mut w, path, path, long, show_all, recursive, multi, &now,
            )
            .await
            {
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
                lst.len,
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
