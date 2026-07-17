use crate::Action;
use crate::ContextSource;
use crate::ExecutionPlan;
use crate::ForkTurns;
use crate::InputDefault;
use crate::WorktreePolicy;
use crate::inputs::kind_name;
use crate::inputs::value_matches;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq)]
#[error("{path}: {message}")]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

pub fn validate_plan(plan: &ExecutionPlan) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if plan.name.trim().is_empty() {
        push(&mut errors, "name", "must not be empty");
    }
    if !(1..=32).contains(&plan.max_parallel) {
        push(&mut errors, "max_parallel", "must be between 1 and 32");
    }
    for (name, definition) in &plan.inputs {
        let path = format!("inputs.{name}");
        if !valid_id(name) {
            push(
                &mut errors,
                &path,
                "must use lowercase letters, digits, `_`, or `-`",
            );
        }
        if let InputDefault::Value(value) = &definition.default
            && !value_matches(&definition.kind, value)
        {
            push(
                &mut errors,
                &format!("{path}.default"),
                &format!("must be {}", kind_name(&definition.kind)),
            );
        }
    }
    let mut ids = BTreeSet::new();
    for (index, step) in plan.steps.iter().enumerate() {
        let path = format!("steps[{index}]");
        if !valid_id(&step.id) {
            push(
                &mut errors,
                &format!("{path}.id"),
                "must use lowercase letters, digits, `_`, or `-`",
            );
        }
        if !ids.insert(step.id.clone()) {
            push(&mut errors, &format!("{path}.id"), "duplicate step id");
        }
        if step.max_attempts == 0 || step.max_attempts > 10 {
            push(
                &mut errors,
                &format!("{path}.max_attempts"),
                "must be between 1 and 10",
            );
        }
        if let Some(repeat) = &step.repeat {
            if repeat.max_rounds == 0 || repeat.max_rounds > 20 {
                push(
                    &mut errors,
                    &format!("{path}.repeat.max_rounds"),
                    "must be between 1 and 20",
                );
            }
            if repeat.until_output.is_empty() {
                push(
                    &mut errors,
                    &format!("{path}.repeat.until_output"),
                    "must name an output",
                );
            }
        }
        match &step.action {
            Action::Agent(agent) => {
                if agent.model.trim().is_empty() {
                    push(
                        &mut errors,
                        &format!("{path}.model"),
                        "explicit model is required",
                    );
                }
                if matches!(agent.fork_turns, ForkTurns::All)
                    && (agent.reasoning_effort.is_some() || agent.service_tier.is_some())
                {
                    push(
                        &mut errors,
                        &format!("{path}.fork_turns"),
                        "full-history forks cannot override reasoning or service tier",
                    );
                }
            }
            Action::Check(check) if check.command.is_empty() => {
                push(&mut errors, &format!("{path}.command"), "must not be empty")
            }
            Action::Approval(approval) => {
                if approval.choices.is_empty() {
                    push(
                        &mut errors,
                        &format!("{path}.choices"),
                        "must include a continuing choice",
                    );
                }
                let mut choices = BTreeSet::new();
                for choice in &approval.choices {
                    if choice.trim().is_empty() {
                        push(
                            &mut errors,
                            &format!("{path}.choices"),
                            "must not include an empty choice",
                        );
                    } else if !choices.insert(choice) {
                        push(
                            &mut errors,
                            &format!("{path}.choices"),
                            "must not include duplicate choices",
                        );
                    }
                }
            }
            Action::Check(_) => {}
        }
    }
    for (index, step) in plan.steps.iter().enumerate() {
        for dependency in &step.needs {
            if !ids.contains(dependency) {
                push(
                    &mut errors,
                    &format!("steps[{index}].needs"),
                    &format!("unknown dependency `{dependency}`"),
                );
            }
        }
        if let Action::Agent(agent) = &step.action {
            let prompt_path = format!("steps[{index}].prompt");
            for reference in template_references(&agent.prompt) {
                validate_reference(plan, &ids, &step.id, &prompt_path, reference, &mut errors);
            }
            let context_path = format!("steps[{index}].context");
            for source in &agent.context {
                match source {
                    ContextSource::Input { input } if !plan.inputs.contains_key(input) => push(
                        &mut errors,
                        &context_path,
                        &format!("unknown input `{input}`"),
                    ),
                    ContextSource::DependencyOutput {
                        step: source_step,
                        output,
                    } => validate_output_reference(
                        plan,
                        &ids,
                        &step.id,
                        &context_path,
                        source_step,
                        output,
                        &mut errors,
                    ),
                    _ => {}
                }
            }
        }
    }
    if let Err(error) = crate::skills::collect_requirements(plan.steps.iter().filter_map(|step| {
        let Action::Agent(agent) = &step.action else {
            return None;
        };
        Some(agent.skills.clone())
    })) {
        push(&mut errors, "steps.skills", &error.to_string());
    }
    detect_cycles(plan, &mut errors);
    detect_write_conflicts(plan, &mut errors);
    errors
}

fn template_references(value: &str) -> Vec<&str> {
    let mut references = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        references.push(&after[..end]);
        rest = &after[end + 1..];
    }
    references
}

