//! Command jobs behind the module WebUI bridge's `ksu.spawn`.
//!
//! The APK hands a module a live ChildProcess: stdout and stderr arrive as they are
//! produced, and an exit event carries the status. A module that streams a long-running
//! command therefore has to see partial output — running the command to completion and
//! returning it in one shot (which is all `exec` can do) leaves the page waiting with no
//! way to observe progress.
//!
//! So the command runs detached with both streams piped into bounded buffers, and the
//! client polls for whatever is new since its last read. The idioms are deliberately the
//! same as `webui_shell.rs`: an id table, reader threads, and a cap that keeps a chatty
//! process from growing the buffer without bound.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Jobs are used one at a time; this only stops them piling up if a client leaks one.
const MAX_JOBS: usize = 8;
/// How much output a stream keeps before the part the client has already been given is
/// dropped, so a chatty process cannot grow memory without bound.
const BUFFER_CAP: usize = 256 * 1024;
/// How much *undelivered* output a stream keeps before the oldest of it is dropped.
///
/// Nothing unread goes at [`BUFFER_CAP`]: a module's `exec` answer is read only after the
/// command has finished writing it, so trimming there would hand back an answer whose front
/// is missing. This is the ceiling for a client that stops reading altogether — high enough
/// that no page a module asks for can reach it, and low enough to bound a leaked job.
const UNREAD_CAP: usize = 8 * 1024 * 1024;

struct Stream {
    text: String,
    /// How much of `text` the client has already been given.
    read_at: usize,
    /// Set by the reader thread when the pipe closes.
    eof: bool,
}

struct Job {
    child: Child,
    stdout: Arc<Mutex<Stream>>,
    stderr: Arc<Mutex<Stream>>,
    /// Cached once the child is reaped: `try_wait` reports the status exactly once.
    code: Option<i32>,
    /// When the reaped child was noticed, so output still in flight can be collected.
    reaped_at: Option<Instant>,
}

/// How long to keep waiting for the last of the output after the child is reaped.
///
/// A command that exits while its output is still in the pipe would otherwise have its tail
/// cut off, which for something like `head -n 1 bigfile` is the whole answer. The grace
/// period covers the other case too: a command that leaves a background process holding the
/// pipe open would never reach EOF, and waiting for it forever would hang the page.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

static JOBS: Mutex<Option<HashMap<u32, Job>>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn with_jobs<T>(f: impl FnOnce(&mut HashMap<u32, Job>) -> T) -> T {
    let mut guard = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let jobs = guard.get_or_insert_with(HashMap::new);
    f(jobs)
}

fn push(stream: &Arc<Mutex<Stream>>, text: &str) {
    let mut s = stream.lock().unwrap_or_else(|e| e.into_inner());
    s.text.push_str(text);

    // Only bytes the client has already been handed are reclaimable. The unread part is the
    // answer a module is waiting for, and its *front* is the part that cannot be spared: a
    // module WebUI of the "encrypted loader" kind asks `ksu.exec` for its whole page and
    // `document.write`s the answer, so a page arriving without its `<html><head><script>` is
    // rendered as plain text instead of as a UI.
    let mut cut = s.text.len().saturating_sub(BUFFER_CAP).min(s.read_at);
    while cut > 0 && !s.text.is_char_boundary(cut) {
        cut -= 1;
    }
    if cut > 0 {
        s.text.drain(..cut);
        s.read_at -= cut;
    }

    // A client that stops reading still may not grow this without bound, and what it loses is
    // said in the stream rather than going missing silently. The middle goes, not the tail:
    // a module polling a streaming command is watching the end of the output.
    let mut excess = s.text.len().saturating_sub(s.read_at + UNREAD_CAP);
    while excess > 0 && !s.text.is_char_boundary(s.read_at + excess) {
        excess -= 1;
    }
    if excess > 0 {
        let notice = format!(
            "\n[ksud] 该命令的输出超过 {} MB，已丢弃中间 {excess} 字节\n",
            UNREAD_CAP / (1024 * 1024)
        );
        let at = s.read_at;
        s.text.drain(at..at + excess);
        s.text.insert_str(at, &notice);
    }
}

fn spawn_reader(mut source: impl Read + Send + 'static, stream: Arc<Mutex<Stream>>) {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = source.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&chunk[..n]).into_owned();
            push(&stream, &text);
        }
        stream.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
    });
}

