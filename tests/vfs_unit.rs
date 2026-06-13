use strands_shell::vfs::*;

// ── Vfs::new ────────────────────────────────────────────────────────

#[test]
fn new_vfs_has_root() {
    let vfs = Vfs::new();
    let ino = vfs.resolve("/", true).unwrap();
    assert_eq!(ino, 1);
}

#[test]
fn new_vfs_root_is_dir() {
    let vfs = Vfs::new();
    let inode = vfs.get(1).unwrap();
    assert!(matches!(inode.data, InodeData::Dir(_)));
    assert_eq!(inode.nlink, 2);
}

// ── create_file ─────────────────────────────────────────────────────

#[test]
fn create_file_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(matches!(vfs.get(ino).unwrap().data, InodeData::File(_)));
}

#[test]
fn create_file_applies_umask() {
    let mut vfs = Vfs::new();
    vfs.umask = 0o022;
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o666, LASH_UID, LASH_GID)
        .unwrap();
    // 0o666 & !0o022 = 0o644, plus 0o100000 prefix
    assert_eq!(vfs.get(ino).unwrap().mode, 0o100644);
}

#[test]
fn create_file_duplicate_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(
        vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
            .is_err()
    );
}

// ── mkdir ───────────────────────────────────────────────────────────

#[test]
fn mkdir_basic() {
    let mut vfs = Vfs::new();
    let ino = vfs.mkdir("/d", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let inode = vfs.get(ino).unwrap();
    assert!(matches!(inode.data, InodeData::Dir(_)));
    assert_eq!(inode.nlink, 2);
}

#[test]
fn mkdir_increments_parent_nlink() {
    let mut vfs = Vfs::new();
    let root_nlink_before = vfs.get(1).unwrap().nlink;
    vfs.mkdir("/d", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert_eq!(vfs.get(1).unwrap().nlink, root_nlink_before + 1);
}

#[test]
fn mkdir_duplicate_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/d", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert!(vfs.mkdir("/d", 0o755, ROOT_UID, ROOT_GID).is_err());
}

// ── mkdir_p ─────────────────────────────────────────────────────────

#[test]
fn mkdir_p_creates_parents() {
    let mut vfs = Vfs::new();
    let ino = vfs.mkdir_p("/a/b/c", 0o755, LASH_UID, LASH_GID).unwrap();
    assert!(vfs.resolve("/a", true).is_ok());
    assert!(vfs.resolve("/a/b", true).is_ok());
    assert_eq!(vfs.resolve("/a/b/c", true).unwrap(), ino);
}

#[test]
fn mkdir_p_existing_parents_ok() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, LASH_UID, LASH_GID).unwrap();
    let ino = vfs.mkdir_p("/a/b/c", 0o755, LASH_UID, LASH_GID).unwrap();
    assert_eq!(vfs.resolve("/a/b/c", true).unwrap(), ino);
}

// ── symlink ─────────────────────────────────────────────────────────

#[test]
fn symlink_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let fno = vfs
        .create_file("/tmp/target", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/tmp/link", "/tmp/target", LASH_UID, LASH_GID)
        .unwrap();
    // Without follow: get the symlink inode
    let link_ino = vfs.resolve("/tmp/link", false).unwrap();
    assert!(matches!(
        vfs.get(link_ino).unwrap().data,
        InodeData::Symlink(_)
    ));
    // With follow: get the target
    let resolved = vfs.resolve("/tmp/link", true).unwrap();
    assert_eq!(resolved, fno);
}

#[test]
fn symlink_relative() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/target", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/tmp/link", "target", LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.resolve("/tmp/link", true).is_ok());
}

#[test]
fn symlink_chain() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let fno = vfs
        .create_file("/tmp/real", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/tmp/a", "/tmp/real", LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/tmp/b", "/tmp/a", LASH_UID, LASH_GID).unwrap();
    assert_eq!(vfs.resolve("/tmp/b", true).unwrap(), fno);
}

#[test]
fn symlink_circular_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.symlink("/tmp/a", "/tmp/b", LASH_UID, LASH_GID).unwrap();
    vfs.symlink("/tmp/b", "/tmp/a", LASH_UID, LASH_GID).unwrap();
    assert!(vfs.resolve("/tmp/a", true).is_err());
}

