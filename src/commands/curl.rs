use crate::prelude::*;

const HELP: &str = "Usage: curl [OPTIONS] URL
Transfer data from or to a server.

Options:
  -s, --silent            Suppress progress output
  -S, --show-error        Show errors even when silent
  -o, --output FILE       Write output to FILE
  -X, --request METHOD    HTTP method (GET, POST, PUT, DELETE, PATCH, HEAD)
  -H, --header HEADER     Add header (e.g. 'Content-Type: application/json')
  -d, --data DATA         Request body (implies POST)
      --json DATA         JSON body (implies POST, sets Content-Type/Accept)
                          Use @filename to read from a file
  -f, --fail              Fail silently on HTTP errors (exit 22)
  -L, --location          Follow redirects
  -i, --include           Include response headers in output
  -k, --insecure          Allow insecure TLS connections
  -v, --verbose           Verbose output
  -w, --write-out FORMAT  Output FORMAT after completion
  -b, --cookie DATA       Send cookies (name=value pairs)
  -u, --user USER:PASS    Basic authentication
  -A, --user-agent NAME   Send User-Agent NAME
  -m, --max-time SECONDS  Maximum time allowed for the transfer
      --proto PROTOCOLS   Enable/disable PROTOCOLS
      --proto-redir PROTOCOLS
                          Enable/disable PROTOCOLS on redirect
  -g, --globoff           Disable URL globbing (always off)";

/// The protocols this `curl` speaks, as a `--proto`/`--proto-redir` allow set.
#[derive(Clone, Copy)]
struct Protocols {
    http: bool,
    https: bool,
}

impl Protocols {
    const ALL: Protocols = Protocols {
        http: true,
        https: true,
    };

    /// Apply curl's comma-separated `--proto` list to `self`, left to right.
    ///
    /// Each entry is a protocol name or `all`, with an optional modifier: `+`
    /// (allow, the default), `-` (deny), or `=` (allow only this). A protocol
    /// this `curl` does not speak is ignored, as curl ignores one it was not
    /// built with. Returns `None` for a malformed modifier.
    fn apply(mut self, list: &str) -> Option<Protocols> {
        for token in list.split(',').filter(|t| !t.is_empty()) {
            let name = token.trim_start_matches(['+', '-', '=']);
            let modifier = token[..token.len() - name.len()].chars().last();
            if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                return None;
            }
            let name = name.to_ascii_lowercase();
            let (http, https) = match name.as_str() {
                "all" => (true, true),
                "http" => (true, false),
                "https" => (false, true),
                _ => (false, false),
            };
            match modifier {
                Some('=') => {
                    self = Protocols { http, https };
                }
                Some('-') => {
                    self.http &= !http;
                    self.https &= !https;
                }
                _ => {
                    self.http |= http;
                    self.https |= https;
                }
            }
        }
        Some(self)
    }

    /// The scheme of `url` when this set refuses it.
    fn refuses(self, url: &str) -> Option<String> {
        let scheme = url
            .split_once("://")
            .map_or("http", |(s, _)| s)
            .to_ascii_lowercase();
        let allowed = match scheme.as_str() {
            "http" => self.http,
            "https" => self.https,
            _ => false,
        };
        (!allowed).then_some(scheme)
    }
}

/// Whether a `Location` value names its own scheme (`scheme://…`) rather than
/// a path relative to the current URL.
fn is_absolute(loc: &str) -> bool {
    loc.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The values a `-w` format can name.
struct WriteOut<'a> {
    /// The response status, or `0` when no response arrived.
    status: u16,
    content_type: &'a str,
    size_download: usize,
    url_effective: &'a str,
}

/// Expand a `-w` format in one pass, so a substituted value, such as a server's
/// `Content-Type`, is never itself read as a variable or an escape.
fn expand_write_out(fmt: &str, vars: &WriteOut) -> String {
    let mut s = String::new();
    let mut rest = fmt;
    while let Some(i) = rest.find(['%', '\\']) {
        s.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(after) = rest.strip_prefix("\\n") {
            s.push('\n');
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix("%{")
            && let Some(end) = after.find('}')
        {
            let value = match &after[..end] {
                "http_code" | "response_code" => Some(format!("{:03}", vars.status)),
                "content_type" => Some(vars.content_type.to_string()),
                "size_download" => Some(vars.size_download.to_string()),
                "url_effective" => Some(vars.url_effective.to_string()),
                _ => None,
            };
            if let Some(value) = value {
                s.push_str(&value);
                rest = &after[end + 1..];
                continue;
            }
        }
        // Not a token this `curl` knows: keep the character as written.
        s.push_str(&rest[..1]);
        rest = &rest[1..];
    }
    s.push_str(rest);
    s
}

/// Bound `fut` by the `--max-time` deadline, reporting a lapse as `TimedOut`.
#[cfg(not(target_arch = "wasm32"))]
async fn within<T>(
    deadline: Option<tokio::time::Instant>,
    fut: impl std::future::Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    match deadline {
        Some(d) => tokio::time::timeout_at(d, fut)
            .await
            .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into())),
        None => fut.await,
    }
}

