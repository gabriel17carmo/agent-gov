use agent_gov::shell::rewrite;
use proptest::prelude::*;

proptest! {
    #[test]
    fn removing_insertions_recovers_the_exact_source(choice in 0_usize..5) {
        let sources = [
            "npm test",
            "cd app && npm run build >out 2>&1",
            "CI=1 ./mvnw clean verify",
            "cargo test --workspace | tee result.log",
            "pnpm lint; echo done",
        ];
        let source = sources[choice];
        let outcome = rewrite(source, "/bin/agent-gov", "a1", &[], true).expect("rewrite");
        let prefix = "'/bin/agent-gov' run --pool heavy --owner a1 -- ";
        prop_assert_eq!(outcome.command.replace(prefix, ""), source);
    }
}