#[test]
fn symlink_intermediate_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/real", 0o755, LASH_UID, LASH_GID).unwrap();
    let fno = vfs
        .create_file("/tmp/real/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/tmp/link", "/tmp/real", LASH_UID, LASH_GID)
        .unwrap();
    // Resolve through symlink directory
    assert_eq!(vfs.resolve("/tmp/link/f", true).unwrap(), fno);
}

// ── hard_link ───────────────────────────────────────────────────────

#[test]
fn hard_link_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let fno = vfs
        .create_file("/tmp/orig", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.hard_link("/tmp/orig", "/tmp/link").unwrap();
    assert_eq!(vfs.resolve("/tmp/link", true).unwrap(), fno);
    assert_eq!(vfs.get(fno).unwrap().nlink, 2);
}

#[test]
fn hard_link_dir_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/d", 0o755, LASH_UID, LASH_GID).unwrap();
    assert!(vfs.hard_link("/tmp/d", "/tmp/link").is_err());
}

// ── mknod ───────────────────────────────────────────────────────────

#[test]
fn mknod_char_device() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/dev/test",
            InodeData::CharDevice(1, 99),
            0o020666,
            ROOT_UID,
            ROOT_GID,
        )
        .unwrap();
    assert!(matches!(
        vfs.get(ino).unwrap().data,
        InodeData::CharDevice(1, 99)
    ));
}

#[test]
fn mknod_block_device() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/dev/blk",
            InodeData::BlockDevice(8, 0),
            0o060660,
            ROOT_UID,
            ROOT_GID,
        )
        .unwrap();
    assert!(matches!(
        vfs.get(ino).unwrap().data,
        InodeData::BlockDevice(8, 0)
    ));
}

#[test]
fn mknod_fifo() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod("/tmp/pipe", InodeData::Fifo, 0o010644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(matches!(vfs.get(ino).unwrap().data, InodeData::Fifo));
}

// ── unlink ──────────────────────────────────────────────────────────

#[test]
fn unlink_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.unlink("/tmp/f").unwrap();
    assert!(vfs.resolve("/tmp/f", true).is_err());
}

#[test]
fn unlink_dir_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/d", 0o755, LASH_UID, LASH_GID).unwrap();
    assert!(vfs.unlink("/tmp/d").is_err());
}

#[test]
fn unlink_hard_link_preserves_data() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/a", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"hello".to_vec()).unwrap();
    vfs.hard_link("/tmp/a", "/tmp/b").unwrap();
    vfs.unlink("/tmp/a").unwrap();
    // File still accessible via /tmp/b
    let ino2 = vfs.resolve("/tmp/b", true).unwrap();
    assert_eq!(vfs.read_file(ino2).unwrap(), b"hello");
    assert_eq!(vfs.get(ino2).unwrap().nlink, 1);
}

// ── rmdir ───────────────────────────────────────────────────────────

#[test]
fn rmdir_empty() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/d", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.rmdir("/tmp/d").unwrap();
    assert!(vfs.resolve("/tmp/d", true).is_err());
}

#[test]
fn rmdir_nonempty_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/d", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.create_file("/tmp/d/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.rmdir("/tmp/d").is_err());
}

#[test]
fn rmdir_file_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.rmdir("/tmp/f").is_err());
}

#[test]
fn rmdir_decrements_parent_nlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let parent_ino = vfs.resolve("/tmp", true).unwrap();
    let before = vfs.get(parent_ino).unwrap().nlink;
    vfs.mkdir("/tmp/d", 0o755, LASH_UID, LASH_GID).unwrap();
    assert_eq!(vfs.get(parent_ino).unwrap().nlink, before + 1);
    vfs.rmdir("/tmp/d").unwrap();
    assert_eq!(vfs.get(parent_ino).unwrap().nlink, before);
}

// ── rename ──────────────────────────────────────────────────────────

