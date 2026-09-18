#[cfg(target_has_atomic = "64")]
use std::sync::atomic::AtomicU64;
use std::{
    collections::HashSet,
    io::Write,
    path::Path,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU8, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

#[cfg(not(target_has_atomic = "64"))]
use portable_atomic::AtomicU64;
use terminal_size::terminal_size_of;

use crate::display::human_readable_number;

// --------------------------------------------------------------------------

pub const ORDERING: Ordering = Ordering::Relaxed;

const SPINNER_SLEEP_TIME: u64 = 100;
const PROGRESS_CHARS: [char; 4] = ['-', '\\', '|', '/'];
const PROGRESS_CHARS_LEN: usize = PROGRESS_CHARS.len();

pub trait ThreadSyncTrait<T> {
    fn set(&self, val: T);
    fn get(&self) -> T;
}

#[derive(Default)]
pub struct ThreadStringWrapper {
    inner: RwLock<String>,
}

impl ThreadSyncTrait<String> for ThreadStringWrapper {
    fn set(&self, val: String) {
        *self.inner.write().unwrap() = val;
    }

    fn get(&self) -> String {
        (*self.inner.read().unwrap()).clone()
    }
}

// --------------------------------------------------------------------------

// creating an enum this way allows to have simpler syntax compared to a Mutex or a RwLock
#[allow(non_snake_case)]
pub mod Operation {
    pub const INDEXING: u8 = 0;
    pub const PREPARING: u8 = 1;
}

#[derive(Default)]
pub struct PAtomicInfo {
    pub num_files: AtomicUsize,
    pub total_file_size: AtomicU64,
    pub state: AtomicU8,
    pub current_path: ThreadStringWrapper,
}

impl PAtomicInfo {
    pub fn clear_state(&self, dir: &Path) {
        self.state.store(Operation::INDEXING, ORDERING);
        // PERF-8: into_owned moves the Cow's buffer when owned instead of
        // cloning it a second time
        let dir_name = dir.to_string_lossy().into_owned();
        self.current_path.set(dir_name);
        self.total_file_size.store(0, ORDERING);
        self.num_files.store(0, ORDERING);
    }
}

#[derive(Default)]
pub struct RuntimeErrors {
    pub no_permissions: HashSet<String>,
    pub file_not_found: HashSet<String>,
    // BUG-17: roots that exist but are neither a directory nor a regular
    // file (FIFO/socket/device). GNU du reports those as "not a directory";
    // lumping them into file_not_found printed a misleading "No such file
    // or directory".
    pub not_a_directory: HashSet<String>,
    // BUG-17: directories whose listing kept failing with EINTR even after
    // MAX_EINTR_RETRIES retries; their subtree is missing from the totals.
    pub eintr_exhausted: HashSet<String>,
    pub unknown_error: HashSet<String>,
    // BUG-13: entries whose stat failed (e.g. directory renamed/deleted
    // between readdir and stat). The walker still walks their subtree but
    // cannot build a Node for them; without this bucket the finished
    // subtree vanished from the totals with zero indication.
    pub metadata_unavailable: HashSet<String>,
    pub interrupted_error: i32,
}

// --------------------------------------------------------------------------

fn format_preparing_str(prog_char: char, data: &PAtomicInfo, output_display: &str) -> String {
    let path_in = data.current_path.get();
    let size = human_readable_number(data.total_file_size.load(ORDERING), output_display);
    format!("Preparing: {path_in} {size} ... {prog_char}")
}

fn format_indexing_str(prog_char: char, data: &PAtomicInfo, output_display: &str) -> String {
    let path_in = data.current_path.get();
    let file_count = data.num_files.load(ORDERING);
    let size = human_readable_number(data.total_file_size.load(ORDERING), output_display);
    let file_str = format!("{file_count} files, {size}");
    format!("Indexing: {path_in} {file_str} ... {prog_char}")
}

pub struct PIndicator {
    pub thread: Option<(Sender<()>, JoinHandle<()>)>,
    pub data: Arc<PAtomicInfo>,
}

impl PIndicator {
    pub fn build_me() -> Self {
        Self {
            thread: None,
            data: Arc::new(PAtomicInfo {
                ..Default::default()
            }),
        }
    }

    pub fn spawn(&mut self, output_display: String) {
        // BUG-12: the spinner is interactive UI. When stderr is not a tty
        // (pipe, file, CI capture) don't write \r-rendered lines into it at
        // all; stop() is a no-op with no thread. terminal_size_of doubles as
        // the isatty check (returns None for non-console handles), same
        // approach as should_init_color uses for stdout.
        if terminal_size_of(std::io::stderr()).is_none() {
            return;
        }
        let data = self.data.clone();
        let (stop_handler, receiver) = mpsc::channel::<()>();

        let time_info_thread = std::thread::spawn(move || {
            let mut progress_char_i: usize = 0;
            let mut stderr = std::io::stderr();
            let mut msg = String::new();

            // While the timeout triggers we go round the loop
            // If we disconnect or the sender sends its message we exit the while loop
            while let Err(RecvTimeoutError::Timeout) =
                receiver.recv_timeout(Duration::from_millis(SPINNER_SLEEP_TIME))
            {
                // Clear the text written by 'write!'& Return at the start of line
                let clear = format!("\r{:width$}", " ", width = msg.len());
                // BUG-12: a failed stderr write (EPIPE, ENOSPC, invalid
                // handle) must kill the spinner quietly; a panicking thread
                // here would cascade into stop() and take the result output
                // with it.
                if write!(stderr, "{clear}")
                    .and_then(|()| stderr.flush())
                    .is_err()
                {
                    break;
                }
                let prog_char = PROGRESS_CHARS[progress_char_i];

                msg = match data.state.load(ORDERING) {
                    Operation::INDEXING => format_indexing_str(prog_char, &data, &output_display),
                    Operation::PREPARING => format_preparing_str(prog_char, &data, &output_display),
                    _ => panic!("Unknown State"),
                };

                if write!(stderr, "\r{msg}")
                    .and_then(|()| stderr.flush())
                    .is_err()
                {
                    break;
                }

                progress_char_i += 1;
                progress_char_i %= PROGRESS_CHARS_LEN;
            }

            // Best-effort cleanup: stderr may already be unusable
            let clear = format!("\r{:width$}", " ", width = msg.len());
            let _ = write!(stderr, "{clear}");
            let _ = write!(stderr, "\r");
            let _ = stderr.flush();
        });
        self.thread = Some((stop_handler, time_info_thread));
    }

    pub fn stop(self) {
        if let Some((stop_handler, thread)) = self.thread {
            // The spinner thread may already be gone (stderr write failure
            // dropped the receiver); losing the results to a panic here
            // would be far worse than a stray spinner line.
            let _ = stop_handler.send(());
            let _ = thread.join();
        }
    }
}
