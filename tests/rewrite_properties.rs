use agent_gov::shell::rewrite;
use proptest::prelude::*;

fn heavy_command() -> impl Strategy<Value = String> {
    (
        prop_oneof![
            Just("npm test"),
            Just("npm run build"),
            Just("cargo check"),
            Just("go test ./..."),
            Just("./mvnw clean verify"),
            Just("docker build ."),
        ],
        prop::option::of("[a-zA-Z0-9_-]{1,12}"),
        any::<bool>(),
    )
        .prop_map(|(command, extra, redirect)| {
            let extra = extra.map_or_else(String::new, |value| format!(" --{value}"));
            let redirect = if redirect { " >build.log 2>&1" } else { "" };
            format!("CI=1 {command}{extra}{redirect}")
        })
}

fn shell_source() -> impl Strategy<Value = String> {
    (
        heavy_command(),
        prop::collection::vec(
            (
                prop_oneof![Just(" && "), Just("; "), Just(" | ")],
                heavy_command(),
            ),
            0..4,
        ),
        0_usize..3,
        0_usize..3,
    )
        .prop_map(|(first, rest, leading, trailing)| {
            let mut source = " ".repeat(leading);
            source.push_str(&first);
            for (operator, command) in rest {
                source.push_str(operator);
                source.push_str(&command);
            }
            source.push_str(&" ".repeat(trailing));
            source
        })
}

proptest! {
    #[test]
    fn removing_insertions_recovers_the_exact_source(source in shell_source()) {
        let outcome = rewrite(&source, "/bin/agent-gov", "a1", &[], true).expect("rewrite");
        let prefix = "'/bin/agent-gov' run --pool heavy --owner a1 -- ";
        prop_assert_eq!(outcome.command.replace(prefix, ""), source.as_str());

        let repeated = rewrite(&outcome.command, "/bin/agent-gov", "a1", &[], true)
            .expect("idempotent rewrite");
        prop_assert!(!repeated.changed);
        prop_assert_eq!(repeated.command, outcome.command);
    }
}
