#![no_main]

use agent_gov::{
    config::Config,
    shell::{analyze, rewrite},
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let config = Config::default();
    let _ = analyze(source, &config.classification.rules);
    let _ = rewrite(
        source,
        "/opt/agent-gov",
        "fuzz",
        &config.classification.rules,
        true,
    );
});
