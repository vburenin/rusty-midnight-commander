//! sftpfs against a local fake SSH/SFTP server (no internet).
#[path = "support/fake_ssh.rs"]
mod fake_ssh;

use fake_ssh::{fixture_tree, lock_host_keys, FakeSsh};
use rmc_fs::composite::CompositeFs;
use rmc_fs::sftpfs::{
    set_host_key_action, set_known_hosts_path, take_host_key_prompt, HostKeyAction,
};
use rmc_fs::Vfs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn names(list: &[rmc_fs::DirEntry]) -> Vec<String> {
    list.iter().map(|e| e.name.clone()).collect()
}

fn find_parent(vfs: &CompositeFs, path: &Path) -> PathBuf {
    let list = vfs.list_dir(path, true).unwrap();
    list.iter()
        .find(|e| e.name == "..")
        .expect("parent marker")
        .path
        .clone()
}

fn prepare_known_hosts() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let kh = dir.path().join("known_hosts");
    set_known_hosts_path(Some(kh));
    set_host_key_action(HostKeyAction::Yes);
    dir
}

#[test]
fn sftpfs_list_stat_copy_out_and_parent_leave() {
    let _serial = lock_host_keys();
    let _kh = prepare_known_hosts();
    let ssh = FakeSsh::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);

    let enter = vfs.enter_path(&ssh.sftp_url("")).expect("sftp enterable");
    assert_eq!(
        enter,
        PathBuf::from(format!("/#sftp:user:pass@127.0.0.1:{}", ssh.port))
    );

    let root = enter.clone();
    let list = vfs.list_dir(&root, false).unwrap();
    let n = names(&list);
    assert!(n.contains(&"..".into()), "parent marker: {n:?}");
    assert!(n.contains(&"hello.txt".into()), "{n:?}");
    assert!(n.contains(&"pub".into()), "{n:?}");
    assert!(!n.contains(&".hidden".into()), "hidden filtered: {n:?}");
    let hello = list.iter().find(|e| e.name == "hello.txt").unwrap();
    assert!(!hello.meta.is_dir);
    assert_eq!(hello.meta.size, b"hello-ssh".len() as u64);
    let pub_ent = list.iter().find(|e| e.name == "pub").unwrap();
    assert!(pub_ent.meta.is_dir);
    assert_eq!(
        pub_ent.path,
        PathBuf::from(format!("/#sftp:user:pass@127.0.0.1:{}/pub", ssh.port))
    );

    let shown = vfs.list_dir(&root, true).unwrap();
    assert!(names(&shown).contains(&".hidden".into()));

    let hello_path = root.join("hello.txt");
    let st = vfs.stat(&hello_path).unwrap();
    assert!(!st.is_dir);
    assert_eq!(st.size, b"hello-ssh".len() as u64);

    let pub_path = root.join("pub");
    let st_dir = vfs.stat(&pub_path).unwrap();
    assert!(st_dir.is_dir);

    let mut r = vfs.read_file(&hello_path).unwrap();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut r, &mut buf).unwrap();
    assert_eq!(buf, b"hello-ssh");

    let tmp = tempdir().unwrap();
    let dst = tmp.path().join("out.txt");
    vfs.copy(&hello_path, &dst).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"hello-ssh");

    let inner = pub_path.join("inner.txt");
    let inner_list = vfs.list_dir(&pub_path, true).unwrap();
    assert!(names(&inner_list).contains(&"inner.txt".into()));
    let inner_dst = tmp.path().join("inner.txt");
    vfs.copy(&inner, &inner_dst).unwrap();
    assert_eq!(std::fs::read(&inner_dst).unwrap(), b"inner-payload");

    let from_pub = find_parent(&vfs, &pub_path);
    assert_eq!(from_pub, root);
    let outside = find_parent(&vfs, &root);
    assert_eq!(outside, PathBuf::from("/"));
    assert_eq!(root.parent(), Some(Path::new("/")));
}

#[test]
fn sftpfs_list_via_url_and_hash_sftp_prefix() {
    let _serial = lock_host_keys();
    let _kh = prepare_known_hosts();
    let ssh = FakeSsh::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);

    let url = ssh.sftp_url("");
    let list = vfs.list_dir(&url, false).unwrap();
    assert!(names(&list).contains(&"hello.txt".into()));
    let parent = list.iter().find(|e| e.name == "..").unwrap();
    assert_eq!(parent.path, PathBuf::from("/"));

    let hash = PathBuf::from(format!("/#sftp:user:pass@127.0.0.1:{}", ssh.port));
    let list2 = vfs.list_dir(&hash, false).unwrap();
    assert!(names(&list2).contains(&"pub".into()));
}

#[test]
fn sftpfs_connect_error_is_plain_fserror() {
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let err = vfs
        .list_dir(Path::new("sftp://127.0.0.1:1/"), false)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("SSH") || msg.contains("SFTP") || msg.contains("connect"),
        "error should be a plain FsError for the existing dialog, got {msg}"
    );
}

#[test]
fn sftpfs_nested_parent_then_leave() {
    let _serial = lock_host_keys();
    let _kh = prepare_known_hosts();
    let ssh = FakeSsh::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let root = vfs.enter_path(&ssh.sftp_url("")).unwrap();
    let pub_dir = root.join("pub");
    assert_eq!(find_parent(&vfs, &pub_dir), root);
    assert_eq!(find_parent(&vfs, &root), PathBuf::from("/"));
}

#[test]
fn sftpfs_unknown_host_key_stashes_prompt_yes_adds() {
    let _serial = lock_host_keys();
    let dir = tempdir().unwrap();
    let kh = dir.path().join("known_hosts");
    set_known_hosts_path(Some(kh.clone()));
    let _ = take_host_key_prompt();
    set_host_key_action(HostKeyAction::No);

    let ssh = FakeSsh::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let url = ssh.sftp_url("");
    let err = vfs.list_dir(&url, false).unwrap_err();
    let prompt = take_host_key_prompt().expect("host key prompt");
    assert!(matches!(prompt.kind, rmc_fs::sftpfs::HostKeyKind::Unknown));
    assert!(err.to_string().contains("known_hosts"));
    assert_eq!(
        rmc_fs::sftpfs::HostKeyPrompt::dialog_title(),
        "SFTP filesystem"
    );
    assert!(prompt.dialog_message().contains("Yes"));
    assert!(prompt.dialog_message().contains("Ignore"));
    assert!(prompt.dialog_message().contains("No"));

    set_host_key_action(HostKeyAction::Yes);
    let list = vfs.list_dir(&url, false).unwrap();
    assert!(names(&list).contains(&"hello.txt".into()));
    let stored = std::fs::read_to_string(&kh).unwrap();
    assert!(stored.contains("127.0.0.1"), "{stored}");
}

#[test]
fn sftpfs_ignore_continues_without_writing_known_hosts() {
    let _serial = lock_host_keys();
    let dir = tempdir().unwrap();
    let kh = dir.path().join("known_hosts");
    set_known_hosts_path(Some(kh.clone()));
    let _ = take_host_key_prompt();
    set_host_key_action(HostKeyAction::Ignore);

    let ssh = FakeSsh::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let list = vfs.list_dir(&ssh.sftp_url(""), false).unwrap();
    assert!(names(&list).contains(&"hello.txt".into()));
    let stored = std::fs::read_to_string(&kh).unwrap_or_default();
    assert!(
        stored.trim().is_empty(),
        "Ignore must not write known_hosts, got {stored:?}"
    );
}
