use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::os::*;
use crate::vfs::{self, InodeData, LASH_GID, LASH_UID, Vfs};
use crate::vfs_config::{CredKind, ResolvedCred};

/// A Kernel backed entirely by the in-memory VFS.
pub struct VfsKernel {
    pub vfs: Arc<Mutex<Vfs>>,
    pub creds: Vec<ResolvedCred>,
    pub allowed_url_prefixes: Vec<String>,
    /// Optional Cedar authorization policy. When `None`, all `check_policy`
    /// calls allow (unchanged behavior); when `Some`, gated actions must be
    /// permitted by the policy. Layers on top of the SSRF/VFS checks.
    #[cfg(not(target_arch = "wasm32"))]
    pub policy: Option<Arc<crate::policy::PolicyEngine>>,
}

impl VfsKernel {
    pub fn new(vfs: Vfs, creds: Vec<ResolvedCred>) -> Self {
        Self {
            vfs: Arc::new(Mutex::new(vfs)),
            creds,
            allowed_url_prefixes: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            policy: None,
        }
    }

    /// Resolve a path relative to the process cwd, producing an absolute virtual path.
    fn abs(proc: &Process, path: &str) -> String {
        if path.starts_with('/') {
            vfs::normalize(path)
        } else {
            vfs::normalize(&format!("{}/{}", proc.cwd.display(), path))
        }
    }

    /// Check that the user has write permission on the parent directory of `abs_path`.
    fn check_parent_write(vfs: &Vfs, abs_path: &str) -> io::Result<()> {
        let parent = match abs_path.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(i) => abs_path[..i].to_string(),
        };
        if let Ok(parent_ino) = vfs.resolve(&parent, true)
            && !vfs.check_permission(parent_ino, LASH_UID, LASH_GID, 2)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            ));
        }
        Ok(())
    }

    /// Check if a VFS path resolves to a host-backed inode (HostFile or HostDir).
    /// If the exact path is a HostFile/HostDir, returns the host path and readonly flag.
    /// If an ancestor is a HostDir, returns the host path with remaining components appended.
    /// For HostDir children, the resolved host path is canonicalized and verified
    /// to remain within the bind mount base to prevent symlink traversal escapes.
    /// Returns (host_path, readonly, canon_base) where canon_base is the
    /// canonicalized bind mount root (used by open_host for fd verification).
    fn resolve_host(vfs: &Vfs, abs_path: &str) -> Option<(PathBuf, bool, PathBuf)> {
        // First try exact match
        if let Ok(ino) = vfs.resolve(abs_path, true)
            && let Ok(inode) = vfs.get(ino)
        {
            match &inode.data {
                InodeData::HostFile(p, ro) | InodeData::HostDir(p, ro) => {
                    let pb = PathBuf::from(p);
                    let base = std::fs::canonicalize(&pb).unwrap_or_else(|_| pb.clone());
                    return Some((pb, *ro, base));
                }
                _ => {}
            }
        }
        // Walk components looking for a HostDir ancestor
        let components: Vec<&str> = abs_path.split('/').filter(|c| !c.is_empty()).collect();
        for i in (0..components.len()).rev() {
            let prefix = format!("/{}", components[..=i].join("/"));
            if let Ok(ino) = vfs.resolve(&prefix, true)
                && let Ok(inode) = vfs.get(ino)
                && let InodeData::HostDir(host_base, ro) = &inode.data
            {
                let rest = &components[i + 1..];
                let mut host = PathBuf::from(host_base);
                for c in rest {
                    host.push(c);
                }
                // Canonicalize and verify the path stays within the bind mount
                let canon_base =
                    std::fs::canonicalize(host_base).unwrap_or_else(|_| PathBuf::from(host_base));
                if let Ok(canon_host) = std::fs::canonicalize(&host) {
                    if !canon_host.starts_with(&canon_base) {
                        return None; // symlink escape — block access
                    }
                    return Some((canon_host, *ro, canon_base));
                }
                // canonicalize failed — check if path is a dangling symlink
                if host.symlink_metadata().is_ok() {
                    return None; // dangling symlink pointing outside mount
                }
                // Path truly doesn't exist — verify parent is safe
                if let Some(parent) = host.parent()
                    && let Ok(canon_parent) = std::fs::canonicalize(parent)
                    && !canon_parent.starts_with(&canon_base)
                {
                    return None;
                }
                return Some((host, *ro, canon_base));
            }
        }
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn open_host(
        &self,
        proc: &mut Process,
        host_path: &std::path::Path,
        flags: &OpenFlags,
        canon_base: &std::path::Path,
    ) -> io::Result<Fd> {
        if flags.read && !flags.write {
            let file = std::fs::File::open(host_path)?;

            // Defense-in-depth TOCTOU re-check (Linux-only): /proc/self/fd has
            // no portable equivalent, and the canonical-path check above is the
            // primary guard, so this extra layer is simply skipped elsewhere.
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::io::AsRawFd;
                let fd_path = format!("/proc/self/fd/{}", file.as_raw_fd());
                if let Ok(real) = std::fs::read_link(&fd_path)
                    && !real.starts_with(canon_base)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "access denied: path escaped bind mount",
                    ));
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = canon_base; // only consumed by the Linux-only check above

            use std::io::Read;
            let mut data = Vec::new();
            let mut file = file;
            file.read_to_end(&mut data)?;
            let (tx, rx) = crate::os::pipe(data.len().max(64));
            let _ = tx.send(bytes::Bytes::from(data)).await;
            drop(tx);
            proc.alloc_fd(FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            })
        } else if flags.write {
            let (tx, rx) = crate::os::pipe(8192);
            let size_error = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let fd = proc.alloc_fd(FdKind::ChannelWriter {
                tx,
                error_flag: Some(size_error.clone()),
            })?;
            let path = host_path.to_path_buf();
            let append = flags.append;
            let max_file_size = self.vfs.lock().await.max_file_size;
            tokio::task::spawn_local(async move {
                let mut rx = rx;
                let mut buf = if append {
                    tokio::fs::read(&path).await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                while let Some(chunk) = rx.recv().await {
                    if max_file_size > 0 && buf.len() + chunk.len() > max_file_size {
                        size_error.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
                let _ = tokio::fs::write(&path, &buf).await;
            });
            Ok(fd)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid open flags",
            ))
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn open_host(
        &self,
        proc: &mut Process,
        host_path: &std::path::Path,
        flags: &OpenFlags,
        _canon_base: &std::path::Path,
    ) -> io::Result<Fd> {
        if flags.read && !flags.write {
            use std::io::Read;
            let mut file = std::fs::File::open(host_path)?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            let (tx, rx) = crate::os::pipe(data.len().max(64));
            let _ = tx.send(bytes::Bytes::from(data)).await;
            drop(tx);
            proc.alloc_fd(FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            })
        } else if flags.write {
            let (tx, rx) = crate::os::pipe(8192);
            let size_error = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let fd = proc.alloc_fd(FdKind::ChannelWriter {
                tx,
                error_flag: Some(size_error.clone()),
            })?;
            let path = host_path.to_path_buf();
            let append = flags.append;
            let max_file_size = self.vfs.lock().await.max_file_size;
            tokio::task::spawn_local(async move {
                let mut rx = rx;
                let mut buf = if append {
                    std::fs::read(&path).unwrap_or_default()
                } else {
                    Vec::new()
                };
                while let Some(chunk) = rx.recv().await {
                    if max_file_size > 0 && buf.len() + chunk.len() > max_file_size {
                        size_error.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
                let _ = std::fs::write(&path, &buf);
            });
            Ok(fd)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid open flags",
            ))
        }
    }
}

