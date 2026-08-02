//! Child-process supervision.

use std::{
    io,
    os::unix::process::CommandExt,
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use signal_hook::{
    consts::signal::{SIGINT, SIGQUIT, SIGTERM, SIGWINCH},
    iterator::Signals,
};

use crate::{error::Result, scheduler::Permit};

pub struct SuperviseOptions<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub label: &'a str,
    pub max_run: Duration,
    pub termination_grace: Duration,
}

pub fn supervise(permit: &Permit, options: &SuperviseOptions<'_>) -> Result<ExitStatus> {
    let mut command = Command::new(options.program);
    command
        .args(options.args)
        .env("AGENT_GOV_ACTIVE", "1")
        .env("AGENT_GOV_JOB_ID", permit.job_id())
        .process_group(0);
    let mut child = command.spawn()?;
    permit.mark_running(child.id(), options.label)?;
    let child_group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGWINCH])?;
    let started = Instant::now();
    let mut terminating: Option<(Instant, Signal)> = None;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
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

pub fn direct(program: &str, args: &[String]) -> io::Result<ExitStatus> {
    Command::new(program).args(args).status()
}

#[must_use]
pub fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}
