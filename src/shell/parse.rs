use std::ops::Range;

use serde::Serialize;
use tree_sitter::{Node, Parser};

use crate::{
    config::CustomRule,
    error::{GovError, Result},
};

use super::{Classification, CommandClass, classify_argv};

const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_TREE_DEPTH: usize = 128;

#[derive(Clone, Debug, Serialize)]
pub struct SegmentPlan {
    #[serde(skip)]
    pub command_span: Range<usize>,
    pub executable_insert_at: usize,
    pub segment: String,
    pub classification: Classification,
    pub background: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub supported: bool,
    pub reason: Option<String>,
    pub segments: Vec<SegmentPlan>,
}

impl Analysis {
    #[must_use]
    pub fn heavy_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|segment| {
                matches!(
                    segment.classification.class,
                    CommandClass::Heavy | CommandClass::UnsafeBackgroundHeavy
                )
            })
            .count()
    }
}

pub fn analyze(source: &str, custom_rules: &[CustomRule]) -> Result<Analysis> {
    if source.len() > MAX_COMMAND_BYTES {
        return Ok(unsupported("command exceeds 64 KiB"));
    }
    if source.contains('\0') {
        return Ok(unsupported("command contains NUL"));
    }
    if source.contains("<<") || source.contains("$((") || source.contains('`') {
        return Ok(unsupported("command contains unsupported shell syntax"));
    }

    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|error| GovError::Internal(format!("cannot initialize Bash parser: {error}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| GovError::Internal("Bash parser returned no tree".into()))?;
    let root = tree.root_node();
    if root.has_error() || root.end_byte() != source.len() {
        return Ok(unsupported("Bash CST contains errors"));
    }

    let mut segments = Vec::new();
    walk(root, source, custom_rules, 0, &mut segments)?;
    segments.sort_by_key(|segment| segment.command_span.start);
    if segments.is_empty() {
        return Ok(unsupported("no simple command found"));
    }
    Ok(Analysis {
        supported: true,
        reason: None,
        segments,
    })
}

fn walk(
    node: Node<'_>,
    source: &str,
    custom_rules: &[CustomRule],
    depth: usize,
    segments: &mut Vec<SegmentPlan>,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(GovError::InvalidInput(
            "Bash CST exceeds depth limit".into(),
        ));
    }
    if node.kind() == "command" {
        if has_unsafe_ancestor(node) || has_nested_dynamic_syntax(node) {
            return Ok(());
        }
        if let Some(name) = node.child_by_field_name("name") {
            let span = node.byte_range();
            let text = source
                .get(span.clone())
                .ok_or_else(|| GovError::Internal("invalid UTF-8 span from parser".into()))?;
            let argv = shlex::split(text).unwrap_or_default();
            let mut classification = classify_argv(&argv, custom_rules);
            let background = is_background(source, span.end);
            if background && classification.class == CommandClass::Heavy {
                classification.class = CommandClass::UnsafeBackgroundHeavy;
            }
            segments.push(SegmentPlan {
                command_span: span,
                executable_insert_at: name.start_byte(),
                segment: text.to_owned(),
                classification,
                background,
            });
        }
        return Ok(());
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, custom_rules, depth + 1, segments)?;
    }
    Ok(())
}

fn has_unsafe_ancestor(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(
            parent.kind(),
            "command_substitution" | "process_substitution" | "subshell" | "heredoc_body"
        ) {
            return true;
        }
        node = parent;
    }
    false
}

fn has_nested_dynamic_syntax(node: Node<'_>) -> bool {
    if matches!(
        node.kind(),
        "command_substitution" | "process_substitution" | "heredoc_redirect" | "ERROR"
    ) {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(has_nested_dynamic_syntax)
}

fn is_background(source: &str, offset: usize) -> bool {
    let suffix = source[offset..].trim_start();
    suffix.starts_with('&') && !suffix.starts_with("&&")
}

fn unsupported(reason: &str) -> Analysis {
    Analysis {
        supported: false,
        reason: Some(reason.to_owned()),
        segments: Vec::new(),
    }
}