#[async_trait]
impl Kernel for VfsKernel {
    fn new_process(&self) -> Process {
        let cwd = PathBuf::from("/home/lash");
        let mut env = HashMap::new();
        env.insert("HOME".into(), "/home/lash".into());
        env.insert("PWD".into(), "/home/lash".into());
        env.insert("PATH".into(), "/usr/bin:/bin".into());
        env.insert("USER".into(), "lash".into());
        Process::new(cwd, env)
    }

    async fn open(&self, proc: &mut Process, path: &str, flags: OpenFlags) -> io::Result<Fd> {
        let abs = Self::abs(proc, path);

        // Policy gate. A pure read is fs:read; otherwise it's a mutation — and
        // we distinguish creating a new file (fs:create) from modifying an
        // existing one (fs:write) by probing existence. Read-write (`<>`) maps
        // to fs:write since it can mutate.
        {
            let action = if !flags.write && !flags.create {
                "fs:read"
            } else if flags.create {
                let exists = self.vfs.lock().await.resolve(&abs, true).is_ok();
                if exists { "fs:write" } else { "fs:create" }
            } else {
                "fs:write"
            };
            self.check_policy(action, &[("path", &abs)])?;
        }

        // Check for host-backed path (bind_direct passthrough)
        {
            let vfs = self.vfs.lock().await;
            if let Some((host_path, ro, canon_base)) = Self::resolve_host(&vfs, &abs) {
                drop(vfs);
                if ro && (flags.write || flags.create || flags.truncate) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "read-only bind mount",
                    ));
                }
                return self.open_host(proc, &host_path, &flags, &canon_base).await;
            }
        }

        let mut vfs = self.vfs.lock().await;

        let ino = if flags.create {
            match vfs.resolve(&abs, true) {
                Ok(ino) => {
                    // File exists — check write permission on the file for truncate
                    if (flags.write || flags.truncate)
                        && !vfs.check_permission(ino, LASH_UID, LASH_GID, 2)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "permission denied",
                        ));
                    }
                    if flags.truncate && matches!(vfs.get(ino)?.data, InodeData::File(_)) {
                        vfs.write_file(ino, Vec::new())?;
                    }
                    ino
                }
                Err(_) => {
                    // Create new file — need write permission on parent directory
                    Self::check_parent_write(&vfs, &abs)?;
                    vfs.create_file(&abs, 0o644, LASH_UID, LASH_GID)?
                }
            }
        } else {
            let ino = vfs.resolve(&abs, true)?;
            if flags.write && !vfs.check_permission(ino, LASH_UID, LASH_GID, 2) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "permission denied",
                ));
            }
            ino
        };

        // For device nodes, return special readers/writers
        let inode = vfs.get(ino)?;
        match &inode.data {
            InodeData::CharDevice(1, 3) => {
                // /dev/null
                drop(vfs);
                return make_dev_null_fd(proc, &flags);
            }
            InodeData::CharDevice(1, 5) => {
                // /dev/zero
                drop(vfs);
                return make_dev_zero_fd(proc, &flags);
            }
            InodeData::CharDevice(1, 8) | InodeData::CharDevice(1, 9) => {
                // /dev/random, /dev/urandom
                drop(vfs);
                return make_dev_urandom_fd(proc, &flags);
            }
            _ => {}
        }

        // For regular files, create a channel-based fd backed by the file data
        let data = match &inode.data {
            InodeData::File(d) => d.clone(),
            InodeData::Dir(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "is a directory",
                ));
            }
            _ => Vec::new(),
        };

        drop(vfs);

        if flags.read && !flags.write {
            let (tx, rx) = crate::os::pipe(data.len().max(64));
            let _ = tx.send(bytes::Bytes::from(data)).await;
            drop(tx);
            proc.alloc_fd(FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            })
        } else if flags.read && flags.write {
            // Read-write (<>): provide existing data as a reader.
            // The channel model doesn't support true read-write on one fd,
            // so we give a reader seeded with the current contents.
            let (tx, rx) = crate::os::pipe(data.len().max(64));
            let _ = tx.send(bytes::Bytes::from(data)).await;
            drop(tx);
            proc.alloc_fd(FdKind::ChannelReader {
                rx,
                buf: Vec::new(),
            })
        } else if flags.write {
            // For writes, we use a channel that collects data and flushes to VFS
            let vfs_ref = self.vfs.clone();
            let max_file_size = self.vfs.lock().await.max_file_size;
            // Check if file already exceeds limit (catches append loops)
            if flags.append && max_file_size > 0 && data.len() >= max_file_size {
                return Err(io::Error::other("file size limit exceeded"));
            }
            let (tx, rx) = crate::os::pipe(8192);
            let size_error = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let fd = proc.alloc_fd(FdKind::ChannelWriter {
                tx,
                error_flag: Some(size_error.clone()),
            })?;
            let append = flags.append;
            // Spawn a task to collect writes and flush to VFS
            tokio::task::spawn_local(async move {
                let mut rx = rx;
                let mut buf = if append {
                    let v = vfs_ref.lock().await;
                    v.read_file(ino).unwrap_or(&[]).to_vec()
                } else {
                    Vec::new()
                };
                while let Some(chunk) = rx.recv().await {
                    if max_file_size > 0 && buf.len() + chunk.len() > max_file_size {
                        size_error.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
                let mut v = vfs_ref.lock().await;
                let _ = v.write_file(ino, buf);
            });
            Ok(fd)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid open flags",
            ))
        }
    }

    async fn list_dir(&self, proc: &Process, path: &str) -> io::Result<Vec<DirEntry>> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:list", &[("path", &abs)])?;
        let vfs = self.vfs.lock().await;

        if let Some((host_path, _ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&host_path)? {
                let entry = entry?;
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_dir,
                });
            }
            return Ok(entries);
        }

        let ino = vfs.resolve(&abs, true)?;
        let entries = vfs.read_dir(ino)?;
        Ok(entries
            .into_iter()
            .map(|(name, child_ino)| {
                let is_dir = vfs
                    .get(child_ino)
                    .map(|i| matches!(i.data, InodeData::Dir(_)))
                    .unwrap_or(false);
                DirEntry { name, is_dir }
            })
            .collect())
    }

    async fn change_dir(&self, proc: &mut Process, path: &str) -> io::Result<()> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:list", &[("path", &abs)])?;
        let vfs = self.vfs.lock().await;

        if let Some((host_path, _ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            let meta = std::fs::metadata(&host_path)?;
            if !meta.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "Not a directory",
                ));
            }
            proc.cwd = PathBuf::from(&abs);
            return Ok(());
        }

        let ino = vfs.resolve(&abs, true)?;
        let inode = vfs.get(ino)?;
        if !matches!(inode.data, InodeData::Dir(_)) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Not a directory",
            ));
        }
        proc.cwd = PathBuf::from(&abs);
        Ok(())
    }

    async fn stat(&self, proc: &Process, path: &str) -> FileStat {
        let abs = Self::abs(proc, path);
        // stat is infallible; a policy denial fails closed to "does not exist".
        if self.check_policy("fs:stat", &[("path", &abs)]).is_err() {
            return FileStat::default();
        }
        let vfs = self.vfs.lock().await;

        if let Some((host_path, _ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            #[cfg(not(target_arch = "wasm32"))]
            return host_stat(&host_path).await;
            #[cfg(target_arch = "wasm32")]
            return host_stat_sync(&host_path);
        }

        match vfs.resolve(&abs, true) {
            Ok(ino) => vfs.inode_to_filestat(ino),
            Err(_) => FileStat::default(),
        }
    }

    async fn lstat(&self, proc: &Process, path: &str) -> FileStat {
        let abs = Self::abs(proc, path);
        // lstat is infallible; a policy denial fails closed to "does not exist".
        if self.check_policy("fs:stat", &[("path", &abs)]).is_err() {
            return FileStat::default();
        }
        let vfs = self.vfs.lock().await;

        if let Some((host_path, _ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            #[cfg(not(target_arch = "wasm32"))]
            return host_stat(&host_path).await;
            #[cfg(target_arch = "wasm32")]
            return host_stat_sync(&host_path);
        }

        match vfs.resolve(&abs, false) {
            Ok(ino) => vfs.inode_to_filestat(ino),
            Err(_) => FileStat::default(),
        }
    }

    async fn access(&self, proc: &Process, path: &str, mode: i32) -> bool {
        let abs = Self::abs(proc, path);
        let vfs = self.vfs.lock().await;

        if let Some((host_path, ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            if mode == 0 {
                return std::fs::metadata(&host_path).is_ok();
            }
            if ro && mode & ACCESS_W != 0 {
                return false;
            }
            let meta = match std::fs::metadata(&host_path) {
                Ok(m) => m,
                Err(_) => return false,
            };
            if mode & ACCESS_W != 0 && meta.permissions().readonly() {
                return false;
            }
            return true;
        }
        {
            let ino = match vfs.resolve(&abs, true) {
                Ok(i) => i,
                Err(_) => return false,
            };
            if mode == 0 {
                return true;
            }
            let want =
                ((mode & ACCESS_R) >> 2) << 2 | ((mode & ACCESS_W) >> 1) << 1 | (mode & ACCESS_X);
            vfs.check_permission(ino, LASH_UID, LASH_GID, want as u32)
        }
    }

    async fn canonicalize(&self, proc: &Process, path: &str) -> io::Result<PathBuf> {
        let abs = Self::abs(proc, path);
        let vfs = self.vfs.lock().await;
        // Resolve to verify the path exists and follow symlinks
        let _ = vfs.resolve(&abs, true)?;
        Ok(PathBuf::from(vfs.canonicalize_path(&abs)?))
    }

    async fn is_executable(&self, proc: &Process, path: &str) -> bool {
        let abs = Self::abs(proc, path);
        let vfs = self.vfs.lock().await;

        if let Some((_host_path, _ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if let Ok(m) = std::fs::metadata(&_host_path) {
                    return m.is_file() && m.mode() & 0o111 != 0;
                }
            }
            return false;
        }

        match vfs.resolve(&abs, true) {
            Ok(ino) => {
                let inode = match vfs.get(ino) {
                    Ok(i) => i,
                    Err(_) => return false,
                };
                matches!(inode.data, InodeData::File(_)) && inode.mode & 0o111 != 0
            }
            Err(_) => false,
        }
    }

    async fn glob(&self, proc: &Process, pattern: &str) -> Vec<String> {
        let abs_pattern = Self::abs(proc, pattern);
        let vfs = self.vfs.lock().await;
        let mut results = Vec::new();
        glob_vfs(&vfs, &abs_pattern, &mut results);
        // Convert back to relative if pattern was relative
        if !pattern.starts_with('/') {
            let cwd = format!("{}/", proc.cwd.display());
            results = results
                .into_iter()
                .map(|p| p.strip_prefix(&cwd).unwrap_or(&p).to_string())
                .collect();
        }
        results.sort();
        results
    }

    fn isatty(&self, _fd: i32) -> bool {
        false
    }

    async fn remove_file(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:delete", &[("path", &abs)])?;
        let vfs = self.vfs.lock().await;
        if let Some((host_path, ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            if ro {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "read-only bind mount",
                ));
            }
            return std::fs::remove_file(&host_path);
        }
        Self::check_parent_write(&vfs, &abs)?;
        drop(vfs);
        self.vfs.lock().await.unlink(&abs)
    }

    async fn remove_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:delete", &[("path", &abs)])?;
        let vfs = self.vfs.lock().await;
        if let Some((host_path, ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            if ro {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "read-only bind mount",
                ));
            }
            return std::fs::remove_dir(&host_path);
        }
        Self::check_parent_write(&vfs, &abs)?;
        drop(vfs);
        self.vfs.lock().await.rmdir(&abs)
    }

    async fn create_dir(&self, proc: &Process, path: &str) -> io::Result<()> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:create", &[("path", &abs)])?;
        let vfs = self.vfs.lock().await;
        if let Some((host_path, ro, _)) = Self::resolve_host(&vfs, &abs) {
            drop(vfs);
            if ro {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "read-only bind mount",
                ));
            }
            return std::fs::create_dir(&host_path);
        }
        Self::check_parent_write(&vfs, &abs)?;
        drop(vfs);
        self.vfs
            .lock()
            .await
            .mkdir(&abs, 0o755, LASH_UID, LASH_GID)?;
        Ok(())
    }

    async fn rename(&self, proc: &Process, from: &str, to: &str) -> io::Result<()> {
        let abs_from = Self::abs(proc, from);
        let abs_to = Self::abs(proc, to);
        self.check_policy("fs:rename", &[("src", &abs_from), ("dst", &abs_to)])?;
        let vfs = self.vfs.lock().await;
        let host_from = Self::resolve_host(&vfs, &abs_from);
        let host_to = Self::resolve_host(&vfs, &abs_to);
        if host_from.is_none() && host_to.is_none() {
            Self::check_parent_write(&vfs, &abs_from)?;
            Self::check_parent_write(&vfs, &abs_to)?;
        }
        drop(vfs);
        match (host_from, host_to) {
            (Some((_, true, _)), _) | (_, Some((_, true, _))) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read-only bind mount",
            )),
            (Some((hf, _, _)), Some((ht, _, _))) => std::fs::rename(&hf, &ht),
            (None, None) => self.vfs.lock().await.rename(&abs_from, &abs_to),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot rename between host and virtual filesystem",
            )),
        }
    }

    async fn symlink(&self, proc: &Process, target: &str, link: &str) -> io::Result<()> {
        let abs_link = Self::abs(proc, link);
        self.check_policy("fs:create", &[("path", &abs_link)])?;
        let mut vfs = self.vfs.lock().await;
        Self::check_parent_write(&vfs, &abs_link)?;
        vfs.symlink(&abs_link, target, LASH_UID, LASH_GID)?;
        Ok(())
    }

    async fn read_link(&self, proc: &Process, path: &str) -> io::Result<String> {
        let abs = Self::abs(proc, path);
        let vfs = self.vfs.lock().await;
        let ino = vfs.resolve(&abs, false)?;
        match &vfs.get(ino)?.data {
            InodeData::Symlink(target) => Ok(target.clone()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a symbolic link",
            )),
        }
    }

    async fn set_permissions(&self, proc: &Process, path: &str, mode: u32) -> io::Result<()> {
        let abs = Self::abs(proc, path);
        self.check_policy("fs:write", &[("path", &abs)])?;
        let mut vfs = self.vfs.lock().await;
        let ino = vfs.resolve(&abs, true)?;
        let inode = vfs.get_mut(ino)?;
        // Only the file owner can chmod
        if inode.uid != LASH_UID && LASH_UID != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            ));
        }
        // Preserve the file type bits, replace permission bits
        inode.mode = (inode.mode & 0o170000) | (mode & 0o7777);
        Ok(())
    }

    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn check_policy(&self, action: &str, fields: &[(&str, &str)]) -> io::Result<()> {
        match &self.policy {
            Some(engine) => engine.check(action, fields),
            None => Ok(()),
        }
    }

    fn check_url(&self, url: &str) -> io::Result<()> {
        // Match the allowlist on the *parsed* URL, not a raw string prefix.
        // String-prefix matching is fooled by userinfo injection: against an
        // allowlist of `http://127.0.0.1:1234`, the URL
        // `http://127.0.0.1:1234@169.254.169.254/` prefix-matches (the `:` is a
        // boundary char) yet its real host is 169.254.169.254 — a clean IMDS
        // escape. Comparing the parsed scheme/host/port closes that class.
        if let Ok(parsed) = url::Url::parse(url) {
            for prefix in &self.allowed_url_prefixes {
                if url_matches_prefix(&parsed, prefix) {
                    return Ok(());
                }
            }
        }
        check_url_safe(url)
    }

    fn resolve_credential(&self, url: &str, method: &str) -> Vec<(String, String)> {
        let method_upper = method.to_uppercase();
        for cred in &self.creds {
            if !url.starts_with(&cred.url) {
                continue;
            }
            // Prevent prefix confusion: cred for https://api.example.com/
            // must not match https://api.example.com.evil.com/
            if url.len() > cred.url.len() && !cred.url.ends_with('/') {
                let next = url.as_bytes()[cred.url.len()];
                if next != b'/' && next != b'?' && next != b'#' {
                    continue;
                }
            }
            if !cred.methods.is_empty() && !cred.methods.contains(&method_upper) {
                continue;
            }
            match &cred.kind {
                CredKind::Bearer => {
                    return vec![("Authorization".into(), format!("Bearer {}", cred.api_key))];
                }
                CredKind::Query => {
                    // Query params are handled by modifying the URL, not headers
                    // Return a special marker that curl command will interpret
                    if let Some(ref param) = cred.param {
                        return vec![(
                            "__query_param__".into(),
                            format!("{}={}", param, cred.api_key),
                        )];
                    }
                }
            }
        }
        Vec::new()
    }

    async fn http_request(&self, req: HttpRequest) -> io::Result<HttpResponse> {
        // Policy gate. This runs *before* the SSRF check below, which always
        // still runs — Cedar can only further restrict, never weaken SSRF.
        self.check_policy("net:request", &[("url", &req.url), ("method", &req.method)])?;

        #[cfg(not(target_arch = "wasm32"))]
        {
            // 1. Check URL via self.check_url
            self.check_url(&req.url)?;

            // 2. Determine if URL is explicitly allowed (for SafeResolver decision)
            let url_explicitly_allowed = check_url_safe(&req.url).is_err();

            // 3. Build reqwest client
            let mut builder = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .danger_accept_invalid_certs(req.insecure);
            if !url_explicitly_allowed {
                builder = builder.dns_resolver(std::sync::Arc::new(SafeResolver));
            }
            let client = builder
                .build()
                .map_err(|e| io::Error::other(e.to_string()))?;

            // 4. Build the request
            let method: reqwest::Method = req.method.parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("bad HTTP method: {}", req.method),
                )
            })?;
            let mut http_req = client.request(method, &req.url);

            // 5. Set headers (including injected credentials)
            for (name, value) in &req.headers {
                http_req = http_req.header(name.as_str(), value.as_str());
            }

            // 6. Set body
            if let Some(body) = req.body {
                http_req = http_req.body(body);
            }

            // 7. Send
            let mut resp = http_req
                .send()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()))?;

            // 8. Build response
            let status = resp.status().as_u16();
            let version = match resp.version() {
                reqwest::Version::HTTP_11 => "1.1",
                reqwest::Version::HTTP_2 => "2",
                _ => "1.0",
            }
            .to_string();
            let reason = resp.status().canonical_reason().unwrap_or("").to_string();
            let headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            let body = if req.max_response > 0 {
                let mut buf = Vec::new();
                while let Some(chunk) = resp
                    .chunk()
                    .await
                    .map_err(|e| io::Error::other(e.to_string()))?
                {
                    if buf.len() + chunk.len() > req.max_response {
                        return Err(io::Error::other("response body too large"));
                    }
                    buf.extend_from_slice(&chunk);
                }
                buf
            } else {
                resp.bytes()
                    .await
                    .map_err(|e| io::Error::other(e.to_string()))?
                    .to_vec()
            };

            Ok(HttpResponse {
                status,
                headers,
                body,
                version,
                reason,
            })
        }

        #[cfg(target_arch = "wasm32")]
        {
            use wasi::http::outgoing_handler;
            use wasi::http::types::{
                Fields, IncomingBody, Method, OutgoingBody, OutgoingRequest, Scheme,
            };

            // 1. Check URL
            self.check_url(&req.url)?;

            // 2. Parse URL
            let parsed = url::Url::parse(&req.url)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

            // 3. Build method
            let method = match req.method.to_uppercase().as_str() {
                "GET" => Method::Get,
                "POST" => Method::Post,
                "PUT" => Method::Put,
                "DELETE" => Method::Delete,
                "PATCH" => Method::Patch,
                "HEAD" => Method::Head,
                other => Method::Other(other.to_string()),
            };

            // 4. Build headers
            let fields = Fields::new();
            for (name, value) in &req.headers {
                let _ = fields.append(&name.to_lowercase(), &value.as_bytes().to_vec());
            }

            // 5. Build scheme, authority, path
            let scheme = if parsed.scheme() == "https" {
                Some(&Scheme::Https)
            } else {
                Some(&Scheme::Http)
            };
            let authority = parsed.host_str().map(|h| {
                if let Some(port) = parsed.port() {
                    format!("{h}:{port}")
                } else {
                    h.to_string()
                }
            });
            let path_and_query = if let Some(q) = parsed.query() {
                format!("{}?{}", parsed.path(), q)
            } else {
                parsed.path().to_string()
            };

            // 6. Create outgoing request
            let out_req = OutgoingRequest::new(fields);
            out_req
                .set_method(&method)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to set method"))?;
            out_req
                .set_scheme(scheme)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to set scheme"))?;
            out_req
                .set_authority(authority.as_deref())
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to set authority"))?;
            out_req
                .set_path_with_query(Some(&path_and_query))
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to set path"))?;

            // 7. Write body if present
            if let Some(body_bytes) = &req.body {
                let out_body = out_req.body().map_err(|_| {
                    io::Error::new(io::ErrorKind::Other, "failed to get outgoing body")
                })?;
                let stream = out_body.write().map_err(|_| {
                    io::Error::new(io::ErrorKind::Other, "failed to get write stream")
                })?;
                stream.blocking_write_and_flush(body_bytes).map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, format!("write body: {e:?}"))
                })?;
                drop(stream);
                OutgoingBody::finish(out_body, None)
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to finish body"))?;
            } else {
                let out_body = out_req.body().map_err(|_| {
                    io::Error::new(io::ErrorKind::Other, "failed to get outgoing body")
                })?;
                OutgoingBody::finish(out_body, None)
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to finish body"))?;
            }

            // 8. Send request
            let future_resp = outgoing_handler::handle(out_req, None).map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("send request: {e:?}"))
            })?;

            // 9. Block until response is ready
            let incoming_resp = loop {
                if let Some(result) = future_resp.get() {
                    break result
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "response error"))?
                        .map_err(|e| {
                            io::Error::new(io::ErrorKind::Other, format!("HTTP error: {e:?}"))
                        })?;
                }
                // Yield to WASI event loop
                future_resp.subscribe().block();
            };

            // 10. Read response status and headers
            let status = incoming_resp.status();
            let resp_headers: Vec<(String, String)> = incoming_resp
                .headers()
                .entries()
                .into_iter()
                .map(|(k, v)| (k, String::from_utf8_lossy(&v).to_string()))
                .collect();

            // 11. Read response body
            let incoming_body = incoming_resp.consume().map_err(|_| {
                io::Error::new(io::ErrorKind::Other, "failed to consume response body")
            })?;
            let body_stream = incoming_body
                .stream()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to get body stream"))?;
            let mut body = Vec::new();
            loop {
                match body_stream.read(65536) {
                    Ok(chunk) => {
                        if req.max_response > 0 && body.len() + chunk.len() > req.max_response {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "response body too large",
                            ));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Err(wasi::io::streams::StreamError::Closed) => break,
                    Err(e) => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("read body: {e:?}"),
                        ));
                    }
                }
            }
            drop(body_stream);
            IncomingBody::finish(incoming_body);

            // 12. Map status to reason
            let reason = match status {
                200 => "OK",
                201 => "Created",
                204 => "No Content",
                301 => "Moved Permanently",
                302 => "Found",
                304 => "Not Modified",
                400 => "Bad Request",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                405 => "Method Not Allowed",
                409 => "Conflict",
                500 => "Internal Server Error",
                502 => "Bad Gateway",
                503 => "Service Unavailable",
                _ => "",
            }
            .to_string();

            Ok(HttpResponse {
                status,
                headers: resp_headers,
                body,
                version: "1.1".to_string(),
                reason,
            })
        }
    }
}

