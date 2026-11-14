//! `date` formats the time the kernel's clock reports. `ClockKernel`
//! forwards everything to the default kernel except `now`, which returns a
//! fixed instant, so each test can check the calendar math for a known date.
//! The expected strings come from GNU `date -u -d @SECONDS`.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use strands_shell::Shell;
use strands_shell::os::{
    DirEntry, Fd, FileStat, HttpRequest, HttpResponse, Kernel, OpenFlags, Process,
};

struct ClockKernel {
    inner: Arc<dyn Kernel>,
    now: SystemTime,
}

#[async_trait]
impl Kernel for ClockKernel {
    fn now(&self) -> SystemTime {
        self.now
    }

    fn new_process(&self) -> Process {
        self.inner.new_process()
    }

    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd> {
        self.inner.open(proc, path, flags).await
    }

    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>> {
        self.inner.list_dir(proc, path).await
    }

    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()> {
        self.inner.change_dir(proc, path).await
    }

    async fn stat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.stat(proc, path).await
    }

    async fn lstat(&self, proc: &Process, path: &str) -> FileStat {
        self.inner.lstat(proc, path).await
    }

    async fn access(&self, proc: &Process, path: &str, mode: i32) -> bool {
        self.inner.access(proc, path, mode).await
    }

    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf> {
        self.inner.canonicalize(proc, path).await
    }

    async fn is_executable(&self, proc: &Process, path: &str) -> bool {
        self.inner.is_executable(proc, path).await
    }

    async fn glob(&self, proc: &Process, pattern: &str) -> Vec<String> {
        self.inner.glob(proc, pattern).await
    }

    fn isatty(&self, fd: i32) -> bool {
        self.inner.isatty(fd)
    }

    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_file(proc, path).await
    }

    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.remove_dir(proc, path).await
    }

    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        self.inner.create_dir(proc, path).await
    }

    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()> {
        self.inner.rename(proc, from, to).await
    }

    async fn symlink(&self, proc: &Process, target: &str, link: &str) -> io::Result<()> {
        self.inner.symlink(proc, target, link).await
    }

    async fn read_link(&self, proc: &Process, path: &str) -> io::Result<String> {
        self.inner.read_link(proc, path).await
    }

    async fn set_permissions(&self, proc: &Process, path: &str, mode: u32) -> io::Result<()> {
        self.inner.set_permissions(proc, path, mode).await
    }

    fn check_url(&self, url: &str) -> io::Result<()> {
        self.inner.check_url(url)
    }

    fn resolve_credential(&self, url: &str, method: &str) -> Vec<(String, String)> {
        self.inner.resolve_credential(url, method)
    }

    async fn http_request(&self, req: HttpRequest) -> io::Result<HttpResponse> {
        self.inner.http_request(req).await
    }
}

/// Runs `date '+FORMAT'` with the clock fixed `secs` seconds after the epoch.
fn date_at(secs: u64, format: &str) -> String {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    let out = rt.block_on(local.run_until(async {
        let inner = Shell::builder().build().unwrap().kernel().clone();
        let mut shell = Shell::with_kernel(Arc::new(ClockKernel {
            inner,
            now: UNIX_EPOCH + Duration::from_secs(secs),
        }));
        shell.run(&format!("date -u '+{format}'")).await
    }));
    assert_eq!(out.status, 0, "date at {secs}: {}", out.stderr);
    out.stdout
}

const FORMAT: &str = "%F %T %a %j %u %s";

fn check(cases: &[(u64, &str)]) {
    for &(secs, expected) in cases {
        assert_eq!(
            date_at(secs, FORMAT),
            format!("{expected} {secs}\n"),
            "at {secs}"
        );
    }
}

#[test]
fn the_epoch() {
    check(&[(0, "1970-01-01 00:00:00 Thu 001 4")]);
}

#[test]
fn year_2000_has_a_leap_day_because_it_divides_by_400() {
    check(&[
        (951782399, "2000-02-28 23:59:59 Mon 059 1"),
        (951827696, "2000-02-29 12:34:56 Tue 060 2"),
        (951868800, "2000-03-01 00:00:00 Wed 061 3"),
        (978307199, "2000-12-31 23:59:59 Sun 366 7"),
    ]);
}

#[test]
fn year_2100_has_no_leap_day_because_it_divides_by_100() {
    check(&[
        (4107542399, "2100-02-28 23:59:59 Sun 059 7"),
        (4107542400, "2100-03-01 00:00:00 Mon 060 1"),
        (4133937600, "2100-12-31 12:00:00 Fri 365 5"),
    ]);
}

#[test]
fn far_future_dates() {
    check(&[
        (2147483648, "2038-01-19 03:14:08 Tue 019 2"),
        (13574585228, "2400-02-29 06:07:08 Tue 060 2"),
        (13601001600, "2400-12-31 00:00:00 Sun 366 7"),
        (253402300799, "9999-12-31 23:59:59 Fri 365 5"),
    ]);
}

#[test]
fn twelve_hour_fields_and_month_names_at_a_fixed_instant() {
    assert_eq!(
        date_at(4107542400, "%A %B %e %I %p %y %D %R"),
        "Monday March  1 12 AM 00 03/01/00 00:00\n"
    );
}
