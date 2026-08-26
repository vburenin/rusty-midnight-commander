use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct BackgroundJob {
    pub id: JobId,
    pub kind: JobKind,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub status: JobStatus,
    pub error: Option<String>,
}

#[derive(Debug)]
struct JobEntry {
    job: BackgroundJob,
    cancel_flag: Arc<AtomicBool>,
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

    /// Spawn a copy job. Returns the JobId.
    pub fn spawn_copy<P: Into<PathBuf>, Q: Into<PathBuf>>(&self, src: P, dst: Q) -> JobId {
        self.enqueue(JobKind::Copy, src.into(), dst.into())
    }

    /// Spawn a move job. Returns the JobId.
    pub fn spawn_move<P: Into<PathBuf>, Q: Into<PathBuf>>(&self, src: P, dst: Q) -> JobId {
        self.enqueue(JobKind::Move, src.into(), dst.into())
    }

    fn enqueue(&self, kind: JobKind, src: PathBuf, dst: PathBuf) -> JobId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Mutex::new(JobEntry {
            job: BackgroundJob {
                id,
                kind,
                src,
                dst,
                bytes_done: 0,
                bytes_total: 0,
                status: JobStatus::Queued,
                error: None,
            },
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }));
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().expect("JobQueue mutex poisoned");
        inner.jobs.push(entry);
        cvar.notify_all();
        id
    }

    /// Cancel a job by id. Best-effort: running copies will stop mid-copy.
    /// - If the job is queued, it transitions to Cancelled immediately.
    /// - If the job is running, it will stop at the next chunk boundary.
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
                if job.job.status == JobStatus::Queued {
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

    /// Drop all finished (Done/Failed/Cancelled) jobs from the queue.
    pub fn drop_finished_jobs(&self) {
        let (lock, _cvar) = &*self.inner;
        let mut inner = lock.lock().expect("JobQueue mutex poisoned");
        inner.jobs.retain(|job_arc| {
            let job = job_arc.lock().expect("JobEntry mutex poisoned");
            !matches!(
                job.job.status,
                JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
            )
        });
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
    let (kind, src, dst, cancel_flag, id) = {
        let job = job_arc.lock().expect("JobEntry mutex poisoned");
        (
            job.job.kind,
            job.job.src.clone(),
            job.job.dst.clone(),
            Arc::clone(&job.cancel_flag),
            job.job.id,
        )
    };

    let result = match kind {
        JobKind::Copy => copy_streaming(job_arc, &src, &dst, &cancel_flag),
        JobKind::Move => move_with_fallback(job_arc, &src, &dst, &cancel_flag),
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

fn copy_streaming(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
) -> io::Result<()> {
    // Prepare target directory when present.
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let meta = fs::metadata(src)?;
    let total = meta.len();
    {
        let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
        job.job.bytes_total = total;
        job.job.bytes_done = 0;
    }

    let mut src_f = File::open(src)?;
    let mut dst_f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(dst)?;

    let mut buf = vec![0_u8; DEFAULT_CHUNK_SIZE];
    let mut done: u64 = 0;
    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            // Best-effort cancel: leave partial file as-is.
            return Ok(());
        }
        let read_n = src_f.read(&mut buf)?;
        if read_n == 0 {
            break;
        }
        dst_f.write_all(&buf[..read_n])?;
        done = done.saturating_add(read_n as u64);
        {
            let mut job = job_arc.lock().expect("JobEntry mutex poisoned");
            job.job.bytes_done = done;
        }
        // Give other threads a chance (useful for tests to trigger cancel).
        if cfg!(test) {
            thread::yield_now();
            thread::sleep(Duration::from_millis(1));
        }
    }
    // Ensure data is flushed to disk.
    dst_f.flush()?;
    Ok(())
}

fn move_with_fallback(
    job_arc: &Arc<Mutex<JobEntry>>,
    src: &Path,
    dst: &Path,
    cancel_flag: &AtomicBool,
) -> io::Result<()> {
    // Try fast path: rename.
    let src_meta = fs::metadata(src);
    match fs::rename(src, dst) {
        Ok(()) => {
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
    // Fallback: copy then remove source if not cancelled.
    copy_streaming(job_arc, src, dst, cancel_flag)?;
    if !cancel_flag.load(Ordering::Relaxed) {
        // Remove original only if not cancelled.
        fs::remove_file(src)?;
    }
    Ok(())
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
}
