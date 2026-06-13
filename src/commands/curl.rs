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
  -u, --user USER:PASS    Basic authentication";

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

            let r = match os.http_request(http_req).await {
                Ok(r) => r,
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
                let next = if loc.starts_with("http://") || loc.starts_with("https://") {
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

            let r = match os.http_request(http_req).await {
                Ok(r) => r,
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
                let next = if loc.starts_with("http://") || loc.starts_with("https://") {
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

    // Take stdout once up front
    let mut out = if output.is_none() {
        Some(io::stdout()?)
    } else {
        None
    };

    if verbose || include {
        let mut hdr = format!("HTTP/{} {} {}\r\n", resp.version, status_code, resp.reason);
        for (name, value) in &resp.headers {
            hdr.push_str(&format!("{}: {}\r\n", name, value));
        }
        hdr.push_str("\r\n");
        if include {
            if let Some(ref mut w) = out {
                w.write_all(hdr.as_bytes()).await?;
            }
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
        return Ok(22);
    }

    let body_bytes = &resp.body;

    if let Some(ref path) = output {
        let fd = io::open(os, path, OpenFlags::write()).await?;
        let mut w = io::take_writer(fd)?;
        w.write_all(body_bytes).await?;
    } else if let Some(ref mut w) = out {
        w.write_all(body_bytes).await?;
    }

    if let Some(ref fmt) = write_out {
        let s = fmt
            .replace("%{http_code}", &status_code.to_string())
            .replace("%{response_code}", &status_code.to_string())
            .replace("%{content_type}", "")
            .replace("%{size_download}", &body_bytes.len().to_string())
            .replace("\\n", "\n");
        if let Some(ref mut w) = out {
            wprint!(w, "{}", s)?;
        }
    }

    Ok(0)
}