#[test]
fn rename_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/a", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"data".to_vec()).unwrap();
    vfs.rename("/tmp/a", "/tmp/b").unwrap();
    assert!(vfs.resolve("/tmp/a", true).is_err());
    let ino2 = vfs.resolve("/tmp/b", true).unwrap();
    assert_eq!(vfs.read_file(ino2).unwrap(), b"data");
}

#[test]
fn rename_file_over_existing() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/a", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"new".to_vec()).unwrap();
    let old = vfs
        .create_file("/tmp/b", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(old, b"old".to_vec()).unwrap();
    vfs.rename("/tmp/a", "/tmp/b").unwrap();
    let ino2 = vfs.resolve("/tmp/b", true).unwrap();
    assert_eq!(vfs.read_file(ino2).unwrap(), b"new");
}

#[test]
fn rename_dir_to_new() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/a", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.create_file("/tmp/a/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.rename("/tmp/a", "/tmp/b").unwrap();
    assert!(vfs.resolve("/tmp/b/f", true).is_ok());
}

#[test]
fn rename_dir_over_empty_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/a", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.create_file("/tmp/a/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.mkdir("/tmp/b", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.rename("/tmp/a", "/tmp/b").unwrap();
    assert!(vfs.resolve("/tmp/b/f", true).is_ok());
}

#[test]
fn rename_dir_over_nonempty_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/a", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.mkdir("/tmp/b", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.create_file("/tmp/b/x", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.rename("/tmp/a", "/tmp/b").is_err());
}

#[test]
fn rename_dir_updates_dotdot() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.mkdir("/b", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.mkdir("/a/child", 0o755, LASH_UID, LASH_GID).unwrap();
    let b_ino = vfs.resolve("/b", true).unwrap();
    vfs.rename("/a/child", "/b/child").unwrap();
    // ".." in child should now point to /b
    let child_ino = vfs.resolve("/b/child", true).unwrap();
    if let InodeData::Dir(entries) = &vfs.get(child_ino).unwrap().data {
        assert_eq!(*entries.get("..").unwrap(), b_ino);
    } else {
        panic!("expected dir");
    }
}

// ── read_file / write_file / append_file ────────────────────────────

#[test]
fn read_write_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"hello".to_vec()).unwrap();
    assert_eq!(vfs.read_file(ino).unwrap(), b"hello");
}

#[test]
fn read_file_on_dir_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    assert!(vfs.read_file(ino).is_err());
}

#[test]
fn write_file_on_dir_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    assert!(vfs.write_file(ino, b"x".to_vec()).is_err());
}

#[test]
fn append_file_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"hello".to_vec()).unwrap();
    vfs.append_file(ino, b" world").unwrap();
    assert_eq!(vfs.read_file(ino).unwrap(), b"hello world");
}

#[test]
fn append_file_on_dir_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    assert!(vfs.append_file(ino, b"x").is_err());
}

#[test]
fn max_file_size_write() {
    let mut vfs = Vfs::new();
    vfs.max_file_size = 10;
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.write_file(ino, vec![0u8; 11]).is_err());
    vfs.write_file(ino, vec![0u8; 10]).unwrap(); // exactly at limit is ok
}

#[test]
fn max_file_size_append() {
    let mut vfs = Vfs::new();
    vfs.max_file_size = 10;
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, vec![0u8; 8]).unwrap();
    assert!(vfs.append_file(ino, &[0u8; 3]).is_err());
    vfs.append_file(ino, &[0u8; 2]).unwrap(); // exactly at limit
}

// ── max_inodes ──────────────────────────────────────────────────────

#[test]
fn max_inodes_limit() {
    let mut vfs = Vfs::new();
    vfs.max_inodes = 3; // root + 2 more
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/b", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert!(vfs.mkdir("/c", 0o755, ROOT_UID, ROOT_GID).is_err());
}

#[test]
fn max_inodes_file() {
    let mut vfs = Vfs::new();
    vfs.max_inodes = 3;
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(
        vfs.create_file("/tmp/g", 0o644, LASH_UID, LASH_GID)
            .is_err()
    );
}