/// WASI HTTP calls block, so a deadline cannot interrupt one; `--max-time` is
/// accepted and not enforced.
#[cfg(target_arch = "wasm32")]
async fn within<T>(
    _deadline: Option<()>,
    fut: impl std::future::Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    fut.await
}

#[command("curl")]
async fn cmd_curl(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut silent = false;
    let mut show_error = false;
    let mut output: Option<String> = None;
    let mut method: Option<String> = None;
    let mut headers: Vec<String> = Vec::new();
    let mut data: Option<String> = None;
    let mut fail = false;
    let mut follow = false;
    let mut include = false;
    let mut insecure = false;
    let mut verbose = false;
    let mut write_out: Option<String> = None;
    let mut cookies: Vec<String> = Vec::new();
    let mut user: Option<String> = None;
    let mut url: Option<String> = None;
    let mut user_agent: Option<String> = None;
    let mut max_time: Option<String> = None;
    let mut proto = Protocols::ALL;
    let mut proto_redir = Protocols::ALL;

    let mut parser = lexopt::Parser::from_args(args);
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') | Long("silent") => silent = true,
            Short('S') | Long("show-error") => show_error = true,
            Short('o') | Long("output") => output = Some(parser.value()?.string()?),
            Short('X') | Long("request") => method = Some(parser.value()?.string()?),
            Short('H') | Long("header") => headers.push(parser.value()?.string()?),
            Short('d') | Long("data") | Long("data-raw") => data = Some(parser.value()?.string()?),
            Long("json") => {
                let val = parser.value()?.string()?;
                let json_body = if let Some(path) = val.strip_prefix('@') {
                    let fd = io::open(os, path, OpenFlags::read()).await?;
                    let mut r = io::take_reader(fd)?;
                    let max_output = io::with_process(|p| p.max_output);
                    crate::os::read_to_string_limited(&mut r, max_output).await?
                } else {
                    val
                };
                data = Some(json_body);
                headers.push("Content-Type: application/json".into());
                headers.push("Accept: application/json".into());
            }
            Short('f') | Long("fail") => fail = true,
            Short('L') | Long("location") => follow = true,
            Short('i') | Long("include") => include = true,
            Short('k') | Long("insecure") => insecure = true,
            Short('v') | Long("verbose") => verbose = true,
            Short('w') | Long("write-out") => write_out = Some(parser.value()?.string()?),
            Short('b') | Long("cookie") => cookies.push(parser.value()?.string()?),
            Short('u') | Long("user") => user = Some(parser.value()?.string()?),
            Short('A') | Long("user-agent") => user_agent = Some(parser.value()?.string()?),
            Short('m') | Long("max-time") => max_time = Some(parser.value()?.string()?),
            Long("proto") | Long("proto-redir") => {
                let redir = matches!(arg, Long("proto-redir"));
                let list = parser.value()?.string()?;
                let set = if redir { &mut proto_redir } else { &mut proto };
                // Each flag starts from every protocol, so the last one wins, as in curl.
                match Protocols::ALL.apply(&list) {
                    Some(p) => *set = p,
                    None => {
                        let mut w = io::stderr()?;
                        wprintln!(w, "curl: bad protocol list: {}", list)?;
                        return Ok(2);
                    }
                }
            }
            // This `curl` never expands `{}` or `[]` in a URL, so globbing is always off.
            Short('g') | Long("globoff") => {}
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => url = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }

    let url = match url {
        Some(u) => u,
        None => {
            let mut w = io::stderr()?;
            wprintln!(w, "curl: no URL specified")?;
            return Ok(2);
        }
    };

    let max_time = match max_time.as_deref().map(str::parse::<f64>) {
        None => None,
        // A limit too large to represent is no limit.
        Some(Ok(secs)) if secs.is_finite() && secs >= 0.0 => {
            std::time::Duration::try_from_secs_f64(secs)
                .ok()
                .filter(|d| !d.is_zero())
        }
        Some(_) => {
            let mut w = io::stderr()?;
            wprintln!(
                w,
                "curl: option --max-time: expected a proper numerical parameter"
            )?;
            return Ok(2);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let deadline = max_time.and_then(|d| tokio::time::Instant::now().checked_add(d));
    #[cfg(target_arch = "wasm32")]
    let deadline = max_time.map(|_| ());

    // `-H 'User-Agent: …'` takes precedence over `-A`, and `-A ''` sends none.
    if let Some(ua) = user_agent.filter(|ua| !ua.is_empty())
        && !headers.iter().any(|h| {
            h.split_once(':')
                .is_some_and(|(n, _)| n.trim().eq_ignore_ascii_case("user-agent"))
        })
    {
        headers.push(format!("User-Agent: {ua}"));
    }

    let method = method.unwrap_or_else(|| {
        if data.is_some() {
            "POST".into()
        } else {
            "GET".into()
        }
    });

    let max_redirects = if follow { 10usize } else { 0 };
    let max_response = io::with_process(|p| p.max_output);

    // Take stderr once up front
    let mut err = io::stderr().ok();

    if verbose && let Some(ref mut w) = err {
        wprintln!(w, "> {} {} HTTP/1.1", method.to_uppercase(), &url)?;
        for h in &headers {
            wprintln!(w, "> {}", h)?;
        }
        wprintln!(w, ">")?;
    }

    // Manual redirect loop
    let mut current_url = url.clone();
    let mut redirects_left = max_redirects;
    let resp = loop {
        // Build header list for this request
        let mut req_headers: Vec<(String, String)> = Vec::new();

        // User-specified headers
        for h in &headers {
            if let Some((name, value)) = h.split_once(':') {
                req_headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }

        // A redirect hop must pass both `--proto` and `--proto-redir`.
        let refused = proto.refuses(&current_url).or_else(|| {
            (current_url != url)
                .then(|| proto_redir.refuses(&current_url))
                .flatten()
        });
        if let Some(scheme) = refused {
            if (!silent || show_error)
                && let Some(ref mut w) = err
            {
                let why = if matches!(scheme.as_str(), "http" | "https") {
                    "disabled"
                } else {
                    "not supported"
                };
                wprintln!(w, "curl: (1) Protocol \"{}\" {}", scheme, why)?;
            }
            return Ok(1);
        }

        // Inject credentials (only for original URL, not redirects)
        if current_url == url {
            // Query param credentials — modify URL
            let mut request_url = current_url.clone();
            for (name, value) in os.resolve_credential(&current_url, &method) {
                if name == "__query_param__" {
                    let sep = if request_url.contains('?') { "&" } else { "?" };
                    request_url = format!("{}{}{}", request_url, sep, value);
                } else {
                    req_headers.push((name, value));
                }
            }

            // Default content-type for POST data
            if data.is_some() {
                let has_ct = req_headers
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
                if !has_ct {
                    req_headers.push((
                        "Content-Type".to_string(),
                        "application/x-www-form-urlencoded".to_string(),
                    ));
                }
            }

            // Cookies
            if !cookies.is_empty() {
                req_headers.push(("Cookie".to_string(), cookies.join("; ")));
            }

            // Basic auth
            if let Some(ref creds) = user {
                let (u, p) = creds.split_once(':').unwrap_or((creds, ""));
                use std::io::Write as _;
                let mut encoded = Vec::new();
                write!(encoded, "{}:{}", u, p).unwrap();
                // Base64 encode credentials
                let b64 = crate::os::base64_encode(&encoded);
                req_headers.push(("Authorization".to_string(), format!("Basic {}", b64)));
            }

            let http_req = crate::os::HttpRequest {
                method: method.clone(),
                url: request_url,
                headers: req_headers,
                body: data.as_ref().map(|d| d.as_bytes().to_vec()),
                insecure,
                max_response,
            };

            let r = match within(deadline, os.http_request(http_req)).await {
                Ok(r) => r,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if (!silent || show_error)
                        && let Some(ref mut w) = err
                    {
                        wprintln!(w, "curl: (28) Operation timed out")?;
                    }
                    if let Some(ref fmt) = write_out {
                        let vars = WriteOut {
                            status: 0,
                            content_type: "",
                            size_download: 0,
                            url_effective: &current_url,
                        };
                        let mut w = io::stdout()?;
                        wprint!(w, "{}", expand_write_out(fmt, &vars))?;
                    }
                    return Ok(28);
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    if let Some(ref mut w) = err {
                        wprintln!(w, "curl: {}", e)?;
                    }
                    return Ok(1);
                }
                Err(e) => {
                    if (!silent || show_error)
                        && let Some(ref mut w) = err
                    {
                        wprintln!(w, "curl: (6) {}", e)?;
                    }
                    return Ok(6);
                }
            };

            if redirects_left > 0
                && (301..=308).contains(&r.status)
                && let Some(loc) = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone())
            {
                let next = if is_absolute(&loc) {
                    loc
                } else if loc.starts_with('/') {
                    let scheme_end = current_url.find("://").map(|i| i + 3).unwrap_or(0);
                    let host_end = current_url[scheme_end..]
                        .find('/')
                        .map(|i| i + scheme_end)
                        .unwrap_or(current_url.len());
                    format!("{}{loc}", &current_url[..host_end])
                } else {
                    let base = current_url
                        .rfind('/')
                        .map(|i| &current_url[..i + 1])
                        .unwrap_or(&current_url);
                    format!("{base}{loc}")
                };
                if verbose && let Some(ref mut w) = err {
                    wprintln!(w, "* Redirecting to {next}")?;
                }
                current_url = next;
                redirects_left -= 1;
                continue;
            }
            break r;
        } else {
            // Redirect hop — minimal headers, no credentials
            let http_req = crate::os::HttpRequest {
                method: method.clone(),
                url: current_url.clone(),
                headers: req_headers,
                body: None,
                insecure,
                max_response,
            };

            let r = match within(deadline, os.http_request(http_req)).await {
                Ok(r) => r,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    if (!silent || show_error)
                        && let Some(ref mut w) = err
                    {
                        wprintln!(w, "curl: (28) Operation timed out")?;
                    }
                    if let Some(ref fmt) = write_out {
                        let vars = WriteOut {
                            status: 0,
                            content_type: "",
                            size_download: 0,
                            url_effective: &current_url,
                        };
                        let mut w = io::stdout()?;
                        wprint!(w, "{}", expand_write_out(fmt, &vars))?;
                    }
                    return Ok(28);
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    if let Some(ref mut w) = err {
                        wprintln!(w, "curl: {}", e)?;
                    }
                    return Ok(1);
                }
                Err(e) => {
                    if (!silent || show_error)
                        && let Some(ref mut w) = err
                    {
                        wprintln!(w, "curl: (6) {}", e)?;
                    }
                    return Ok(6);
                }
            };

            if redirects_left > 0
                && (301..=308).contains(&r.status)
                && let Some(loc) = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone())
            {
                let next = if is_absolute(&loc) {
                    loc
                } else if loc.starts_with('/') {
                    let scheme_end = current_url.find("://").map(|i| i + 3).unwrap_or(0);
                    let host_end = current_url[scheme_end..]
                        .find('/')
                        .map(|i| i + scheme_end)
                        .unwrap_or(current_url.len());
                    format!("{}{loc}", &current_url[..host_end])
                } else {
                    let base = current_url
                        .rfind('/')
                        .map(|i| &current_url[..i + 1])
                        .unwrap_or(&current_url);
                    format!("{base}{loc}")
                };
                if verbose && let Some(ref mut w) = err {
                    wprintln!(w, "* Redirecting to {next}")?;
                }
                current_url = next;
                redirects_left -= 1;
                continue;
            }
            break r;
        }
    };

    let status_code = resp.status;
    let content_type = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map_or("", |(_, v)| v.as_str());

    // `-o` takes the body and the `-i` headers; `-w` always goes to stdout.
    let mut out = io::stdout()?;

    let mut included: Option<String> = None;
    if verbose || include {
        let mut hdr = format!("HTTP/{} {} {}\r\n", resp.version, status_code, resp.reason);
        for (name, value) in &resp.headers {
            hdr.push_str(&format!("{}: {}\r\n", name, value));
        }
        hdr.push_str("\r\n");
        if include {
            included = Some(hdr);
        } else if let Some(ref mut w) = err {
            w.write_all(hdr.as_bytes()).await?;
        }
    }

    if fail && status_code >= 400 {
        if show_error && let Some(ref mut w) = err {
            wprintln!(
                w,
                "curl: (22) The requested URL returned error: {}",
                status_code
            )?;
        }
        if let Some(ref fmt) = write_out {
            let vars = WriteOut {
                status: status_code,
                content_type,
                size_download: 0,
                url_effective: &current_url,
            };
            wprint!(out, "{}", expand_write_out(fmt, &vars))?;
        }
        return Ok(22);
    }

    let body_bytes = &resp.body;

    if let Some(ref path) = output {
        let fd = io::open(os, path, OpenFlags::write()).await?;
        let mut w = io::take_writer(fd)?;
        if let Some(ref hdr) = included {
            w.write_all(hdr.as_bytes()).await?;
        }
        w.write_all(body_bytes).await?;
    } else {
        if let Some(ref hdr) = included {
            out.write_all(hdr.as_bytes()).await?;
        }
        out.write_all(body_bytes).await?;
    }

    if let Some(ref fmt) = write_out {
        let vars = WriteOut {
            status: status_code,
            content_type,
            size_download: body_bytes.len(),
            url_effective: &current_url,
        };
        wprint!(out, "{}", expand_write_out(fmt, &vars))?;
    }

    Ok(0)
}