// --- host stat helper ---

#[cfg(not(target_arch = "wasm32"))]
async fn host_stat(path: &std::path::Path) -> FileStat {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return FileStat::default(),
    };
    FileStat {
        exists: true,
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        is_symlink: meta.is_symlink(),
        len: meta.len(),
        is_socket: false,
        is_fifo: false,
        is_block_device: false,
        is_char_device: false,
        mode: {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.mode()
            }
            #[cfg(not(unix))]
            {
                if meta.is_dir() { 0o040755 } else { 0o100644 }
            }
        },
        dev: 0,
        ino: 0,
        modified: meta.modified().ok(),
    }
}

#[cfg(target_arch = "wasm32")]
fn host_stat_sync(path: &std::path::Path) -> FileStat {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return FileStat::default(),
    };
    FileStat {
        exists: true,
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        is_symlink: meta.is_symlink(),
        len: meta.len(),
        is_socket: false,
        is_fifo: false,
        is_block_device: false,
        is_char_device: false,
        mode: if meta.is_dir() { 0o040755 } else { 0o100644 },
        dev: 0,
        ino: 0,
        modified: meta.modified().ok(),
    }
}

// --- device fd helpers ---

fn make_dev_null_fd(proc: &mut Process, flags: &OpenFlags) -> io::Result<Fd> {
    if flags.read {
        let (_tx, rx) = crate::os::pipe(1);
        // tx dropped immediately → reader gets EOF
        proc.alloc_fd(FdKind::ChannelReader {
            rx,
            buf: Vec::new(),
        })
    } else {
        let (tx, _rx) = crate::os::pipe(8192);
        // Spawn a drain task
        tokio::task::spawn_local(async move {
            let mut _rx = _rx;
            while _rx.recv().await.is_some() {}
        });
        proc.alloc_fd(FdKind::ChannelWriter {
            tx,
            error_flag: None,
        })
    }
}

