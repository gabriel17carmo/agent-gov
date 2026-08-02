use serde::Serialize;

use crate::{config::CustomRule, error::Result};

use super::{CommandClass, analyze};

#[derive(Clone, Debug, Serialize)]
pub struct RewriteOutcome {
    pub command: String,
    pub changed: bool,
    pub denied: bool,
    pub heavy_segments: usize,
    pub reason: Option<String>,
}

pub fn rewrite(
    source: &str,
    binary_path: &str,
    owner: &str,
    custom_rules: &[CustomRule],
    deny_background: bool,
) -> Result<RewriteOutcome> {
    let analysis = analyze(source, custom_rules)?;
    if !analysis.supported {
        return Ok(RewriteOutcome {
            command: source.to_owned(),
            changed: false,
            denied: false,
            heavy_segments: 0,
            reason: analysis.reason,
        });
    }

    if deny_background
        && analysis
            .segments
            .iter()
            .any(|segment| segment.classification.class == CommandClass::UnsafeBackgroundHeavy)
    {
        return Ok(RewriteOutcome {
            command: source.to_owned(),
            changed: false,
            denied: true,
            heavy_segments: analysis.heavy_count(),
            reason: Some("heavy workloads must run in the foreground".into()),
        });
    }

    let prefix = format!(
        "{} run --pool heavy --owner {} -- ",
        quote_posix(binary_path),
        safe_owner(owner)
    );
    let mut command = source.to_owned();
    let mut offsets: Vec<usize> = analysis
        .segments
        .iter()
        .filter(|segment| {
            segment.classification.class == CommandClass::Heavy
                && !segment.classification.already_wrapped
        })
        .map(|segment| segment.executable_insert_at)
        .collect();
    offsets.sort_unstable_by(|left, right| right.cmp(left));
    offsets.dedup();
    for offset in &offsets {
        command.insert_str(*offset, &prefix);
    }

    Ok(RewriteOutcome {
        changed: !offsets.is_empty(),
        denied: false,
        heavy_segments: offsets.len(),
        command,
        reason: None,
    })
}

fn safe_owner(owner: &str) -> String {
    let sanitized: String = owner
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(32)
        .collect();
    if sanitized.is_empty() {
        "anonymous".into()
    } else {
        sanitized
    }
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_without_reserializing_shell() {
        let source = "cd app && NODE_ENV=test npm run build >build.log 2>&1";
        let result = rewrite(
            source,
            "/Applications/Agent Gov/bin/agent-gov",
            "7f2a",
            &[],
            true,
        )
        .expect("rewrite");
        assert_eq!(
            result.command,
            "cd app && NODE_ENV=test '/Applications/Agent Gov/bin/agent-gov' run --pool heavy --owner 7f2a -- npm run build >build.log 2>&1"
        );
        assert_eq!(result.heavy_segments, 1);
    }

    #[test]
    fn wraps_multiple_heavy_segments() {
        let result =
            rewrite("npm ci && npm test", "/bin/agent-gov", "a1", &[], true).expect("rewrite");
        assert_eq!(result.heavy_segments, 2);
        assert_eq!(result.command.matches("agent-gov").count(), 2);
    }

    #[test]
    fn is_idempotent() {
        let first = rewrite("npm test", "/bin/agent-gov", "a1", &[], true).expect("first");
        let second = rewrite(&first.command, "/bin/agent-gov", "a1", &[], true).expect("second");
        assert!(!second.changed);
        assert_eq!(first.command, second.command);
    }

    #[test]
    fn denies_background_heavy() {
        let result = rewrite("npm test &", "/bin/agent-gov", "a1", &[], true).expect("rewrite");
        assert!(result.denied);
        assert!(!result.changed);
    }
}
