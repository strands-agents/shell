use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Inode number type.
pub type Ino = u64;

/// User/group IDs.
pub type Uid = u32;
pub type Gid = u32;

/// Default unprivileged user for lash processes.
pub const LASH_UID: Uid = 1000;
pub const LASH_GID: Gid = 1000;
pub const ROOT_UID: Uid = 0;
pub const ROOT_GID: Gid = 0;

/// Root inode is always 1.
const ROOT_INO: Ino = 1;

static NEXT_INO: AtomicU64 = AtomicU64::new(2);

fn alloc_ino() -> Ino {
    NEXT_INO.fetch_add(1, Ordering::Relaxed)
}

/// The data payload of an inode.
#[derive(Clone)]
pub enum InodeData {
    /// Regular file with in-memory contents.
    File(Vec<u8>),
    /// Directory: maps child name → inode number.
    Dir(HashMap<String, Ino>),
    /// Symbolic link target path.
    Symlink(String),
    /// Character device (major, minor).
    CharDevice(u32, u32),
    /// Block device (major, minor).
    BlockDevice(u32, u32),
    /// Named pipe (FIFO) — no data stored.
    Fifo,
    /// Bind-mounted host file (host path, readonly).
    HostFile(String, bool),
    /// Bind-mounted host directory (host path, readonly).
    HostDir(String, bool),
}

/// A single inode in the virtual filesystem.
#[derive(Clone)]
pub struct Inode {
    pub ino: Ino,
    pub data: InodeData,
    pub mode: u32,
    pub uid: Uid,
    pub gid: Gid,
    pub nlink: u32,
    pub mtime: SystemTime,
}

impl Inode {
    fn new(data: InodeData, mode: u32, uid: Uid, gid: Gid) -> Self {
        let nlink = match &data {
            InodeData::Dir(_) => 2, // . and parent
            _ => 1,
        };
        Self {
            ino: alloc_ino(),
            data,
            mode,
            uid,
            gid,
            nlink,
            mtime: SystemTime::now(),
        }
    }
}