fn make_dev_zero_fd(proc: &mut Process, flags: &OpenFlags) -> io::Result<Fd> {
    if flags.write {
        return make_dev_null_fd(proc, flags);
    }
    // Infinite stream of zeros
    let (tx, rx) = crate::os::pipe(1);
    tokio::task::spawn_local(async move {
        let zeros = bytes::Bytes::from(vec![0u8; 4096]);
        while tx.send(zeros.clone()).await.is_ok() {}
    });
    proc.alloc_fd(FdKind::ChannelReader {
        rx,
        buf: Vec::new(),
    })
}

fn make_dev_urandom_fd(proc: &mut Process, flags: &OpenFlags) -> io::Result<Fd> {
    if flags.write {
        return make_dev_null_fd(proc, flags);
    }
    // Generate pseudo-random data
    let (tx, rx) = crate::os::pipe(1);
    #[cfg(not(target_arch = "wasm32"))]
    tokio::task::spawn_local(async move {
        use tokio::io::AsyncReadExt;
        if let Ok(mut f) = tokio::fs::File::open("/dev/urandom").await {
            let mut buf = vec![0u8; 4096];
            while let Ok(n) = f.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if tx
                    .send(bytes::Bytes::copy_from_slice(&buf[..n]))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    tokio::task::spawn_local(async move {
        // Simple PRNG fallback for WASM — produce pseudo-random bytes
        // using a basic xorshift seeded from the system time.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        let mut state = seed;
        let mut buf = vec![0u8; 4096];
        loop {
            for byte in buf.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            if tx.send(bytes::Bytes::copy_from_slice(&buf)).await.is_err() {
                break;
            }
        }
    });
    proc.alloc_fd(FdKind::ChannelReader {
        rx,
        buf: Vec::new(),
    })
}

// --- glob matching for VFS ---

fn glob_vfs(vfs: &Vfs, pattern: &str, results: &mut Vec<String>) {
    let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    glob_recurse(vfs, vfs::Ino::from(1u64), "/", &parts, 0, results);
}

fn glob_recurse(
    vfs: &Vfs,
    dir_ino: vfs::Ino,
    dir_path: &str,
    parts: &[&str],
    idx: usize,
    results: &mut Vec<String>,
) {
    if idx >= parts.len() {
        results.push(dir_path.to_string());
        return;
    }
    let pat = parts[idx];
    let is_last = idx == parts.len() - 1;

    let entries = match vfs.read_dir(dir_ino) {
        Ok(e) => e,
        Err(_) => return,
    };

    for (name, child_ino) in &entries {
        if !glob_match_simple(pat, name) {
            continue;
        }
        let child_path = if dir_path == "/" {
            format!("/{name}")
        } else {
            format!("{dir_path}/{name}")
        };
        if is_last {
            results.push(child_path);
        } else {
            // Must be a directory to continue
            if let Ok(inode) = vfs.get(*child_ino)
                && matches!(inode.data, InodeData::Dir(_))
            {
                glob_recurse(vfs, *child_ino, &child_path, parts, idx + 1, results);
            }
        }
    }
}

/// Simple glob matching (supports * and ?).
fn glob_match_simple(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            glob_match_chars(&p[1..], t) || (!t.is_empty() && glob_match_chars(p, &t[1..]))
        }
        (Some('?'), Some(_)) => glob_match_chars(&p[1..], &t[1..]),
        (Some(a), Some(b)) if a == b => glob_match_chars(&p[1..], &t[1..]),
        _ => false,
    }
}

/// Whether a parsed request URL is covered by an allowlist `prefix` entry.
///
/// Matches on parsed components (scheme + host + port + a path-segment prefix),
/// never raw string prefixes — string matching is defeated by userinfo
/// injection (`http://allowed:port@attacker/`). A prefix that fails to parse
/// never matches.
fn url_matches_prefix(url: &url::Url, prefix: &str) -> bool {
    let Ok(p) = url::Url::parse(prefix) else {
        return false;
    };
    if url.scheme() != p.scheme() || url.host() != p.host() {
        return false;
    }
    if url.port_or_known_default() != p.port_or_known_default() {
        return false;
    }
    // Path: the prefix path must cover the request path on a segment boundary,
    // so `/v1` matches `/v1/x` but not `/v10`. An empty/`"/"` prefix path (the
    // common host-only allowlist entry) covers every path.
    let (req, pre) = (url.path(), p.path());
    if pre == "/" || pre.is_empty() {
        return true;
    }
    if req == pre {
        return true;
    }
    let boundary = pre.strip_suffix('/').unwrap_or(pre);
    req.starts_with(boundary) && req.as_bytes().get(boundary.len()) == Some(&b'/')
}

/// Check if an IP address is private/loopback/link-local/IMDS.
fn is_ip_blocked(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.octets()[..2] == [169, 254]
        }
        std::net::IpAddr::V6(v6) => {
            // Loopback (::1)
            if v6.is_loopback() {
                return true;
            }
            // Unspecified (::)
            if v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped (::ffff:x.x.x.x) and IPv4-compatible (::x.x.x.x)
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_ip_blocked(std::net::IpAddr::V4(v4));
            }
            let segs = v6.segments();
            // IPv4-compatible addresses (deprecated but still routable)
            if segs[..6] == [0, 0, 0, 0, 0, 0] && (segs[6] != 0 || segs[7] > 1) {
                let o = v6.octets();
                let v4 = std::net::Ipv4Addr::new(o[12], o[13], o[14], o[15]);
                return is_ip_blocked(std::net::IpAddr::V4(v4));
            }
            // ULA (fc00::/7)
            if segs[0] & 0xfe00 == 0xfc00 {
                return true;
            }
            // Link-local (fe80::/10)
            if segs[0] & 0xffc0 == 0xfe80 {
                return true;
            }
            // 6to4 (2002::/16) — check embedded IPv4
            if segs[0] == 0x2002 {
                let v4 = std::net::Ipv4Addr::new(
                    (segs[1] >> 8) as u8,
                    segs[1] as u8,
                    (segs[2] >> 8) as u8,
                    segs[2] as u8,
                );
                return is_ip_blocked(std::net::IpAddr::V4(v4));
            }
            // Teredo (2001:0000::/32) — check embedded IPv4 (bitwise NOT of last 32 bits)
            if segs[0] == 0x2001 && segs[1] == 0x0000 {
                let o = v6.octets();
                let v4 = std::net::Ipv4Addr::new(!o[12], !o[13], !o[14], !o[15]);
                return is_ip_blocked(std::net::IpAddr::V4(v4));
            }
            false
        }
    }
}

