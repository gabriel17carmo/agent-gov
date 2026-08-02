#![no_main]

use std::path::Path;

use agent_gov::{
    config::Config,
    hook::{HookOptions, Host, handle},
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let config = Config::default();
    let host = if data.first().is_some_and(|byte| byte & 1 == 0) {
        Host::Claude
    } else {
        Host::Cursor
    };
    let _ = handle(
        data,
        &HookOptions {
            host,
            binary_path: Path::new("/opt/agent-gov"),
            rtk_path: None,
            config: &config,
        },
    );
});
