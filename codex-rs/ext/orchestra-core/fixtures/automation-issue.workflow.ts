import { agent, pipeline, workflow } from "@codex-orchestra/workflow";

export default workflow({
  name: "automation-issue",
  description: "Run one claimed coding issue inside its persistent native Issue task.",
  max_parallel: 1,
  inputs: {
    issue: { type: "object" },
    task_prompt: { type: "string" },
    automation: { type: "object" },
  },
  steps: [
    pipeline([
      agent({
        id: "implement",
        prompt: "{{inputs.task_prompt}}\n\nReturn tracker_comment as an object with one body string summarizing the result for the issue tracker.",
        model: "gpt-5.4",
        reasoning_effort: "high",
        outputs: ["summary", "tracker_comment"],
        write_scope: ["."],
      }),
    ]),
  ],
});