/// Check a URL for blocked schemes, hostnames, and IP literals.
/// Does NOT resolve DNS — use `SafeResolver` on the reqwest client for
/// connect-time DNS filtering.
pub fn check_url_safe(url: &str) -> io::Result<()> {
    // Scheme whitelist
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("access denied: unsupported scheme in {url}"),
        ));
    }
    // Match on the parsed `url::Host` rather than `host_str()` + string parse.
    // `host_str()` keeps IPv6 literals bracketed (`[::1]`), which made the
    // `parse::<IpAddr>()` below fail silently and skip `is_ip_blocked` entirely
    // — a full IPv6 SSRF bypass (incl. IMDS via `[::ffff:169.254.169.254]`).
    // The `Host` enum gives us a real `Ipv4Addr`/`Ipv6Addr` with no brackets.
    let parsed = url::Url::parse(url).map_err(|_| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("access denied: cannot parse host from {url}"),
        )
    })?;
    match parsed.host() {
        Some(url::Host::Ipv4(v4)) => {
            if is_ip_blocked(std::net::IpAddr::V4(v4)) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("access denied: {v4}"),
                ));
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if is_ip_blocked(std::net::IpAddr::V6(v6)) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("access denied: {v6}"),
                ));
            }
        }
        Some(url::Host::Domain(d)) => {
            if d == "localhost" || d.ends_with(".localhost") {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("access denied: {d}"),
                ));
            }
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("access denied: cannot parse host from {url}"),
            ));
        }
    }
    Ok(())
}

