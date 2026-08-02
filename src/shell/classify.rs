use std::path::Path;

use serde::Serialize;

use crate::config::{CustomClass, CustomRule};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandClass {
    Light,
    Heavy,
    Service,
    Unknown,
    UnsafeBackgroundHeavy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Classification {
    pub class: CommandClass,
    pub rule_id: String,
    pub confidence: &'static str,
    #[serde(skip)]
    pub already_wrapped: bool,
}

impl Classification {
    fn new(class: CommandClass, rule_id: impl Into<String>, confidence: &'static str) -> Self {
        Self {
            class,
            rule_id: rule_id.into(),
            confidence,
            already_wrapped: false,
        }
    }
}

#[must_use]
pub fn classify_argv(argv: &[String], custom_rules: &[CustomRule]) -> Classification {
    let normalized = match normalize(argv) {
        Normalized::Command(command) => command,
        Normalized::AlreadyWrapped => {
            let mut result = Classification::new(CommandClass::Light, "governor.reentrant", "high");
            result.already_wrapped = true;
            return result;
        }
        Normalized::UnsafeWrapper => {
            return Classification::new(CommandClass::Unknown, "wrapper.unsupported", "low");
        }
    };

    for rule in custom_rules {
        if normalized.starts_with(&rule.argv_prefix) {
            let class = match rule.class {
                CustomClass::Heavy => CommandClass::Heavy,
                CustomClass::Light => CommandClass::Light,
                CustomClass::Service => CommandClass::Service,
            };
            return Classification::new(class, rule.id.clone(), "user");
        }
    }

    let Some(executable) = normalized.first() else {
        return Classification::new(CommandClass::Unknown, "empty", "low");
    };
    let name = basename(executable);
    let command_args = &normalized[1..];

    match name {
        "mvn" | "mvnw" => classify_maven(command_args),
        "gradle" | "gradlew" => classify_gradle(command_args),
        "npm" => classify_node("npm", command_args),
        "yarn" | "yarnpkg" => classify_node("yarn", command_args),
        "pnpm" => classify_node("pnpm", command_args),
        "cargo" => classify_set("cargo", command_args, &["build", "test", "check", "clippy"]),
        "go" => classify_set("go", command_args, &["build", "test"]),
        "dotnet" => classify_set("dotnet", command_args, &["restore", "build", "test"]),
        "bazel" | "bazelisk" => classify_set("bazel", command_args, &["build", "test"]),
        "swift" => classify_set("swift", command_args, &["build", "test"]),
        "docker" => classify_docker(command_args),
        "xcodebuild" | "make" | "gmake" | "ninja" => {
            Classification::new(CommandClass::Heavy, format!("tier1.{name}"), "medium")
        }
        _ => Classification::new(CommandClass::Unknown, "no-match", "low"),
    }
}

fn classify_maven(args: &[String]) -> Classification {
    if args.iter().any(|arg| arg == "spring-boot:run") {
        return Classification::new(CommandClass::Service, "maven.spring-boot-run", "high");
    }
    let goals = ["clean", "compile", "test", "package", "verify", "install"];
    if args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .any(|arg| goals.contains(&arg.as_str()))
    {
        Classification::new(CommandClass::Heavy, "maven.lifecycle", "high")
    } else {
        Classification::new(CommandClass::Unknown, "maven.unknown-goal", "low")
    }
}

fn classify_gradle(args: &[String]) -> Classification {
    if args.iter().any(|arg| arg == "--continuous" || arg == "-t") {
        return Classification::new(CommandClass::Service, "gradle.continuous", "high");
    }
    let tasks = ["clean", "assemble", "build", "check", "test"];
    if args.iter().filter(|arg| !arg.starts_with('-')).any(|arg| {
        let task = arg.rsplit(':').next().unwrap_or(arg);
        tasks.contains(&task) || task.ends_with("Test")
    }) {
        Classification::new(CommandClass::Heavy, "gradle.task", "high")
    } else {
        Classification::new(CommandClass::Unknown, "gradle.unknown-task", "low")
    }
}

fn classify_node(ecosystem: &str, args: &[String]) -> Classification {
    let Some((index, command)) = args
        .iter()
        .enumerate()
        .find(|(_, arg)| !arg.starts_with('-'))
    else {
        return Classification::new(CommandClass::Unknown, format!("{ecosystem}.missing"), "low");
    };

    if ["start", "dev", "watch"].contains(&command.as_str()) {
        return Classification::new(
            CommandClass::Service,
            format!("{ecosystem}.service"),
            "high",
        );
    }
    if ["install", "ci", "test", "build", "lint", "typecheck"].contains(&command.as_str()) {
        return Classification::new(
            CommandClass::Heavy,
            format!("{ecosystem}.{command}"),
            "high",
        );
    }

    if command == "run" {
        let Some(script) = args[index + 1..].iter().find(|arg| !arg.starts_with('-')) else {
            return Classification::new(
                CommandClass::Unknown,
                format!("{ecosystem}.run-missing"),
                "low",
            );
        };
        if ["start", "dev", "watch"].contains(&script.as_str()) || script.contains("watch") {
            return Classification::new(
                CommandClass::Service,
                format!("{ecosystem}.run-service"),
                "high",
            );
        }
        if ["build", "test", "lint", "typecheck"].contains(&script.as_str()) {
            return Classification::new(
                CommandClass::Heavy,
                format!("{ecosystem}.run-{script}"),
                "high",
            );
        }
        return Classification::new(
            CommandClass::Unknown,
            format!("{ecosystem}.run-unknown"),
            "low",
        );
    }

    Classification::new(CommandClass::Unknown, format!("{ecosystem}.unknown"), "low")
}

fn classify_set(tool: &str, args: &[String], heavy: &[&str]) -> Classification {
    match args.iter().find(|arg| !arg.starts_with('-')) {
        Some(command) if heavy.contains(&command.as_str()) => Classification::new(
            CommandClass::Heavy,
            format!("tier1.{tool}-{command}"),
            "high",
        ),
        _ => Classification::new(CommandClass::Unknown, format!("{tool}.unknown"), "low"),
    }
}

fn classify_docker(args: &[String]) -> Classification {
    let filtered: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    if filtered.first() == Some(&"build") || filtered.starts_with(&["compose", "build"]) {
        Classification::new(CommandClass::Heavy, "tier1.docker-build", "high")
    } else {
        Classification::new(CommandClass::Unknown, "docker.unknown", "low")
    }
}

fn basename(value: &str) -> &str {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(value)
}

enum Normalized {
    Command(Vec<String>),
    AlreadyWrapped,
    UnsafeWrapper,
}

fn normalize(argv: &[String]) -> Normalized {
    let mut index = 0;
    while argv.get(index).is_some_and(|arg| is_assignment(arg)) {
        index += 1;
    }
    loop {
        let Some(value) = argv.get(index) else {
            return Normalized::Command(Vec::new());
        };
        let name = basename(value);
        if name == "agent-gov" && argv.get(index + 1).is_some_and(|arg| arg == "run") {
            return Normalized::AlreadyWrapped;
        }
        match name {
            "sudo" | "xargs" | "bash" | "sh" | "zsh" => return Normalized::UnsafeWrapper,
            "rtk" | "command" => {
                index += 1;
                while argv.get(index).is_some_and(|arg| arg == "--") {
                    index += 1;
                }
            }
            "time" => {
                index += 1;
                while argv.get(index).is_some_and(|arg| arg.starts_with('-')) {
                    index += 1;
                }
            }
            "nice" => {
                index += 1;
                if argv.get(index).is_some_and(|arg| arg == "-n") {
                    index += 2;
                } else if argv.get(index).is_some_and(|arg| arg.starts_with("-n")) {
                    index += 1;
                }
            }
            "env" => {
                index += 1;
                while let Some(arg) = argv.get(index) {
                    if arg == "--" || arg.starts_with('-') || is_assignment(arg) {
                        index += 1;
                    } else {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    Normalized::Command(argv[index..].to_vec())
}

fn is_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().enumerate().all(|(index, ch)| {
            ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(values: &[&str]) -> CommandClass {
        classify_argv(
            &values.iter().map(ToString::to_string).collect::<Vec<_>>(),
            &[],
        )
        .class
    }

    #[test]
    fn classifies_tier_zero() {
        assert_eq!(
            classify(&["./mvnw", "clean", "verify"]),
            CommandClass::Heavy
        );
        assert_eq!(
            classify(&["gradle", "--continuous", "test"]),
            CommandClass::Service
        );
        assert_eq!(classify(&["npm", "run", "build"]), CommandClass::Heavy);
        assert_eq!(classify(&["pnpm", "run", "dev"]), CommandClass::Service);
        assert_eq!(classify(&["npm", "run", "deploy"]), CommandClass::Unknown);
    }

    #[test]
    fn understands_safe_wrappers() {
        assert_eq!(
            classify(&[
                "FOO=bar", "env", "CI=1", "nice", "-n", "5", "rtk", "npm", "test"
            ]),
            CommandClass::Heavy
        );
        assert_eq!(classify(&["sudo", "npm", "test"]), CommandClass::Unknown);
    }
}
