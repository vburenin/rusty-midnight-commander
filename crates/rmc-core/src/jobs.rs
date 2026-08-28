use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rmc_fs::composite::CompositeFs;
use rmc_fs::pathutil::is_virtual_path;
use rmc_fs::{CopyFlags, Vfs};

const DEFAULT_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    /// GNU mc(1) Background jobs **Stop**: paused without cancelling.
    /// A queued job that was stopped never started; a running job is suspended
    /// at a chunk/file boundary until **Restart**.
    Stopped,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundJob {
    pub id: JobId,
    pub kind: JobKind,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Bytes copied of the file currently being written.
    pub file_done: u64,
    /// Size of the file currently being written.
    pub file_total: u64,
    /// Files fully copied so far (not including the file in progress).
    pub files_done: u64,
    /// Basename of the file currently being written.
    pub current_name: String,
    pub status: JobStatus,
    pub error: Option<String>,
}

#[derive(Debug)]
struct JobEntry {
    job: BackgroundJob,
    cancel_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    /// True once the worker has entered [`run_one_job`] for this entry.
    /// Distinguishes a Stopped job that is mid-transfer (Restart resumes) from
    /// one that was stopped while still queued (Restart re-queues).
    started: bool,
    copy_flags: CopyFlags,
}

#[derive(Debug, Default)]
struct Inner {
    jobs: Vec<Arc<Mutex<JobEntry>>>,
    shutdown: bool,
}

/// Background job queue that executes file copy/move operations on a single worker thread.
#[derive(Debug)]
pub struct JobQueue {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    next_id: AtomicU64,
    worker: Option<JoinHandle<()>>,
}

impl JobQueue {
    pub fn new() -> Self {
        let inner = Arc::new((Mutex::new(Inner::default()), Condvar::new()));
        let worker = {
            let inner_clone = Arc::clone(&inner);
            thread::spawn(move || worker_loop(inner_clone))
        };
        Self {
            inner,
            next_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    /// Spawn a copy job. Returns the JobId. Uses GNU Configuration defaults
    /// (preallocate off, COW clone on).
    pub fn spawn_copy<P: Into<PathBuf>, Q: Into<PathBuf>>(&self, src: P, dst: Q) -> JobId {
        self.spawn_copy_with_flags(src, dst, CopyFlags::default())
    }

    /// Spawn a copy job honoring Options → Configuration copy flags.
    pub fn spawn_copy_with_flags<P: Into<PathBuf>, Q: Into<PathBuf>>(
        &self,
        src: P,
        dst: Q,
        flags: CopyFlags,
    ) -> JobId {
        self.enqueue(JobKind::Copy, src.into(), dst.into(), flags)
    }

    /// Spawn a move job. Returns the JobId.
    pub fn spawn_move<P: Into<PathBuf>, Q: Into<PathBuf>>(&self, src: P, dst: Q) -> JobId {
        self.spawn_move_with_flags(src, dst, CopyFlags::default())
    }

    /// Spawn a move job. Copy+delete fallback honors the same flags as Copy.
    pub fn spawn_move_with_flags<P: Into<PathBuf>, Q: Into<PathBuf>>(
        &self,
        src: P,
        dst: Q,
        flags: CopyFlags,
    ) -> JobId {
        self.enqueue(JobKind::Move, src.into(), dst.into(), flags)
    }

    fn enqueue(&self, kind: JobKind, src: PathBuf, dst: PathBuf, copy_flags: CopyFlags) -> JobId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Mutex::new(JobEntry {
            job: BackgroundJob {
                id,
                kind,
                src,
                dst,
                bytes_done: 0,
                bytes_total: 0,
                file_done: 0,
                file_total: 0,
                files_done: 0,
                current_name: String::new(),
                status: JobStatus::Queued,
                error: None,
            },
            cancel_flag: Arc::new(AtomicBool::new(false)),
            pause_flag: Arc::new(AtomicBool::new(false)),
            started: false,
            copy_flags,
        }));
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().expect("JobQueue mutex poisoned");
        inner.jobs.push(entry);
        cvar.notify_all();
        id
    }

    /// Cancel a job by id. Best-effort: running copies will stop mid-copy.
    /// - If the job is queued or stopped-before-start, it transitions to Cancelled immediately.
    /// - If the job is running or paused mid-transfer, it will stop at the next chunk boundary.
    ///   Returns true if a job with this id existed.
    pub fn cancel(&self, id: JobId) -> bool {
        let (lock, cvar) = &*self.inner;
        let inner = lock.lock().expect("JobQueue mutex poisoned");
        let mut found = false;
        for job_arc in &inner.jobs {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            if job.job.id == id {
                found = true;
                job.cancel_flag.store(true, Ordering::Relaxed);
                job.pause_flag.store(false, Ordering::Relaxed);
                if matches!(job.job.status, JobStatus::Queued | JobStatus::Stopped) && !job.started
                {
                    job.job.status = JobStatus::Cancelled;
                }
                break;
            }
        }
        if found {
            cvar.notify_all();
        }
        found
    }

