use crate::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

const HELP: &str = "Usage: readlink [OPTION]... FILE
Print the target of a symbolic link, or a canonical file name.

  -f, --canonicalize            follow every symlink; all but the last
                                component must exist
  -e, --canonicalize-existing   follow every symlink; all components must exist
  -m, --canonicalize-missing    follow every symlink; components need not exist
  -n, --no-newline              do not output the trailing newline";

/// How strictly `canonicalize` requires path components to exist.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    /// `-f`: every component but the last must exist.
    AllButLast,
    /// `-e`: every component must exist.
    Existing,
    /// `-m`: no component needs to exist.
    Missing,
}

/// Why `canonicalize` failed, worded like the matching `errno` message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum CanonError {
    NotFound,
    NotDir,
    Loop,
}

impl std::fmt::Display for CanonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CanonError::NotFound => "No such file or directory",
            CanonError::NotDir => "Not a directory",
            CanonError::Loop => "Too many levels of symbolic links",
        })
    }
}

/// Safety nets that bound the work for one path; hitting any is an error in
/// every mode. Real loops are caught by the seen-set; these stop links that
/// grow the path forever (`a -> a/x`) or expand into huge `..` chains.
/// Linux itself gives up after 40 symlinks and 4096-byte paths.
const MAX_SYMLINKS: usize = 256;
const MAX_PENDING: usize = 2048;
const MAX_STEPS: usize = 32 * 1024;

/// Pending-component marker for a trailing slash: the component before it
/// must be a directory.
const DIR_MARK: &str = "";

#[command("readlink")]
async fn cmd_readlink(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut path = None;
    let mut mode = None;
    let mut newline = true;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Short('f') | Long("canonicalize") => mode = Some(Mode::AllButLast),
            Short('e') | Long("canonicalize-existing") => mode = Some(Mode::Existing),
            Short('m') | Long("canonicalize-missing") => mode = Some(Mode::Missing),
            Short('n') | Long("no-newline") => newline = false,
            Value(val) if path.is_none() => path = Some(val.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let path = path.ok_or("readlink: missing operand")?;
    let target = match mode {
        None => io::read_link(os, &path).await?,
        // Like GNU readlink without -v, a failed canonicalization is silent.
        Some(mode) => match canonicalize(os, &path, mode, true).await {
            Ok(target) => target,
            Err(_) => return Ok(1),
        },
    };
    let mut w = io::stdout()?;
    if newline {
        wprintln!(w, "{}", target)?;
    } else {
        wprint!(w, "{}", target)?;
    }
    Ok(0)
}

/// Resolve `path` to an absolute path with no `.`, `..` or symlink
/// components, walking it one component at a time through the kernel.
/// Fails when `mode` requires a component that is missing, a non-directory
/// is used as a directory, or symlinks loop.
///
/// With `follow_links` false, symlinks are kept as they are, but each one
/// is still checked through its target, like `stat`, to see whether it
/// exists and is a directory.
pub(super) async fn canonicalize(
    os: &dyn Kernel,
    path: &str,
    mode: Mode,
    follow_links: bool,
) -> Result<String, CanonError> {
    if path.is_empty() {
        return Err(CanonError::NotFound);
    }
    let mut pending: VecDeque<String> = VecDeque::new();
    if !path.starts_with('/') {
        let cwd = io::with_process(|p| p.cwd.to_string_lossy().into_owned());
        pending.extend(components(&cwd));
    }
    pending.extend(components(path));

    let mut resolved: Vec<String> = Vec::new();
    // A followed link is keyed by its path and a hash of what is left to
    // resolve after it: meeting the same pair again means a loop.
    let mut seen: HashSet<(String, usize, u64)> = HashSet::new();
    let mut followed = 0;
    let mut steps = 0;
    // Whether the last pushed component was found to exist and be a directory
    // (vacuously true for `/`).
    let mut is_dir = true;
    // How many trailing components of `resolved` do not exist. Past the first
    // one nothing below can exist, so (in Missing mode) they are not looked up.
    let mut missing = 0usize;
    // Lookups by absolute path, so a link met again costs no kernel call.
    let mut lookups: HashMap<String, Lookup> = HashMap::new();
    // Without following links, like GNU, the default mode needs a missing
    // component to be a directory only when a `..`, a final `.` or the last
    // component depends on it.
    let lexical = !follow_links && mode == Mode::AllButLast;

    while let Some(comp) = pending.pop_front() {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(CanonError::Loop);
        }
        // Like a trailing slash, `.` needs the path so far to be a directory.
        if comp == "." || comp == DIR_MARK {
            match mode {
                _ if is_dir => {}
                Mode::Missing => {}
                // `-f` allows a missing last component even with a slash.
                Mode::AllButLast if missing > 0 && comp == DIR_MARK => {}
                _ if lexical && missing > 0 && !only_dots(&pending) => {}
                _ => return Err(not_dir_or_missing(missing)),
            }
            continue;
        }
        if comp == ".." {
            if !is_dir && mode != Mode::Missing {
                return Err(not_dir_or_missing(missing));
            }
            resolved.pop();
            // Every earlier component was checked to be a directory (or, in
            // Missing mode, it does not matter).
            is_dir = true;
            missing = missing.saturating_sub(1);
            continue;
        }
        if !is_dir && mode != Mode::Missing && !(lexical && missing > 0) {
            // The previous component is a missing path or a non-directory.
            return Err(not_dir_or_missing(missing));
        }
        if missing > 0 {
            resolved.push(comp);
            missing += 1;
            continue;
        }
        let candidate = join(&resolved, &comp);
        let st = match lookups.get(&candidate) {
            Some(st) => st.clone(),
            None => {
                let mut st = lookup(os, &candidate).await;
                if st.is_symlink && !follow_links {
                    st = match Box::pin(canonicalize(os, &candidate, Mode::Existing, true)).await {
                        Ok(target) => Lookup {
                            exists: true,
                            is_dir: io::stat(os, &target).await.is_dir,
                            is_symlink: false,
                            link_target: None,
                        },
                        Err(e) if e != CanonError::NotFound && mode != Mode::Missing => {
                            return Err(e);
                        }
                        // A dangling link counts as a missing component.
                        Err(_) => Lookup {
                            exists: false,
                            is_dir: false,
                            is_symlink: false,
                            link_target: None,
                        },
                    };
                }
                lookups.insert(candidate.clone(), st.clone());
                st
            }
        };
        if !st.exists {
            let is_last = only_dots(&pending);
            match mode {
                Mode::Missing => {}
                Mode::AllButLast if is_last || lexical => {}
                _ => return Err(CanonError::NotFound),
            }
            resolved.push(comp);
            missing = 1;
            is_dir = false;
            continue;
        }
        if st.is_symlink {
            followed += 1;
            if followed > MAX_SYMLINKS || pending.len() > MAX_PENDING {
                return Err(CanonError::Loop);
            }
            let mut hasher = DefaultHasher::new();
            pending.hash(&mut hasher);
            if !seen.insert((candidate.clone(), pending.len(), hasher.finish())) {
                if mode == Mode::Missing {
                    resolved.push(comp);
                    is_dir = false;
                    missing = 1;
                    continue;
                }
                return Err(CanonError::Loop);
            }
            let target = st.link_target.ok_or(CanonError::NotFound)?;
            if target.starts_with('/') {
                resolved.clear();
            }
            for c in components(&target).into_iter().rev() {
                pending.push_front(c);
            }
            is_dir = true;
            continue;
        }
        resolved.push(comp);
        is_dir = st.is_dir;
    }

    Ok(join(&[], &resolved.join("/")))
}

