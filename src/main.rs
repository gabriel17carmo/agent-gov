use std::{
    env,
    io::{self, Read, Write},
    path::PathBuf,
    process,
};

use agent_gov::{
    config::Config,
    doctor::Report,
    error::{GovError, Result},
    hook::{HookOptions, Host, handle},
    install::{self, Agent},
    scheduler::{Runtime, Scheduler, set_capacity_transactional, set_drain},
    shell::analyze,
    status::Status,
    supervisor::{self, SuperviseOptions},
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};

#[derive(Debug, Parser)]
#[command(name = "agent-gov", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run a workload under the global heavy pool.
    Run(RunArgs),
    /// Handle a host hook protocol on stdin.
    Hook(HookArgs),
    /// Explain command classification without executing it.
    Classify(ClassifyArgs),
    /// Show active and queued workloads.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Validate runtime, RTK, and hook installation.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Stop admitting new heavy workloads.
    Drain,
    /// Resume heavy workload admission.
    Resume,
    /// Cancel one exact active job.
    Cancel { job_id: String },
    /// Install one composed hook per selected agent.
    Install(InstallArgs),
    /// Remove only hooks managed by agent-gov.
    Uninstall {
        #[arg(long, default_value = "claude,cursor")]
        agents: String,
    },
    /// Apply scheduler configuration safely.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long, default_value = "heavy")]
    pool: String,
    #[arg(long, default_value = "anonymous")]
    owner: String,
    #[arg(long, default_value = "heavy")]
    label: String,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HookHost {
    Claude,
    Cursor,
}

#[derive(Debug, Args)]
struct HookArgs {
    #[arg(value_enum)]
    host: HookHost,
    #[arg(long)]
    rtk: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ClassifyArgs {
    #[arg(long)]
    json: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct InstallArgs {
    #[arg(long, default_value = "claude,cursor")]
    agents: String,
    #[arg(long)]
    with_rtk: bool,
    #[arg(long, requires = "with_rtk")]
    rtk: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Change capacity only while drained and idle.
    SetCapacity {
        capacity: u8,
        #[arg(long)]
        drain: bool,
    },
}

fn main() {
    let code = match dispatch(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("agent-gov: {error}");
            error.exit_code()
        }
    };
    process::exit(code);
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Commands::Run(args) => run(&args),
        Commands::Hook(args) => hook(&args),
        Commands::Classify(args) => classify(&args),
        Commands::Status { json } => status(json),
        Commands::Doctor { json } => doctor(json),
        Commands::Drain => drain(true),
        Commands::Resume => drain(false),
        Commands::Cancel { job_id } => cancel(&job_id),
        Commands::Install(args) => install_hooks(args),
        Commands::Uninstall { agents } => uninstall_hooks(&agents),
        Commands::Config { command } => configure(&command),
    }
}

fn run(args: &RunArgs) -> Result<i32> {
    if args.pool != "heavy" {
        return Err(GovError::InvalidInput(
            "the MVP supports only --pool heavy".into(),
        ));
    }
    let (program, child_args) = args
        .command
        .split_first()
        .ok_or_else(|| GovError::InvalidInput("missing workload command".into()))?;
    if env::var_os("AGENT_GOV_ACTIVE").is_some() {
        return spawn_result(supervisor::direct(program, child_args));
    }
    let config = load_safe_config("runner");
    let scheduler = Scheduler::new(config.clone())?;
    let permit = scheduler.acquire(&args.owner)?;
    let status = supervisor::supervise(
        &permit,
        &SuperviseOptions {
            program,
            args: child_args,
            label: &args.label,
            max_run: config.scheduler.max_run,
            termination_grace: config.scheduler.termination_grace,
        },
    );
    drop(permit);
    match status {
        Ok(status) => Ok(supervisor::exit_code(&status)),
        Err(GovError::Io(error)) => spawn_result(Err(error)),
        Err(error) => Err(error),
    }
}

fn hook(args: &HookArgs) -> Result<i32> {
    let mut input = Vec::new();
    io::stdin().take(128 * 1024 + 1).read_to_end(&mut input)?;
    let config = load_safe_config("hook");
    let binary = env::current_exe()?;
    let host = match args.host {
        HookHost::Claude => Host::Claude,
        HookHost::Cursor => Host::Cursor,
    };
    let output = handle(
        &input,
        &HookOptions {
            host,
            binary_path: &binary,
            rtk_path: args.rtk.as_deref(),
            config: &config,
        },
    )?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.flush()?;
    Ok(0)
}

fn classify(args: &ClassifyArgs) -> Result<i32> {
    let source = args.command.join(" ");
    let config = load_safe_config("classifier");
    let analysis = analyze(&source, &config.classification.rules)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&analysis)?);
    } else if !analysis.supported {
        println!(
            "class: unknown\nrewrite: no\nreason: {}",
            analysis.reason.as_deref().unwrap_or("unsupported")
        );
    } else {
        for segment in analysis.segments {
            println!(
                "class: {:?}\nrule: {}\nconfidence: {}\nsegment: {}\nrewrite: {}",
                segment.classification.class,
                segment.classification.rule_id,
                segment.classification.confidence,
                segment.segment,
                if matches!(
                    segment.classification.class,
                    agent_gov::shell::CommandClass::Heavy
                        | agent_gov::shell::CommandClass::UnsafeBackgroundHeavy
                ) {
                    "yes"
                } else {
                    "no"
                }
            );
        }
    }
    Ok(0)
}