/// The in-memory virtual filesystem.
pub struct Vfs {
    pub inodes: HashMap<Ino, Inode>,
    pub umask: u32,
    /// Maximum bytes for a single file (0 = unlimited).
    pub max_file_size: usize,
    /// Maximum number of inodes (0 = unlimited).
    pub max_inodes: usize,
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs {
    /// Create a new VFS with an empty root directory owned by root.
    pub fn new() -> Self {
        let mut root_entries = HashMap::new();
        root_entries.insert(".".into(), ROOT_INO);
        root_entries.insert("..".into(), ROOT_INO);
        let root = Inode {
            ino: ROOT_INO,
            data: InodeData::Dir(root_entries),
            mode: 0o040755,
            uid: ROOT_UID,
            gid: ROOT_GID,
            nlink: 2,
            mtime: SystemTime::now(),
        };
        let mut inodes = HashMap::new();
        inodes.insert(ROOT_INO, root);
        Self {
            inodes,
            umask: 0o022,
            max_file_size: 0,
            max_inodes: 0,
        }
    }

    /// Resolve a normalized absolute path to its inode number.
    /// Does NOT follow a final symlink (lstat semantics).
    /// `follow_last`: if true, follow symlinks on the final component.
    pub fn resolve(&self, path: &str, follow_last: bool) -> io::Result<Ino> {
        self.resolve_depth(path, follow_last, 0)
    }

    fn resolve_depth(&self, path: &str, follow_last: bool, depth: u32) -> io::Result<Ino> {
        if depth > 40 {
            return Err(io::Error::other("too many levels of symbolic links"));
        }
        let path = normalize(path);
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let mut current = ROOT_INO;

        for (i, comp) in components.iter().enumerate() {
            let is_last = i == components.len() - 1;
            // Resolve current inode — if it's a symlink, follow it
            let inode = self.get(current)?;
            match &inode.data {
                InodeData::Dir(entries) => {
                    let child_ino = *entries.get(*comp).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("no such file or directory: {}", path),
                        )
                    })?;
                    // Check if child is a symlink
                    let child = self.get(child_ino)?;
                    if let InodeData::Symlink(target) = &child.data
                        && (!is_last || follow_last)
                    {
                        // Resolve the symlink
                        let base = if components[..i].is_empty() {
                            "/".to_string()
                        } else {
                            format!("/{}", components[..i].join("/"))
                        };
                        let resolved_target = resolve_relative(&base, target);
                        let remaining: String = if is_last {
                            String::new()
                        } else {
                            format!("/{}", components[i + 1..].join("/"))
                        };
                        let full = format!("{}{}", resolved_target, remaining);
                        return self.resolve_depth(&full, follow_last, depth + 1);
                    }
                    current = child_ino;
                }
                InodeData::Symlink(target) => {
                    // Intermediate component is a symlink — resolve it
                    let base = if i == 0 {
                        "/".to_string()
                    } else {
                        format!("/{}", components[..i].join("/"))
                    };
                    let resolved_target = resolve_relative(&base, target);
                    let remaining = format!("/{}", components[i..].join("/"));
                    let full = format!("{}{}", resolved_target, remaining);
                    return self.resolve_depth(&full, follow_last, depth + 1);
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "not a directory",
                    ));
                }
            }
        }
        // If we ended on a symlink and follow_last, resolve it
        if follow_last {
            let inode = self.get(current)?;
            if let InodeData::Symlink(target) = &inode.data {
                let base = if components.is_empty() {
                    "/".to_string()
                } else {
                    let parent_comps = &components[..components.len().saturating_sub(1)];
                    if parent_comps.is_empty() {
                        "/".to_string()
                    } else {
                        format!("/{}", parent_comps.join("/"))
                    }
                };
                let resolved = resolve_relative(&base, target);
                return self.resolve_depth(&resolved, true, depth + 1);
            }
        }
        Ok(current)
    }

    /// Get an inode by number.
    pub fn get(&self, ino: Ino) -> io::Result<&Inode> {
        self.inodes
            .get(&ino)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "stale inode"))
    }

    /// Get a mutable inode by number.
    pub fn get_mut(&mut self, ino: Ino) -> io::Result<&mut Inode> {
        self.inodes
            .get_mut(&ino)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "stale inode"))
    }

    /// Resolve the parent directory inode and the final component name.
    fn resolve_parent(&self, path: &str) -> io::Result<(Ino, String)> {
        let path = normalize(path);
        let (parent, name) = split_path(&path);
        let parent_ino = self.resolve(&parent, true)?;
        Ok((parent_ino, name))
    }

    /// Return the canonical path with all symlinks resolved.
    pub fn canonicalize_path(&self, path: &str) -> io::Result<String> {
        self.canonicalize_depth(path, 0)
    }

    fn canonicalize_depth(&self, path: &str, depth: u32) -> io::Result<String> {
        if depth > 40 {
            return Err(io::Error::other("too many levels of symbolic links"));
        }
        let path = normalize(path);
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let mut result = Vec::new();
        let mut current = ROOT_INO;

        for comp in &components {
            let inode = self.get(current)?;
            match &inode.data {
                InodeData::Dir(entries) => {
                    let child_ino = *entries.get(*comp).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "no such file or directory")
                    })?;
                    let child = self.get(child_ino)?;
                    if let InodeData::Symlink(target) = &child.data {
                        let base = if result.is_empty() {
                            "/".to_string()
                        } else {
                            format!("/{}", result.join("/"))
                        };
                        let resolved = resolve_relative(&base, target);
                        let canonical = self.canonicalize_depth(&resolved, depth + 1)?;
                        // Replace result with the resolved canonical components
                        result = canonical
                            .split('/')
                            .filter(|c| !c.is_empty())
                            .map(String::from)
                            .collect();
                        // Update current to the resolved inode
                        current = self.resolve(&canonical, true)?;
                    } else {
                        result.push(comp.to_string());
                        current = child_ino;
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "not a directory",
                    ));
                }
            }
        }

        Ok(format!("/{}", result.join("/")))
    }

    fn check_inode_limit(&self) -> io::Result<()> {
        if self.max_inodes > 0 && self.inodes.len() >= self.max_inodes {
            return Err(io::Error::other("filesystem inode limit exceeded"));
        }
        Ok(())
    }

    /// Create a regular file. Returns the new inode number.
    pub fn create_file(&mut self, path: &str, mode: u32, uid: Uid, gid: Gid) -> io::Result<Ino> {
        self.check_inode_limit()?;
        let effective_mode = 0o100000 | (mode & !self.umask);
        let (parent_ino, name) = self.resolve_parent(path)?;
        let inode = Inode::new(InodeData::File(Vec::new()), effective_mode, uid, gid);
        let ino = inode.ino;
        self.inodes.insert(ino, inode);
        self.dir_insert(parent_ino, &name, ino)?;
        Ok(ino)
    }

    /// Create a directory. Returns the new inode number.
    pub fn mkdir(&mut self, path: &str, mode: u32, uid: Uid, gid: Gid) -> io::Result<Ino> {
        self.check_inode_limit()?;
        let effective_mode = 0o040000 | (mode & !self.umask);
        let (parent_ino, name) = self.resolve_parent(path)?;
        let mut entries = HashMap::new();
        let ino = alloc_ino();
        entries.insert(".".into(), ino);
        entries.insert("..".into(), parent_ino);
        let inode = Inode {
            ino,
            data: InodeData::Dir(entries),
            mode: effective_mode,
            uid,
            gid,
            nlink: 2,
            mtime: SystemTime::now(),
        };
        self.inodes.insert(ino, inode);
        self.dir_insert(parent_ino, &name, ino)?;
        // Increment parent nlink for the ".." entry
        if let Some(p) = self.inodes.get_mut(&parent_ino) {
            p.nlink += 1;
        }
        Ok(ino)
    }

    /// Create a directory and all missing parents (like mkdir -p).
    pub fn mkdir_p(&mut self, path: &str, mode: u32, uid: Uid, gid: Gid) -> io::Result<Ino> {
        let path = normalize(path);
        let mut current = ROOT_INO;
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            let inode = self.get(current)?;
            if let InodeData::Dir(entries) = &inode.data
                && let Some(&child) = entries.get(comp)
            {
                current = child;
                continue;
            }
            // Need to create this component
            let child_path = {
                // Build the full path up to this component
                let parent_path = self.inode_path(current);
                if parent_path == "/" {
                    format!("/{}", comp)
                } else {
                    format!("{}/{}", parent_path, comp)
                }
            };
            current = self.mkdir(&child_path, mode, uid, gid)?;
        }
        Ok(current)
    }

    /// Create a symbolic link.
    pub fn symlink(
        &mut self,
        link_path: &str,
        target: &str,
        uid: Uid,
        gid: Gid,
    ) -> io::Result<Ino> {
        self.check_inode_limit()?;
        let (parent_ino, name) = self.resolve_parent(link_path)?;
        let inode = Inode::new(InodeData::Symlink(target.to_string()), 0o120777, uid, gid);
        let ino = inode.ino;
        self.inodes.insert(ino, inode);
        self.dir_insert(parent_ino, &name, ino)?;
        Ok(ino)
    }

    /// Create a hard link: new_path points to the same inode as existing_path.
    pub fn hard_link(&mut self, existing_path: &str, new_path: &str) -> io::Result<()> {
        let target_ino = self.resolve(existing_path, true)?;
        // Can't hard-link directories
        if let InodeData::Dir(_) = &self.get(target_ino)?.data {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "hard link to directory not allowed",
            ));
        }
        let (parent_ino, name) = self.resolve_parent(new_path)?;
        self.dir_insert(parent_ino, &name, target_ino)?;
        self.get_mut(target_ino)?.nlink += 1;
        Ok(())
    }

    /// Create a device node.
    pub fn mknod(
        &mut self,
        path: &str,
        data: InodeData,
        mode: u32,
        uid: Uid,
        gid: Gid,
    ) -> io::Result<Ino> {
        self.check_inode_limit()?;
        let (parent_ino, name) = self.resolve_parent(path)?;
        let inode = Inode::new(data, mode, uid, gid);
        let ino = inode.ino;
        self.inodes.insert(ino, inode);
        self.dir_insert(parent_ino, &name, ino)?;
        Ok(ino)
    }

    /// Remove a file (unlink).
    pub fn unlink(&mut self, path: &str) -> io::Result<()> {
        let (parent_ino, name) = self.resolve_parent(path)?;
        let child_ino = self.dir_lookup(parent_ino, &name)?;
        let child = self.get(child_ino)?;
        if let InodeData::Dir(_) = &child.data {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "is a directory",
            ));
        }
        self.dir_remove(parent_ino, &name)?;
        let inode = self.get_mut(child_ino)?;
        inode.nlink -= 1;
        if inode.nlink == 0 {
            self.inodes.remove(&child_ino);
        }
        Ok(())
    }

    /// Remove an empty directory.
    pub fn rmdir(&mut self, path: &str) -> io::Result<()> {
        let (parent_ino, name) = self.resolve_parent(path)?;
        let child_ino = self.dir_lookup(parent_ino, &name)?;
        let child = self.get(child_ino)?;
        match &child.data {
            InodeData::Dir(entries) => {
                // Only . and .. should remain
                if entries.len() > 2 {
                    return Err(io::Error::other("directory not empty"));
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "not a directory",
                ));
            }
        }
        self.dir_remove(parent_ino, &name)?;
        // Decrement parent nlink
        if let Some(p) = self.inodes.get_mut(&parent_ino) {
            p.nlink = p.nlink.saturating_sub(1);
        }
        self.inodes.remove(&child_ino);
        Ok(())
    }

    /// Rename a file or directory.
    pub fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let (from_parent, from_name) = self.resolve_parent(from)?;
        let child_ino = self.dir_lookup(from_parent, &from_name)?;

        let (to_parent, to_name) = self.resolve_parent(to)?;

        // If destination exists, remove it first
        if let Ok(existing) = self.dir_lookup(to_parent, &to_name) {
            let ex = self.get(existing)?;
            if let InodeData::Dir(entries) = &ex.data {
                if entries.len() > 2 {
                    return Err(io::Error::other("directory not empty"));
                }
                self.inodes.remove(&existing);
                if let Some(p) = self.inodes.get_mut(&to_parent) {
                    p.nlink = p.nlink.saturating_sub(1);
                }
            } else {
                let ex_mut = self.get_mut(existing)?;
                ex_mut.nlink -= 1;
                if ex_mut.nlink == 0 {
                    self.inodes.remove(&existing);
                }
            }
            self.dir_remove(to_parent, &to_name)?;
        }

        self.dir_remove(from_parent, &from_name)?;
        self.dir_insert(to_parent, &to_name, child_ino)?;

        // Update ".." in moved directory
        if let InodeData::Dir(_) = &self.get(child_ino)?.data
            && from_parent != to_parent
        {
            if let Some(p) = self.inodes.get_mut(&from_parent) {
                p.nlink = p.nlink.saturating_sub(1);
            }
            if let Some(p) = self.inodes.get_mut(&to_parent) {
                p.nlink += 1;
            }
            if let InodeData::Dir(entries) = &mut self.get_mut(child_ino)?.data {
                entries.insert("..".into(), to_parent);
            }
        }
        Ok(())
    }

    /// Read file contents.
    pub fn read_file(&self, ino: Ino) -> io::Result<&[u8]> {
        match &self.get(ino)?.data {
            InodeData::File(data) => Ok(data),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            )),
        }
    }

    /// Write file contents (replace entirely).
    pub fn write_file(&mut self, ino: Ino, data: Vec<u8>) -> io::Result<()> {
        if self.max_file_size > 0 && data.len() > self.max_file_size {
            return Err(io::Error::other("file size limit exceeded"));
        }
        let inode = self.get_mut(ino)?;
        match &mut inode.data {
            InodeData::File(buf) => {
                *buf = data;
                inode.mtime = SystemTime::now();
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            )),
        }
    }

    /// Append to file contents.
    pub fn append_file(&mut self, ino: Ino, data: &[u8]) -> io::Result<()> {
        let max = self.max_file_size;
        let inode = self.get_mut(ino)?;
        match &mut inode.data {
            InodeData::File(buf) => {
                if max > 0 && buf.len() + data.len() > max {
                    return Err(io::Error::other("file size limit exceeded"));
                }
                buf.extend_from_slice(data);
                inode.mtime = SystemTime::now();
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            )),
        }
    }

    /// List directory entries (excluding . and ..).
    pub fn read_dir(&self, ino: Ino) -> io::Result<Vec<(String, Ino)>> {
        match &self.get(ino)?.data {
            InodeData::Dir(entries) => {
                let mut result: Vec<_> = entries
                    .iter()
                    .filter(|(name, _)| name.as_str() != "." && name.as_str() != "..")
                    .map(|(name, &ino)| (name.clone(), ino))
                    .collect();
                result.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(result)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )),
        }
    }

    /// Check if a user has the given permission bits on an inode.
    pub fn check_permission(&self, ino: Ino, uid: Uid, gid: Gid, want: u32) -> bool {
        let inode = match self.get(ino) {
            Ok(i) => i,
            Err(_) => return false,
        };
        if uid == ROOT_UID {
            return true;
        }
        let bits = if uid == inode.uid {
            (inode.mode >> 6) & 7
        } else if gid == inode.gid {
            (inode.mode >> 3) & 7
        } else {
            inode.mode & 7
        };
        bits & want == want
    }

    /// Convert an inode to a FileStat.
    pub fn inode_to_filestat(&self, ino: Ino) -> crate::os::FileStat {
        match self.get(ino) {
            Err(_) => crate::os::FileStat::default(),
            Ok(inode) => {
                let (is_file, is_dir, is_symlink, is_char_device, is_block_device, is_fifo, len) =
                    match &inode.data {
                        InodeData::File(d) => {
                            (true, false, false, false, false, false, d.len() as u64)
                        }
                        InodeData::Dir(_) => (false, true, false, false, false, false, 0),
                        InodeData::Symlink(t) => {
                            (false, false, true, false, false, false, t.len() as u64)
                        }
                        InodeData::CharDevice(_, _) => (false, false, false, true, false, false, 0),
                        InodeData::BlockDevice(_, _) => {
                            (false, false, false, false, true, false, 0)
                        }
                        InodeData::Fifo => (false, false, false, false, false, true, 0),
                        InodeData::HostFile(_, _) => (true, false, false, false, false, false, 0),
                        InodeData::HostDir(_, _) => (false, true, false, false, false, false, 0),
                    };
                crate::os::FileStat {
                    exists: true,
                    is_file,
                    is_dir,
                    is_symlink,
                    len,
                    is_socket: false,
                    is_fifo,
                    is_block_device,
                    is_char_device,
                    mode: inode.mode,
                    dev: 0,
                    ino: inode.ino,
                    modified: Some(inode.mtime),
                }
            }
        }
    }

    /// Get the path of an inode (for mkdir_p helper). Slow but only used during setup.
    fn inode_path(&self, target: Ino) -> String {
        if target == ROOT_INO {
            return "/".to_string();
        }
        // BFS from root
        fn find(vfs: &Vfs, current: Ino, target: Ino, path: &str) -> Option<String> {
            if let InodeData::Dir(entries) = &vfs.inodes.get(&current)?.data {
                for (name, &child) in entries {
                    if name == "." || name == ".." {
                        continue;
                    }
                    let child_path = if path == "/" {
                        format!("/{name}")
                    } else {
                        format!("{path}/{name}")
                    };
                    if child == target {
                        return Some(child_path);
                    }
                    if let Some(InodeData::Dir(_)) = vfs.inodes.get(&child).map(|i| &i.data)
                        && let Some(p) = find(vfs, child, target, &child_path)
                    {
                        return Some(p);
                    }
                }
            }
            None
        }
        find(self, ROOT_INO, target, "/").unwrap_or_else(|| "/".to_string())
    }

    // --- internal helpers ---

    fn dir_lookup(&self, dir_ino: Ino, name: &str) -> io::Result<Ino> {
        match &self.get(dir_ino)?.data {
            InodeData::Dir(entries) => entries.get(name).copied().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{}: no such file or directory", name),
                )
            }),
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )),
        }
    }

    fn dir_insert(&mut self, dir_ino: Ino, name: &str, child_ino: Ino) -> io::Result<()> {
        let dir = self.get_mut(dir_ino)?;
        match &mut dir.data {
            InodeData::Dir(entries) => {
                if entries.contains_key(name) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{}: already exists", name),
                    ));
                }
                entries.insert(name.to_string(), child_ino);
                dir.mtime = SystemTime::now();
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )),
        }
    }

    fn dir_remove(&mut self, dir_ino: Ino, name: &str) -> io::Result<()> {
        let dir = self.get_mut(dir_ino)?;
        match &mut dir.data {
            InodeData::Dir(entries) => {
                entries.remove(name).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("{}: not found", name))
                })?;
                dir.mtime = SystemTime::now();
                Ok(())
            }
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )),
        }
    }
}

