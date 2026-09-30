
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

const MAX_JOBS: usize = 8;
const BUFFER_CAP: usize = 256 * 1024;
const UNREAD_CAP: usize = 8 * 1024 * 1024;

struct Stream {
    text: String,
    read_at: usize,
    eof: bool,
}

struct Job {
    child: Child,
    stdout: Arc<Mutex<Stream>>,
    stderr: Arc<Mutex<Stream>>,
    code: Option<i32>,
    reaped_at: Option<Instant>,
}

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

    let mut cut = s.text.len().saturating_sub(BUFFER_CAP).min(s.read_at);
    while cut > 0 && !s.text.is_char_boundary(cut) {
        cut -= 1;
    }
    if cut > 0 {
        s.text.drain(..cut);
        s.read_at -= cut;
    }

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

pub fn poll(id: u32) -> Result<(String, String, bool, Option<i32>)> {
    with_jobs(|jobs| {
        let job = jobs
            .get_mut(&id)
            .with_context(|| format!("命令任务 {id} 不存在（可能已结束并被清理）"))?;

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
