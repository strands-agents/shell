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
enum Mode {
    /// `-f`: every component but the last must exist.
    AllButLast,
    /// `-e`: every component must exist.
    Existing,
    /// `-m`: no component needs to exist.
    Missing,
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
        Some(mode) => match canonicalize(os, &path, mode).await {
            Some(target) => target,
            None => return Ok(1),
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
/// Returns `None` when `mode` requires a component that is missing, a
/// non-directory is used as a directory, or symlinks loop.
async fn canonicalize(os: &dyn Kernel, path: &str, mode: Mode) -> Option<String> {
    if path.is_empty() {
        return None;
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

    while let Some(comp) = pending.pop_front() {
        steps += 1;
        if steps > MAX_STEPS {
            return None;
        }
        // Like a trailing slash, `.` needs the path so far to be a directory.
        if comp == "." || comp == DIR_MARK {
            match mode {
                _ if is_dir => {}
                Mode::Missing => {}
                // `-f` allows a missing last component even with a slash.
                Mode::AllButLast if missing > 0 && comp == DIR_MARK => {}
                _ => return None,
            }
            continue;
        }
        if comp == ".." {
            if !is_dir && mode != Mode::Missing {
                return None;
            }
            resolved.pop();
            // Every earlier component was checked to be a directory (or, in
            // Missing mode, it does not matter).
            is_dir = true;
            missing = missing.saturating_sub(1);
            continue;
        }
        if !is_dir && mode != Mode::Missing {
            // The previous component is a missing path or a non-directory.
            return None;
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
                let st = lookup(os, &candidate).await;
                lookups.insert(candidate.clone(), st.clone());
                st
            }
        };
        if !st.exists {
            let is_last = pending.iter().all(|c| c == "." || c == DIR_MARK);
            match mode {
                Mode::Missing => {}
                Mode::AllButLast if is_last => {}
                _ => return None,
            }
            resolved.push(comp);
            missing = 1;
            is_dir = false;
            continue;
        }
        if st.is_symlink {
            followed += 1;
            if followed > MAX_SYMLINKS || pending.len() > MAX_PENDING {
                return None;
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
                return None;
            }
            let target = st.link_target?;
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

    Some(join(&[], &resolved.join("/")))
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