#[test]
fn max_inodes_symlink() {
    let mut vfs = Vfs::new();
    vfs.max_inodes = 3;
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.symlink("/tmp/l", "/tmp/f", LASH_UID, LASH_GID).is_err());
}

#[test]
fn max_inodes_mknod() {
    let mut vfs = Vfs::new();
    vfs.max_inodes = 2;
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert!(
        vfs.mknod(
            "/dev/x",
            InodeData::CharDevice(1, 1),
            0o020666,
            ROOT_UID,
            ROOT_GID
        )
        .is_err()
    );
}

// ── read_dir ────────────────────────────────────────────────────────

#[test]
fn read_dir_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/b", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.create_file("/tmp/a", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    let entries = vfs.read_dir(ino).unwrap();
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]); // sorted
}

#[test]
fn read_dir_excludes_dot() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    let entries = vfs.read_dir(ino).unwrap();
    assert!(entries.iter().all(|(n, _)| n != "." && n != ".."));
}

#[test]
fn read_dir_on_file_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.read_dir(ino).is_err());
}

// ── check_permission ────────────────────────────────────────────────

#[test]
fn permission_root_always_allowed() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o000, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.check_permission(ino, ROOT_UID, ROOT_GID, 7));
}

#[test]
fn permission_owner_read() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // Override mode to avoid umask
    vfs.get_mut(ino).unwrap().mode = 0o100400;
    assert!(vfs.check_permission(ino, LASH_UID, LASH_GID, 4));
    assert!(!vfs.check_permission(ino, LASH_UID, LASH_GID, 2));
}

#[test]
fn permission_group() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.get_mut(ino).unwrap().mode = 0o100070; // group rwx only
    // Same group, different user
    assert!(vfs.check_permission(ino, 2000, LASH_GID, 7));
    assert!(!vfs.check_permission(ino, 2000, 9999, 1)); // wrong group
}

#[test]
fn permission_other() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.get_mut(ino).unwrap().mode = 0o100004; // other read only
    assert!(vfs.check_permission(ino, 9999, 9999, 4));
    assert!(!vfs.check_permission(ino, 9999, 9999, 2));
}

#[test]
fn permission_invalid_ino() {
    let vfs = Vfs::new();
    assert!(!vfs.check_permission(99999, LASH_UID, LASH_GID, 4));
}

// ── canonicalize_path ───────────────────────────────────────────────

#[test]
fn canonicalize_no_symlinks() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/b", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert_eq!(vfs.canonicalize_path("/a/b").unwrap(), "/a/b");
}

#[test]
fn canonicalize_with_symlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/real", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.symlink("/a/link", "/a/real", LASH_UID, LASH_GID)
        .unwrap();
    assert_eq!(vfs.canonicalize_path("/a/link").unwrap(), "/a/real");
}

#[test]
fn canonicalize_chain() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/real", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.symlink("/a/l1", "/a/real", LASH_UID, LASH_GID).unwrap();
    vfs.symlink("/a/l2", "/a/l1", LASH_UID, LASH_GID).unwrap();
    assert_eq!(vfs.canonicalize_path("/a/l2").unwrap(), "/a/real");
}

#[test]
fn canonicalize_circular_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.symlink("/tmp/a", "/tmp/b", LASH_UID, LASH_GID).unwrap();
    vfs.symlink("/tmp/b", "/tmp/a", LASH_UID, LASH_GID).unwrap();
    assert!(vfs.canonicalize_path("/tmp/a").is_err());
}

#[test]
fn canonicalize_not_a_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // Try to canonicalize a path through a file
    assert!(vfs.canonicalize_path("/tmp/f/child").is_err());
}

// ── inode_to_filestat ───────────────────────────────────────────────

#[test]
fn filestat_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino, b"hello".to_vec()).unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_file && !st.is_dir && !st.is_symlink);
    assert_eq!(st.len, 5);
}

#[test]
fn filestat_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs.resolve("/tmp", true).unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_dir && !st.is_file);
}

#[test]
fn filestat_symlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .symlink("/tmp/l", "/tmp/target", LASH_UID, LASH_GID)
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_symlink && !st.is_file && !st.is_dir);
    assert_eq!(st.len, "/tmp/target".len() as u64);
}

