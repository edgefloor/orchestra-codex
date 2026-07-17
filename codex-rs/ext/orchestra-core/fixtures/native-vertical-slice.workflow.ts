import { agent, approval, check, parallel, pipeline, repeat, workflow, worktree } from "@codex-orchestra/workflow";

export default workflow({
  name: "native-vertical-slice",
  max_parallel: 2,
  steps: [pipeline([
    parallel([
      agent({ id: "inspect-runtime", prompt: "Inspect the runtime.", model: "gpt-5.4", reasoning_effort: "high", context: [{ type: "range", path: "CONTEXT.md", start: 1, end: 30 }], outputs: ["findings"] }),
      agent({ id: "inspect-tests", prompt: "Inspect tests.", model: "gpt-5.4-mini", reasoning_effort: "medium", context: [{ type: "diff", from: "HEAD~1", to: "HEAD", paths: ["tests/"] }], outputs: ["findings"] }),
    ]),
    worktree(repeat(agent({ id: "implement", prompt: "Implement using declared findings.", model: "gpt-5.4", reasoning_effort: "high", context: [{ type: "dependency_output", step: "inspect-runtime", output: "findings" }], outputs: ["complete"], write_scope: ["crates/"] }), { max_rounds: 2, until_output: "complete", equals: true }), "isolated"),
    check({ id: "tests", command: ["cargo", "test", "--workspace"] }),
    approval({ id: "accept", prompt: "Accept the verified result?", choices: ["accept", "reject"] }),
  ])],
});
