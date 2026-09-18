//! Shell sessions behind the web UI's terminal.
//!
//! The preferred backend is a real pty, opened with `openpty` and handed to `sh -i`. That is
//! what makes a script's output appear immediately and lets its own `read` receive what the
//! user types. With a plain pipe the child's stdout is block-buffered, scripts that test
//! `[ -t 1 ]` take a non-interactive branch, and the shell competes with the script for
//! stdin — all of which make a working script look silent. A pipe remains the fallback where
//! `openpty` is unavailable or fails.
//!
//! Even with a pty this is not a terminal emulator: the frontend renders text and basic
//! colours, so full-screen programs are out of scope.

#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, bail};

/// Sessions are used one at a time; this only stops them piling up if a client leaks one.
const MAX_SESSIONS: usize = 4;
/// Keep only the tail of the output so a chatty script cannot grow the buffer without bound.
const BUFFER_CAP: usize = 256 * 1024;

struct Output {
    text: String,
    /// How much of `text` the client has already been given.
    read_at: usize,
}

/// A pty master, which serves as both the session's input and its output.
#[cfg(unix)]
struct Pty {
    pid: libc::pid_t,
    master: std::fs::File,
}

enum Backend {
    Pipe {
        child: Child,
        stdin: ChildStdin,
    },
    #[cfg(unix)]
    Pty(Box<Pty>),
}

struct Session {
    backend: Backend,
    output: Arc<Mutex<Output>>,
}

static SESSIONS: Mutex<Option<HashMap<u32, Session>>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn with_sessions<T>(f: impl FnOnce(&mut HashMap<u32, Session>) -> T) -> T {
    let mut guard = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let sessions = guard.get_or_insert_with(HashMap::new);
    f(sessions)
}

fn push_output(output: &Arc<Mutex<Output>>, text: &str) {
    let mut out = output.lock().unwrap_or_else(|e| e.into_inner());
    out.text.push_str(text);

    if out.text.len() > BUFFER_CAP {
        // Trim from the front on a char boundary, keeping the client's read position valid.
        let mut cut = out.text.len() - BUFFER_CAP;
        while cut < out.text.len() && !out.text.is_char_boundary(cut) {
            cut += 1;
        }
        out.text.drain(..cut);
        out.read_at = out.read_at.saturating_sub(cut);
    }
}

fn spawn_reader(mut source: impl Read + Send + 'static, output: Arc<Mutex<Output>>) {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = source.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&chunk[..n]).into_owned();
            push_output(&output, &text);
        }
    });
}

/// Fork a shell onto a fresh pty. Returns the child pid and the master end.
#[cfg(unix)]
fn spawn_pty(dir: &str) -> Result<Box<Pty>> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;

    // Built before forking: the child may only call async-signal-safe functions, and these
    // allocate.
    let program = CString::new(crate::defs::SHELL_PATH)?;
    let interactive = CString::new("-i")?;
    let workdir = CString::new(dir)?;

    unsafe {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        if libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        ) != 0
        {
            return Err(std::io::Error::last_os_error()).with_context(|| "openpty 失败");
        }

        let pid = libc::fork();
        if pid < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(master);
            libc::close(slave);
            return Err(err).with_context(|| "fork 失败");
        }

        if pid == 0 {
            // Child: new session with the pty as its controlling terminal, then exec.
            libc::close(master);
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
            libc::chdir(workdir.as_ptr());
            let argv = [program.as_ptr(), interactive.as_ptr(), std::ptr::null()];
            libc::execvp(program.as_ptr(), argv.as_ptr());
            libc::_exit(127); // exec failed
        }

        libc::close(slave);
        Ok(Box::new(Pty {
            pid,
            master: std::fs::File::from_raw_fd(master),
        }))
    }
}

fn spawn_pipe(dir: &str, output: &Arc<Mutex<Output>>) -> Result<Backend> {
    let mut child = Command::new(crate::defs::SHELL_PATH)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("无法启动 {}（工作目录 {dir}）", crate::defs::SHELL_PATH))?;

    let stdin = child.stdin.take().context("无法获取 shell 的 stdin")?;
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, Arc::clone(output));
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, Arc::clone(output));
    }
    Ok(Backend::Pipe { child, stdin })
}

/// Start a session, optionally in `cwd`. Returns its id and whether it got a pty.
pub fn open(cwd: &str) -> Result<(u32, bool)> {
    let dir = if Path::new(cwd).is_dir() { cwd } else { "/" };
    let output = Arc::new(Mutex::new(Output {
        text: String::new(),
        read_at: 0,
    }));

    #[cfg(unix)]
    let backend = match spawn_pty(dir) {
        Ok(pty_handle) => {
            if let Ok(reader) = pty_handle.master.try_clone() {
                spawn_reader(reader, Arc::clone(&output));
            }
            Backend::Pty(pty_handle)
        }
        Err(e) => {
            log::warn!("terminal: 无法创建 pty（{e:#}），改用管道");
            spawn_pipe(dir, &output)?
        }
    };

    #[cfg(not(unix))]
    let backend = spawn_pipe(dir, &output)?;

    // Read back off the backend that was chosen rather than tracked alongside it: the flag and
    // the thing it describes cannot then disagree.
    #[cfg(unix)]
    let pty = matches!(backend, Backend::Pty(_));
    #[cfg(not(unix))]
    let pty = false;

    if !pty {
        // Said out loud rather than only logged: without a pty a script's output and input
        // behave differently, and that is invisible otherwise.
        push_output(
            &output,
            "[提示] 拿不到 PTY，终端运行在管道模式：\n\
             [提示] 脚本可能不显示输出、也可能把你的输入当成命令。\n\n",
        );
    }

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    with_sessions(|sessions| {
        if sessions.len() >= MAX_SESSIONS {
            if let Some(oldest) = sessions.keys().copied().min() {
                close_in(sessions, oldest);
            }
        }
        sessions.insert(id, Session { backend, output });
    });
    Ok((id, pty))
}