#[test]
fn filestat_char_device() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/dev/x",
            InodeData::CharDevice(1, 3),
            0o020666,
            ROOT_UID,
            ROOT_GID,
        )
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_char_device && !st.is_file);
}

#[test]
fn filestat_block_device() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/dev", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/dev/x",
            InodeData::BlockDevice(8, 0),
            0o060660,
            ROOT_UID,
            ROOT_GID,
        )
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_block_device && !st.is_file);
}

#[test]
fn filestat_fifo() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod("/tmp/p", InodeData::Fifo, 0o010644, LASH_UID, LASH_GID)
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_fifo && !st.is_file);
}

#[test]
fn filestat_host_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/mnt", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/mnt/f",
            InodeData::HostFile("/tmp/x".into(), false),
            0o100644,
            LASH_UID,
            LASH_GID,
        )
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_file && !st.is_dir);
}

#[test]
fn filestat_host_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/mnt", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .mknod(
            "/mnt/d",
            InodeData::HostDir("/tmp".into(), false),
            0o040755,
            LASH_UID,
            LASH_GID,
        )
        .unwrap();
    let st = vfs.inode_to_filestat(ino);
    assert!(st.exists && st.is_dir && !st.is_file);
}

#[test]
fn filestat_invalid_ino() {
    let vfs = Vfs::new();
    let st = vfs.inode_to_filestat(99999);
    assert!(!st.exists);
}

// ── resolve edge cases ──────────────────────────────────────────────

#[test]
fn resolve_not_a_directory() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // Try to resolve a path through a file
    let err = vfs.resolve("/tmp/f/child", true).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
}

#[test]
fn resolve_nonexistent() {
    let vfs = Vfs::new();
    assert!(vfs.resolve("/no/such/path", true).is_err());
}

#[test]
fn resolve_root() {
    let vfs = Vfs::new();
    assert_eq!(vfs.resolve("/", true).unwrap(), 1);
}

#[test]
fn resolve_symlink_no_follow() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/target", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    let link_ino = vfs
        .symlink("/tmp/link", "/tmp/target", LASH_UID, LASH_GID)
        .unwrap();
    // Without follow: returns the symlink inode itself
    assert_eq!(vfs.resolve("/tmp/link", false).unwrap(), link_ino);
}

// ── normalize ───────────────────────────────────────────────────────

#[test]
fn normalize_basic() {
    assert_eq!(normalize("/a/b/c"), "/a/b/c");
}

#[test]
fn normalize_dots() {
    assert_eq!(normalize("/a/./b/../c"), "/a/c");
}

#[test]
fn normalize_double_slash() {
    assert_eq!(normalize("//a//b"), "/a/b");
}

#[test]
fn normalize_root() {
    assert_eq!(normalize("/"), "/");
}

#[test]
fn normalize_trailing_slash() {
    assert_eq!(normalize("/a/b/"), "/a/b");
}

#[test]
fn normalize_dotdot_past_root() {
    assert_eq!(normalize("/../../a"), "/a");
}

// ── create_dev_nodes ────────────────────────────────────────────────

#[test]
fn create_dev_nodes_all() {
    let mut vfs = Vfs::new();
    create_dev_nodes(&mut vfs).unwrap();
    assert!(vfs.resolve("/dev/null", true).is_ok());
    assert!(vfs.resolve("/dev/zero", true).is_ok());
    assert!(vfs.resolve("/dev/urandom", true).is_ok());
    assert!(vfs.resolve("/dev/random", true).is_ok());
}

// ── create_bin_links ────────────────────────────────────────────────

#[test]
fn create_bin_links_basic() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/bin", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/usr", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/usr/bin", 0o755, ROOT_UID, ROOT_GID).unwrap();
    create_bin_links(&mut vfs).unwrap();
    assert!(vfs.resolve("/bin/lash", true).is_ok());
    assert!(vfs.resolve("/bin/sh", false).is_ok());
    assert!(vfs.resolve("/bin/echo", false).is_ok());
}

// ── get / get_mut with invalid ino ──────────────────────────────────

