use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use wait_timeout::ChildExt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RtkDecision {
    Preserve,
    Ask,
    Deny,
    Rewrite { command: String, ask: bool },
}

#[must_use]
pub fn invoke(path: &Path, command: &str, timeout: Duration) -> RtkDecision {
    let started = Instant::now();
    let Ok(mut child) = Command::new(path)
        .arg("rewrite")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return RtkDecision::Preserve;
    };
    let stdout = child.stdout.take();
    let child_group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let _ = sender.send(stdout.map_or(Ok(Vec::new()), read_limited));
    });
    let Ok(Some(status)) = child.wait_timeout(timeout) else {
        kill_group_and_reap(&mut child, child_group);
        finish_reader(&receiver, reader, Duration::from_millis(100));
        return RtkDecision::Preserve;
    };
    let remaining = timeout.saturating_sub(started.elapsed());
    let Ok(result) = receiver.recv_timeout(remaining) else {
        // A descendant may have inherited stdout after the RTK parent exited. Killing the
        // process group closes the pipe in the normal case; never join an unbounded reader.
        let _ = signal::killpg(child_group, Signal::SIGKILL);
        finish_reader(&receiver, reader, Duration::from_millis(100));
        return RtkDecision::Preserve;
    };
    let _ = reader.join();
    let Ok(output) = result else {
        return RtkDecision::Preserve;
    };
    let code = status.code().unwrap_or(-1);
    if code == 2 {
        return RtkDecision::Deny;
    }
    if code == 1 {
        return RtkDecision::Preserve;
    }
    if !matches!(code, 0 | 3) || output.is_empty() || output.len() > 64 * 1024 {
        return RtkDecision::Preserve;
    }
    let Ok(value) = String::from_utf8(output) else {
        return RtkDecision::Preserve;
    };
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains('\0') {
        return RtkDecision::Preserve;
    }
    RtkDecision::Rewrite {
        command: value.to_owned(),
        ask: code == 3,
    }
}

fn kill_group_and_reap(child: &mut std::process::Child, child_group: Pid) {
    let _ = signal::killpg(child_group, Signal::SIGKILL);
    let _ = child.wait();
}

fn finish_reader(
    receiver: &mpsc::Receiver<io::Result<Vec<u8>>>,
    reader: thread::JoinHandle<()>,
    timeout: Duration,
) {
    if receiver.recv_timeout(timeout).is_ok() {
        let _ = reader.join();
    }
}

fn read_limited(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if retained.len() <= 64 * 1024 {
            let remaining = 64 * 1024 + 1 - retained.len();
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    Ok(retained)
}