fn validate_reference(
    plan: &ExecutionPlan,
    ids: &BTreeSet<String>,
    consumer: &str,
    path: &str,
    reference: &str,
    errors: &mut Vec<ValidationError>,
) {
    let parts: Vec<_> = reference.split('.').collect();
    match parts.as_slice() {
        ["inputs", name] if !name.is_empty() => {
            if !plan.inputs.contains_key(*name) {
                push(errors, path, &format!("unknown input `{name}`"));
            }
        }
        ["steps", step, "outputs", output] if !step.is_empty() && !output.is_empty() => {
            validate_output_reference(plan, ids, consumer, path, step, output, errors);
        }
        _ => push(
            errors,
            path,
            &format!("unsupported reference `{reference}`"),
        ),
    }
}

fn validate_output_reference(
    plan: &ExecutionPlan,
    ids: &BTreeSet<String>,
    consumer: &str,
    path: &str,
    step: &str,
    output: &str,
    errors: &mut Vec<ValidationError>,
) {
    if !ids.contains(step) {
        push(errors, path, &format!("unknown output step `{step}`"));
        return;
    }
    let Some(source) = plan.steps.iter().find(|candidate| candidate.id == step) else {
        return;
    };
    if !depends_on(plan, consumer, step) {
        push(
            errors,
            path,
            &format!("output step `{step}` must be a dependency of `{consumer}`"),
        );
        return;
    }
    let Action::Agent(agent) = &source.action else {
        push(
            errors,
            path,
            &format!("step `{step}` does not declare outputs"),
        );
        return;
    };
    if !agent.outputs.iter().any(|candidate| candidate == output) {
        push(
            errors,
            path,
            &format!("unknown dependency output `{step}.{output}`"),
        );
    }
}