/// Normalize a path: resolve `.` and `..` without touching the filesystem.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Split a path into (parent, basename).
fn split_path(path: &str) -> (String, String) {
    let path = normalize(path);
    if path == "/" {
        return ("/".into(), "/".into());
    }
    match path.rfind('/') {
        Some(0) => ("/".into(), path[1..].into()),
        Some(i) => (path[..i].into(), path[i + 1..].into()),
        None => ("/".into(), path),
    }
}

/// Resolve a possibly-relative symlink target against a base directory.
fn resolve_relative(base: &str, target: &str) -> String {
    if target.starts_with('/') {
        normalize(target)
    } else {
        normalize(&format!("{}/{}", base, target))
    }
}

/// Populate standard device nodes.
pub fn create_dev_nodes(vfs: &mut Vfs) -> io::Result<()> {
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID)?;
    vfs.mknod(
        "/dev/null",
        InodeData::CharDevice(1, 3),
        0o020666,
        ROOT_UID,
        ROOT_GID,
    )?;
    vfs.mknod(
        "/dev/zero",
        InodeData::CharDevice(1, 5),
        0o020666,
        ROOT_UID,
        ROOT_GID,
    )?;
    vfs.mknod(
        "/dev/urandom",
        InodeData::CharDevice(1, 9),
        0o020666,
        ROOT_UID,
        ROOT_GID,
    )?;
    vfs.mknod(
        "/dev/random",
        InodeData::CharDevice(1, 8),
        0o020666,
        ROOT_UID,
        ROOT_GID,
    )?;
    Ok(())
}