    /// GNU Background jobs **Stop**: pause a Queued or Running job. The job stays
    /// listed as Stopped and does not continue transferring until [`Self::restart`].
    pub fn stop(&self, id: JobId) -> bool {
        let (lock, cvar) = &*self.inner;
        let inner = lock.lock().expect("JobQueue mutex poisoned");
        let mut found = false;
        for job_arc in &inner.jobs {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            if job.job.id == id {
                found = true;
                if matches!(job.job.status, JobStatus::Queued | JobStatus::Running) {
                    job.pause_flag.store(true, Ordering::Relaxed);
                    job.job.status = JobStatus::Stopped;
                }
                break;
            }
        }
        if found {
            cvar.notify_all();
        }
        found
    }

    /// GNU Background jobs **Restart** / **Resume**:
    /// - Stopped mid-transfer: continue from the pause point.
    /// - Stopped while still queued: re-queue so the worker will start it.
    /// - Failed or Cancelled: re-run from the start with the same src/dst/flags.
    pub fn restart(&self, id: JobId) -> bool {
        let (lock, cvar) = &*self.inner;
        let inner = lock.lock().expect("JobQueue mutex poisoned");
        let mut found = false;
        for job_arc in &inner.jobs {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            if job.job.id == id {
                found = true;
                match job.job.status {
                    JobStatus::Stopped => {
                        job.pause_flag.store(false, Ordering::Relaxed);
                        job.cancel_flag.store(false, Ordering::Relaxed);
                        if job.started {
                            job.job.status = JobStatus::Running;
                        } else {
                            job.job.status = JobStatus::Queued;
                        }
                    }
                    JobStatus::Failed | JobStatus::Cancelled => {
                        reset_entry_for_rerun(&mut job);
                    }
                    _ => {}
                }
                break;
            }
        }
        if found {
            cvar.notify_all();
        }
        found
    }

    /// GNU Background jobs **Kill**: abort the transfer and remove the job from
    /// the list (Cancel + drop, including in-flight jobs).
    pub fn kill(&self, id: JobId) -> bool {
        if !self.cancel(id) {
            return false;
        }
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().expect("JobQueue mutex poisoned");
        inner.jobs.retain(|job_arc| {
            let job = job_arc.lock().expect("JobEntry mutex poisoned");
            job.job.id != id
        });
        cvar.notify_all();
        true
    }

    /// Drop all finished (Done/Failed/Cancelled) jobs from the queue.
    pub fn drop_finished_jobs(&self) {
        let (lock, _cvar) = &*self.inner;
        let mut inner = lock.lock().expect("JobQueue mutex poisoned");
        inner.jobs.retain(|job_arc| {
            let job = job_arc.lock().expect("JobEntry mutex poisoned");
            !job.job.status.is_finished()
        });
    }

    /// Snapshot one job by id.
    pub fn get(&self, id: JobId) -> Option<BackgroundJob> {
        self.snapshot().into_iter().find(|j| j.id == id)
    }