fn alive(backend: &mut Backend) -> bool {
    match backend {
        Backend::Pipe { child, .. } => matches!(child.try_wait(), Ok(None)),
        #[cfg(unix)]
        Backend::Pty(pty) => unsafe {
            let mut status = 0;
            libc::waitpid(pty.pid, &raw mut status, libc::WNOHANG) == 0
        },
    }
}

/// Send input to the session.
pub fn write(id: u32, data: &str) -> Result<()> {
    with_sessions(|sessions| {
        let session = sessions
            .get_mut(&id)
            .with_context(|| format!("终端会话 {id} 不存在（可能已关闭）"))?;
        match &mut session.backend {
            Backend::Pipe { stdin, .. } => {
                stdin
                    .write_all(data.as_bytes())
                    .context("写入 shell 失败")?;
                stdin.flush().context("刷新 shell 输入失败")
            }
            #[cfg(unix)]
            Backend::Pty(pty) => {
                pty.master
                    .write_all(data.as_bytes())
                    .context("写入 pty 失败")?;
                pty.master.flush().context("刷新 pty 失败")
            }
        }
    })
}

/// Output produced since the last read, plus whether the shell is still running.
pub fn read(id: u32) -> Result<(String, bool)> {
    with_sessions(|sessions| {
        let session = sessions
            .get_mut(&id)
            .with_context(|| format!("终端会话 {id} 不存在（可能已关闭）"))?;
        let running = alive(&mut session.backend);

        let mut out = session.output.lock().unwrap_or_else(|e| e.into_inner());
        let from = out.read_at.min(out.text.len());
        let fresh = out.text[from..].to_string();
        out.read_at = out.text.len();
        Ok((fresh, running))
    })
}

fn close_in(sessions: &mut HashMap<u32, Session>, id: u32) {
    if let Some(session) = sessions.remove(&id) {
        match session.backend {
            Backend::Pipe { mut child, .. } => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(unix)]
            Backend::Pty(pty) => unsafe {
                libc::kill(pty.pid, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(pty.pid, &raw mut status, 0);
            },
        }
    }
}

pub fn close(id: u32) -> Result<()> {
    with_sessions(|sessions| -> Result<()> {
        if !sessions.contains_key(&id) {
            bail!("终端会话 {id} 不存在");
        }
        close_in(sessions, id);
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

    fn read_until(id: u32, needle: &str) -> String {
        let mut seen = String::new();
        for _ in 0..100 {
            if let Ok((chunk, _)) = read(id) {
                seen.push_str(&chunk);
            }
            if seen.contains(needle) {
                return seen;
            }
            thread::sleep(Duration::from_millis(50));
        }
        seen
    }

    #[test]
    fn runs_a_command_and_returns_its_output() {
        if !shell_available() {
            return;
        }
        let (id, _) = open("/").expect("open session");
        write(id, "echo ksu-shell-ok\n").expect("write");
        let seen = read_until(id, "ksu-shell-ok");
        assert!(seen.contains("ksu-shell-ok"), "got: {seen:?}");
        close(id).expect("close");
    }

    /// The whole point of a persistent session: a script's `read` must receive typed input.
    #[test]
    fn typed_input_reaches_a_script_read() {
        if !shell_available() {
            return;
        }
        let (id, _) = open("/").expect("open session");
        write(id, "read answer; echo \"you said $answer\"\n").expect("write");
        thread::sleep(Duration::from_millis(300));
        write(id, "42\n").expect("write input");
        let seen = read_until(id, "you said 42");
        assert!(seen.contains("you said 42"), "got: {seen:?}");
        close(id).expect("close");
    }

    #[test]
    fn reading_twice_does_not_repeat_output() {
        if !shell_available() {
            return;
        }
        let (id, _) = open("/").expect("open session");
        write(id, "echo once-only\n").expect("write");
        let first = read_until(id, "once-only");
        assert!(first.contains("once-only"));
        let (second, _) = read(id).expect("read");
        assert!(!second.contains("once-only"), "output repeated: {second:?}");
        close(id).expect("close");
    }

    #[test]
    fn closing_removes_the_session() {
        if !shell_available() {
            return;
        }
        let (id, _) = open("/").expect("open session");
        close(id).expect("close");
        assert!(read(id).is_err(), "session should be gone after close");
    }

    /// On a pty the reported flag must be true, and a bare newline must reach a waiting
    /// `read` — the two things the pipe backend could not do.
    #[cfg(unix)]
    #[test]
    fn pty_reports_itself_and_accepts_a_bare_newline() {
        if !shell_available() {
            return;
        }
        let (id, pty) = open("/").expect("open session");
        assert!(pty, "expected a pty on unix");

        write(id, "echo in-tty=$( [ -t 1 ] && echo YES || echo NO )\n").expect("write");
        let seen = read_until(id, "in-tty=");
        assert!(
            seen.contains("in-tty=YES"),
            "child did not see a tty: {seen:?}"
        );

        // A prompt that just needs Enter to accept its default.
        write(id, "read x; echo \"got=[$x]\"\n").expect("write");
        thread::sleep(Duration::from_millis(300));
        write(id, "\n").expect("bare newline");
        let seen = read_until(id, "got=[]");
        assert!(
            seen.contains("got=[]"),
            "bare newline did not satisfy read: {seen:?}"
        );
        close(id).expect("close");
    }
}