fn depends_on(plan: &ExecutionPlan, consumer: &str, producer: &str) -> bool {
    fn visit(
        plan: &ExecutionPlan,
        current: &str,
        producer: &str,
        visited: &mut BTreeSet<String>,
    ) -> bool {
        if !visited.insert(current.into()) {
            return false;
        }
        plan.steps
            .iter()
            .find(|step| step.id == current)
            .is_some_and(|step| {
                step.needs.iter().any(|dependency| {
                    dependency == producer || visit(plan, dependency, producer, visited)
                })
            })
    }
    visit(plan, consumer, producer, &mut BTreeSet::new())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

fn push(errors: &mut Vec<ValidationError>, path: &str, message: &str) {
    errors.push(ValidationError {
        path: path.into(),
        message: message.into(),
    });
}

fn detect_cycles(plan: &ExecutionPlan, errors: &mut Vec<ValidationError>) {
    let graph: BTreeMap<_, _> = plan
        .steps
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.needs.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .collect();
    fn visit<'a>(
        id: &'a str,
        graph: &BTreeMap<&'a str, Vec<&'a str>>,
        visiting: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> bool {
        if done.contains(id) {
            return false;
        }
        if !visiting.insert(id) {
            return true;
        }
        if graph
            .get(id)
            .is_some_and(|deps| deps.iter().any(|dep| visit(dep, graph, visiting, done)))
        {
            return true;
        }
        visiting.remove(id);
        done.insert(id);
        false
    }
    let mut done = BTreeSet::new();
    for id in graph.keys() {
        if visit(id, &graph, &mut BTreeSet::new(), &mut done) {
            push(errors, "steps", "dependency cycle detected");
            break;
        }
    }
}

fn detect_write_conflicts(plan: &ExecutionPlan, errors: &mut Vec<ValidationError>) {
    for (i, left) in plan.steps.iter().enumerate() {
        for right in plan.steps.iter().skip(i + 1) {
            let ordered = left.needs.contains(&right.id) || right.needs.contains(&left.id);
            if ordered || left.write_scope.is_empty() || right.write_scope.is_empty() {
                continue;
            }
            let overlaps = left.write_scope.iter().any(|a| {
                right
                    .write_scope
                    .iter()
                    .any(|b| a.starts_with(b) || b.starts_with(a))
            });
            if overlaps
                && (left.worktree == WorktreePolicy::Shared
                    || right.worktree == WorktreePolicy::Shared)
            {
                push(
                    errors,
                    "steps",
                    &format!(
                        "parallel writers `{}` and `{}` overlap without isolated worktrees",
                        left.id, right.id
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::AgentStep;
    use crate::ExecutionPlan;
    use crate::ForkTurns;
    use crate::Step;

    fn writer(id: &str, scope: &str, worktree: WorktreePolicy) -> Step {
        Step {
            id: id.into(),
            needs: vec![],
            max_attempts: 1,
            repeat: None,
            worktree,
            write_scope: vec![scope.into()],
            action: Action::Agent(AgentStep {
                prompt: "write".into(),
                model: "gpt-5.4".into(),
                reasoning_effort: None,
                service_tier: None,
                fork_turns: ForkTurns::None,
                context: vec![],
                skills: vec![],
                outputs: vec![],
                allow_delegation: false,
            }),
        }
    }

    #[test]
    fn rejects_cycles_and_unknown_dependencies() {
        let mut a = writer("a", "a/", WorktreePolicy::Shared);
        let mut b = writer("b", "b/", WorktreePolicy::Shared);
        a.needs = vec!["b".into()];
        b.needs = vec!["a".into(), "missing".into()];
        let errors = validate_plan(&ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "bad".into(),
            description: String::new(),
            max_parallel: 2,
            steps: vec![a, b],
        });
        assert!(errors.iter().any(|error| error.message.contains("cycle")));
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("unknown dependency"))
        );
    }

    #[test]
    fn approval_requires_a_unique_nonempty_continuing_choice() {
        let approval = |choices: Vec<&str>| Step {
            id: "accept".into(),
            needs: vec![],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Approval(crate::ApprovalStep {
                prompt: "Accept?".into(),
                choices: choices.into_iter().map(Into::into).collect(),
            }),
        };
        for choices in [vec![], vec![""], vec!["accept", "accept"]] {
            let errors = validate_plan(&ExecutionPlan {
                inputs: BTreeMap::new(),
                name: "approval".into(),
                description: String::new(),
                max_parallel: 1,
                steps: vec![approval(choices)],
            });
            assert!(errors.iter().any(|error| error.path.ends_with("choices")));
        }
    }

    #[test]
    fn overlapping_parallel_writers_require_isolation() {
        let plan = ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "conflict".into(),
            description: String::new(),
            max_parallel: 2,
            steps: vec![
                writer("a", "src/", WorktreePolicy::Shared),
                writer("b", "src/lib", WorktreePolicy::Isolated),
            ],
        };
        assert!(
            validate_plan(&plan)
                .iter()
                .any(|error| error.message.contains("parallel writers"))
        );
    }

    #[test]
    fn full_history_rejects_explicit_overrides() {
        let mut step = writer("a", "src/", WorktreePolicy::Isolated);
        let Action::Agent(agent) = &mut step.action else {
            unreachable!()
        };
        agent.fork_turns = ForkTurns::All;
        agent.reasoning_effort = Some("high".into());
        assert!(
            validate_plan(&ExecutionPlan {
                inputs: BTreeMap::new(),
                name: "fork".into(),
                description: String::new(),
                max_parallel: 1,
                steps: vec![step]
            })
            .iter()
            .any(|error| error.message.contains("full-history"))
        );
    }

    #[test]
    fn validates_input_and_forward_output_references_after_collecting_step_ids() {
        let mut consumer = writer("consumer", "consumer/", WorktreePolicy::Isolated);
        consumer.needs = vec!["producer".into()];
        let Action::Agent(agent) = &mut consumer.action else {
            unreachable!()
        };
        agent.prompt = "Use ${inputs.ticket} and ${steps.producer.outputs.result}".into();
        agent.context = vec![ContextSource::DependencyOutput {
            step: "producer".into(),
            output: "result".into(),
        }];
        let mut producer = writer("producer", "producer/", WorktreePolicy::Isolated);
        let Action::Agent(agent) = &mut producer.action else {
            unreachable!()
        };
        agent.outputs = vec!["result".into()];
        let plan = ExecutionPlan {
            inputs: BTreeMap::from([(
                "ticket".into(),
                crate::InputDefinition {
                    kind: crate::InputKind::String,
                    required: true,
                    default: crate::InputDefault::Missing,
                },
            )]),
            name: "references".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![consumer, producer],
        };
        assert!(validate_plan(&plan).is_empty());

        let mut concurrent = plan;
        concurrent.steps[0].needs.clear();
        assert!(
            validate_plan(&concurrent)
                .iter()
                .any(|error| { error.message.contains("must be a dependency of `consumer`") })
        );
    }

    #[test]
    fn prompt_output_references_must_target_declared_dependency_outputs() {
        let mut producer = writer("producer", "producer/", WorktreePolicy::Isolated);
        let Action::Agent(agent) = &mut producer.action else {
            unreachable!()
        };
        agent.outputs = vec!["result".into()];

        let mut consumer = writer("consumer", "consumer/", WorktreePolicy::Isolated);
        consumer.needs = vec!["producer".into()];
        let Action::Agent(agent) = &mut consumer.action else {
            unreachable!()
        };
        agent.prompt = "Use ${steps.producer.outputs.result}".into();

        let plan = ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "prompt-output".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![producer.clone(), consumer.clone()],
        };
        assert!(validate_plan(&plan).is_empty());

        let mut missing_dependency = consumer.clone();
        missing_dependency.needs.clear();
        let errors = validate_plan(&ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "prompt-output".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![producer.clone(), missing_dependency],
        });
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("must be a dependency of `consumer`"))
        );

        let mut wrong_output = producer;
        let Action::Agent(agent) = &mut wrong_output.action else {
            unreachable!()
        };
        agent.outputs = vec!["other".into()];
        let errors = validate_plan(&ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "prompt-output".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![wrong_output, consumer],
        });
        assert!(errors.iter().any(|error| {
            error
                .message
                .contains("unknown dependency output `producer.result`")
        }));
    }
}