/// Whether nothing but `.` and trailing-slash markers is left to resolve.
fn only_dots(pending: &VecDeque<String>) -> bool {
    pending.iter().all(|c| c == "." || c == DIR_MARK)
}

/// The error for a component that cannot be a directory: missing when
/// some earlier component is missing, otherwise not a directory.
fn not_dir_or_missing(missing: usize) -> CanonError {
    if missing > 0 {
        CanonError::NotFound
    } else {
        CanonError::NotDir
    }
}

/// What `canonicalize` needs to know about one path.
#[derive(Clone)]
struct Lookup {
    exists: bool,
    is_dir: bool,
    is_symlink: bool,
    /// The link's target; `None` if it is not a link or cannot be read.
    link_target: Option<String>,
}

async fn lookup(os: &dyn Kernel, path: &str) -> Lookup {
    let st = io::lstat(os, path).await;
    // Direct-bind metadata follows links, but read_link returns their stored target.
    let link_target = io::read_link(os, path).await.ok();
    let is_symlink = st.is_symlink || link_target.is_some();
    Lookup {
        exists: st.exists || is_symlink,
        is_dir: st.is_dir && !is_symlink,
        is_symlink,
        link_target,
    }
}

/// Split `path` into its non-empty components, ending with `DIR_MARK` when
/// it has a trailing slash.
fn components(path: &str) -> Vec<String> {
    let mut out: Vec<String> = path
        .split('/')
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect();
    if path.ends_with('/') && !out.is_empty() {
        out.push(DIR_MARK.to_string());
    }
    out
}

fn join(dir: &[String], name: &str) -> String {
    let mut out = String::from("/");
    for c in dir {
        out.push_str(c);
        out.push('/');
    }
    out.push_str(name);
    out
}
