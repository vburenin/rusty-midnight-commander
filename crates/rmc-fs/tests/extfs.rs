use rmc_fs::composite::CompositeFs;
use rmc_fs::Vfs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn anchor_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

#[test]
fn extfs_list_and_leave_parent() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    // Create some files
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    // Create .lsar listing relative paths
    let arc = root.join("sample.lsar");
    std::fs::write(&arc, "a.txt\nb.txt\n").unwrap();

    let vfs = CompositeFs::new();
    // Enter extfs should be suggested for .lsar
    let enter = vfs.enter_path(&arc).expect("enterable extfs");
    assert_eq!(enter, anchor_path(&arc));

    // List extfs root
    let root_anchor = anchor_path(&arc);
    let list = vfs.list_dir(&root_anchor, false).unwrap();
    let names: Vec<_> = list.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"a.txt".to_string()));
    assert!(names.contains(&"b.txt".to_string()));

    // Parent from root leaves the extfs to the directory containing the archive
    let parent = list.iter().find(|e| e.name == "..").unwrap();
    assert_eq!(parent.path, arc.parent().unwrap().to_path_buf());
}