/// A DNS resolver that filters out blocked IPs at resolution time,
/// eliminating TOCTOU between DNS check and connection.
#[cfg(not(target_arch = "wasm32"))]
pub struct SafeResolver;

#[cfg(not(target_arch = "wasm32"))]
impl reqwest::dns::Resolve for SafeResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str();
            let host_port = format!("{host}:0");
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&host_port)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();
            let safe: Vec<std::net::SocketAddr> = addrs
                .into_iter()
                .filter(|a| !is_ip_blocked(a.ip()))
                .collect();
            if safe.is_empty() {
                return Err(format!("access denied: {host} resolves to blocked address").into());
            }
            Ok(Box::new(safe.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod url_safety_tests {
    use super::{check_url_safe, url_matches_prefix};

    // A1: IPv6 literals (incl. IPv4-mapped IMDS) must be blocked by
    // `check_url_safe` ITSELF — not merely by the SafeResolver/connection
    // backstop. Asserting on the function return value (rather than a curl exit
    // code) is what proves the fix: against the old `host_str()` code these all
    // returned Ok(()) because the bracketed literal failed to parse as an IP.
    #[test]
    fn check_url_safe_blocks_ipv6_literals() {
        for u in [
            "http://[::1]/",                    // loopback
            "http://[::ffff:169.254.169.254]/", // IPv4-mapped IMDS
            "http://[fe80::1]/",                // link-local
            "http://[fc00::1]/",                // ULA
            "http://[::]/",                     // unspecified
        ] {
            assert!(check_url_safe(u).is_err(), "{u} should be blocked");
        }
    }

    #[test]
    fn check_url_safe_blocks_ipv4_and_localhost() {
        for u in [
            "http://169.254.169.254/", // IMDS
            "http://127.0.0.1/",       // loopback
            "http://10.0.0.1/",        // private
            "http://localhost/",       // localhost domain
            "http://x.localhost/",     // .localhost subdomain
            "ftp://example.com/",      // non-http scheme
        ] {
            assert!(check_url_safe(u).is_err(), "{u} should be blocked");
        }
    }

    #[test]
    fn check_url_safe_allows_public_hosts() {
        for u in [
            "http://example.com/",
            "https://example.com/path",
            "http://[2606:4700:4700::1111]/", // public IPv6 (Cloudflare DNS)
        ] {
            assert!(check_url_safe(u).is_ok(), "{u} should be allowed");
        }
    }

    fn matches(url: &str, prefix: &str) -> bool {
        url_matches_prefix(&url::Url::parse(url).unwrap(), prefix)
    }

    // A2: userinfo injection must not let an allowlist entry cover a different
    // real host. The `:port@` form is the one the old string-prefix code missed
    // (`:` was a boundary char), reaching IMDS.
    #[test]
    fn allowlist_rejects_userinfo_injection() {
        assert!(!matches(
            "http://127.0.0.1:1234@169.254.169.254/",
            "http://127.0.0.1:1234"
        ));
        assert!(!matches(
            "http://good.example.com@169.254.169.254/",
            "http://good.example.com"
        ));
    }

    #[test]
    fn allowlist_matches_legitimate_urls() {
        // Host-only prefix covers any path.
        assert!(matches("http://h.example.com/a/b", "http://h.example.com"));
        assert!(matches("http://h.example.com/a/b", "http://h.example.com/"));
        // Default-port equivalence (http=80, https=443).
        assert!(matches("http://h.example.com:80/x", "http://h.example.com"));
        assert!(matches(
            "https://h.example.com/x",
            "https://h.example.com:443"
        ));
        // Path-prefix on a segment boundary.
        assert!(matches(
            "http://h.example.com/v1/x",
            "http://h.example.com/v1"
        ));
        assert!(matches(
            "http://h.example.com/v1",
            "http://h.example.com/v1"
        ));
    }

    #[test]
    fn allowlist_rejects_near_misses() {
        // Path prefix must not match across a non-boundary (`/v1` vs `/v10`).
        assert!(!matches(
            "http://h.example.com/v10",
            "http://h.example.com/v1"
        ));
        // Different port.
        assert!(!matches(
            "http://h.example.com:8080/",
            "http://h.example.com:9090"
        ));
        // Different scheme.
        assert!(!matches("http://h.example.com/", "https://h.example.com"));
        // Different host.
        assert!(!matches("http://evil.example.com/", "http://h.example.com"));
    }
}
