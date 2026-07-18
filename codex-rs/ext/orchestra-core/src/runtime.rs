use crate::Action;
use crate::AgentHandle;
use crate::AgentStatus;
use crate::ExecutionPlan;
use crate::InputError;
use crate::NativeHost;
use crate::PromotionStatus;
use crate::RunCheckpoint;
use crate::RunStatus;
use crate::SpawnRequest;
use crate::Step;
use crate::StepOutputs;
use crate::StepStatus;
use crate::context::materialize_context_with_inputs;
use crate::resolve_inputs;
use crate::resolve_template;
use crate::skills::collect_requirements;
use crate::skills::prepare_skills;
use crate::skills::verify_and_load;
use crate::state::RunCreation;
use crate::state::RunStore;
use crate::validate_plan;
use crate::verify_inputs;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinSet;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum RunOutcome {
    Completed(RunCheckpoint),
    Paused(RunCheckpoint),
    Failed(RunCheckpoint),
    Cancelled(RunCheckpoint),
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error("workflow validation failed: {0}")]
    Validation(String),
    #[error("run inputs failed: {0}")]
    Inputs(#[from] InputError),
    #[error("skill requirements failed: {0}")]
    Skills(#[from] crate::SkillError),
    #[error("run storage failed: {0}")]
    Storage(#[from] std::io::Error),
    #[error("native host failed: {0}")]
    Host(String),
    #[error("runtime task failed: {0}")]
    Join(String),
}

#[derive(Clone)]
struct ActiveRun {
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    agent_handles: Arc<Mutex<HashMap<String, AgentHandle>>>,
    step_tasks: Arc<Mutex<HashMap<String, ActiveStepTask>>>,
    finished: Arc<Notify>,
    completed: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ActiveStepTask {
    kind: ActiveStepTaskKind,
    abort: tokio::task::AbortHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveStepTaskKind {
    Agent,
    Check,
}

pub struct OrchestraRuntime<H: NativeHost> {
    host: Arc<H>,
    active: Arc<Mutex<HashMap<String, ActiveRun>>>,
}

impl<H: NativeHost> Clone for OrchestraRuntime<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            active: Arc::clone(&self.active),
        }
    }
}

impl<H: NativeHost> OrchestraRuntime<H> {
    pub fn new(host: H) -> Self {
        Self {
            host: Arc::new(host),
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn run(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        plan: ExecutionPlan,
    ) -> Result<RunOutcome, RunError> {
        self.run_with_inputs(repository, parent_thread_id, plan, None)
            .await
    }

    pub async fn run_with_inputs(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        plan: ExecutionPlan,
        provided_inputs: Option<&Value>,
    ) -> Result<RunOutcome, RunError> {
        self.run_with_inputs_observed(repository, parent_thread_id, plan, provided_inputs, |_| {
            Ok(())
        })
        .await
    }

    /// Run a workflow while publishing its durable identity immediately after
    /// checkpoint creation. Automation uses this to attach cancellation to the
    /// owning Issue claim before any workflow child can start.
    pub async fn run_with_inputs_observed<F>(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        plan: ExecutionPlan,
        provided_inputs: Option<&Value>,
        on_created: F,
    ) -> Result<RunOutcome, RunError>
    where
        F: FnOnce(&RunCheckpoint) -> Result<(), String>,
    {
        self.run_with_inputs_observed_inner(
            repository,
            parent_thread_id,
            None,
            plan,
            provided_inputs,
            on_created,
        )
        .await
    }

    /// Run a workflow under a caller-reserved durable identity. Automation
    /// persists this identity in its Issue claim before workflow creation so a
    /// crash cannot orphan one Run and create a second Run during recovery.
    pub async fn run_with_inputs_observed_as<F>(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        run_id: &str,
        plan: ExecutionPlan,
        provided_inputs: Option<&Value>,
        on_created: F,
    ) -> Result<RunOutcome, RunError>
    where
        F: FnOnce(&RunCheckpoint) -> Result<(), String>,
    {
        if !valid_reserved_run_id(run_id) {
            return Err(RunError::Validation(
                "reserved run id must be 1-128 ASCII letters, digits, '-' or '_'".into(),
            ));
        }
        self.run_with_inputs_observed_inner(
            repository,
            parent_thread_id,
            Some(run_id),
            plan,
            provided_inputs,
            on_created,
        )
        .await
    }

    async fn run_with_inputs_observed_inner<F>(
        &self,
        repository: &Path,
        parent_thread_id: &str,
        reserved_run_id: Option<&str>,
        plan: ExecutionPlan,
        provided_inputs: Option<&Value>,
        on_created: F,
    ) -> Result<RunOutcome, RunError>
    where
        F: FnOnce(&RunCheckpoint) -> Result<(), String>,
    {
        let errors = validate_plan(&plan);
        if !errors.is_empty() {
            return Err(RunError::Validation(
                errors
                    .into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        let inputs = resolve_inputs(&plan.inputs, provided_inputs)?;
        let skill_requirements = collect_requirements(plan.steps.iter().filter_map(|step| {
            let Action::Agent(agent) = &step.action else {
                return None;
            };
            Some(agent.skills.clone())
        }))?;
        let plan_bytes = serde_json::to_vec(&plan).expect("plan serializes");
        let hash = format!("{:x}", Sha256::digest(&plan_bytes));
        let run_id = reserved_run_id
            .map(str::to_owned)
            .unwrap_or_else(|| new_run_id(&hash));
        let revision = repository_revision(repository)?;
        let resolved_skills = if skill_requirements.is_empty() {
            self.host
                .resolve_skills(parent_thread_id, repository, &revision, &[])
                .await
                .map_err(RunError::Host)?
        } else {
            let workspace = self
                .host
                .create_worktree(
                    parent_thread_id,
                    repository,
                    &run_id,
                    "skill-resolution",
                    &crate::WorktreePolicy::Isolated,
                    &revision,
                )
                .await
                .map_err(RunError::Host)?;
            let resolution = self
                .host
                .resolve_skills(
                    parent_thread_id,
                    &workspace,
                    &revision,
                    &skill_requirements.values().cloned().collect::<Vec<_>>(),
                )
                .await;
            let cleanup = self
                .host
                .remove_worktree(parent_thread_id, repository, &workspace)
                .await;
            match (resolution, cleanup) {
                (Ok(skills), Ok(())) => skills,
                (Err(error), Ok(())) => return Err(RunError::Host(error)),
                (Ok(_), Err(error)) => return Err(RunError::Host(error)),
                (Err(error), Err(cleanup)) => {
                    return Err(RunError::Host(format!(
                        "{error}; skill-resolution cleanup failed: {cleanup}"
                    )));
                }
            }
        };
        let skills = prepare_skills(&skill_requirements, resolved_skills)?;
        let (store, checkpoint) = RunStore::create(RunCreation {
            repository,
            run_id: &run_id,
            plan: &plan,
            workflow_sha256: &hash,
            parent_thread_id,
            source_revision: revision,
            inputs: &inputs,
            skills: &skills,
        })?;
        on_created(&checkpoint).map_err(RunError::Host)?;
        let skill_instructions = verify_and_load(store.root(), &skills.manifest, &skills.sha256)?;
        self.execute(store, plan, checkpoint, skill_instructions)
            .await
    }

    pub async fn resume(&self, repository: &Path, run_id: &str) -> Result<RunOutcome, RunError> {
        self.resume_with_approval(repository, run_id, None).await
    }

    pub async fn resume_with_approval(
        &self,
        repository: &Path,
        run_id: &str,
        approval_decision: Option<&str>,
    ) -> Result<RunOutcome, RunError> {
        self.resume_with_approval_and_inputs(repository, run_id, approval_decision, None)
            .await
    }

    pub async fn resume_with_approval_and_inputs(
        &self,
        repository: &Path,
        run_id: &str,
        approval_decision: Option<&str>,
        provided_inputs: Option<&Value>,
    ) -> Result<RunOutcome, RunError> {
        let (store, plan, mut checkpoint) = RunStore::open(repository, run_id)?;
        let workflow_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&plan).expect("plan serializes"))
        );
        if workflow_sha256 != checkpoint.workflow_sha256 {
            return Err(RunError::Validation(
                "recorded workflow does not match its checkpoint digest".into(),
            ));
        }
        verify_inputs(&checkpoint.inputs, &checkpoint.inputs_sha256)?;
        let persisted_inputs = store.inputs()?;
        verify_inputs(&persisted_inputs, &checkpoint.inputs_sha256)?;
        if persisted_inputs != checkpoint.inputs {
            return Err(InputError::SnapshotMismatch.into());
        }
        let skill_instructions = if checkpoint.schema_version >= 4 {
            let persisted_skills = store.skill_manifest()?;
            if persisted_skills != checkpoint.skills {
                return Err(crate::SkillError::ArtifactChanged(
                    "evidence/skills/manifest.json".into(),
                )
                .into());
            }
            verify_and_load(store.root(), &checkpoint.skills, &checkpoint.skills_sha256)?
        } else {
            BTreeMap::new()
        };
        if let Some(provided) = provided_inputs {
            let supplied = resolve_inputs(&plan.inputs, Some(provided))?;
            if supplied.sha256 != checkpoint.inputs_sha256 {
                let names = plan
                    .inputs
                    .keys()
                    .filter(|name| supplied.values.get(*name) != checkpoint.inputs.get(*name))
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(InputError::Changed {
                    names: if names.is_empty() {
                        "resolved values".into()
                    } else {
                        names
                    },
                }
                .into());
            }
        }
        if matches!(
            checkpoint.status,
            RunStatus::Completed | RunStatus::Cancelled
        ) {
            self.cleanup_run_worktrees(&checkpoint).await?;
            return Ok(if checkpoint.status == RunStatus::Completed {
                RunOutcome::Completed(checkpoint)
            } else {
                RunOutcome::Cancelled(checkpoint)
            });
        }
        let mut approval_decision = approval_decision;
        let mut rejected_approval = None;
        for step in &plan.steps {
            let state = checkpoint
                .steps
                .get_mut(&step.id)
                .expect("snapshot and state agree");
            if state.status == StepStatus::WaitingApproval
                && let Some(decision) = approval_decision.take()
            {
                let Action::Approval(spec) = &step.action else {
                    unreachable!()
                };
                validate_approval_decision(step, spec, decision)?;
                state.approval_decision = Some(decision.to_string());
                state.status = StepStatus::Completed;
                store.approval(&step.id, decision)?;
                if spec
                    .choices
                    .first()
                    .is_some_and(|choice| choice != decision)
                {
                    rejected_approval = Some(step.id.clone());
                }
                continue;
            }
            if matches!(
                state.status,
                StepStatus::Running | StepStatus::Retrying | StepStatus::WaitingApproval
            ) {
                if state.attempts >= step.max_attempts
                    && !matches!(step.action, Action::Approval(_))
                {
                    state.status = StepStatus::Failed;
                    state.error = Some("interrupted after attempt budget was exhausted".into());
                } else {
                    state.status = StepStatus::Pending;
                }
            }
        }
        if let Some(step_id) = rejected_approval {
            checkpoint.status = RunStatus::Cancelled;
            checkpoint.promotion = PromotionStatus::NotRequired;
            checkpoint.next_action = format!("approval `{step_id}` rejected the verified result");
            store.save(&checkpoint)?;
            store.summary(&summary(&checkpoint))?;
            self.cleanup_run_worktrees(&checkpoint).await?;
            return Ok(RunOutcome::Cancelled(checkpoint));
        }
        checkpoint.status = RunStatus::Running;
        checkpoint.next_action = "resume dependency-ready steps from checkpoint".into();
        store.save(&checkpoint)?;
        self.execute(store, plan, checkpoint, skill_instructions)
            .await
    }

    pub async fn status(&self, repository: &Path, run_id: &str) -> Result<RunCheckpoint, RunError> {
        let (_, _, checkpoint) = RunStore::open(repository, run_id)?;
        Ok(checkpoint)
    }

    pub async fn pause(&self, repository: &Path, run_id: &str) -> Result<RunCheckpoint, RunError> {
        if let Some(active) = self.active.lock().await.get(run_id).cloned() {
            active.paused.store(true, Ordering::SeqCst);
            let agent_handles: Vec<_> = active
                .agent_handles
                .lock()
                .await
                .values()
                .cloned()
                .collect();
            for handle in agent_handles {
                self.host.cancel(&handle).await.map_err(RunError::Host)?;
            }
            let tasks = active
                .step_tasks
                .lock()
                .await
                .values()
                .map(|task| task.abort.clone())
                .collect::<Vec<_>>();
            for task in tasks {
                task.abort();
            }
            let finished = active.finished.notified();
            if !active.completed.load(Ordering::SeqCst) {
                finished.await;
            }
            return self.status(repository, run_id).await;
        }
        let (store, _, mut checkpoint) = RunStore::open(repository, run_id)?;
        if matches!(
            checkpoint.status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
        ) {
            return Ok(checkpoint);
        }
        checkpoint.status = RunStatus::WaitingApproval;
        checkpoint.next_action = "run paused; resume from the retained checkpoint".into();
        mark_active_steps_paused(&mut checkpoint);
        store.save(&checkpoint)?;
        store.summary(&summary(&checkpoint))?;
        Ok(checkpoint)
    }

    pub async fn cancel(&self, repository: &Path, run_id: &str) -> Result<RunCheckpoint, RunError> {
        if let Some(active) = self.active.lock().await.get(run_id).cloned() {
            active.cancelled.store(true, Ordering::SeqCst);
            let agent_handles: Vec<_> = active
                .agent_handles
                .lock()
                .await
                .values()
                .cloned()
                .collect();
            for handle in agent_handles {
                self.host.cancel(&handle).await.map_err(RunError::Host)?;
            }
            let check_tasks: Vec<_> = active
                .step_tasks
                .lock()
                .await
                .values()
                .filter(|task| task.kind == ActiveStepTaskKind::Check)
                .map(|task| task.abort.clone())
                .collect();
            for task in check_tasks {
                task.abort();
            }
            let finished = active.finished.notified();
            if !active.completed.load(Ordering::SeqCst) {
                finished.await;
            }
            let (store, _plan, checkpoint) = RunStore::open(repository, run_id)?;
            if matches!(
                checkpoint.status,
                RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
            ) {
                self.cleanup_run_worktrees(&checkpoint).await?;
                return Ok(checkpoint);
            }
            let mut checkpoint = checkpoint;
            checkpoint.status = RunStatus::Cancelled;
            checkpoint.next_action = "run cancelled".into();
            mark_active_steps_cancelled(&mut checkpoint);
            store.save(&checkpoint)?;
            store.summary(&summary(&checkpoint))?;
            self.cleanup_run_worktrees(&checkpoint).await?;
            return Ok(checkpoint);
        }
        let (store, _, mut checkpoint) = RunStore::open(repository, run_id)?;
        if matches!(
            checkpoint.status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
        ) {
            self.cleanup_run_worktrees(&checkpoint).await?;
            return Ok(checkpoint);
        }
        checkpoint.status = RunStatus::Cancelled;
        checkpoint.next_action = "run cancelled".into();
        mark_active_steps_cancelled(&mut checkpoint);
        store.save(&checkpoint)?;
        store.summary(&summary(&checkpoint))?;
        self.cleanup_run_worktrees(&checkpoint).await?;
        Ok(checkpoint)
    }

    async fn execute(
        &self,
        store: RunStore,
        plan: ExecutionPlan,
        mut checkpoint: RunCheckpoint,
        skill_instructions: BTreeMap<String, String>,
    ) -> Result<RunOutcome, RunError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let agent_handles = Arc::new(Mutex::new(HashMap::new()));
        let step_tasks = Arc::new(Mutex::new(HashMap::new()));
        let finished = Arc::new(Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        self.active.lock().await.insert(
            checkpoint.run_id.clone(),
            ActiveRun {
                cancelled: Arc::clone(&cancelled),
                paused: Arc::clone(&paused),
                agent_handles: Arc::clone(&agent_handles),
                step_tasks: Arc::clone(&step_tasks),
                finished: Arc::clone(&finished),
                completed: Arc::clone(&completed),
            },
        );
        checkpoint.status = RunStatus::Running;
        store.save(&checkpoint)?;
        self.host
            .emit_activity(
                &checkpoint.parent_thread_id,
                &format!("Orchestra run `{}` started", checkpoint.run_id),
            )
            .await;

        loop {
            if paused.load(Ordering::SeqCst) {
                checkpoint.status = RunStatus::WaitingApproval;
                checkpoint.next_action = "run paused; resume from the retained checkpoint".into();
                mark_active_steps_paused(&mut checkpoint);
                break;
            }
            if cancelled.load(Ordering::SeqCst) {
                checkpoint.status = RunStatus::Cancelled;
                checkpoint.next_action = "run cancelled".into();
                break;
            }
            if checkpoint
                .steps
                .values()
                .any(|step| step.status == StepStatus::Failed)
            {
                checkpoint.status = RunStatus::Failed;
                checkpoint.next_action = "inspect failed step evidence".into();
                break;
            }
            if checkpoint
                .steps
                .values()
                .all(|step| step.status == StepStatus::Completed)
            {
                checkpoint.status = RunStatus::Completed;
                checkpoint.next_action = "run complete".into();
                break;
            }
            let ready: Vec<_> = plan
                .steps
                .iter()
                .filter(|step| {
                    checkpoint.steps[&step.id].status == StepStatus::Pending
                        && step.needs.iter().all(|dependency| {
                            checkpoint.steps[dependency].status == StepStatus::Completed
                        })
                })
                .take(plan.max_parallel)
                .cloned()
                .collect();
            if ready.is_empty() {
                checkpoint.status = RunStatus::Failed;
                checkpoint.next_action = "no dependency-ready steps remain".into();
                break;
            }

            if let Some(approval) = ready
                .iter()
                .find(|step| matches!(step.action, Action::Approval(_)))
                .cloned()
            {
                checkpoint.steps.get_mut(&approval.id).unwrap().status =
                    StepStatus::WaitingApproval;
                checkpoint.status = RunStatus::WaitingApproval;
                checkpoint.next_action = format!("approval required for `{}`", approval.id);
                store.save(&checkpoint)?;
                let Action::Approval(spec) = &approval.action else {
                    unreachable!()
                };
                match self
                    .host
                    .request_approval(&checkpoint.parent_thread_id, &spec.prompt, &spec.choices)
                    .await
                    .map_err(RunError::Host)?
                {
                    Some(decision) => {
                        validate_approval_decision(&approval, spec, &decision)?;
                        let state = checkpoint.steps.get_mut(&approval.id).unwrap();
                        state.approval_decision = Some(decision.clone());
                        state.status = StepStatus::Completed;
                        store.approval(&approval.id, &decision)?;
                        if spec
                            .choices
                            .first()
                            .is_some_and(|choice| choice != &decision)
                        {
                            checkpoint.status = RunStatus::Cancelled;
                            checkpoint.promotion = PromotionStatus::NotRequired;
                            checkpoint.next_action =
                                format!("approval `{}` rejected the verified result", approval.id);
                            break;
                        }
                        checkpoint.status = RunStatus::Running;
                        checkpoint.next_action = "continue after approval".into();
                        store.save(&checkpoint)?;
                        continue;
                    }
                    None => {
                        break;
                    }
                }
            }

            let dependency_outputs = all_outputs(&checkpoint);
            let mut join_set = JoinSet::new();
            for mut step in ready
                .into_iter()
                .filter(|step| !matches!(step.action, Action::Approval(_)))
            {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let mut skill_context = String::new();
                if let Action::Agent(agent) = &mut step.action {
                    agent.prompt =
                        resolve_template(&agent.prompt, &checkpoint.inputs, &dependency_outputs)?;
                    skill_context = skill_snapshot_context(
                        agent,
                        &checkpoint,
                        store.root(),
                        &skill_instructions,
                    )?;
                }
                let state = checkpoint.steps.get_mut(&step.id).unwrap();
                state.status = StepStatus::Running;
                state.attempts += 1;
                let workspace = self
                    .host
                    .create_worktree(
                        &checkpoint.parent_thread_id,
                        &checkpoint.repository,
                        &checkpoint.run_id,
                        &step.id,
                        &step.worktree,
                        &checkpoint.source_revision,
                    )
                    .await
                    .map_err(RunError::Host)?;
                let context = match &mut step.action {
                    Action::Agent(agent) => {
                        match materialize_context_with_inputs(
                            &workspace,
                            &agent.context,
                            &dependency_outputs,
                            &checkpoint.inputs,
                        ) {
                            Ok(context) => context,
                            Err(error) => {
                                if step.worktree == crate::WorktreePolicy::Isolated {
                                    let _ = self
                                        .host
                                        .remove_worktree(
                                            &checkpoint.parent_thread_id,
                                            &checkpoint.repository,
                                            &workspace,
                                        )
                                        .await;
                                }
                                return Err(RunError::Host(error.to_string()));
                            }
                        }
                    }
                    _ => crate::ContextBundle {
                        sha256: format!("{:x}", Sha256::digest([])),
                        content: String::new(),
                        sources: Vec::new(),
                    },
                };
                state.context_sha256 = Some(context.sha256.clone());
                let task = StepTask {
                    host: Arc::clone(&self.host),
                    handles: Arc::clone(&agent_handles),
                    parent_thread_id: checkpoint.parent_thread_id.clone(),
                    run_id: checkpoint.run_id.clone(),
                    step: step.clone(),
                    attempt: state.attempts,
                    round: state.rounds + 1,
                    workspace,
                    context,
                    skill_context,
                };
                let step_id = step.id.clone();
                let abort = join_set.spawn(async move {
                    let workspace = task.workspace.clone();
                    (step_id, workspace, task.execute().await)
                });
                let kind = match &step.action {
                    Action::Agent(_) => ActiveStepTaskKind::Agent,
                    Action::Check(_) => ActiveStepTaskKind::Check,
                    Action::Approval(_) => unreachable!(),
                };
                step_tasks
                    .lock()
                    .await
                    .insert(step.id.clone(), ActiveStepTask { kind, abort });
            }
            store.save(&checkpoint)?;
            while let Some(result) = join_set.join_next().await {
                let (step_id, workspace, mut result) = match result {
                    Ok(result) => result,
                    Err(error)
                        if (cancelled.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst))
                            && error.is_cancelled() =>
                    {
                        continue;
                    }
                    Err(error) => return Err(RunError::Join(error.to_string())),
                };
                step_tasks.lock().await.remove(&step_id);
                let step = plan.steps.iter().find(|step| step.id == step_id).unwrap();
                if step.worktree == crate::WorktreePolicy::Isolated {
                    if let Ok(step_result) = &mut result
                        && step_result.error.is_none()
                        && let Some(changes) = step_result.changes.take()
                        && let Err(error) = self
                            .integrate_changes(
                                &store,
                                &checkpoint,
                                step,
                                checkpoint.steps[&step_id].attempts,
                                &changes,
                            )
                            .await
                    {
                        result = Err(error);
                    }
                    if let Err(cleanup_error) = self
                        .host
                        .remove_worktree(
                            &checkpoint.parent_thread_id,
                            &checkpoint.repository,
                            &workspace,
                        )
                        .await
                    {
                        result = Err(format!("isolated worktree cleanup failed: {cleanup_error}"));
                    }
                }
                let state = checkpoint.steps.get_mut(&step_id).unwrap();
                match result {
                    Ok(result) => {
                        if let Some(evidence) = result.check_evidence {
                            store.evidence(&step_id, state.attempts, &evidence)?;
                        }
                        if let Some(error) = result.error {
                            state.error = Some(error);
                            state.status = if state.attempts < step.max_attempts {
                                StepStatus::Pending
                            } else {
                                StepStatus::Failed
                            };
                            store.save(&checkpoint)?;
                            continue;
                        }
                        let previous_outputs = state.outputs.clone();
                        state.final_response = result.final_response;
                        state.agent = result.agent;
                        state.outputs = result.outputs;
                        state.error = None;
                        state.rounds += 1;
                        let repeat = step.repeat.as_ref();
                        let condition_met = repeat.is_none_or(|policy| {
                            state.outputs.get(&policy.until_output) == Some(&policy.equals)
                        });
                        if condition_met {
                            state.status = StepStatus::Completed;
                            store.output(&step_id, &state.outputs)?;
                            self.host
                                .persist_outputs(&checkpoint.run_id, &step_id, &state.outputs)
                                .await;
                        } else if let Some(policy) = repeat {
                            if state.rounds < policy.max_rounds {
                                if policy.stop_on_no_progress && state.outputs == previous_outputs {
                                    state.status = StepStatus::Failed;
                                    state.error = Some(
                                        "repeat stopped because outputs made no progress".into(),
                                    );
                                } else {
                                    state.status = StepStatus::Pending;
                                    state.attempts = 0;
                                }
                            } else {
                                state.status = StepStatus::Failed;
                                state.error =
                                    Some("repeat condition was not met before max_rounds".into());
                            }
                        }
                    }
                    Err(error) => {
                        state.error = Some(error);
                        state.status = if state.attempts < step.max_attempts {
                            StepStatus::Pending
                        } else {
                            StepStatus::Failed
                        };
                    }
                }
                store.save(&checkpoint)?;
            }
            if cancelled.load(Ordering::SeqCst) {
                checkpoint.status = RunStatus::Cancelled;
                checkpoint.next_action = "run cancelled".into();
                mark_active_steps_cancelled(&mut checkpoint);
                break;
            }
            if paused.load(Ordering::SeqCst) {
                checkpoint.status = RunStatus::WaitingApproval;
                checkpoint.next_action = "run paused; resume from the retained checkpoint".into();
                mark_active_steps_paused(&mut checkpoint);
                break;
            }
        }

        let mut promotion_failed = false;
        if checkpoint.status == RunStatus::Completed
            && checkpoint.promotion == PromotionStatus::Pending
        {
            match self.promote_verified_changes(&store, &checkpoint).await {
                Ok(status) => {
                    checkpoint.promotion = status;
                    store.save(&checkpoint)?;
                }
                Err(error) => {
                    checkpoint.status = RunStatus::Failed;
                    checkpoint.next_action = format!("promote verified changes: {error}");
                    promotion_failed = true;
                }
            }
        }
        if checkpoint.status != RunStatus::WaitingApproval
            && !promotion_failed
            && let Err(error) = self.cleanup_run_worktrees(&checkpoint).await
        {
            checkpoint.status = RunStatus::Failed;
            checkpoint.next_action = format!("clean up run worktrees: {error}");
        }
        store.save(&checkpoint)?;
        store.summary(&summary(&checkpoint))?;
        self.host
            .emit_activity(
                &checkpoint.parent_thread_id,
                &format!(
                    "Orchestra run `{}` finished as {:?}",
                    checkpoint.run_id, checkpoint.status
                ),
            )
            .await;
        completed.store(true, Ordering::SeqCst);
        finished.notify_waiters();
        self.active.lock().await.remove(&checkpoint.run_id);
        Ok(match checkpoint.status {
            RunStatus::Completed => RunOutcome::Completed(checkpoint),
            RunStatus::Cancelled => RunOutcome::Cancelled(checkpoint),
            RunStatus::WaitingApproval => RunOutcome::Paused(checkpoint),
            _ => RunOutcome::Failed(checkpoint),
        })
    }

    async fn integrate_changes(
        &self,
        store: &RunStore,
        checkpoint: &RunCheckpoint,
        step: &Step,
        attempt: u32,
        changes: &WorktreeChanges,
    ) -> Result<(), String> {
        let patch_path = store
            .change_patch(&step.id, attempt, changes.patch.as_bytes())
            .map_err(|error| format!("failed to persist isolated changes: {error}"))?;
        if changes.patch.is_empty() {
            return Ok(());
        }
        let shared = self
            .host
            .create_worktree(
                &checkpoint.parent_thread_id,
                &checkpoint.repository,
                &checkpoint.run_id,
                &step.id,
                &crate::WorktreePolicy::Shared,
                &checkpoint.source_revision,
            )
            .await?;
        let outcome = self
            .host
            .run_command(
                &checkpoint.parent_thread_id,
                &shared,
                &[
                    "git".into(),
                    "apply".into(),
                    "--index".into(),
                    "--3way".into(),
                    patch_path.to_string_lossy().into_owned(),
                ],
                Some(&shared),
                120_000,
            )
            .await?;
        if outcome.exit_code == 0 {
            Ok(())
        } else {
            Err(format!(
                "failed to integrate isolated changes from `{}`: {}",
                step.id, outcome.stderr
            ))
        }
    }

    async fn cleanup_run_worktrees(&self, checkpoint: &RunCheckpoint) -> Result<(), RunError> {
        let root = checkpoint.repository.join(".codex/orchestra/worktrees");
        if !root.exists() {
            return Ok(());
        }
        let prefix = format!("{}-", checkpoint.run_id);
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with(&prefix) {
                continue;
            }
            self.host
                .remove_worktree(&checkpoint.parent_thread_id, &checkpoint.repository, &path)
                .await
                .map_err(RunError::Host)?;
        }
        Ok(())
    }

    async fn promote_verified_changes(
        &self,
        store: &RunStore,
        checkpoint: &RunCheckpoint,
    ) -> Result<PromotionStatus, String> {
        let shared = shared_worktree_path(&checkpoint.repository, &checkpoint.run_id);
        if !shared.exists() {
            return Ok(PromotionStatus::NotRequired);
        }
        let diff = self
            .host
            .run_command(
                &checkpoint.parent_thread_id,
                &shared,
                &[
                    "git".into(),
                    "diff".into(),
                    "--cached".into(),
                    "--binary".into(),
                    "--full-index".into(),
                    "--no-color".into(),
                ],
                Some(&shared),
                120_000,
            )
            .await?;
        if diff.exit_code != 0 {
            return Err(format!("failed to collect verified patch: {}", diff.stderr));
        }
        if diff.stdout.is_empty() {
            return Ok(PromotionStatus::NotRequired);
        }
        let patch_path = store
            .promotion_patch(diff.stdout.as_bytes())
            .map_err(|error| format!("failed to persist verified patch: {error}"))?;
        let patch = patch_path.to_string_lossy().into_owned();
        let check = self.run_git_apply(checkpoint, &["--check", &patch]).await?;
        if check.exit_code != 0 {
            let reverse = self
                .run_git_apply(checkpoint, &["--reverse", "--check", &patch])
                .await?;
            if reverse.exit_code == 0 {
                return Ok(PromotionStatus::Applied);
            }
            return Err(format!(
                "target checkout no longer accepts the verified patch: {}",
                check.stderr.trim()
            ));
        }
        let applied = self.run_git_apply(checkpoint, &[&patch]).await?;
        if applied.exit_code == 0 {
            Ok(PromotionStatus::Applied)
        } else {
            Err(format!(
                "failed to apply verified patch to target checkout: {}",
                applied.stderr.trim()
            ))
        }
    }

    async fn run_git_apply(
        &self,
        checkpoint: &RunCheckpoint,
        arguments: &[&str],
    ) -> Result<crate::CommandOutcome, String> {
        let mut argv = vec!["git".into(), "apply".into()];
        argv.extend(arguments.iter().map(|argument| (*argument).to_string()));
        self.host
            .run_command(
                &checkpoint.parent_thread_id,
                &checkpoint.repository,
                &argv,
                Some(&checkpoint.repository),
                120_000,
            )
            .await
    }
}

fn skill_snapshot_context(
    agent: &crate::AgentStep,
    checkpoint: &RunCheckpoint,
    run_root: &Path,
    instructions: &BTreeMap<String, String>,
) -> Result<String, crate::SkillError> {
    let mut context = String::new();
    for requirement in &agent.skills {
        let entry = checkpoint
            .skills
            .entries
            .get(&requirement.name)
            .ok_or_else(|| crate::SkillError::Missing(requirement.name.clone()))?;
        let text = instructions
            .get(&requirement.name)
            .ok_or_else(|| crate::SkillError::Missing(requirement.name.clone()))?;
        context.push_str(&format!(
            "\n\n<<< ORCHESTRA SKILL {} >>>\nSource: {}\n{}\n<<< END ORCHESTRA SKILL >>>",
            entry.identity.canonical_name, entry.identity.source_locator, text
        ));
        if !entry.resources.is_empty() {
            context.push_str("\nSnapshotted skill resources:");
            for (name, artifact) in &entry.resources {
                context.push_str(&format!(
                    "\n- {name}: {}",
                    run_root.join(&artifact.path).display()
                ));
            }
        }
    }
    Ok(context)
}

struct StepTask<H: NativeHost> {
    host: Arc<H>,
    handles: Arc<Mutex<HashMap<String, AgentHandle>>>,
    parent_thread_id: String,
    run_id: String,
    step: Step,
    attempt: u32,
    round: u32,
    workspace: PathBuf,
    context: crate::ContextBundle,
    skill_context: String,
}

struct StepResult {
    outputs: StepOutputs,
    final_response: Option<String>,
    agent: Option<AgentHandle>,
    check_evidence: Option<CheckEvidence>,
    changes: Option<WorktreeChanges>,
    error: Option<String>,
}

struct WorktreeChanges {
    patch: String,
}

#[derive(Serialize)]
struct CheckEvidence {
    argv: Vec<String>,
    cwd: Option<String>,
    timeout_ms: u64,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

impl<H: NativeHost> StepTask<H> {
    async fn execute(self) -> Result<StepResult, String> {
        match &self.step.action {
            Action::Agent(agent) => {
                let delegation = if agent.allow_delegation {
                    "Recursive delegation is explicitly allowed for this step."
                } else {
                    "Do not spawn or delegate to child agents."
                };
                let output_contract = if agent.outputs.is_empty() {
                    "Return a JSON object.".into()
                } else {
                    format!(
                        "Return exactly one JSON object containing these keys: {}.",
                        agent.outputs.join(", ")
                    )
                };
                let prompt = format!(
                    "{}\n\n{}\n{}\nContext SHA-256: {}\n{}",
                    agent.prompt,
                    delegation,
                    output_contract,
                    self.context.sha256,
                    self.context.content
                );
                let request = SpawnRequest {
                    parent_thread_id: self.parent_thread_id.clone(),
                    task_name: format!(
                        "orchestra_{}_{}_r{}_a{}",
                        self.run_id
                            .replace(|character: char| !character.is_ascii_alphanumeric(), ""),
                        self.step.id.replace('-', "_"),
                        self.round,
                        self.attempt
                    ),
                    prompt,
                    skill_context: self.skill_context.clone(),
                    cwd: self.workspace.clone(),
                    model: agent.model.clone(),
                    reasoning_effort: agent.reasoning_effort.clone(),
                    service_tier: agent.service_tier.clone(),
                    fork_turns: agent.fork_turns.clone(),
                    allow_delegation: agent.allow_delegation,
                    minimum_descendant_depth: 0,
                };
                let handle = self.host.spawn(request).await?;
                self.handles
                    .lock()
                    .await
                    .insert(self.step.id.clone(), handle.clone());
                let _ = self.host.status(&handle).await?;
                let outcome = self.host.wait(&handle).await?;
                self.handles.lock().await.remove(&self.step.id);
                match outcome.status {
                    AgentStatus::Completed => {
                        let response = outcome.final_response.ok_or_else(|| {
                            "agent completed without a final response".to_string()
                        })?;
                        let value: Value = serde_json::from_str(&response)
                            .map_err(|e| format!("malformed agent output: {e}"))?;
                        let object = value
                            .as_object()
                            .ok_or_else(|| "agent output must be a JSON object".to_string())?;
                        let mut outputs = StepOutputs::new();
                        for name in &agent.outputs {
                            outputs.insert(
                                name.clone(),
                                object
                                    .get(name)
                                    .cloned()
                                    .ok_or_else(|| format!("agent output is missing `{name}`"))?,
                            );
                        }
                        if agent.outputs.is_empty() {
                            outputs.extend(object.clone());
                        }
                        let changes = if self.step.worktree == crate::WorktreePolicy::Isolated {
                            Some(self.collect_changes().await?)
                        } else {
                            None
                        };
                        Ok(StepResult {
                            outputs,
                            final_response: Some(response),
                            agent: Some(handle),
                            check_evidence: None,
                            changes,
                            error: None,
                        })
                    }
                    AgentStatus::Cancelled => Err("agent was cancelled".into()),
                    AgentStatus::Failed(error) => Err(format!("agent failed: {error}")),
                    status => Err(format!("agent ended in non-final status {status:?}")),
                }
            }
            Action::Check(check) => {
                let cwd = check.cwd.as_ref().map(|value| self.workspace.join(value));
                let outcome = self
                    .host
                    .run_command(
                        &self.parent_thread_id,
                        &self.workspace,
                        &check.command,
                        cwd.as_deref(),
                        check.timeout_ms,
                    )
                    .await?;
                let evidence = CheckEvidence {
                    argv: check.command.clone(),
                    cwd: check.cwd.clone(),
                    timeout_ms: check.timeout_ms,
                    exit_code: outcome.exit_code,
                    stdout: outcome.stdout,
                    stderr: outcome.stderr,
                };
                if evidence.exit_code == 0 {
                    Ok(StepResult {
                        outputs: BTreeMap::from([("passed".into(), Value::Bool(true))]),
                        final_response: None,
                        agent: None,
                        check_evidence: Some(evidence),
                        changes: None,
                        error: None,
                    })
                } else {
                    Ok(StepResult {
                        outputs: BTreeMap::from([("passed".into(), Value::Bool(false))]),
                        final_response: None,
                        agent: None,
                        error: Some(format!("check exited with {}", evidence.exit_code)),
                        check_evidence: Some(evidence),
                        changes: None,
                    })
                }
            }
            Action::Approval(_) => unreachable!(),
        }
    }

    async fn collect_changes(&self) -> Result<WorktreeChanges, String> {
        self.git(&["add", "-A"]).await?;
        let names = self.git(&["diff", "--cached", "--name-only", "-z"]).await?;
        let changed_paths: Vec<_> = names
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .collect();
        let outside_scope: Vec<_> = changed_paths
            .iter()
            .filter(|path| !path_in_write_scope(path, &self.step.write_scope))
            .copied()
            .collect();
        if !outside_scope.is_empty() {
            return Err(format!(
                "isolated step changed paths outside write_scope: {}",
                outside_scope.join(", ")
            ));
        }
        let patch = self
            .git(&["diff", "--cached", "--binary", "--full-index", "--no-color"])
            .await?;
        Ok(WorktreeChanges {
            patch: patch.stdout,
        })
    }

    async fn git(&self, arguments: &[&str]) -> Result<crate::CommandOutcome, String> {
        let mut argv = vec!["git".to_string()];
        argv.extend(arguments.iter().map(|value| value.to_string()));
        let outcome = self
            .host
            .run_command(
                &self.parent_thread_id,
                &self.workspace,
                &argv,
                Some(&self.workspace),
                120_000,
            )
            .await?;
        if outcome.exit_code == 0 {
            Ok(outcome)
        } else {
            Err(format!(
                "git {} failed: {}",
                arguments.join(" "),
                outcome.stderr
            ))
        }
    }
}

fn path_in_write_scope(path: &str, scopes: &[String]) -> bool {
    scopes.iter().any(|scope| {
        let scope = scope.trim_end_matches('/');
        !scope.is_empty()
            && (path == scope
                || path
                    .strip_prefix(scope)
                    .is_some_and(|suffix| suffix.starts_with('/')))
    })
}

fn all_outputs(checkpoint: &RunCheckpoint) -> BTreeMap<String, StepOutputs> {
    checkpoint
        .steps
        .iter()
        .map(|(id, state)| (id.clone(), state.outputs.clone()))
        .collect()
}

fn mark_active_steps_cancelled(checkpoint: &mut RunCheckpoint) {
    for step in checkpoint.steps.values_mut() {
        if matches!(
            step.status,
            StepStatus::Running | StepStatus::Retrying | StepStatus::WaitingApproval
        ) {
            step.status = StepStatus::Cancelled;
        }
    }
}

fn mark_active_steps_paused(checkpoint: &mut RunCheckpoint) {
    for step in checkpoint.steps.values_mut() {
        if matches!(step.status, StepStatus::Running | StepStatus::Retrying) {
            step.status = StepStatus::Pending;
            step.agent = None;
            step.error = None;
        }
    }
}

fn new_run_id(hash: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{millis}-{}", &hash[..12])
}
fn valid_reserved_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn shared_worktree_path(repository: &Path, run_id: &str) -> PathBuf {
    repository
        .join(".codex/orchestra/worktrees")
        .join(format!("{run_id}-shared"))
}
fn validate_approval_decision(
    step: &Step,
    spec: &crate::ApprovalStep,
    decision: &str,
) -> Result<(), RunError> {
    if spec.choices.iter().any(|choice| choice == decision) {
        Ok(())
    } else {
        Err(RunError::Validation(format!(
            "approval `{}` decision `{decision}` is not one of: {}",
            step.id,
            spec.choices.join(", ")
        )))
    }
}
pub fn repository_revision(repository: &Path) -> Result<String, std::io::Error> {
    let snapshot = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["stash", "create", "codex-orchestra run snapshot"])
        .output()?;
    if snapshot.status.success() {
        let revision = String::from_utf8_lossy(&snapshot.stdout).trim().to_string();
        if !revision.is_empty() {
            return Ok(revision);
        }
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().into())
    } else {
        Ok("unborn".into())
    }
}
fn summary(checkpoint: &RunCheckpoint) -> String {
    let mut text = format!(
        "# Orchestra run `{}`\n\nStatus: `{:?}`\n\n",
        checkpoint.run_id, checkpoint.status
    );
    for (id, step) in &checkpoint.steps {
        text.push_str(&format!(
            "- `{id}`: `{:?}` (attempts {}, rounds {})",
            step.status, step.attempts, step.rounds
        ));
        if let Some(error) = &step.error {
            text.push_str(&format!(" — {error}"));
        }
        text.push('\n');
    }
    text.push_str(&format!("\nNext action: {}\n", checkpoint.next_action));
    text.push_str(&format!("Promotion: `{:?}`\n", checkpoint.promotion));
    text.push_str(&format!("Skill snapshot: `{}`\n", checkpoint.skills_sha256));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentOutcome;
    use crate::CommandOutcome;
    use crate::ForkTurns;
    use crate::WorktreePolicy;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    struct FakeHost {
        responses: Mutex<Vec<String>>,
        approvals: Mutex<Vec<Option<String>>>,
        exit_code: i32,
        spawned: Mutex<Vec<SpawnRequest>>,
        cancelled: AtomicUsize,
        running: AtomicUsize,
        max_running: AtomicUsize,
        resolved_skills: Mutex<Vec<crate::ResolvedSkill>>,
        skill_resolution_roots: Mutex<Vec<PathBuf>>,
    }
    impl FakeHost {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().map(Into::into).collect()),
                approvals: Mutex::new(vec![]),
                exit_code: 0,
                spawned: Mutex::new(vec![]),
                cancelled: AtomicUsize::new(0),
                running: AtomicUsize::new(0),
                max_running: AtomicUsize::new(0),
                resolved_skills: Mutex::new(vec![]),
                skill_resolution_roots: Mutex::new(vec![]),
            }
        }
        fn with_skills(mut self, skills: Vec<crate::ResolvedSkill>) -> Self {
            *self.resolved_skills.get_mut() = skills;
            self
        }
    }
    #[async_trait]
    impl NativeHost for FakeHost {
        async fn resolve_skills(
            &self,
            _: &str,
            repository: &Path,
            _: &str,
            requirements: &[crate::SkillRequirement],
        ) -> Result<Vec<crate::ResolvedSkill>, String> {
            self.skill_resolution_roots
                .lock()
                .await
                .push(repository.to_path_buf());
            if requirements.is_empty() {
                Ok(vec![])
            } else {
                Ok(self.resolved_skills.lock().await.clone())
            }
        }
        async fn spawn(&self, request: SpawnRequest) -> Result<AgentHandle, String> {
            self.spawned.lock().await.push(request.clone());
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_running.fetch_max(now, Ordering::SeqCst);
            Ok(AgentHandle {
                thread_id: format!("t{now}"),
                task_path: request.task_name,
                parent_thread_id: request.parent_thread_id,
            })
        }
        async fn status(&self, _: &AgentHandle) -> Result<AgentStatus, String> {
            Ok(AgentStatus::Running)
        }
        async fn wait(&self, _: &AgentHandle) -> Result<AgentOutcome, String> {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.running.fetch_sub(1, Ordering::SeqCst);
            Ok(AgentOutcome {
                status: AgentStatus::Completed,
                final_response: self.responses.lock().await.pop(),
            })
        }
        async fn cancel(&self, _: &AgentHandle) -> Result<(), String> {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn run_command(
            &self,
            _: &str,
            _: &Path,
            _: &[String],
            _: Option<&Path>,
            _: u64,
        ) -> Result<CommandOutcome, String> {
            Ok(CommandOutcome {
                exit_code: self.exit_code,
                stdout: "ok".into(),
                stderr: String::new(),
            })
        }
        async fn create_worktree(
            &self,
            _: &str,
            repository: &Path,
            run_id: &str,
            step_id: &str,
            policy: &WorktreePolicy,
            _: &str,
        ) -> Result<PathBuf, String> {
            let path = if *policy == WorktreePolicy::Shared {
                repository
                    .join(".codex/orchestra/worktrees")
                    .join(format!("{run_id}-shared"))
            } else {
                repository
                    .join(".codex/orchestra/worktrees")
                    .join(format!("{run_id}-{step_id}"))
            };
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok(path)
        }
        async fn remove_worktree(&self, _: &str, _: &Path, path: &Path) -> Result<(), String> {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
        async fn request_approval(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<String>, String> {
            Ok(self.approvals.lock().await.pop().unwrap_or(None))
        }
        async fn emit_activity(&self, _: &str, _: &str) {}
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir.path())
            .status()
            .unwrap();
        dir
    }

    struct GitHost {
        response: String,
        workspaces: Mutex<HashMap<String, PathBuf>>,
    }

    #[async_trait]
    impl NativeHost for GitHost {
        async fn spawn(&self, request: SpawnRequest) -> Result<AgentHandle, String> {
            let thread_id = request.task_name.clone();
            self.workspaces
                .lock()
                .await
                .insert(thread_id.clone(), request.cwd);
            Ok(AgentHandle {
                thread_id,
                task_path: request.task_name,
                parent_thread_id: request.parent_thread_id,
            })
        }

        async fn status(&self, _: &AgentHandle) -> Result<AgentStatus, String> {
            Ok(AgentStatus::Running)
        }

        async fn wait(&self, handle: &AgentHandle) -> Result<AgentOutcome, String> {
            let workspace = self.workspaces.lock().await[&handle.thread_id].clone();
            std::fs::create_dir_all(workspace.join("scope")).map_err(|e| e.to_string())?;
            std::fs::write(workspace.join("scope/change.txt"), "integrated\n")
                .map_err(|e| e.to_string())?;
            Ok(AgentOutcome {
                status: AgentStatus::Completed,
                final_response: Some(self.response.clone()),
            })
        }

        async fn cancel(&self, _: &AgentHandle) -> Result<(), String> {
            Ok(())
        }

        async fn run_command(
            &self,
            _: &str,
            repository: &Path,
            argv: &[String],
            cwd: Option<&Path>,
            _: u64,
        ) -> Result<CommandOutcome, String> {
            if argv == ["assert-integrated"] {
                let content = std::fs::read_to_string(repository.join("scope/change.txt"));
                return Ok(CommandOutcome {
                    exit_code: i32::from(
                        !content
                            .as_deref()
                            .is_ok_and(|value| value == "integrated\n"),
                    ),
                    stdout: content.unwrap_or_default(),
                    stderr: String::new(),
                });
            }
            let output = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(cwd.unwrap_or(repository))
                .output()
                .map_err(|e| e.to_string())?;
            Ok(CommandOutcome {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }

        async fn create_worktree(
            &self,
            _: &str,
            repository: &Path,
            run_id: &str,
            step_id: &str,
            policy: &WorktreePolicy,
            source_revision: &str,
        ) -> Result<PathBuf, String> {
            let suffix = if *policy == WorktreePolicy::Shared {
                "shared".to_string()
            } else {
                step_id.to_string()
            };
            let path = repository
                .join(".codex/orchestra/worktrees")
                .join(format!("{run_id}-{suffix}"));
            if path.exists() {
                return Ok(path);
            }
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(["worktree", "add", "--detach"])
                .arg(&path)
                .arg(source_revision)
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() {
                Ok(path)
            } else {
                Err(String::from_utf8_lossy(&output.stderr).into_owned())
            }
        }

        async fn remove_worktree(
            &self,
            _: &str,
            repository: &Path,
            path: &Path,
        ) -> Result<(), String> {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(["worktree", "remove", "--force"])
                .arg(path)
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).into_owned())
            }
        }

        async fn request_approval(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<String>, String> {
            Ok(None)
        }

        async fn emit_activity(&self, _: &str, _: &str) {}
    }

    struct BlockingCheckHost {
        running_checks: AtomicUsize,
        removed_while_running: AtomicBool,
    }

    #[async_trait]
    impl NativeHost for BlockingCheckHost {
        async fn spawn(&self, _: SpawnRequest) -> Result<AgentHandle, String> {
            Err("unexpected agent spawn".into())
        }

        async fn status(&self, _: &AgentHandle) -> Result<AgentStatus, String> {
            Err("unexpected agent status".into())
        }

        async fn wait(&self, _: &AgentHandle) -> Result<AgentOutcome, String> {
            Err("unexpected agent wait".into())
        }

        async fn cancel(&self, _: &AgentHandle) -> Result<(), String> {
            Ok(())
        }

        async fn run_command(
            &self,
            _: &str,
            _: &Path,
            _: &[String],
            _: Option<&Path>,
            _: u64,
        ) -> Result<CommandOutcome, String> {
            struct RunningCheckGuard<'a>(&'a AtomicUsize);
            impl Drop for RunningCheckGuard<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }

            self.running_checks.fetch_add(1, Ordering::SeqCst);
            let _guard = RunningCheckGuard(&self.running_checks);
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(CommandOutcome {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }

        async fn create_worktree(
            &self,
            _: &str,
            repository: &Path,
            run_id: &str,
            step_id: &str,
            policy: &WorktreePolicy,
            _: &str,
        ) -> Result<PathBuf, String> {
            let path = if *policy == WorktreePolicy::Shared {
                repository
                    .join(".codex/orchestra/worktrees")
                    .join(format!("{run_id}-shared"))
            } else {
                repository
                    .join(".codex/orchestra/worktrees")
                    .join(format!("{run_id}-{step_id}"))
            };
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok(path)
        }

        async fn remove_worktree(&self, _: &str, _: &Path, path: &Path) -> Result<(), String> {
            if self.running_checks.load(Ordering::SeqCst) != 0 {
                self.removed_while_running.store(true, Ordering::SeqCst);
            }
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        }

        async fn request_approval(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<String>, String> {
            Ok(None)
        }

        async fn emit_activity(&self, _: &str, _: &str) {}
    }

    fn committed_repo() -> tempfile::TempDir {
        let dir = repo();
        std::fs::write(dir.path().join("README.md"), "source\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args([
                    "-c",
                    "user.name=Orchestra Test",
                    "-c",
                    "user.email=orchestra@example.invalid",
                    "commit",
                    "-qm",
                    "source",
                ])
                .status()
                .unwrap()
                .success()
        );
        dir
    }
    fn agent(id: &str, needs: Vec<&str>) -> Step {
        Step {
            id: id.into(),
            needs: needs.into_iter().map(Into::into).collect(),
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Agent(crate::AgentStep {
                prompt: "do it".into(),
                model: "gpt-5.4".into(),
                reasoning_effort: Some("high".into()),
                service_tier: None,
                fork_turns: ForkTurns::None,
                context: vec![],
                skills: vec![],
                outputs: vec!["ok".into()],
                allow_delegation: false,
            }),
        }
    }

    fn isolated_integration_plan() -> ExecutionPlan {
        let mut implement = agent("implement", vec![]);
        implement.worktree = WorktreePolicy::Isolated;
        implement.write_scope = vec!["scope/".into()];
        let check = Step {
            id: "verify".into(),
            needs: vec!["implement".into()],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Check(crate::CheckStep {
                command: vec!["assert-integrated".into()],
                cwd: None,
                timeout_ms: 1000,
            }),
        };
        let approval = Step {
            id: "accept".into(),
            needs: vec!["verify".into()],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Approval(crate::ApprovalStep {
                prompt: "accept?".into(),
                choices: vec!["accept".into(), "reject".into()],
            }),
        };
        ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "integrate".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![implement, check, approval],
        }
    }

    #[tokio::test]
    async fn schedules_parallel_agents_with_explicit_native_settings() {
        let host = FakeHost::new(vec![r#"{"ok":true}"#, r#"{"ok":true}"#]);
        let runtime = OrchestraRuntime::new(host);
        let result = runtime
            .run(
                repo().path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "parallel".into(),
                    description: String::new(),
                    max_parallel: 2,
                    steps: vec![
                        agent("inspect-runtime", vec![]),
                        agent("inspect-tests", vec![]),
                    ],
                },
            )
            .await
            .unwrap();
        assert!(matches!(result, RunOutcome::Completed(_)));
        assert_eq!(runtime.host.max_running.load(Ordering::SeqCst), 2);
        let requests = runtime.host.spawned.lock().await;
        assert!(requests.iter().all(|r| {
            r.cwd
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.ends_with("-shared"))
                && r.cwd.is_absolute()
                && r.model == "gpt-5.4"
                && r.reasoning_effort.as_deref() == Some("high")
                && r.fork_turns == ForkTurns::None
                && !r.allow_delegation
                && r.task_name.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        }));
        assert!(
            requests
                .iter()
                .any(|request| request.task_name.contains("inspect_runtime"))
        );
    }

    #[tokio::test]
    async fn caller_reserved_run_identity_is_persisted_before_execution() {
        let repository = repo();
        let runtime = OrchestraRuntime::new(FakeHost::new(vec![r#"{"ok":true}"#]));
        let result = runtime
            .run_with_inputs_observed_as(
                repository.path(),
                "parent",
                "automation-intent-1",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "reserved".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![agent("reserved", vec![])],
                },
                None,
                |checkpoint| {
                    assert_eq!(checkpoint.run_id, "automation-intent-1");
                    assert_eq!(checkpoint.status, RunStatus::Pending);
                    Ok(())
                },
            )
            .await
            .unwrap();
        let RunOutcome::Completed(checkpoint) = result else {
            panic!("reserved run should complete")
        };
        assert_eq!(checkpoint.run_id, "automation-intent-1");
        assert!(
            repository
                .path()
                .join(".codex/orchestra/runs/automation-intent-1/state.json")
                .is_file()
        );
    }

    #[tokio::test]
    async fn resolves_persists_and_rejects_changed_run_inputs_on_resume() {
        let repository = repo();
        let host = FakeHost::new(vec![r#"{"ok":true}"#]);
        let runtime = OrchestraRuntime::new(host);
        let mut work = agent("work", vec![]);
        let Action::Agent(agent) = &mut work.action else {
            unreachable!()
        };
        agent.prompt = "Implement ${inputs.ticket} from ${inputs.base}".into();
        agent.context = vec![crate::ContextSource::Input {
            input: "ticket".into(),
        }];
        let plan = ExecutionPlan {
            inputs: BTreeMap::from([
                (
                    "ticket".into(),
                    crate::InputDefinition {
                        kind: crate::InputKind::String,
                        required: true,
                        default: crate::InputDefault::Missing,
                    },
                ),
                (
                    "base".into(),
                    crate::InputDefinition {
                        kind: crate::InputKind::String,
                        required: false,
                        default: crate::InputDefault::Value(Value::String("main".into())),
                    },
                ),
            ]),
            name: "input-run".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![work],
        };
        let outcome = runtime
            .run_with_inputs(
                repository.path(),
                "parent",
                plan,
                Some(&serde_json::json!({"ticket":"#3"})),
            )
            .await
            .unwrap();
        let RunOutcome::Completed(checkpoint) = outcome else {
            panic!()
        };
        let persisted: Value = serde_json::from_slice(
            &std::fs::read(
                repository
                    .path()
                    .join(".codex/orchestra/runs")
                    .join(&checkpoint.run_id)
                    .join("inputs.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(persisted, serde_json::json!({"base":"main","ticket":"#3"}));
        let requests = runtime.host.spawned.lock().await;
        assert!(requests[0].prompt.contains("Implement #3 from main"));
        assert!(requests[0].prompt.contains("input:ticket >>>\n#3"));
        drop(requests);

        let error = runtime
            .resume_with_approval_and_inputs(
                repository.path(),
                &checkpoint.run_id,
                None,
                Some(&serde_json::json!({"ticket":"#4"})),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed run inputs: `ticket`"));
        assert!(error.to_string().contains("omit `inputs`"));
    }

    #[tokio::test]
    async fn resolves_step_output_templates_before_spawning_downstream_agents() {
        let repository = repo();
        let host = FakeHost::new(vec![r#"{"ok":"ready"}"#, r#"{"ok":true}"#]);
        let runtime = OrchestraRuntime::new(host);

        let producer = agent("producer", vec![]);
        let mut consumer = agent("consumer", vec!["producer"]);
        let Action::Agent(agent) = &mut consumer.action else {
            unreachable!()
        };
        agent.prompt = "Use ${steps.producer.outputs.ok}".into();

        let outcome = runtime
            .run(
                repository.path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "step-output-templates".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![producer, consumer],
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Completed(_)));

        let requests = runtime.host.spawned.lock().await;
        assert!(requests[1].prompt.contains("Use ready"));
        assert!(!requests[1].prompt.contains("${steps.producer.outputs.ok}"));
    }

    #[tokio::test]
    async fn snapshots_required_skills_before_pause_and_resumes_from_recorded_bytes() {
        let repository = repo();
        let resolved = crate::ResolvedSkill {
            requirement: "wayfinder".into(),
            identity: crate::SkillIdentity {
                canonical_name: "wayfinder".into(),
                source_kind: crate::SkillSourceKind::User,
                source_locator: "/skills/wayfinder/SKILL.md".into(),
                plugin_id: None,
            },
            instructions: b"Use one question at a time.".to_vec(),
            resources: BTreeMap::from([(
                "references/checklist.md".into(),
                b"Check the domain language.".to_vec(),
            )]),
            tool_dependencies: vec![],
        };
        let host = FakeHost::new(vec![r#"{"ok":true}"#]).with_skills(vec![resolved]);
        let runtime = OrchestraRuntime::new(host);
        let approval = Step {
            id: "accept".into(),
            needs: vec![],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Approval(crate::ApprovalStep {
                prompt: "continue?".into(),
                choices: vec!["accept".into(), "reject".into()],
            }),
        };
        let mut work = agent("work", vec!["accept"]);
        let Action::Agent(agent) = &mut work.action else {
            unreachable!()
        };
        agent.skills = vec![crate::SkillRequirement {
            name: "wayfinder".into(),
            requires: vec![],
            resources: vec!["references/checklist.md".into()],
        }];
        let plan = ExecutionPlan {
            inputs: BTreeMap::new(),
            name: "skills".into(),
            description: String::new(),
            max_parallel: 1,
            steps: vec![approval, work],
        };
        let RunOutcome::Paused(checkpoint) = runtime
            .run(repository.path(), "parent", plan)
            .await
            .unwrap()
        else {
            panic!()
        };
        let run_root = repository
            .path()
            .join(".codex/orchestra/runs")
            .join(&checkpoint.run_id);
        let resolution_roots = runtime.host.skill_resolution_roots.lock().await;
        assert_eq!(resolution_roots.len(), 1);
        assert!(
            resolution_roots[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-skill-resolution")
        );
        assert!(!resolution_roots[0].exists());
        drop(resolution_roots);
        assert!(run_root.join("evidence/skills/manifest.json").is_file());
        let entry = &checkpoint.skills.entries["wayfinder"];
        assert_eq!(
            std::fs::read(run_root.join(&entry.instructions.path)).unwrap(),
            b"Use one question at a time."
        );
        let workflow_path = run_root.join("workflow.json");
        let workflow_bytes = std::fs::read(&workflow_path).unwrap();
        let mut changed_workflow: Value = serde_json::from_slice(&workflow_bytes).unwrap();
        changed_workflow["steps"][1]["skills"] = Value::Array(vec![]);
        std::fs::write(
            &workflow_path,
            serde_json::to_vec(&changed_workflow).unwrap(),
        )
        .unwrap();
        let error = runtime
            .resume_with_approval(repository.path(), &checkpoint.run_id, Some("accept"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("workflow does not match"));
        std::fs::write(&workflow_path, workflow_bytes).unwrap();
        std::fs::write(
            run_root.join(&entry.instructions.path),
            b"tampered snapshot",
        )
        .unwrap();
        let error = runtime
            .resume_with_approval(repository.path(), &checkpoint.run_id, Some("accept"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("snapshot artifact changed"));
        std::fs::write(
            run_root.join(&entry.instructions.path),
            b"Use one question at a time.",
        )
        .unwrap();
        runtime.host.resolved_skills.lock().await[0].instructions =
            b"changed ambient body".to_vec();

        let outcome = runtime
            .resume_with_approval(repository.path(), &checkpoint.run_id, Some("accept"))
            .await
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Completed(_)));
        let requests = runtime.host.spawned.lock().await;
        assert!(
            requests[0]
                .skill_context
                .contains("Use one question at a time.")
        );
        assert!(!requests[0].skill_context.contains("changed ambient body"));
        assert!(
            requests[0]
                .skill_context
                .contains("references/checklist.md")
        );
        assert!(!requests[0].prompt.contains("Use one question at a time."));
    }

    #[tokio::test]
    async fn malformed_output_retries_then_completes() {
        let host = FakeHost::new(vec!["not-json", r#"{"ok":true}"#]);
        let runtime = OrchestraRuntime::new(host);
        let mut step = agent("a", vec![]);
        step.max_attempts = 2;
        let RunOutcome::Completed(state) = runtime
            .run(
                repo().path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "retry".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![step],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(state.steps["a"].attempts, 2);
    }

    #[tokio::test]
    async fn bounded_repeat_exhausts() {
        let host = FakeHost::new(vec![r#"{"ok":false}"#, r#"{"ok":false}"#]);
        let runtime = OrchestraRuntime::new(host);
        let mut step = agent("a", vec![]);
        step.repeat = Some(crate::RepeatPolicy {
            max_rounds: 2,
            until_output: "ok".into(),
            equals: Value::Bool(true),
            stop_on_no_progress: false,
        });
        let RunOutcome::Failed(state) = runtime
            .run(
                repo().path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "repeat".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![step],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(state.steps["a"].rounds, 2);
        let requests = runtime.host.spawned.lock().await;
        assert_eq!(requests.len(), 2);
        assert_ne!(requests[0].task_name, requests[1].task_name);
        assert!(requests[0].task_name.contains("_r1_a1"));
        assert!(requests[1].task_name.contains("_r2_a1"));
    }

    #[tokio::test]
    async fn approval_can_pause_and_resume() {
        let runtime = OrchestraRuntime::new(FakeHost::new(vec![]));
        let step = Step {
            id: "approve".into(),
            needs: vec![],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Shared,
            write_scope: vec![],
            action: Action::Approval(crate::ApprovalStep {
                prompt: "ship?".into(),
                choices: vec!["yes".into()],
            }),
        };
        let dir = repo();
        let RunOutcome::Paused(state) = runtime
            .run(
                dir.path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "approval".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![step],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        runtime.host.approvals.lock().await.push(Some("yes".into()));
        assert!(matches!(
            runtime.resume(dir.path(), &state.run_id).await.unwrap(),
            RunOutcome::Completed(_)
        ));
        assert!(
            dir.path()
                .join(".codex/orchestra/runs")
                .join(&state.run_id)
                .join("summary.md")
                .is_file()
        );
    }

    #[tokio::test]
    async fn failed_check_persists_sandbox_evidence_and_exhausts() {
        let mut host = FakeHost::new(vec![]);
        host.exit_code = 7;
        let runtime = OrchestraRuntime::new(host);
        let step = Step {
            id: "check".into(),
            needs: vec![],
            max_attempts: 1,
            repeat: None,
            worktree: WorktreePolicy::Isolated,
            write_scope: vec![],
            action: Action::Check(crate::CheckStep {
                command: vec!["false".into()],
                cwd: None,
                timeout_ms: 1000,
            }),
        };
        let dir = repo();
        let RunOutcome::Failed(state) = runtime
            .run(
                dir.path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "check".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![step],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let evidence = dir
            .path()
            .join(".codex/orchestra/runs")
            .join(&state.run_id)
            .join("evidence/checks/check-1.json");
        assert!(evidence.is_file());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(evidence).unwrap()).unwrap()["exit_code"],
            7
        );
        assert!(
            !dir.path()
                .join(".codex/orchestra/worktrees")
                .join(format!("{}-check", state.run_id))
                .exists()
        );
    }

    #[tokio::test]
    async fn missing_final_response_exhausts_attempt_budget() {
        let runtime = OrchestraRuntime::new(FakeHost::new(vec![]));
        let RunOutcome::Failed(state) = runtime
            .run(
                repo().path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "exhaust".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![agent("a", vec![])],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(state.steps["a"].attempts, 1);
        assert!(
            state.steps["a"]
                .error
                .as_deref()
                .unwrap()
                .contains("final response")
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_active_native_handle() {
        let runtime = OrchestraRuntime::new(FakeHost::new(vec![r#"{"ok":true}"#]));
        let dir = repo();
        let runner = runtime.clone();
        let repository = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            runner
                .run(
                    &repository,
                    "parent",
                    ExecutionPlan {
                        inputs: BTreeMap::new(),
                        name: "cancel".into(),
                        description: String::new(),
                        max_parallel: 1,
                        steps: vec![agent("a", vec![])],
                    },
                )
                .await
                .unwrap()
        });
        let runs = dir.path().join(".codex/orchestra/runs");
        let run_id = loop {
            if let Ok(mut entries) = std::fs::read_dir(&runs)
                && let Some(Ok(entry)) = entries.next()
            {
                break entry.file_name().to_string_lossy().into_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        };
        while runtime.host.running.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        runtime.cancel(dir.path(), &run_id).await.unwrap();
        assert!(matches!(task.await.unwrap(), RunOutcome::Cancelled(_)));
        assert_eq!(runtime.host.cancelled.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn explicit_pause_interrupts_active_work_and_resumes_the_same_run_checkpoint() {
        let runtime =
            OrchestraRuntime::new(FakeHost::new(vec![r#"{"ok":true}"#, r#"{"ok":true}"#]));
        let dir = repo();
        let runner = runtime.clone();
        let repository = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            runner
                .run(
                    &repository,
                    "parent",
                    ExecutionPlan {
                        inputs: BTreeMap::new(),
                        name: "pause".into(),
                        description: String::new(),
                        max_parallel: 1,
                        steps: vec![agent("a", vec![])],
                    },
                )
                .await
                .unwrap()
        });
        let runs = dir.path().join(".codex/orchestra/runs");
        let run_id = loop {
            if let Ok(mut entries) = std::fs::read_dir(&runs)
                && let Some(Ok(entry)) = entries.next()
            {
                break entry.file_name().to_string_lossy().into_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        };
        while runtime.host.running.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let paused = runtime.pause(dir.path(), &run_id).await.unwrap();
        assert_eq!(paused.run_id, run_id);
        assert_eq!(paused.status, RunStatus::WaitingApproval);
        assert!(matches!(task.await.unwrap(), RunOutcome::Paused(_)));
        assert_eq!(runtime.host.cancelled.load(Ordering::SeqCst), 1);
        let completed = runtime.resume(dir.path(), &run_id).await.unwrap();
        assert!(matches!(completed, RunOutcome::Completed(_)));
    }

    #[tokio::test]
    async fn cancellation_waits_for_running_check_before_cleaning_worktree() {
        let runtime = OrchestraRuntime::new(BlockingCheckHost {
            running_checks: AtomicUsize::new(0),
            removed_while_running: AtomicBool::new(false),
        });
        let dir = repo();
        let runner = runtime.clone();
        let repository = dir.path().to_path_buf();
        let task = tokio::spawn(async move {
            runner
                .run(
                    &repository,
                    "parent",
                    ExecutionPlan {
                        inputs: BTreeMap::new(),
                        name: "cancel-check".into(),
                        description: String::new(),
                        max_parallel: 1,
                        steps: vec![Step {
                            id: "check".into(),
                            needs: vec![],
                            max_attempts: 1,
                            repeat: None,
                            worktree: WorktreePolicy::Shared,
                            write_scope: vec![],
                            action: Action::Check(crate::CheckStep {
                                command: vec!["sleep".into()],
                                cwd: None,
                                timeout_ms: 60_000,
                            }),
                        }],
                    },
                )
                .await
                .unwrap()
        });
        let runs = dir.path().join(".codex/orchestra/runs");
        let run_id = loop {
            if let Ok(mut entries) = std::fs::read_dir(&runs)
                && let Some(Ok(entry)) = entries.next()
            {
                break entry.file_name().to_string_lossy().into_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        };
        while runtime.host.running_checks.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        let cancelled = runtime.cancel(dir.path(), &run_id).await.unwrap();
        assert_eq!(cancelled.status, RunStatus::Cancelled);
        assert!(matches!(task.await.unwrap(), RunOutcome::Cancelled(_)));
        assert_eq!(runtime.host.running_checks.load(Ordering::SeqCst), 0);
        assert!(!runtime.host.removed_while_running.load(Ordering::SeqCst));
        assert!(
            !dir.path()
                .join(".codex/orchestra/worktrees")
                .join(format!("{run_id}-shared"))
                .exists()
        );
    }

    #[tokio::test]
    async fn cancel_does_not_rewrite_completed_run() {
        let runtime = OrchestraRuntime::new(FakeHost::new(vec![r#"{"ok":true}"#]));
        let dir = repo();
        let RunOutcome::Completed(state) = runtime
            .run(
                dir.path(),
                "parent",
                ExecutionPlan {
                    inputs: BTreeMap::new(),
                    name: "done".into(),
                    description: String::new(),
                    max_parallel: 1,
                    steps: vec![agent("a", vec![])],
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let summary_path = dir
            .path()
            .join(".codex/orchestra/runs")
            .join(&state.run_id)
            .join("summary.md");
        let before = std::fs::read_to_string(&summary_path).unwrap();
        let cancelled = runtime.cancel(dir.path(), &state.run_id).await.unwrap();
        let after = std::fs::read_to_string(&summary_path).unwrap();
        assert_eq!(cancelled.status, RunStatus::Completed);
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn isolated_changes_are_verified_promoted_and_worktrees_cleaned_up() {
        let runtime = OrchestraRuntime::new(GitHost {
            response: r#"{"ok":true}"#.into(),
            workspaces: Mutex::new(HashMap::new()),
        });
        let dir = committed_repo();

        let RunOutcome::Paused(state) = runtime
            .run(dir.path(), "parent", isolated_integration_plan())
            .await
            .unwrap()
        else {
            panic!("run should pause only after verifying the integrated change")
        };
        let worktrees = dir.path().join(".codex/orchestra/worktrees");
        assert_eq!(state.steps["verify"].outputs["passed"], Value::Bool(true));
        assert!(!dir.path().join("scope/change.txt").exists());
        assert_eq!(
            std::fs::read_to_string(
                worktrees
                    .join(format!("{}-shared", state.run_id))
                    .join("scope/change.txt")
            )
            .unwrap(),
            "integrated\n"
        );
        assert!(
            !worktrees
                .join(format!("{}-implement", state.run_id))
                .exists()
        );
        assert!(
            dir.path()
                .join(".codex/orchestra/runs")
                .join(&state.run_id)
                .join("evidence/changes/implement-1.patch")
                .is_file()
        );

        let RunOutcome::Completed(completed) = runtime
            .resume_with_approval(dir.path(), &state.run_id, Some("accept"))
            .await
            .unwrap()
        else {
            panic!("accepted verified changes should complete")
        };
        assert_eq!(completed.promotion, PromotionStatus::Applied);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scope/change.txt")).unwrap(),
            "integrated\n"
        );
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["status", "--short", "--", "scope"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "?? scope/\n");
        assert!(
            dir.path()
                .join(".codex/orchestra/runs")
                .join(&state.run_id)
                .join("evidence/changes/promoted.patch")
                .is_file()
        );
        assert!(!worktrees.join(format!("{}-shared", state.run_id)).exists());
    }

    #[tokio::test]
    async fn rejected_verified_changes_are_not_promoted() {
        let runtime = OrchestraRuntime::new(GitHost {
            response: r#"{"ok":true}"#.into(),
            workspaces: Mutex::new(HashMap::new()),
        });
        let dir = committed_repo();
        let RunOutcome::Paused(state) = runtime
            .run(dir.path(), "parent", isolated_integration_plan())
            .await
            .unwrap()
        else {
            panic!("run should pause for approval")
        };

        let error = runtime
            .resume_with_approval(dir.path(), &state.run_id, Some("maybe"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("is not one of"));
        assert_eq!(
            runtime
                .status(dir.path(), &state.run_id)
                .await
                .unwrap()
                .status,
            RunStatus::WaitingApproval
        );

        let RunOutcome::Cancelled(cancelled) = runtime
            .resume_with_approval(dir.path(), &state.run_id, Some("reject"))
            .await
            .unwrap()
        else {
            panic!("rejecting verified changes should cancel the run")
        };
        assert_eq!(cancelled.promotion, PromotionStatus::NotRequired);
        assert!(!dir.path().join("scope/change.txt").exists());
        assert!(
            !dir.path()
                .join(".codex/orchestra/worktrees")
                .join(format!("{}-shared", state.run_id))
                .exists()
        );
    }

    #[tokio::test]
    async fn promotion_conflict_preserves_target_and_shared_worktree_for_retry() {
        let runtime = OrchestraRuntime::new(GitHost {
            response: r#"{"ok":true}"#.into(),
            workspaces: Mutex::new(HashMap::new()),
        });
        let dir = committed_repo();
        let RunOutcome::Paused(state) = runtime
            .run(dir.path(), "parent", isolated_integration_plan())
            .await
            .unwrap()
        else {
            panic!("run should pause for approval")
        };
        std::fs::create_dir_all(dir.path().join("scope")).unwrap();
        std::fs::write(dir.path().join("scope/change.txt"), "user change\n").unwrap();

        let RunOutcome::Failed(failed) = runtime
            .resume_with_approval(dir.path(), &state.run_id, Some("accept"))
            .await
            .unwrap()
        else {
            panic!("a target conflict must fail promotion")
        };
        assert_eq!(failed.promotion, PromotionStatus::Pending);
        assert!(failed.next_action.contains("target checkout"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scope/change.txt")).unwrap(),
            "user change\n"
        );
        let shared = dir
            .path()
            .join(".codex/orchestra/worktrees")
            .join(format!("{}-shared", state.run_id));
        assert!(shared.exists());

        std::fs::remove_file(dir.path().join("scope/change.txt")).unwrap();
        let RunOutcome::Completed(completed) =
            runtime.resume(dir.path(), &state.run_id).await.unwrap()
        else {
            panic!("promotion should retry from the retained shared worktree")
        };
        assert_eq!(completed.promotion, PromotionStatus::Applied);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scope/change.txt")).unwrap(),
            "integrated\n"
        );
        assert!(!shared.exists());
    }

    #[test]
    fn write_scope_matches_only_complete_path_segments() {
        assert!(path_in_write_scope("scope/file.rs", &["scope/".into()]));
        assert!(path_in_write_scope("scope", &["scope".into()]));
        assert!(!path_in_write_scope(
            "scope-other/file.rs",
            &["scope".into()]
        ));
        assert!(!path_in_write_scope("scope/file.rs", &[]));
    }
}