/// Quote one argument so a value containing spaces or quotes still arrives as one word.
///
/// The APK joins argv with single spaces and lets the shell re-split it; anything a module
/// passes with a space in it silently becomes two arguments there. Single-quoting with the
/// usual `'\''` escape keeps the caller's intent instead.
fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Start `cmd` (with optional `args`) detached, running in `cwd`.
///
/// Returns the job id to poll with [`poll`].
pub fn start(cmd: &str, args: &[String], cwd: &str, env: &[(String, String)]) -> Result<u32> {
    let dir = if Path::new(cwd).is_dir() { cwd } else { "/" };
    let mut line = cmd.to_string();
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }

    let mut command = Command::new(crate::defs::SHELL_PATH);
    command
        .arg("-c")
        .arg(&line)
        .current_dir(dir)
        // Closed rather than inherited: a command that reads stdin would otherwise consume
        // whatever the server's own stdin is, and could never see the module's input anyway.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("无法启动命令（工作目录 {dir}）：{line}"))?;

    let stdout = Arc::new(Mutex::new(Stream {
        text: String::new(),
        read_at: 0,
        eof: false,
    }));
    let stderr = Arc::new(Mutex::new(Stream {
        text: String::new(),
        read_at: 0,
        eof: false,
    }));

    if let Some(out) = child.stdout.take() {
        spawn_reader(out, Arc::clone(&stdout));
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, Arc::clone(&stderr));
    }

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    with_jobs(|jobs| {
        if jobs.len() >= MAX_JOBS {
            if let Some(oldest) = jobs.keys().copied().min() {
                kill_in(jobs, oldest);
            }
        }
        jobs.insert(
            id,
            Job {
                child,
                stdout,
                stderr,
                code: None,
                reaped_at: None,
            },
        );
    });
    Ok(id)
}

fn take_new(stream: &Arc<Mutex<Stream>>) -> String {
    let mut s = stream.lock().unwrap_or_else(|e| e.into_inner());
    let from = s.read_at.min(s.text.len());
    let fresh = s.text[from..].to_string();
    s.read_at = s.text.len();
    fresh
}

/// Output produced since the last poll, whether the job is still running, and its exit code.
///
/// The exit code is `None` while the command runs; a job killed by a signal reports
/// `Some(-1)` once it is reaped.
///
/// A reaped child is not by itself the end: its last output may still be sitting in the
/// pipe, so the job is reported as running until both readers have hit EOF (or the grace
/// period lapses, for a command that left the pipe open in a background process).
pub fn poll(id: u32) -> Result<(String, String, bool, Option<i32>)> {
    with_jobs(|jobs| {
        let job = jobs
            .get_mut(&id)
            .with_context(|| format!("命令任务 {id} 不存在（可能已结束并被清理）"))?;

        // try_wait reaps the child as a side effect, so the exit code is cached on the job
        // rather than lost the second time this is called.
        if job.code.is_none() {
            match job.child.try_wait() {
                Ok(Some(status)) => {
                    job.code = Some(status.code().unwrap_or(-1));
                    job.reaped_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("spawn: 等待任务 {id} 失败：{e}");
                    job.code = Some(-1);
                    job.reaped_at = Some(Instant::now());
                }
            }
        }

        let stdout = take_new(&job.stdout);
        let stderr = take_new(&job.stderr);

        let drained = job.stdout.lock().unwrap_or_else(|e| e.into_inner()).eof
            && job.stderr.lock().unwrap_or_else(|e| e.into_inner()).eof;
        let grace_over = job.reaped_at.is_some_and(|at| at.elapsed() >= DRAIN_GRACE);
        let alive = job.code.is_none() || !(drained || grace_over);

        Ok((stdout, stderr, alive, job.code))
    })
}

fn kill_in(jobs: &mut HashMap<u32, Job>, id: u32) {
    if let Some(mut job) = jobs.remove(&id) {
        let _ = job.child.kill();
        let _ = job.child.wait();
    }
}