#[test]
fn get_invalid_ino() {
    let vfs = Vfs::new();
    assert!(vfs.get(99999).is_err());
}

#[test]
fn get_mut_invalid_ino() {
    let mut vfs = Vfs::new();
    assert!(vfs.get_mut(99999).is_err());
}

// ── resolve_depth: intermediate symlink as current inode (lines 148-158) ──

#[test]
fn resolve_symlink_to_dir_as_intermediate() {
    // Create a scenario where resolve_depth encounters a symlink as the
    // "current" inode (not as a child lookup). This happens when a symlink
    // target is itself a symlink that needs further resolution.
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/real", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/a/real/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // /a/link -> /a/real (symlink to dir)
    vfs.symlink("/a/link", "/a/real", LASH_UID, LASH_GID)
        .unwrap();
    // Resolve /a/link/f — link is intermediate, resolved as child symlink
    assert!(vfs.resolve("/a/link/f", true).is_ok());
}

// ── resolve_depth: follow_last on root-level symlink (lines 167-180) ──

#[test]
fn resolve_follow_last_root_symlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/target", 0o755, ROOT_UID, ROOT_GID).unwrap();
    // /link -> /target at root level
    vfs.symlink("/link", "/target", LASH_UID, LASH_GID).unwrap();
    let target_ino = vfs.resolve("/target", true).unwrap();
    let resolved = vfs.resolve("/link", true).unwrap();
    assert_eq!(resolved, target_ino);
}

#[test]
fn resolve_follow_last_nested_symlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/b", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/a/b/target", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // /a/b/link -> /a/b/target
    vfs.symlink("/a/b/link", "/a/b/target", LASH_UID, LASH_GID)
        .unwrap();
    let target_ino = vfs.resolve("/a/b/target", true).unwrap();
    let resolved = vfs.resolve("/a/b/link", true).unwrap();
    assert_eq!(resolved, target_ino);
}

// ── canonicalize_depth: symlink in non-root position (lines 226-232) ──

#[test]
fn canonicalize_symlink_in_middle() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/real", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/a/real/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.symlink("/a/link", "/a/real", LASH_UID, LASH_GID)
        .unwrap();
    // Canonicalize path through symlink
    assert_eq!(vfs.canonicalize_path("/a/link/f").unwrap(), "/a/real/f");
}

#[test]
fn canonicalize_relative_symlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/a/real", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.symlink("/a/link", "real", LASH_UID, LASH_GID).unwrap();
    assert_eq!(vfs.canonicalize_path("/a/link").unwrap(), "/a/real");
}

// ── inode_path (lines 564-585) ──────────────────────────────────────

#[test]
fn mkdir_p_deep_uses_inode_path() {
    // mkdir_p calls inode_path internally to build paths for intermediate dirs
    let mut vfs = Vfs::new();
    vfs.mkdir_p("/a/b/c/d/e", 0o755, LASH_UID, LASH_GID)
        .unwrap();
    assert!(vfs.resolve("/a/b/c/d/e", true).is_ok());
}

// ── dir_lookup / dir_insert / dir_remove error paths ────────────────

#[test]
fn dir_insert_duplicate_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    // Try to create another file with same name
    assert!(
        vfs.create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
            .is_err()
    );
}

#[test]
fn dir_remove_nonexistent_fails() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    // Try to unlink a file that doesn't exist
    assert!(vfs.unlink("/tmp/nonexistent").is_err());
}

// ── copy_from_host ──────────────────────────────────────────────────

#[test]
fn copy_from_host_file() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/mnt", 0o755, ROOT_UID, ROOT_GID).unwrap();
    // Use Cargo.toml as a known file
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    copy_from_host(&mut vfs, &src, "/mnt/Cargo.toml", LASH_UID, LASH_GID).unwrap();
    let ino = vfs.resolve("/mnt/Cargo.toml", true).unwrap();
    let data = vfs.read_file(ino).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn copy_from_host_dir() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/mnt", 0o755, ROOT_UID, ROOT_GID).unwrap();
    // Use the tests directory as a known directory
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    copy_from_host(&mut vfs, &src, "/mnt/tests", LASH_UID, LASH_GID).unwrap();
    assert!(vfs.resolve("/mnt/tests", true).is_ok());
    // Should have copied at least one file
    let ino = vfs.resolve("/mnt/tests", true).unwrap();
    let entries = vfs.read_dir(ino).unwrap();
    assert!(!entries.is_empty());
}