fn status(json: bool) -> Result<i32> {
    let config = load_safe_config("status");
    let runtime = Runtime::initialize(&config)?;
    let status = Status::collect(&runtime, config.scheduler.max_queue)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{}", status.human());
    }
    Ok(0)
}

fn doctor(json: bool) -> Result<i32> {
    let (config, config_error) = match Config::load() {
        Ok(config) => (config, None),
        Err(error) => (Config::default(), Some(error.to_string())),
    };
    let runtime = Runtime::initialize(&config)?;
    let binary = env::current_exe()?;
    let mut report = Report::run(&config, &runtime, &binary);
    if let Some(error) = config_error {
        report.record_config_error(error);
    }
    if json {
        println!("{}", agent_gov::doctor::to_json(&report)?);
    } else {
        println!("{}", report.human());
    }
    Ok(report.exit_code())
}

fn drain(enabled: bool) -> Result<i32> {
    let config = load_safe_config("drain");
    let runtime = Runtime::initialize(&config)?;
    set_drain(&runtime, enabled)?;
    println!(
        "agent-gov: {}",
        if enabled {
            "draining"
        } else {
            "admission resumed"
        }
    );
    Ok(0)
}

fn cancel(job_id: &str) -> Result<i32> {
    let config = load_safe_config("cancel");
    let runtime = Runtime::initialize(&config)?;
    let mut matches = Vec::new();
    for slot in 0..runtime.capacity()? {
        if let Some(metadata) = runtime.active_metadata(slot)?
            && metadata.job_id == job_id
        {
            matches.push((slot, metadata));
        }
    }
    if matches.len() != 1 {
        return Err(GovError::InvalidInput(format!(
            "job id must resolve to exactly one active workload; found {}",
            matches.len()
        )));
    }
    let (slot, metadata) = &matches[0];
    if !runtime.slot_locked(*slot)? || !agent_gov::scheduler::process_alive(metadata.supervisor_pid)
    {
        return Err(GovError::Temporary(
            "job identity is stale or ambiguous; refusing to signal".into(),
        ));
    }
    let pid = i32::try_from(metadata.supervisor_pid)
        .map_err(|_| GovError::Internal("invalid supervisor pid".into()))?;
    signal::kill(Pid::from_raw(pid), Signal::SIGTERM)
        .map_err(|error| GovError::Runtime(format!("cannot signal supervisor: {error}")))?;
    println!("agent-gov: cancellation requested for {job_id}");
    Ok(0)
}

fn install_hooks(args: InstallArgs) -> Result<i32> {
    let agents = parse_agents(&args.agents)?;
    let binary = env::current_exe()?.canonicalize()?;
    let rtk = if args.with_rtk {
        args.rtk.or_else(|| find_executable("rtk")).ok_or_else(|| {
            GovError::InvalidInput("--with-rtk requires --rtk PATH or RTK on PATH".into())
        })?
    } else {
        PathBuf::new()
    };
    let changed = install::install(&agents, &binary, args.with_rtk.then_some(rtk.as_path()))?;
    for path in changed {
        println!("installed: {}", path.display());
    }
    Ok(0)
}

fn uninstall_hooks(agents: &str) -> Result<i32> {
    let agents = parse_agents(agents)?;
    let binary = env::current_exe()?.canonicalize()?;
    for path in install::uninstall(&agents, &binary)? {
        println!("updated: {}", path.display());
    }
    Ok(0)
}

fn configure(command: &ConfigCommand) -> Result<i32> {
    match command {
        ConfigCommand::SetCapacity { capacity, drain } => {
            let mut config = load_safe_config("config");
            let runtime = Runtime::initialize(&config)?;
            let enabled_drain = *drain && !runtime.is_draining();
            if enabled_drain {
                set_drain(&runtime, true)?;
            }
            config.scheduler.capacity = *capacity;
            let update = set_capacity_transactional(&runtime, *capacity, || config.save());
            let resume = if enabled_drain {
                set_drain(&runtime, false)
            } else {
                Ok(())
            };
            update?;
            resume?;
            println!("agent-gov: capacity set to {capacity}");
            Ok(0)
        }
    }
}

fn parse_agents(value: &str) -> Result<Vec<Agent>> {
    let mut agents = Vec::new();
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let agent = match item {
            "claude" => Agent::Claude,
            "cursor" => Agent::Cursor,
            other => {
                return Err(GovError::InvalidInput(format!(
                    "unsupported agent {other}; expected claude,cursor"
                )));
            }
        };
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    if agents.is_empty() {
        return Err(GovError::InvalidInput(
            "at least one agent is required".into(),
        ));
    }
    Ok(agents)
}

fn find_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|path| path.is_file())
    })
}

fn load_safe_config(context: &str) -> Config {
    Config::load().unwrap_or_else(|error| {
        eprintln!("agent-gov: invalid config in {context}; using conservative defaults: {error}");
        Config::default()
    })
}

fn spawn_result(result: io::Result<std::process::ExitStatus>) -> Result<i32> {
    match result {
        Ok(status) => Ok(supervisor::exit_code(
            &agent_gov::supervisor::SupervisedExit::direct(status),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("agent-gov: command not found");
            Ok(127)
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("agent-gov: command is not executable");
            Ok(126)
        }
        Err(error) => Err(error.into()),
    }
}