pub fn close(id: u32) -> Result<()> {
    with_jobs(|jobs| {
        if !jobs.contains_key(&id) {
            bail!("命令任务 {id} 不存在");
        }
        kill_in(jobs, id);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn shell_available() -> bool {
        if Command::new(crate::defs::SHELL_PATH)
            .arg("-c")
            .arg("true")
            .status()
            .is_ok()
        {
            true
        } else {
            eprintln!("skipping: no `sh` on this host");
            false
        }
    }

    /// Polls until the job finishes, accumulating everything it printed.
    fn drain(id: u32) -> (String, String, Option<i32>) {
        let (mut out, mut err, mut code) = (String::new(), String::new(), None);
        for _ in 0..200 {
            let (o, e, alive, c) = poll(id).expect("poll");
            out.push_str(&o);
            err.push_str(&e);
            if !alive {
                code = c;
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        (out, err, code)
    }

    #[test]
    fn collects_output_and_the_exit_code() {
        if !shell_available() {
            return;
        }
        let id = start("echo hello-from-spawn", &[], "/", &[]).expect("start");
        let (out, _err, code) = drain(id);
        assert!(out.contains("hello-from-spawn"), "got: {out:?}");
        assert_eq!(code, Some(0));
        close(id).expect("close");
    }

    /// The point of a job rather than an exec: output must be readable while it is still
    /// running, which is what keeps a module's streaming view alive.
    #[test]
    fn partial_output_is_readable_before_the_command_ends() {
        if !shell_available() {
            return;
        }
        let id = start("echo first; sleep 2; echo second", &[], "/", &[]).expect("start");
        let mut seen = String::new();
        for _ in 0..60 {
            let (chunk, _e, alive, _c) = poll(id).expect("poll");
            seen.push_str(&chunk);
            if seen.contains("first") {
                break;
            }
            assert!(alive, "job ended before printing: {seen:?}");
            thread::sleep(Duration::from_millis(25));
        }
        assert!(seen.contains("first"), "got: {seen:?}");
        assert!(
            !seen.contains("second"),
            "read output before it existed: {seen:?}"
        );
        close(id).expect("close");
    }

    #[test]
    fn stderr_is_kept_separate() {
        if !shell_available() {
            return;
        }
        let id = start("echo to-stderr >&2", &[], "/", &[]).expect("start");
        let (out, err, _code) = drain(id);
        assert!(err.contains("to-stderr"), "stderr: {err:?}");
        assert!(
            !out.contains("to-stderr"),
            "stderr leaked into stdout: {out:?}"
        );
        close(id).expect("close");
    }

    /// A command that exits immediately still has output in the pipe; the tail must survive
    /// the child being reaped, or a module reading the last lines gets a truncated answer.
    #[test]
    fn output_is_complete_when_the_command_exits_at_once() {
        if !shell_available() {
            return;
        }
        let id = start("seq 1 20000", &[], "/", &[]).expect("start");
        let (out, _err, code) = drain(id);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(code, Some(0));
        assert_eq!(
            lines.first(),
            Some(&"1"),
            "lost the start: {:?}",
            lines.first()
        );
        assert_eq!(
            lines.last(),
            Some(&"20000"),
            "lost the tail: {:?}",
            lines.last()
        );
        close(id).expect("close");
    }

    /// An answer bigger than the buffer still has to arrive whole.
    ///
    /// A module WebUI of the "encrypted loader" kind asks `ksu.exec` for its entire page and
    /// hands the answer to `document.write`, so the *head* is the part that cannot be spared:
    /// arriving without its `<html><head><script>` leaves the browser rendering the rest of
    /// that page's script as plain text on an empty screen.
    ///
    /// The sleep is the load-bearing part: it is the shape of that call — the command writes
    /// its whole answer while the reader has not asked for anything yet, so a buffer that
    /// keeps only its last 256 KB has already thrown the front away by the first read.
    #[test]
    fn a_large_answer_keeps_its_head() {
        if !shell_available() {
            return;
        }
        let id = start("printf HEAD; seq 1 60000; printf TAIL", &[], "/", &[]).expect("start");
        thread::sleep(Duration::from_millis(300));
        let (out, _err, code) = drain(id);
        assert_eq!(code, Some(0));
        assert!(
            out.starts_with("HEAD"),
            "lost the start of a {} byte answer: {:?}",
            out.len(),
            out.chars().take(40).collect::<String>()
        );
        assert!(
            out.ends_with("TAIL"),
            "lost the end of a {} byte answer",
            out.len()
        );
        close(id).expect("close");
    }

    #[test]
    fn arguments_are_passed_as_written() {
        if !shell_available() {
            return;
        }
        let args = vec!["two words".to_string()];
        let id = start("printf '[%s]'", &args, "/", &[]).expect("start");
        let (out, _err, _code) = drain(id);
        assert!(out.contains("[two words]"), "got: {out:?}");
        close(id).expect("close");
    }

    #[test]
    fn environment_and_working_directory_are_honoured() {
        if !shell_available() {
            return;
        }
        let env = vec![("KSU_SPAWN_TEST".to_string(), "yes".to_string())];
        let id = start("echo $KSU_SPAWN_TEST", &[], "/", &env).expect("start");
        let (out, _err, _code) = drain(id);
        assert!(out.contains("yes"), "got: {out:?}");
        close(id).expect("close");
    }

    #[test]
    fn polling_does_not_repeat_output() {
        if !shell_available() {
            return;
        }
        let id = start("echo once-only", &[], "/", &[]).expect("start");
        let (first, _e, _c) = drain(id);
        assert!(first.contains("once-only"));
        let (second, _, _, _) = poll(id).expect("poll again");
        assert!(!second.contains("once-only"), "output repeated: {second:?}");
        close(id).expect("close");
    }

    #[test]
    fn closing_removes_the_job() {
        if !shell_available() {
            return;
        }
        let id = start("sleep 5", &[], "/", &[]).expect("start");
        close(id).expect("close");
        assert!(poll(id).is_err(), "job should be gone after close");
    }
}