// ── rename: file over hard-linked file ──────────────────────────────

#[test]
fn rename_file_over_hardlinked() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino_a = vfs
        .create_file("/tmp/a", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino_a, b"new".to_vec()).unwrap();
    let ino_b = vfs
        .create_file("/tmp/b", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    vfs.write_file(ino_b, b"old".to_vec()).unwrap();
    vfs.hard_link("/tmp/b", "/tmp/b2").unwrap();
    // Rename a over b — b has nlink=2, so it shouldn't be removed from inodes
    vfs.rename("/tmp/a", "/tmp/b").unwrap();
    let ino = vfs.resolve("/tmp/b", true).unwrap();
    assert_eq!(vfs.read_file(ino).unwrap(), b"new");
    // b2 should still exist with old data
    let ino2 = vfs.resolve("/tmp/b2", true).unwrap();
    assert_eq!(vfs.read_file(ino2).unwrap(), b"old");
}

// ── rename: dir across parents ──────────────────────────────────────

#[test]
fn rename_dir_across_parents() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/a", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.mkdir("/b", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.mkdir("/a/sub", 0o755, LASH_UID, LASH_GID).unwrap();
    vfs.create_file("/a/sub/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    let a_nlink = vfs.get(vfs.resolve("/a", true).unwrap()).unwrap().nlink;
    let b_nlink = vfs.get(vfs.resolve("/b", true).unwrap()).unwrap().nlink;
    vfs.rename("/a/sub", "/b/sub").unwrap();
    // /a nlink should decrease, /b nlink should increase
    assert_eq!(
        vfs.get(vfs.resolve("/a", true).unwrap()).unwrap().nlink,
        a_nlink - 1
    );
    assert_eq!(
        vfs.get(vfs.resolve("/b", true).unwrap()).unwrap().nlink,
        b_nlink + 1
    );
    assert!(vfs.resolve("/b/sub/f", true).is_ok());
}

// ── rename: same parent (no nlink change) ───────────────────────────

#[test]
fn rename_dir_same_parent() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    vfs.mkdir("/tmp/old", 0o755, LASH_UID, LASH_GID).unwrap();
    let parent_ino = vfs.resolve("/tmp", true).unwrap();
    let nlink_before = vfs.get(parent_ino).unwrap().nlink;
    vfs.rename("/tmp/old", "/tmp/new").unwrap();
    assert_eq!(vfs.get(parent_ino).unwrap().nlink, nlink_before);
}

// ── resolve: depth limit ────────────────────────────────────────────

#[test]
fn resolve_depth_limit() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    // Create a long chain of symlinks
    vfs.create_file("/tmp/target", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    for i in 0..42 {
        let name = format!("/tmp/l{}", i);
        let target = if i == 0 {
            "/tmp/target".to_string()
        } else {
            format!("/tmp/l{}", i - 1)
        };
        vfs.symlink(&name, &target, LASH_UID, LASH_GID).unwrap();
    }
    // Should fail with too many levels
    assert!(vfs.resolve("/tmp/l41", true).is_err());
}

// ── Inode::new nlink for Dir ────────────────────────────────────────

#[test]
fn inode_new_dir_nlink() {
    let mut vfs = Vfs::new();
    let ino = vfs.mkdir("/d", 0o755, ROOT_UID, ROOT_GID).unwrap();
    assert_eq!(vfs.get(ino).unwrap().nlink, 2);
}

#[test]
fn inode_new_file_nlink() {
    let mut vfs = Vfs::new();
    vfs.mkdir("/tmp", 0o755, ROOT_UID, ROOT_GID).unwrap();
    let ino = vfs
        .create_file("/tmp/f", 0o644, LASH_UID, LASH_GID)
        .unwrap();
    assert_eq!(vfs.get(ino).unwrap().nlink, 1);
}