/// Create /bin/lash and symlink all supported commands to it.
pub fn create_bin_links(vfs: &mut Vfs) -> io::Result<()> {
    // Create the lash binary (empty file, mode 711)
    let ino = vfs.create_file("/bin/lash", 0o711, ROOT_UID, ROOT_GID)?;
    // Override mode directly since create_file applies umask
    vfs.get_mut(ino)?.mode = 0o100711;

    // Builtins that should appear in /bin
    const BUILTINS: &[&str] = &[
        "echo", "false", "find", "printf", "pwd", "test", "true", "xargs",
    ];

    for &name in BUILTINS {
        vfs.symlink(&format!("/bin/{name}"), "lash", ROOT_UID, ROOT_GID)?;
    }

    // /bin/sh -> lash
    vfs.symlink("/bin/sh", "lash", ROOT_UID, ROOT_GID)?;

    // /bin/lua -> lash (Lua interpreter)
    vfs.symlink("/bin/lua", "lash", ROOT_UID, ROOT_GID)?;

    // External commands registered via inventory (native) or static list (WASM)
    #[cfg(not(target_arch = "wasm32"))]
    for entry in inventory::iter::<crate::commands::CommandEntry> {
        let path = format!("/bin/{}", entry.name);
        if vfs.resolve(&path, false).is_err() {
            vfs.symlink(&path, "lash", ROOT_UID, ROOT_GID)?;
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Create /bin symlinks for all known commands on WASM
        let cmds = [
            "basename", "cat", "chmod", "cp", "cut", "date", "dirname", "echo", "env", "false",
            "grep", "head", "jq", "ln", "ls", "mkdir", "mktemp", "mv", "pwd", "readlink", "rm",
            "rmdir", "sed", "sort", "tail", "tee", "touch", "tr", "true", "uniq", "wc",
        ];
        for name in cmds {
            let path = format!("/bin/{name}");
            if vfs.resolve(&path, false).is_err() {
                vfs.symlink(&path, "lash", ROOT_UID, ROOT_GID)?;
            }
        }
    }

    Ok(())
}

/// Populate the VFS by copying a host directory tree into a virtual destination.
pub fn copy_from_host(
    vfs: &mut Vfs,
    src: &std::path::Path,
    dest: &str,
    uid: Uid,
    gid: Gid,
) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() {
        // Create dest dir if it doesn't exist
        if vfs.resolve(dest, true).is_err() {
            vfs.mkdir(dest, 0o755, uid, gid)?;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_dest = if dest == "/" {
                format!("/{name}")
            } else {
                format!("{dest}/{name}")
            };
            copy_from_host(vfs, &entry.path(), &child_dest, uid, gid)?;
        }
    } else if meta.is_symlink() {
        let target = std::fs::read_link(src)?;
        vfs.symlink(dest, &target.to_string_lossy(), uid, gid)?;
    } else if meta.is_file() {
        let data = std::fs::read(src)?;
        let ino = vfs.create_file(dest, 0o644, uid, gid)?;
        vfs.write_file(ino, data)?;
        // Preserve executable bit
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let host_mode = meta.permissions().mode();
            if host_mode & 0o111 != 0 {
                let inode = vfs.get_mut(ino)?;
                inode.mode = 0o100755;
            }
        }
    }
    Ok(())
}