    /// Return a snapshot of all jobs.
    pub fn snapshot(&self) -> Vec<BackgroundJob> {
        let (lock, _cvar) = &*self.inner;
        let inner = lock.lock().expect("JobQueue mutex poisoned");
        inner
            .jobs
            .iter()
            .map(|job_arc| {
                let job = job_arc.lock().expect("JobEntry mutex poisoned");
                job.job.clone()
            })
            .collect()
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        // Signal shutdown and join the worker thread.
        let (lock, cvar) = &*self.inner;
        {
            let mut inner = lock.lock().expect("JobQueue mutex poisoned");
            inner.shutdown = true;
            cvar.notify_all();
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(inner: Arc<(Mutex<Inner>, Condvar)>) {
    let (lock, cvar) = &*inner;
    loop {
        let job_to_run = {
            let mut guard = lock.lock().expect("JobQueue mutex poisoned");
            // Wait for a queued job or shutdown.
            while !guard.shutdown && !has_queued_job(&guard.jobs) {
                guard = cvar.wait(guard).expect("JobQueue condvar poisoned");
            }
            if guard.shutdown {
                return;
            }
            // Pick the next queued job.
            let Some(idx) = next_queued_index(&guard.jobs) else {
                // Spurious wakeup or queued job was cancelled/consumed.
                continue;
            };
            let job_arc = Arc::clone(&guard.jobs[idx]);
            {
                let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
                // Only run queued jobs; anything else is skipped.
                if job.job.status != JobStatus::Queued {
                    continue;
                }
                // If this job was cancelled while queued, mark it cancelled to avoid re-picking it.
                if job.cancel_flag.load(Ordering::Relaxed) {
                    job.job.status = JobStatus::Cancelled;
                    continue;
                }
                // Stopped-while-queued stays Stopped until Restart.
                if job.pause_flag.load(Ordering::Relaxed) {
                    job.job.status = JobStatus::Stopped;
                    continue;
                }
                job.started = true;
                job.job.status = JobStatus::Running;
            }
            job_arc
        };

        // Run the job outside of the global lock.
        run_one_job(&job_to_run);
        // Notify anyone waiting that job statuses have changed.
        cvar.notify_all();
    }
}

fn reset_entry_for_rerun(job: &mut JobEntry) {
    job.cancel_flag.store(false, Ordering::Relaxed);
    job.pause_flag.store(false, Ordering::Relaxed);
    job.started = false;
    job.job.bytes_done = 0;
    job.job.bytes_total = 0;
    job.job.file_done = 0;
    job.job.file_total = 0;
    job.job.files_done = 0;
    job.job.current_name.clear();
    job.job.error = None;
    job.job.status = JobStatus::Queued;
}

/// Block while the job is Stopped. Returns true if the job was cancelled.
fn wait_if_paused(job_arc: &Arc<Mutex<JobEntry>>) -> bool {
    loop {
        let (paused, cancelled) = {
            let job = job_arc.lock().expect("JobEntry mutex poisoned");
            (
                job.pause_flag.load(Ordering::Relaxed),
                job.cancel_flag.load(Ordering::Relaxed),
            )
        };
        if cancelled {
            return true;
        }
        if !paused {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            if job.job.status == JobStatus::Stopped && job.started {
                job.job.status = JobStatus::Running;
            }
            return false;
        }
        {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            if job.job.status == JobStatus::Running {
                job.job.status = JobStatus::Stopped;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn has_queued_job(jobs: &[Arc<Mutex<JobEntry>>]) -> bool {
    jobs.iter().any(|j| {
        let j = j.lock().expect("JobEntry mutex poisoned");
        j.job.status == JobStatus::Queued
    })
}

fn next_queued_index(jobs: &[Arc<Mutex<JobEntry>>]) -> Option<usize> {
    jobs.iter().position(|j| {
        let j = j.lock().expect("JobEntry mutex poisoned");
        j.job.status == JobStatus::Queued
    })
}

fn run_one_job(job_arc: &Arc<Mutex<JobEntry>>) {
    let (kind, src, dst, cancel_flag, id, copy_flags) = {
        let job = job_arc.lock().expect("JobEntry mutex poisoned");
        (
            job.job.kind,
            job.job.src.clone(),
            job.job.dst.clone(),
            Arc::clone(&job.cancel_flag),
            job.job.id,
            job.copy_flags,
        )
    };

    let result = if is_virtual_path(&src) || is_virtual_path(&dst) {
        // GNU mc Copy/Move goes through VFS. Local-only `std::fs` cannot open
        // archive `#` paths or ftp/sftp/extfs URLs. Abort is checked between
        // files; a non-chunked VFS op (one `vfs.copy`) finishes the current
        // file before the cancel flag is observed — no live byte counters.
        // Preallocate / COW flags are no-ops on non-local VFS.
        let vfs = CompositeFs::new();
        match kind {
            JobKind::Copy => vfs_copy_tree(&vfs, job_arc, &src, &dst, &cancel_flag, &mut 0),
            JobKind::Move => vfs_move(&vfs, &src, &dst, &cancel_flag),
        }
    } else {
        match kind {
            JobKind::Copy => copy_streaming(job_arc, &src, &dst, &cancel_flag, copy_flags),
            JobKind::Move => move_with_fallback(job_arc, &src, &dst, &cancel_flag, copy_flags),
        }
    };

    let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
    match result {
        Ok(()) => {
            if job.cancel_flag.load(Ordering::Relaxed) {
                job.job.status = JobStatus::Cancelled;
            } else {
                job.job.status = JobStatus::Done;
            }
        }
        Err(e) => {
            // If the copy/delete failed due to cancellation, prefer Cancelled.
            if job.cancel_flag.load(Ordering::Relaxed) {
                job.job.status = JobStatus::Cancelled;
            } else {
                job.job.status = JobStatus::Failed;
                job.job.error = Some(format!("{e}"));
            }
        }
    }
    // Ensure bytes_done does not exceed bytes_total on completion.
    if job.job.bytes_done > job.job.bytes_total && job.job.bytes_total > 0 {
        job.job.bytes_done = job.job.bytes_total;
    }
    // Small signal to help tests that read snapshots frequently.
    drop(job);
    // Avoid holding the lock across potentially expensive filesystem ops elsewhere.
    let _ = id; // keep id for potential future logging
}

struct LinkState {
    hardlinks: HashMap<(u64, u64), PathBuf>,
    visited: HashSet<(u64, u64)>,
}

fn copy_streaming(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
    flags: CopyFlags,
) -> io::Result<()> {
    let mut overall: u64 = 0;
    let mut links = LinkState {
        hardlinks: HashMap::new(),
        visited: HashSet::new(),
    };
    copy_any(
        job_arc,
        src,
        dst,
        cancel_flag,
        &mut overall,
        flags,
        &mut links,
    )
}

fn copy_any(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
    overall: &mut u64,
    flags: CopyFlags,
    links: &mut LinkState,
) -> io::Result<()> {
    if wait_if_paused(job_arc) {
        return Ok(());
    }
    let lmd = fs::symlink_metadata(src)?;
    if lmd.file_type().is_symlink() && !flags.follow_links {
        rmc_fs::copy_local::copy_symlink(src, dst, flags.stable_symlinks)?;
        if flags.preserve_attrs {
            rmc_fs::copy_local::preserve_attrs(src, dst)?;
        }
        note_local_link_done(job_arc, src, overall);
        return Ok(());
    }
    let md = if flags.follow_links {
        fs::metadata(src).unwrap_or(lmd)
    } else {
        lmd
    };
    if md.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let key = (md.dev(), md.ino());
            if !links.visited.insert(key) {
                return Ok(());
            }
        }
        fs::create_dir_all(dst)?;
        for ent in fs::read_dir(src)? {
            if wait_if_paused(job_arc) {
                return Ok(());
            }
            let ent = ent?;
            let name = ent.file_name();
            if name == "." || name == ".." {
                continue;
            }
            copy_any(
                job_arc,
                &ent.path(),
                &dst.join(name),
                cancel_flag,
                overall,
                flags,
                links,
            )?;
        }
        if flags.preserve_attrs {
            rmc_fs::copy_local::preserve_attrs(src, dst)?;
        }
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !flags.follow_links && md.nlink() > 1 {
            let key = (md.dev(), md.ino());
            if let Some(prev) = links.hardlinks.get(&key) {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::hard_link(prev, dst)?;
                note_local_link_done(job_arc, src, overall);
                return Ok(());
            }
            copy_file_chunks(job_arc, src, dst, cancel_flag, overall, flags)?;
            links.hardlinks.insert(key, dst.to_path_buf());
            return Ok(());
        }
    }
    copy_file_chunks(job_arc, src, dst, cancel_flag, overall, flags)
}

fn note_local_link_done(job_arc: &Arc<Mutex<JobEntry>>, src: &Path, overall: &mut u64) {
    let current_name = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
    job.job.files_done = job.job.files_done.saturating_add(1);
    job.job.file_done = 0;
    job.job.file_total = 0;
    job.job.bytes_done = *overall;
    if !current_name.is_empty() {
        job.job.current_name = current_name;
    }
}

fn copy_file_chunks(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    _cancel_flag: &AtomicBool,
    overall: &mut u64,
    flags: CopyFlags,
) -> io::Result<()> {
    if wait_if_paused(job_arc) {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let meta = fs::metadata(src)?;
    let total = meta.len();
    let current_name = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    {
        let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
        job.job.file_total = total;
        job.job.file_done = 0;
        job.job.current_name = current_name;
        if job.job.bytes_total == 0 {
            job.job.bytes_total = total;
        }
    }

    let mut src_f = File::open(src)?;
    let mut dst_f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(dst)?;

    if flags.use_cow_file_cloning && rmc_fs::copy_local::try_cow_clone(&src_f, &dst_f) {
        drop(dst_f);
        *overall = overall.saturating_add(total);
        {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            job.job.files_done = job.job.files_done.saturating_add(1);
            job.job.file_done = total;
            job.job.file_total = total;
            job.job.bytes_done = *overall;
        }
        if flags.preserve_attrs {
            rmc_fs::copy_local::preserve_attrs(src, dst)?;
        }
        return Ok(());
    }

    if flags.preallocate_space {
        rmc_fs::copy_local::try_preallocate(&dst_f, total);
    }

    let mut buf = vec![0_u8; DEFAULT_CHUNK_SIZE];
    let mut done: u64 = 0;
    loop {
        if wait_if_paused(job_arc) {
            // Best-effort cancel: leave partial file as-is.
            return Ok(());
        }
        let read_n = src_f.read(&mut buf)?;
        if read_n == 0 {
            break;
        }
        dst_f.write_all(&buf[..read_n])?;
        done = done.saturating_add(read_n as u64);
        *overall = overall.saturating_add(read_n as u64);
        {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            job.job.file_done = done;
            job.job.bytes_done = *overall;
        }
        // Let the UI / tests observe live counters between 64 KiB chunks.
        thread::yield_now();
        if cfg!(test) {
            thread::sleep(Duration::from_millis(1));
        }
    }
    dst_f.flush()?;
    drop(dst_f);
    if flags.preserve_attrs {
        rmc_fs::copy_local::preserve_attrs(src, dst)?;
    }
    {
        let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
        job.job.files_done = job.job.files_done.saturating_add(1);
        job.job.file_done = done;
        job.job.bytes_done = *overall;
    }
    Ok(())
}

fn move_with_fallback(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
    flags: CopyFlags,
) -> io::Result<()> {
    // Follow links cannot be a rename: dest must be the referent content.
    if flags.follow_links {
        copy_streaming(job_arc, src, dst, cancel_flag, flags)?;
        if !cancel_flag.load(Ordering::Relaxed) {
            let md = fs::symlink_metadata(src)?;
            if md.file_type().is_dir() {
                fs::remove_dir_all(src)?;
            } else {
                fs::remove_file(src)?;
            }
        }
        return Ok(());
    }
    // Try fast path: rename.
    let src_meta = fs::symlink_metadata(src);
    match fs::rename(src, dst) {
        Ok(()) => {
            if flags.stable_symlinks {
                rmc_fs::copy_local::rewrite_stable_symlinks_after_move(src, dst)?;
            }
            // Update byte counters best-effort.
            if let Ok(m) = src_meta.as_ref().map(|m| m.len()) {
                let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
                job.job.bytes_total = m;
                job.job.bytes_done = m;
            }
            return Ok(());
        }
        Err(e) => {
            // Cross-device move? Fallback to copy + remove.
            if e.kind() != io::ErrorKind::CrossesDevices {
                // Not a cross-device error; attempt fallback anyway.
                // We'll proceed to copy-streaming and then remove on success.
            }
        }
    }
    // Fallback: copy then remove source if not cancelled. Honors the same
    // Preallocate / COW flags as a plain Copy.
    copy_streaming(job_arc, src, dst, cancel_flag, flags)?;
    if !cancel_flag.load(Ordering::Relaxed) {
        // Remove original only if not cancelled.
        let md = fs::symlink_metadata(src)?;
        if md.file_type().is_dir() {
            fs::remove_dir_all(src)?;
        } else {
            fs::remove_file(src)?;
        }
    }
    Ok(())
}

fn fs_to_io(err: rmc_fs::FsError) -> io::Error {
    io::Error::other(err.to_string())
}

/// Copy through [`CompositeFs`] (tar/zip/ftp/sftp/extfs). Directories are
/// walked so Abort can stop after the current file; a single-file `vfs.copy`
/// is not byte-chunked, so cancel mid-file finishes that file first.
fn vfs_copy_tree(
    vfs: &CompositeFs,
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
    overall: &mut u64,
) -> io::Result<()> {
    if wait_if_paused(job_arc) {
        return Ok(());
    }
    match vfs.stat(src) {
        Ok(meta) if meta.is_dir => {
            let _ = vfs.mkdir(dst);
            let entries = match vfs.list_dir(src, true) {
                Ok(entries) => entries,
                Err(_) => {
                    vfs.copy(src, dst).map_err(fs_to_io)?;
                    return Ok(());
                }
            };
            for entry in entries {
                if entry.name == ".." || entry.name == "." {
                    continue;
                }
                if wait_if_paused(job_arc) {
                    return Ok(());
                }
                vfs_copy_tree(
                    vfs,
                    job_arc,
                    &entry.path,
                    &dst.join(&entry.name),
                    cancel_flag,
                    overall,
                )?;
            }
            Ok(())
        }
        _ => {
            vfs.copy(src, dst).map_err(fs_to_io)?;
            note_vfs_file_done(job_arc, vfs, src, overall);
            Ok(())
        }
    }
}

fn vfs_move(vfs: &CompositeFs, src: &Path, dst: &Path, cancel_flag: &AtomicBool) -> io::Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(());
    }
    vfs.move_path(src, dst).map_err(fs_to_io)
}

/// Update file/total counters after a VFS file completes. Not live-during-copy.
fn note_vfs_file_done(
    job_arc: &Arc<Mutex<JobEntry>>,
    vfs: &CompositeFs,
    src: &Path,
    overall: &mut u64,
) {
    let size = vfs.stat(src).map(|m| m.size).unwrap_or(0);
    *overall = overall.saturating_add(size);
    let current_name = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
    job.job.files_done = job.job.files_done.saturating_add(1);
    job.job.file_done = size;
    job.job.file_total = size;
    job.job.bytes_done = *overall;
    if job.job.bytes_total == 0 {
        job.job.bytes_total = size;
    }
    if !current_name.is_empty() {
        job.job.current_name = current_name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn wait_for_status<F>(
        queue: &JobQueue,
        id: JobId,
        mut predicate: F,
        timeout_ms: u64,
    ) -> JobStatus
    where
        F: FnMut(JobStatus) -> bool,
    {
        let start = std::time::Instant::now();
        loop {
            let snap = queue.snapshot();
            if let Some(j) = snap.iter().find(|j| j.id == id) {
                if predicate(j.status) {
                    return j.status;
                }
            }
            if start.elapsed() > Duration::from_millis(timeout_ms) {
                // Return last known or Queued if missing.
                let snap = queue.snapshot();
                if let Some(j) = snap.iter().find(|j| j.id == id) {
                    return j.status;
                }
                return JobStatus::Queued;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn copy_small_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");

        // Write ~8 KiB
        let mut f = File::create(&src).unwrap();
        let data = vec![0xABu8; 8 * 1024];
        f.write_all(&data).unwrap();
        drop(f);

        let queue = JobQueue::new();
        let id = queue.spawn_copy(&src, &dst);
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(status, JobStatus::Done, "copy should complete successfully");

        let src_data = fs::read(&src).unwrap();
        let dst_data = fs::read(&dst).unwrap();
        assert_eq!(src_data, dst_data, "copied content must match");

        let snap = queue.snapshot();
        let j = snap.iter().find(|j| j.id == id).unwrap();
        assert_eq!(j.bytes_done, j.bytes_total);
        assert_eq!(j.bytes_total as usize, data.len());
    }

    #[test]
    fn cancel_large_copy() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("big_src.bin");
        let dst = dir.path().join("big_dst.bin");

        // Write ~8 MiB
        let mut f = File::create(&src).unwrap();
        let chunk = vec![0xCDu8; 1024];
        for _ in 0..(8 * 1024) {
            f.write_all(&chunk).unwrap();
        }
        drop(f);

        let queue = JobQueue::new();
        let id = queue.spawn_copy(&src, &dst);

        // Wait for running then cancel.
        let _ = wait_for_status(&queue, id, |s| s == JobStatus::Running, 5_000);
        let _ = queue.cancel(id);

        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Cancelled | JobStatus::Done | JobStatus::Failed
                )
            },
            10_000,
        );

        assert_eq!(status, JobStatus::Cancelled, "copy should be cancelled");

        // Destination file may exist and be partial.
        if let Ok(dst_meta) = fs::metadata(&dst) {
            let src_meta = fs::metadata(&src).unwrap();
            assert!(
                dst_meta.len() < src_meta.len(),
                "partial destination should be smaller than source"
            );
        }
    }

    #[test]
    fn move_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("m_src.bin");
        let dst = dir.path().join("m_dst.bin");

        let mut f = File::create(&src).unwrap();
        f.write_all(&[1u8, 2, 3, 4, 5]).unwrap();
        drop(f);

        let queue = JobQueue::new();
        let id = queue.spawn_move(&src, &dst);
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(status, JobStatus::Done, "move should complete successfully");

        assert!(!src.exists(), "source should be removed after move");
        assert!(dst.exists(), "destination should exist after move");
        let data = fs::read(&dst).unwrap();
        assert_eq!(data, vec![1u8, 2, 3, 4, 5]);
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let options = zip::write::FileOptions::default();
        for (name, data) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    fn zip_inner(archive: &Path, inner: &str) -> PathBuf {
        let mut s = archive.as_os_str().to_string_lossy().into_owned();
        s.push('#');
        PathBuf::from(s).join(inner)
    }

    #[test]
    fn copy_from_zip_archive_uses_vfs_and_writes_dest() {
        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        write_zip(&zip_path, &[("hello.txt", b"from-archive")]);
        let src = zip_inner(&zip_path, "hello.txt");
        let dst = dir.path().join("out.txt");

        // std::fs cannot open the virtual `#` path; the worker must use vfs.copy.
        assert!(
            fs::symlink_metadata(&src).is_err(),
            "virtual archive path is not a real local file"
        );

        let queue = JobQueue::new();
        let id = queue.spawn_copy(&src, &dst);
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "archive copy-out should complete via vfs.copy: {:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert_eq!(fs::read(&dst).unwrap(), b"from-archive");
        let snap = queue.snapshot();
        let j = snap.iter().find(|j| j.id == id).unwrap();
        assert!(j.files_done >= 1, "VFS copy records the completed file");
    }

    #[test]
    fn copy_with_cow_off_writes_file_bytes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let data = b"ordinary-byte-copy-no-clone";
        File::create(&src).unwrap().write_all(data).unwrap();

        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(
            &src,
            &dst,
            CopyFlags {
                preallocate_space: false,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        );
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "{:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn copy_with_preallocate_on_writes_file_bytes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let data = vec![0x3Cu8; 8192];
        File::create(&src).unwrap().write_all(&data).unwrap();

        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(
            &src,
            &dst,
            CopyFlags {
                preallocate_space: true,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        );
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "{:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[cfg(unix)]
    #[test]
    fn background_follow_links_off_copies_symlink() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let src = dir.path().join("link");
        let dst = dir.path().join("copied");
        fs::write(&target, b"payload").unwrap();
        std::os::unix::fs::symlink(&target, &src).unwrap();
        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(
            &src,
            &dst,
            CopyFlags {
                follow_links: false,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        );
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "{:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn background_follow_links_on_copies_referent() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let src = dir.path().join("link");
        let dst = dir.path().join("copied");
        fs::write(&target, b"payload").unwrap();
        std::os::unix::fs::symlink(&target, &src).unwrap();
        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(
            &src,
            &dst,
            CopyFlags {
                follow_links: true,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        );
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "{:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert!(!fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
    }

    #[cfg(unix)]
    #[test]
    fn background_preserve_attrs_on_keeps_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        fs::write(&src, b"x").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o707)).unwrap();
        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(
            &src,
            &dst,
            CopyFlags {
                preserve_attrs: true,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        );
        let status = wait_for_status(
            &queue,
            id,
            |s| {
                matches!(
                    s,
                    JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
                )
            },
            5_000,
        );
        assert_eq!(
            status,
            JobStatus::Done,
            "{:?}",
            queue.get(id).and_then(|j| j.error)
        );
        assert_eq!(
            fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o707
        );
    }

    fn no_cow() -> CopyFlags {
        CopyFlags {
            use_cow_file_cloning: false,
            preallocate_space: false,
            ..CopyFlags::default()
        }
    }

    fn write_big(path: &Path, mib: usize) {
        let mut f = File::create(path).unwrap();
        let chunk = vec![0xCDu8; 1024];
        for _ in 0..(mib * 1024) {
            f.write_all(&chunk).unwrap();
        }
    }

    #[test]
    fn stop_running_copy_pauses_then_restart_completes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("big_src.bin");
        let dst = dir.path().join("big_dst.bin");
        write_big(&src, 8);

        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(&src, &dst, no_cow());
        let _ = wait_for_status(&queue, id, |s| s == JobStatus::Running, 5_000);
        // Wait until some bytes have landed so Stop is mid-transfer.
        let start = std::time::Instant::now();
        loop {
            if queue.get(id).map(|j| j.bytes_done).unwrap_or(0) > 0 {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "copy never advanced"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(queue.stop(id));
        let status = wait_for_status(&queue, id, |s| s == JobStatus::Stopped, 5_000);
        assert_eq!(status, JobStatus::Stopped);

        let paused_at = queue.get(id).unwrap().bytes_done;
        thread::sleep(Duration::from_millis(40));
        let still = queue.get(id).unwrap();
        assert_eq!(still.status, JobStatus::Stopped);
        assert_eq!(
            still.bytes_done, paused_at,
            "stopped job must not keep transferring"
        );
        assert!(
            paused_at < still.bytes_total || still.bytes_total == 0,
            "stop should land before the copy finishes"
        );

        assert!(queue.restart(id));
        let status = wait_for_status(
            &queue,
            id,
            |s| s.is_finished() || s == JobStatus::Running,
            5_000,
        );
        // Running is fine; wait for Done.
        let status = if status == JobStatus::Running {
            wait_for_status(&queue, id, JobStatus::is_finished, 10_000)
        } else {
            status
        };
        assert_eq!(status, JobStatus::Done, "{:?}", queue.get(id));
        assert_eq!(fs::read(&src).unwrap(), fs::read(&dst).unwrap());
    }

    #[test]
    fn stop_queued_job_stays_stopped_until_restart() {
        let dir = tempdir().unwrap();
        let src_a = dir.path().join("a.bin");
        let dst_a = dir.path().join("a.out");
        let src_b = dir.path().join("b.bin");
        let dst_b = dir.path().join("b.out");
        write_big(&src_a, 4);
        File::create(&src_b).unwrap().write_all(b"queued").unwrap();

        let queue = JobQueue::new();
        let id_a = queue.spawn_copy_with_flags(&src_a, &dst_a, no_cow());
        let id_b = queue.spawn_copy_with_flags(&src_b, &dst_b, no_cow());
        assert!(queue.stop(id_b));
        let _ = wait_for_status(&queue, id_b, |s| s == JobStatus::Stopped, 2_000);
        assert_eq!(queue.get(id_b).unwrap().status, JobStatus::Stopped);

        let _ = wait_for_status(&queue, id_a, JobStatus::is_finished, 10_000);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            queue.get(id_b).unwrap().status,
            JobStatus::Stopped,
            "stopped queued job must not start after the running job finishes"
        );
        assert!(!dst_b.exists(), "stopped job must not write dest");

        assert!(queue.restart(id_b));
        let status = wait_for_status(&queue, id_b, JobStatus::is_finished, 5_000);
        assert_eq!(status, JobStatus::Done, "{:?}", queue.get(id_b));
        assert_eq!(fs::read(&dst_b).unwrap(), b"queued");
    }

    #[test]
    fn kill_removes_running_job() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("k_src.bin");
        let dst = dir.path().join("k_dst.bin");
        write_big(&src, 8);
        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(&src, &dst, no_cow());
        let _ = wait_for_status(&queue, id, |s| s == JobStatus::Running, 5_000);
        assert!(queue.kill(id));
        assert!(
            queue.get(id).is_none(),
            "Kill must abort and remove the job from the list"
        );
        // Worker may still be unwinding; dest should not become a full copy.
        thread::sleep(Duration::from_millis(80));
        if let Ok(meta) = fs::metadata(&dst) {
            let src_len = fs::metadata(&src).unwrap().len();
            assert!(meta.len() < src_len, "killed copy should be incomplete");
        }
    }

    #[cfg(unix)]
    #[test]
    fn restart_failed_reruns_with_same_flags() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let blocker = dir.path().join("notadir");
        fs::write(&src, b"payload").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o707)).unwrap();
        fs::write(&blocker, b"file").unwrap();
        let dst = blocker.join("out.bin");

        let queue = JobQueue::new();
        let flags = CopyFlags {
            preserve_attrs: true,
            use_cow_file_cloning: false,
            ..CopyFlags::default()
        };
        let id = queue.spawn_copy_with_flags(&src, &dst, flags);
        let status = wait_for_status(&queue, id, JobStatus::is_finished, 5_000);
        assert_eq!(status, JobStatus::Failed, "{:?}", queue.get(id));

        fs::remove_file(&blocker).unwrap();
        fs::create_dir(&blocker).unwrap();
        assert!(queue.restart(id));
        let status = wait_for_status(&queue, id, JobStatus::is_finished, 5_000);
        assert_eq!(status, JobStatus::Done, "{:?}", queue.get(id));
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
        assert_eq!(
            fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o707,
            "Restart of a failed job must keep the original copy flags"
        );
    }

    #[test]
    fn drop_finished_keeps_stopped_jobs() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("s.bin");
        let dst = dir.path().join("d.bin");
        write_big(&src, 4);
        let queue = JobQueue::new();
        let id = queue.spawn_copy_with_flags(&src, &dst, no_cow());
        let _ = wait_for_status(&queue, id, |s| s == JobStatus::Running, 5_000);
        assert!(queue.stop(id));
        let _ = wait_for_status(&queue, id, |s| s == JobStatus::Stopped, 5_000);
        queue.drop_finished_jobs();
        assert_eq!(
            queue.get(id).unwrap().status,
            JobStatus::Stopped,
            "Clean up must not drop a stopped job"
        );
        // Finished jobs are still dropped.
        let tiny = dir.path().join("t.bin");
        File::create(&tiny).unwrap().write_all(b"x").unwrap();
        // Kill the stopped job so the worker can run a tiny copy.
        assert!(queue.kill(id));
        let id_done = queue.spawn_copy_with_flags(&tiny, &dir.path().join("t.out"), no_cow());
        let _ = wait_for_status(&queue, id_done, JobStatus::is_finished, 5_000);
        queue.drop_finished_jobs();
        assert!(queue.get(id_done).is_none());
    }
}
