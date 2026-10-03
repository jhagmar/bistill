//! `q` on a pseudoterminal restores the previous screen.

use std::fs;
use std::process::Command;

#[cfg(unix)]
#[test]
fn q_restores_the_terminal() {
    let cwd = std::env::temp_dir().join(format!("bistill-restore-{}", std::process::id()));
    let _ = fs::remove_dir_all(&cwd);
    fs::create_dir_all(&cwd).unwrap();
    fs::write(
        cwd.join("bistill.conf"),
        format!(
            "base_url = https://127.0.0.1:1\nusername = jcitizen\nstate_dir = {}\npoll_seconds = 15\n",
            cwd.display()
        ),
    )
    .unwrap();
    let script = r#"
import os, pty, select, sys, time
binary, cwd = sys.argv[1], sys.argv[2]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(cwd)
    os.execv(binary, [binary])
data = b""
sent = False
deadline = time.time() + 8
while time.time() < deadline:
    ready, _, _ = select.select([fd], [], [], 0.2)
    if ready:
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            break
        if not chunk:
            break
        data += chunk
        if not sent and b"\x1b[?1049h" in data:
            os.write(fd, b"q")
            sent = True
    if sent and b"\x1b[?1049l" in data:
        break
deadline = time.time() + 3
status = None
while time.time() < deadline:
    got, st = os.waitpid(pid, os.WNOHANG)
    if got == pid:
        status = st
        break
    time.sleep(0.05)
if status is None:
    os.kill(pid, 15)
    os.waitpid(pid, 0)
    sys.stderr.buffer.write(data)
    sys.exit(2)
ok = sent and b"\x1b[?1049l" in data and os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
if not ok:
    sys.stderr.buffer.write(data)
    sys.exit(1)
"#;
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_bistill"))
        .arg(&cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("BISTILL_URL", "https://127.0.0.1:1")
        .env("BISTILL_USER", "jcitizen")
        .env("BISTILL_TOKEN", "test-token")
        .output()
        .expect("python3");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status {:?}\n{stderr}",
        output.status
    );
    assert!(!stderr.contains("test-token"));
    assert!(!cwd.join("poll.lock").exists());
    fs::remove_dir_all(&cwd).unwrap();
}
