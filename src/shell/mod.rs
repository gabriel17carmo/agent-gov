mod classify;
mod parse;
mod rewrite;

pub use classify::{Classification, CommandClass, classify_argv};
pub use parse::{Analysis, SegmentPlan, analyze};
pub use rewrite::{RewriteOutcome, rewrite};
