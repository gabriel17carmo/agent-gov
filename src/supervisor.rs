//! Child-process supervision.

use std::{
    fs,
    io::{self, Read},
    os::unix::{fs::PermissionsExt, net::UnixListener, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{self, SigSet, SigmaskHow, Signal},
    unistd::{Pid, isatty, tcgetpgrp, tcsetpgrp},
};
use signal_hook::{
    consts::signal::{SIGINT, SIGQUIT, SIGTERM, SIGWINCH},
    iterator::Signals,
    low_level::emulate_default_handler,
};

use crate::{error::Result, scheduler::Permit};

pub struct SuperviseOptions<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub label: &'a str,
    pub max_run: Duration,
    pub termination_grace: Duration,
}

pub struct SupervisedExit {
    status: ExitStatus,
    forwarded_signal: Option<Signal>,
}

impl SupervisedExit {
    #[must_use]
    pub const fn direct(status: ExitStatus) -> Self {
        Self {
            status,
            forwarded_signal: None,
        }
    }
}

pub fn supervise(permit: &Permit, options: &SuperviseOptions<'_>) -> Result<SupervisedExit> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGWINCH])?;
    let control = ControlEndpoint::bind(permit.control_path())?;
    let mut command = Command::new(options.program);
    command
        .args(options.args)
        .env("AGENT_GOV_ACTIVE", "1")
        .env("AGENT_GOV_JOB_ID", permit.job_id())
        .process_group(0);
    let mut child = command.spawn()?;
    let child_group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    let mut cleanup = ChildCleanup {
        child: &mut child,
        child_group,
        armed: true,
    };
    let _terminal = TerminalForeground::transfer(child_group);
    permit.mark_running(cleanup.child.id(), options.label)?;
    let started = Instant::now();
    let mut terminating: Option<(Instant, Signal)> = None;

    loop {
        if let Some(status) = cleanup.child.try_wait()? {
            cleanup.armed = false;
            return Ok(SupervisedExit {
                status,
                forwarded_signal: terminating.map(|(_, signal)| signal),
            });
        }

        for raw in signals.pending() {
            let Ok(signal) = Signal::try_from(raw) else {
                continue;
            };
            if signal == Signal::SIGWINCH {
                let _ = signal::killpg(child_group, signal);
            } else if terminating.is_none() {
                let _ = signal::killpg(child_group, signal);
                terminating = Some((Instant::now(), signal));
            }
        }

        if terminating.is_none() && control.cancellation_requested() {
            let _ = signal::killpg(child_group, Signal::SIGTERM);
            terminating = Some((Instant::now(), Signal::SIGTERM));
        }

        if terminating.is_none() && started.elapsed() >= options.max_run {
            eprintln!("agent-gov: execution timeout; terminating workload group");
            let _ = signal::killpg(child_group, Signal::SIGTERM);
            terminating = Some((Instant::now(), Signal::SIGTERM));
        }
        if let Some((since, _)) = terminating
            && since.elapsed() >= options.termination_grace
        {
            let _ = signal::killpg(child_group, Signal::SIGKILL);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

struct ControlEndpoint {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlEndpoint {
    fn bind(path: &Path) -> Result<Self> {
        let listener = UnixListener::bind(path)?;
        let endpoint = Self {
            listener,
            path: path.to_owned(),
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        endpoint.listener.set_nonblocking(true)?;
        Ok(endpoint)
    }

    fn cancellation_requested(&self) -> bool {
        let Ok((mut stream, _)) = self.listener.accept() else {
            return false;
        };
        if stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut request = [0_u8; 7];
        stream.read_exact(&mut request).is_ok() && &request == b"cancel\n"
    }
}

impl Drop for ControlEndpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct TerminalForeground {
    original_group: Pid,
}

impl TerminalForeground {
    fn transfer(child_group: Pid) -> Option<Self> {
        let stdin = io::stdin();
        if !isatty(&stdin).unwrap_or(false) {
            return None;
        }
        let original_group = tcgetpgrp(&stdin).ok()?;
        set_foreground_group(child_group).ok()?;
        Some(Self { original_group })
    }
}

impl Drop for TerminalForeground {
    fn drop(&mut self) {
        let _ = set_foreground_group(self.original_group);
    }
}

fn set_foreground_group(group: Pid) -> nix::Result<()> {
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut previous = SigSet::empty();
    signal::pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))?;
    let result = tcsetpgrp(io::stdin(), group);
    let restore = signal::pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None);
    result.and(restore)
}

struct ChildCleanup<'a> {
    child: &'a mut Child,
    child_group: Pid,
    armed: bool,
}

impl Drop for ChildCleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = signal::killpg(self.child_group, Signal::SIGKILL);
            let _ = self.child.wait();
        }
    }
}

pub fn direct(program: &str, args: &[String]) -> io::Result<ExitStatus> {
    Command::new(program).args(args).status()
}

#[must_use]
pub fn exit_code(outcome: &SupervisedExit) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(signal) = outcome
        .forwarded_signal
        .map(|signal| signal as i32)
        .or_else(|| outcome.status.signal())
    {
        if let Err(error) = emulate_default_handler(signal) {
            eprintln!("agent-gov: cannot propagate child signal {signal}: {error}");
        }
        return 128 + signal;
    }
    outcome.status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_guard_kills_and_reaps_a_started_process_group() {
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .process_group(0)
            .spawn()
            .expect("spawn sleep");
        let child_group = Pid::from_raw(i32::try_from(child.id()).expect("child pid"));
        {
            let _cleanup = ChildCleanup {
                child: &mut child,
                child_group,
                armed: true,
            };
        }
        assert!(child.try_wait().expect("poll child").is_some());
    }
}
