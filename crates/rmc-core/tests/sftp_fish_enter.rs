//! Panel enter / `..` leave for sftpfs and FISH (GNU `/#sftp:` / `/#sh:` wiring).
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, Metadata, Vfs};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::tempdir;

#[derive(Debug)]
struct RemoteAwareFs {
    inner: LocalFs,
}

fn epoch_dir() -> Metadata {
    Metadata {
        is_dir: true,
        is_symlink: false,
        symlink_target: None,
        is_executable: false,
        size: 0,
        modified: SystemTime::UNIX_EPOCH,
        accessed: SystemTime::UNIX_EPOCH,
        changed: SystemTime::UNIX_EPOCH,
        permissions: 0o755,
        owner: None,
        group: None,
        nlink: 1,
        inode: 0,
    }
}

fn epoch_file(size: u64) -> Metadata {
    Metadata {
        is_dir: false,
        is_symlink: false,
        symlink_target: None,
        is_executable: false,
        size,
        modified: SystemTime::UNIX_EPOCH,
        accessed: SystemTime::UNIX_EPOCH,
        changed: SystemTime::UNIX_EPOCH,
        permissions: 0o644,
        owner: None,
        group: None,
        nlink: 1,
        inode: 0,
    }
}

fn is_sftp_panel(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("#sftp:") || s.starts_with("sftp://")
}

fn is_sh_panel(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("#sh:") || s.starts_with("sh://") || s.starts_with("fish://") || s.contains("#fish:")
}

impl Vfs for RemoteAwareFs {
    fn cwd(&self) -> rmc_fs::FsResult<PathBuf> {
        self.inner.cwd()
    }
    fn canonicalize_path(&self, path: &Path) -> PathBuf {
        let p = rmc_fs::sftpfs::canonicalize_panel_path(path);
        rmc_fs::fish::canonicalize_panel_path(&p)
    }
    fn enter_path(&self, path: &Path) -> Option<PathBuf> {
        if rmc_fs::remote::is_remote_url(path) {
            Some(self.canonicalize_path(path))
        } else {
            self.inner.enter_path(path)
        }
    }
    fn list_dir(&self, path: &Path, show_hidden: bool) -> rmc_fs::FsResult<Vec<DirEntry>> {
        let path = self.canonicalize_path(path);
        if !is_sftp_panel(&path) && !is_sh_panel(&path) {
            return self.inner.list_dir(&path, show_hidden);
        }
        let url = rmc_fs::remote::parse_remote_url(&path)?;
        let parent = if url.path == "/" || url.path.is_empty() {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"))
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"))
        };
        let mut out = vec![DirEntry {
            name: "..".into(),
            path: parent,
            meta: epoch_dir(),
        }];
        if url.path == "/" || url.path.is_empty() {
            out.push(DirEntry {
                name: "pub".into(),
                path: path.join("pub"),
                meta: epoch_dir(),
            });
            out.push(DirEntry {
                name: "hello.txt".into(),
                path: path.join("hello.txt"),
                meta: epoch_file(9),
            });
        } else if url.path == "/pub" {
            out.push(DirEntry {
                name: "inner.txt".into(),
                path: path.join("inner.txt"),
                meta: epoch_file(5),
            });
        }
        Ok(out)
    }
    fn mkdir(&self, path: &Path) -> rmc_fs::FsResult<()> {
        self.inner.mkdir(path)
    }
    fn remove(&self, path: &Path, recursive: bool) -> rmc_fs::FsResult<()> {
        self.inner.remove(path, recursive)
    }
    fn copy(&self, src: &Path, dst: &Path) -> rmc_fs::FsResult<()> {
        self.inner.copy(src, dst)
    }
    fn move_path(&self, src: &Path, dst: &Path) -> rmc_fs::FsResult<()> {
        self.inner.move_path(src, dst)
    }
    fn read_file(&self, path: &Path) -> rmc_fs::FsResult<Box<dyn std::io::Read + Send>> {
        self.inner.read_file(path)
    }
    fn write_file(&self, path: &Path) -> rmc_fs::FsResult<Box<dyn std::io::Write + Send>> {
        self.inner.write_file(path)
    }
    fn stat(&self, path: &Path) -> rmc_fs::FsResult<Metadata> {
        if is_sftp_panel(path) || is_sh_panel(path) {
            Ok(epoch_file(0))
        } else {
            self.inner.stat(path)
        }
    }
}

#[test]
fn change_dir_sftp_url_then_enter_dotdot_leaves() -> Result<()> {
    let dir = tempdir()?;
    let home = dir.path().join("home");
    std::fs::create_dir(&home)?;
    let vfs = RemoteAwareFs {
        inner: LocalFs::new(),
    };
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(&home)?;

    app.change_dir(Path::new("sftp://example.com/pub"))?;
    assert_eq!(
        app.active_panel().cwd,
        PathBuf::from("/#sftp:example.com/pub"),
        "sftp:// must canonicalize to GNU /#sftp: so Path::parent can leave"
    );
    let names: Vec<_> = app
        .active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"..".into()), "{names:?}");
    assert!(names.contains(&"inner.txt".into()), "{names:?}");

    let parent_idx = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.name == "..")
        .unwrap();
    app.active_panel_mut().cursor = parent_idx;
    app.handle_action(Action::Enter)?;
    assert_eq!(
        app.active_panel().cwd,
        PathBuf::from("/#sftp:example.com"),
        "first .. stays inside sftpfs"
    );

    let parent_idx = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.name == "..")
        .unwrap();
    app.active_panel_mut().cursor = parent_idx;
    app.handle_action(Action::Enter)?;
    assert_eq!(
        app.active_panel().cwd,
        PathBuf::from("/"),
        "second .. leaves sftpfs like archive #"
    );
    Ok(())
}

#[test]
fn parent_dir_action_leaves_sftp_root() -> Result<()> {
    let vfs = RemoteAwareFs {
        inner: LocalFs::new(),
    };
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(Path::new("/#sftp:example.com"))?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/#sftp:example.com"));
    app.handle_action(Action::ParentDir)?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/"));
    Ok(())
}

#[test]
fn change_dir_sh_url_then_enter_dotdot_leaves() -> Result<()> {
    let dir = tempdir()?;
    let home = dir.path().join("home");
    std::fs::create_dir(&home)?;
    let vfs = RemoteAwareFs {
        inner: LocalFs::new(),
    };
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(&home)?;

    app.change_dir(Path::new("sh://joe@host/pub"))?;
    assert_eq!(
        app.active_panel().cwd,
        PathBuf::from("/#sh:joe@host/pub"),
        "sh:// must canonicalize to GNU /#sh: so Path::parent can leave"
    );
    let names: Vec<_> = app
        .active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"inner.txt".into()), "{names:?}");

    let parent_idx = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.name == "..")
        .unwrap();
    app.active_panel_mut().cursor = parent_idx;
    app.handle_action(Action::Enter)?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/#sh:joe@host"));

    let parent_idx = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.name == "..")
        .unwrap();
    app.active_panel_mut().cursor = parent_idx;
    app.handle_action(Action::Enter)?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/"));
    Ok(())
}

#[test]
fn parent_dir_action_leaves_sh_root() -> Result<()> {
    let vfs = RemoteAwareFs {
        inner: LocalFs::new(),
    };
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(Path::new("fish://example.com"))?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/#sh:example.com"));
    app.handle_action(Action::ParentDir)?;
    assert_eq!(app.active_panel().cwd, PathBuf::from("/"));
    Ok(())
}
