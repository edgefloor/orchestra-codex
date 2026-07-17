use async_trait::async_trait;
use codex_core::ThreadManager;
use codex_core::orchestra::OrchestraAgentHandle;
use codex_core::orchestra::OrchestraCommandRequest;
use codex_core::orchestra::OrchestraControl;
use codex_core::orchestra::OrchestraForkTurns;
use codex_core::orchestra::OrchestraSkillRequirement;
use codex_core::orchestra::OrchestraSpawnRequest;
use codex_extension_api::ExtensionData;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolSpec;
use codex_http_client::HttpClient;
use codex_http_client::build_reqwest_client_with_custom_ca;
use codex_orchestra_core::AgentHandle;
use codex_orchestra_core::AgentOutcome;
use codex_orchestra_core::AgentStatus;
use codex_orchestra_core::AutomationClaimLiveness;
use codex_orchestra_core::AutomationClaimReconciliation;
use codex_orchestra_core::AutomationClaimStatus;
use codex_orchestra_core::AutomationCleanupStatus;
use codex_orchestra_core::AutomationEffect;
use codex_orchestra_core::AutomationEffectExecution;
use codex_orchestra_core::AutomationEffectReceipt;
use codex_orchestra_core::AutomationEffectStatus;
use codex_orchestra_core::AutomationGatePolicy;
use codex_orchestra_core::AutomationHookKind;
use codex_orchestra_core::AutomationHookStatus;
use codex_orchestra_core::AutomationIssue;
use codex_orchestra_core::AutomationProfile;
use codex_orchestra_core::AutomationQueueCategory;
use codex_orchestra_core::AutomationQueuePage;
use codex_orchestra_core::AutomationRetryKind;
use codex_orchestra_core::AutomationRootCheckpoint;
use codex_orchestra_core::AutomationRootStatus;
use codex_orchestra_core::AutomationRunStart;
use codex_orchestra_core::AutomationRunStore;
use codex_orchestra_core::AutomationSecretKind;
use codex_orchestra_core::AutomationSteeringReceipt;
use codex_orchestra_core::AutomationTrackerCommentRequest;
use codex_orchestra_core::AutomationTrackerPullRequestLinkRequest;
use codex_orchestra_core::AutomationTrackerTransitionRequest;
use codex_orchestra_core::AutomationValidationRequest;
use codex_orchestra_core::AutomationValidationResult;
use codex_orchestra_core::CommandOutcome;
use codex_orchestra_core::ExecutionHistoryRecord;
use codex_orchestra_core::ExecutionHistorySource;
use codex_orchestra_core::ExecutionPlan;
use codex_orchestra_core::ExecutionQueryBudget;
use codex_orchestra_core::ExecutionQueryLimits;
use codex_orchestra_core::ExecutionQueryResult;
use codex_orchestra_core::ExecutionQueryService;
use codex_orchestra_core::ExecutionSelector;
use codex_orchestra_core::ForkTurns;
use codex_orchestra_core::HistoryCursor;
use codex_orchestra_core::HistoryReadRequest;
use codex_orchestra_core::InheritedCodexPolicy;
use codex_orchestra_core::NativeHost;
use codex_orchestra_core::OrchestraRuntime;
use codex_orchestra_core::ResolvedSkill;
use codex_orchestra_core::RunCheckpoint;
use codex_orchestra_core::RunDigest;
use codex_orchestra_core::RunOutcome;
use codex_orchestra_core::RunStatus;
use codex_orchestra_core::SkillIdentity;
use codex_orchestra_core::SkillRequirement;
use codex_orchestra_core::SkillSourceKind;
use codex_orchestra_core::SkillToolDependency;
use codex_orchestra_core::SpawnRequest;
use codex_orchestra_core::WorktreePolicy;
use codex_orchestra_core::automation_claim_liveness;
use codex_orchestra_core::automation_source_sha256;
use codex_orchestra_core::compile_workflow;
use codex_orchestra_core::normalize_linear_issue;
use codex_orchestra_core::normalize_linear_issue_page;
use codex_orchestra_core::normalize_pull_request_url;
use codex_orchestra_core::repository_revision;
use codex_orchestra_core::validate_automation_profile;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AgentStatus as CodexAgentStatus;
use codex_protocol::protocol::OrchestraLifecycleKind;
use codex_protocol::protocol::OrchestraPromotionStatus as CodexOrchestraPromotionStatus;
use codex_protocol::protocol::OrchestraRolloutItem;
use codex_protocol::protocol::OrchestraRunProjection as CodexOrchestraRunProjection;
use codex_protocol::protocol::OrchestraRunStatus as CodexOrchestraRunStatus;
use codex_protocol::protocol::OrchestraStepProjection as CodexOrchestraStepProjection;
use codex_protocol::protocol::OrchestraStepStatus as CodexOrchestraStepStatus;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SandboxPolicy;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomationLinearReadKind {
    Candidates,
    Terminal,
    Refresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomationLinearReadStatus {
    Ready,
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AutomationLinearRead {
    pub status: AutomationLinearReadStatus,
    pub issues: Vec<AutomationIssue>,
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
    pub next_action: String,
}

#[derive(Clone, Copy)]
enum AutomationTrackerBackend {
    Fixture,
    Live,
}

const LINEAR_ISSUE_FIELDS: &str = r#"
id identifier title description priority branchName url createdAt updatedAt
state { name }
labels(first: 50) { nodes { name } }
relations(first: 50) { nodes { type relatedIssue { id identifier state { name } } issue { id identifier state { name } } } }
inverseRelations(first: 50) { nodes { type relatedIssue { id identifier state { name } } issue { id identifier state { name } } } }
"#;

#[derive(Clone)]
struct CodexHost {
    manager: Weak<ThreadManager>,
}

#[derive(Clone)]
struct CodexExecutionHistory {
    manager: Weak<ThreadManager>,
}

#[async_trait]
impl ExecutionHistorySource for CodexExecutionHistory {
    async fn read(
        &self,
        request: &HistoryReadRequest,
    ) -> Result<Vec<ExecutionHistoryRecord>, String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id =
            ThreadId::from_string(&request.parent_thread_id).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;

        let mut events = if let Some(state_db) = thread.state_db() {
            state_db
                .orchestra_task_snapshot(thread_id)
                .await
                .map_err(|error| error.to_string())?
                .map(|snapshot| snapshot.replay)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if events.is_empty() {
            events = thread
                .load_history(/*include_archived*/ true)
                .await
                .map_err(|error| error.to_string())?
                .items
                .into_iter()
                .filter_map(|item| match item {
                    RolloutItem::Orchestra(event) => Some(event),
                    _ => None,
                })
                .collect();
        }
        events.sort_by_key(|event| (event.sequence, event.event_id.clone(), event.revision));
        let after = request.after.as_ref();
        Ok(events
            .into_iter()
            .filter(|event| event.run_id == request.run_id)
            .filter(|event| {
                after.is_none_or(|cursor| {
                    (event.sequence, &event.event_id, event.revision)
                        > (cursor.sequence, &cursor.item_id, cursor.revision)
                })
            })
            .take(request.limit)
            .map(|event| ExecutionHistoryRecord {
                sequence: event.sequence,
                item_id: event.event_id,
                revision: event.revision,
                kind: lifecycle_kind_name(event.kind).into(),
                step_id: None,
                summary: format!(
                    "run {} {} ({})",
                    event.run_id,
                    lifecycle_kind_name(event.kind),
                    rollout_status_name(event.projection.status)
                ),
            })
            .collect())
    }
}

fn lifecycle_kind_name(kind: OrchestraLifecycleKind) -> &'static str {
    match kind {
        OrchestraLifecycleKind::Invoked => "invoked",
        OrchestraLifecycleKind::Resumed => "resumed",
        OrchestraLifecycleKind::Cancelled => "cancelled",
        OrchestraLifecycleKind::Recovered => "recovered",
    }
}

fn rollout_status_name(status: CodexOrchestraRunStatus) -> &'static str {
    match status {
        CodexOrchestraRunStatus::Pending => "pending",
        CodexOrchestraRunStatus::Running => "running",
        CodexOrchestraRunStatus::WaitingApproval => "waiting approval",
        CodexOrchestraRunStatus::Completed => "completed",
        CodexOrchestraRunStatus::Failed => "failed",
        CodexOrchestraRunStatus::Cancelled => "cancelled",
    }
}

fn is_nonterminal_rollout_status(status: CodexOrchestraRunStatus) -> bool {
    matches!(
        status,
        CodexOrchestraRunStatus::Pending
            | CodexOrchestraRunStatus::Running
            | CodexOrchestraRunStatus::WaitingApproval
    )
}

impl CodexHost {
    async fn control(&self, parent: &str) -> Result<OrchestraControl, String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id = ThreadId::from_string(parent).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(thread.orchestra_control().await)
    }

    async fn send_input(&self, handle: &AgentHandle, input: String) -> Result<String, String> {
        self.control(&handle.parent_thread_id)
            .await?
            .send_input(&native_handle(handle)?, input)
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl NativeHost for CodexHost {
    async fn resolve_skills(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        source_revision: &str,
        requirements: &[SkillRequirement],
    ) -> Result<Vec<ResolvedSkill>, String> {
        let resolved = self
            .control(parent_thread_id)
            .await?
            .resolve_skills(
                AbsolutePathBuf::try_from(repository.to_path_buf())
                    .map_err(|error| error.to_string())?,
                source_revision,
                &requirements
                    .iter()
                    .map(|requirement| OrchestraSkillRequirement {
                        name: requirement.name.clone(),
                        resources: requirement.resources.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(resolved
            .into_iter()
            .map(|skill| ResolvedSkill {
                requirement: skill.requirement,
                identity: SkillIdentity {
                    canonical_name: skill.canonical_name,
                    source_kind: match skill.source_kind {
                        codex_core::orchestra::OrchestraSkillSourceKind::Admin => {
                            SkillSourceKind::Admin
                        }
                        codex_core::orchestra::OrchestraSkillSourceKind::User => {
                            SkillSourceKind::User
                        }
                        codex_core::orchestra::OrchestraSkillSourceKind::Repo => {
                            SkillSourceKind::Repo
                        }
                        codex_core::orchestra::OrchestraSkillSourceKind::System => {
                            SkillSourceKind::System
                        }
                    },
                    source_locator: skill.source_locator,
                    plugin_id: skill.plugin_id,
                },
                instructions: skill.instructions,
                resources: skill.resources,
                tool_dependencies: skill
                    .tool_dependencies
                    .into_iter()
                    .map(|tool| SkillToolDependency {
                        kind: tool.kind,
                        value: tool.value,
                        description: tool.description,
                        transport: tool.transport,
                        command: tool.command,
                        url: tool.url,
                    })
                    .collect(),
            })
            .collect())
    }
    async fn spawn(&self, request: SpawnRequest) -> Result<AgentHandle, String> {
        let control = self.control(&request.parent_thread_id).await?;
        let reasoning_effort = request
            .reasoning_effort
            .map(|value| {
                serde_json::from_value::<ReasoningEffort>(Value::String(value))
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        let fork_turns = match request.fork_turns {
            ForkTurns::None => OrchestraForkTurns::None,
            ForkTurns::All => OrchestraForkTurns::All,
            ForkTurns::Last(value) => OrchestraForkTurns::Last(value),
        };
        let handle = control
            .spawn(OrchestraSpawnRequest {
                task_name: request.task_name,
                prompt: request.prompt,
                skill_context: request.skill_context,
                cwd: AbsolutePathBuf::try_from(request.cwd).map_err(|error| error.to_string())?,
                model: request.model,
                reasoning_effort,
                service_tier: request.service_tier,
                fork_turns,
                allow_delegation: request.allow_delegation,
                minimum_descendant_depth: request.minimum_descendant_depth,
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(AgentHandle {
            thread_id: handle.thread_id.to_string(),
            task_path: handle.task_path.to_string(),
            parent_thread_id: request.parent_thread_id,
        })
    }

    async fn status(&self, handle: &AgentHandle) -> Result<AgentStatus, String> {
        let control = self.control(&handle.parent_thread_id).await?;
        Ok(map_status(control.status(&native_handle(handle)?).await))
    }

    async fn wait(&self, handle: &AgentHandle) -> Result<AgentOutcome, String> {
        let control = self.control(&handle.parent_thread_id).await?;
        let status = control
            .wait(&native_handle(handle)?)
            .await
            .map_err(|error| error.to_string())?;
        let final_response = match &status {
            CodexAgentStatus::Completed(message) => message.clone(),
            _ => None,
        };
        Ok(AgentOutcome {
            status: map_status(status),
            final_response,
        })
    }

    async fn cancel(&self, handle: &AgentHandle) -> Result<(), String> {
        let control = self.control(&handle.parent_thread_id).await?;
        control
            .cancel(&native_handle(handle)?)
            .await
            .map_err(|error| error.to_string())
    }

    async fn run_command(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        argv: &[String],
        cwd: Option<&Path>,
        timeout_ms: u64,
    ) -> Result<CommandOutcome, String> {
        let control = self.control(parent_thread_id).await?;
        let cwd = AbsolutePathBuf::try_from(cwd.unwrap_or(repository).to_path_buf())
            .map_err(|error| error.to_string())?;
        let output = control
            .run_command(OrchestraCommandRequest {
                argv: argv.to_vec(),
                cwd,
                timeout_ms,
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(CommandOutcome {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn create_worktree(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        run_id: &str,
        step_id: &str,
        policy: &WorktreePolicy,
        source_revision: &str,
    ) -> Result<PathBuf, String> {
        if source_revision == "unborn" && *policy == WorktreePolicy::Shared {
            return Ok(repository.to_path_buf());
        }
        if source_revision == "unborn" {
            return Err("isolated worktrees require a committed source revision".into());
        }
        let root = repository.join(".codex/orchestra/worktrees");
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let path = if *policy == WorktreePolicy::Shared {
            root.join(format!("{run_id}-shared"))
        } else {
            root.join(format!("{run_id}-{step_id}"))
        };
        if path.exists() {
            if *policy == WorktreePolicy::Shared {
                return Ok(path);
            }
            self.remove_worktree(parent_thread_id, repository, &path)
                .await?;
        }
        let outcome = self
            .run_command(
                parent_thread_id,
                repository,
                &[
                    "git".into(),
                    "worktree".into(),
                    "add".into(),
                    "--detach".into(),
                    path.to_string_lossy().into_owned(),
                    source_revision.into(),
                ],
                Some(repository),
                120_000,
            )
            .await?;
        if outcome.exit_code != 0 {
            return Err(format!("git worktree add failed: {}", outcome.stderr));
        }
        Ok(path)
    }

    async fn remove_worktree(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        path: &Path,
    ) -> Result<(), String> {
        if !path.exists() {
            let outcome = self
                .run_command(
                    parent_thread_id,
                    repository,
                    &["git".into(), "worktree".into(), "prune".into()],
                    Some(repository),
                    30_000,
                )
                .await?;
            return if outcome.exit_code == 0 {
                Ok(())
            } else {
                Err(outcome.stderr)
            };
        }
        let outcome = self
            .run_command(
                parent_thread_id,
                repository,
                &[
                    "git".into(),
                    "worktree".into(),
                    "remove".into(),
                    "--force".into(),
                    path.to_string_lossy().into_owned(),
                ],
                Some(repository),
                120_000,
            )
            .await?;
        if outcome.exit_code == 0 {
            Ok(())
        } else {
            Err(outcome.stderr)
        }
    }

    async fn create_persistent_worktree(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        path: &Path,
        source_revision: &str,
    ) -> Result<PathBuf, String> {
        if source_revision == "unborn" {
            return Err("Automation worktrees require a committed source revision".into());
        }
        if path.exists() {
            let canonical = path.canonicalize().map_err(|error| error.to_string())?;
            let canonical_parent = path
                .parent()
                .ok_or("Automation worktree is missing its configured root")?
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if canonical == canonical_parent || !canonical.starts_with(&canonical_parent) {
                return Err(format!(
                    "Automation worktree escapes its configured root: {}",
                    path.display()
                ));
            }
            let outcome = self
                .run_command(
                    parent_thread_id,
                    repository,
                    &[
                        "git".into(),
                        "worktree".into(),
                        "list".into(),
                        "--porcelain".into(),
                        "-z".into(),
                    ],
                    Some(repository),
                    30_000,
                )
                .await?;
            if persistent_worktree_matches_recorded_base(&outcome, &canonical, source_revision) {
                return Ok(canonical);
            }
            return Err(format!(
                "stale Automation worktree at {} does not match recorded base {}",
                path.display(),
                source_revision
            ));
        }
        let parent = path
            .parent()
            .ok_or("Automation worktree is missing its configured root")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let outcome = self
            .run_command(
                parent_thread_id,
                repository,
                &[
                    "git".into(),
                    "worktree".into(),
                    "add".into(),
                    "--detach".into(),
                    path.to_string_lossy().into_owned(),
                    source_revision.into(),
                ],
                Some(repository),
                120_000,
            )
            .await?;
        if outcome.exit_code != 0 {
            return Err(format!("git worktree add failed: {}", outcome.stderr));
        }
        path.canonicalize().map_err(|error| error.to_string())
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

fn native_handle(handle: &AgentHandle) -> Result<OrchestraAgentHandle, String> {
    Ok(OrchestraAgentHandle {
        thread_id: ThreadId::from_string(&handle.thread_id).map_err(|error| error.to_string())?,
        task_path: AgentPath::try_from(handle.task_path.as_str())
            .map_err(|error| error.to_string())?,
    })
}

fn persistent_worktree_matches_recorded_base(
    outcome: &CommandOutcome,
    expected_path: &Path,
    source_revision: &str,
) -> bool {
    if outcome.exit_code != 0 {
        return false;
    }
    let expected_path = expected_path.to_string_lossy();
    let mut worktree = None;
    let mut head = None;
    for field in outcome.stdout.split('\0') {
        if field.is_empty() {
            if worktree == Some(expected_path.as_ref()) && head == Some(source_revision) {
                return true;
            }
            worktree = None;
            head = None;
        } else if let Some(value) = field.strip_prefix("worktree ") {
            worktree = Some(value);
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            head = Some(value);
        }
    }
    worktree == Some(expected_path.as_ref()) && head == Some(source_revision)
}
fn map_status(status: CodexAgentStatus) -> AgentStatus {
    match status {
        CodexAgentStatus::PendingInit => AgentStatus::Pending,
        CodexAgentStatus::Running | CodexAgentStatus::Interrupted => AgentStatus::Running,
        CodexAgentStatus::Completed(_) => AgentStatus::Completed,
        CodexAgentStatus::Errored(error) => AgentStatus::Failed(error),
        CodexAgentStatus::Shutdown | CodexAgentStatus::NotFound => AgentStatus::Cancelled,
    }
}

#[derive(Clone)]
pub struct OrchestraService {
    host: CodexHost,
    runtime: OrchestraRuntime<CodexHost>,
    queries: ExecutionQueryService,
    manager: Weak<ThreadManager>,
    automation_shutdown: Arc<AutomationShutdownFence>,
}

type AutomationStartResult = Result<AutomationRootCheckpoint, String>;
type AutomationStartSignal =
    Arc<Mutex<Option<tokio::sync::oneshot::Sender<AutomationStartResult>>>>;

#[derive(Default)]
struct AutomationShutdownFence {
    roots: Mutex<BTreeMap<(PathBuf, String), ()>>,
}

impl AutomationShutdownFence {
    fn track(&self, repository: &Path, run_id: &str) {
        if let Ok(mut roots) = self.roots.lock() {
            roots.insert((repository.to_path_buf(), run_id.to_owned()), ());
        }
    }

    fn remove(&self, repository: &Path, run_id: &str) {
        if let Ok(mut roots) = self.roots.lock() {
            roots.remove(&(repository.to_path_buf(), run_id.to_owned()));
        }
    }
}

impl Drop for AutomationShutdownFence {
    fn drop(&mut self) {
        let Ok(roots) = self.roots.get_mut() else {
            return;
        };
        for (repository, run_id) in roots.keys() {
            let result = (|| {
                let store = AutomationRunStore::open(repository, run_id)?;
                let mut root = store.load()?;
                if root.status == AutomationRootStatus::Running {
                    store.pause(&mut root, "graceful Codex host shutdown")?;
                }
                Ok::<(), codex_orchestra_core::AutomationRunError>(())
            })();
            if let Err(error) = result {
                eprintln!("failed to fence Automation `{run_id}` during host shutdown: {error}");
            }
        }
    }
}

impl OrchestraService {
    pub fn new(manager: Weak<ThreadManager>) -> Self {
        let host = CodexHost {
            manager: manager.clone(),
        };
        Self {
            runtime: OrchestraRuntime::new(host.clone()),
            host,
            queries: ExecutionQueryService::with_history_source(
                ExecutionQueryLimits::default(),
                Arc::new(CodexExecutionHistory {
                    manager: manager.clone(),
                }),
            ),
            manager,
            automation_shutdown: Arc::new(AutomationShutdownFence::default()),
        }
    }

    pub async fn validate(
        &self,
        parent_thread_id: &str,
        workflow_path: &str,
    ) -> Result<ExecutionPlan, String> {
        let repository = self.repository(parent_thread_id).await?;
        let path = safe_workflow(&repository, workflow_path)?;
        let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        compile_workflow(&source).map_err(|error| error.to_string())
    }

    pub async fn validate_automation(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
        fixture_issue: AutomationIssue,
        attempt: Option<u32>,
    ) -> Result<AutomationValidationResult, String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id =
            ThreadId::from_string(parent_thread_id).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        let config = thread.config_snapshot().await;
        let repository = config.cwd().as_path().to_path_buf();
        let profile_path = safe_automation_profile(&repository, profile_path)?;
        let sandbox_policy = config.sandbox_policy();
        let thread_sandbox = match sandbox_policy {
            SandboxPolicy::ReadOnly { .. } => "read-only",
            SandboxPolicy::WorkspaceWrite { .. } => "workspace-write",
            SandboxPolicy::DangerFullAccess => "danger-full-access",
            SandboxPolicy::ExternalSandbox { .. } => "read-only",
        }
        .to_owned();
        let inherited_policy = InheritedCodexPolicy {
            approval_policy: serde_json::to_value(config.approval_policy)
                .map_err(|error| error.to_string())?,
            thread_sandbox,
            turn_sandbox_policy: serde_json::to_value(sandbox_policy)
                .map_err(|error| error.to_string())?,
        };
        Ok(validate_automation_profile(AutomationValidationRequest {
            workflow_md_path: profile_path,
            repository_root: repository,
            fixture_issue,
            attempt,
            environment: std::env::vars().collect(),
            home_dir: std::env::var_os("HOME").map(PathBuf::from),
            inherited_policy,
        }))
    }

    pub async fn read_linear_automation(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
        kind: AutomationLinearReadKind,
        after: Option<&str>,
        first: Option<u32>,
        issue_identifier: Option<&str>,
    ) -> Result<AutomationLinearRead, String> {
        let validation = self
            .validate_automation(
                parent_thread_id,
                profile_path,
                AutomationIssue {
                    id: "live-preview".into(),
                    identifier: issue_identifier.unwrap_or("LIVE-PREVIEW").into(),
                    title: "Live Linear intake preview".into(),
                    description: None,
                    priority: None,
                    state: "live".into(),
                    branch_name: None,
                    url: None,
                    labels: Vec::new(),
                    blocked_by: Vec::new(),
                    created_at: None,
                    updated_at: None,
                },
                None,
            )
            .await?;
        if !validation.valid {
            return Err(format!(
                "Automation profile is invalid: {}",
                validation
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity
                            == codex_orchestra_core::AutomationValidationSeverity::Error
                    })
                    .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let profile = validation
            .profile
            .ok_or("valid Automation profile is missing its canonical snapshot")?;
        self.read_linear_automation_with_profile(&profile, kind, after, first, issue_identifier)
            .await
    }

    /// Execute a prepared transition through the task-owned Automation Root
    /// Run. The durable `executing` receipt is written before provider I/O and
    /// the lease revision fences late results after pause or reconciliation.
    pub async fn transition_live_automation_issue(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        claim_id: &str,
        refreshed_state: &str,
        target_state: &str,
        gate_policy: AutomationGatePolicy,
    ) -> Result<AutomationEffectReceipt, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        let claim = root
            .claims
            .get(claim_id)
            .ok_or_else(|| format!("Automation claim `{claim_id}` was not found"))?;
        let profile = store
            .load_profile_revision(&claim.profile_digest)
            .map_err(|error| error.to_string())?;
        let (receipt, request) = store
            .prepare_tracker_transition(
                &mut root,
                claim_id,
                &profile,
                refreshed_state,
                target_state,
                gate_policy,
            )
            .map_err(|error| error.to_string())?;
        let Some(request) = request else {
            return Ok(receipt);
        };
        let execution = match linear_credential(&profile) {
            Some(credential) => {
                execute_live_linear_transition(&profile, &credential, &request).await
            }
            None => AutomationEffectExecution::Failed {
                message: "referenced Linear credential is unavailable for a live mutation".into(),
            },
        };
        store
            .complete_tracker_transition(&mut root, claim_id, &request, execution)
            .map_err(|error| error.to_string())
    }

    /// Link one normalized pull request to the current task-owned claim using
    /// the same durable two-phase effect protocol as tracker comments.
    pub async fn link_live_automation_pull_request(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        claim_id: &str,
        pull_request_url: &str,
        gate_policy: AutomationGatePolicy,
    ) -> Result<AutomationEffectReceipt, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        let claim = root
            .claims
            .get(claim_id)
            .ok_or_else(|| format!("Automation claim `{claim_id}` was not found"))?;
        let profile = store
            .load_profile_revision(&claim.profile_digest)
            .map_err(|error| error.to_string())?;
        let (receipt, request) = store
            .prepare_tracker_pull_request_link(
                &mut root,
                claim_id,
                &profile,
                pull_request_url,
                gate_policy,
            )
            .map_err(|error| error.to_string())?;
        let Some(request) = request else {
            return Ok(receipt);
        };
        let execution = match linear_credential(&profile) {
            Some(credential) => {
                execute_live_linear_pull_request_link(&profile, &credential, &request).await
            }
            None => AutomationEffectExecution::Failed {
                message: "referenced Linear credential is unavailable for a live mutation".into(),
            },
        };
        store
            .complete_tracker_effect(&mut root, claim_id, &request.idempotency_key, execution)
            .map_err(|error| error.to_string())
    }

    async fn read_linear_automation_with_profile(
        &self,
        profile: &AutomationProfile,
        kind: AutomationLinearReadKind,
        after: Option<&str>,
        first: Option<u32>,
        issue_identifier: Option<&str>,
    ) -> Result<AutomationLinearRead, String> {
        let credential = linear_credential(profile);
        let Some(credential) = credential else {
            return Ok(AutomationLinearRead {
                status: AutomationLinearReadStatus::Skipped,
                issues: Vec::new(),
                has_next_page: false,
                end_cursor: None,
                next_action: "re-resolve the referenced Linear credential, then retry".into(),
            });
        };
        validate_linear_endpoint(&profile.tracker.endpoint)?;
        let first = first.unwrap_or(25);
        if !(1..=50).contains(&first) {
            return Err("Linear page size must be between 1 and 50".into());
        }
        if after.is_some_and(|cursor| cursor.len() > 512) {
            return Err("Linear cursor exceeds the 512-byte limit".into());
        }

        let (query, variables) = match kind {
            AutomationLinearReadKind::Candidates | AutomationLinearReadKind::Terminal => (
                linear_project_issues_query(),
                json!({
                    "projectId": profile.tracker.project_slug,
                    "first": first,
                    "after": after,
                }),
            ),
            AutomationLinearReadKind::Refresh => {
                let identifier = issue_identifier
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or("Linear refresh requires an issue identifier")?;
                (linear_issue_query(), json!({"issueId": identifier}))
            }
        };
        let response = execute_linear_read(
            &profile.tracker.endpoint,
            &credential,
            profile.codex.read_timeout_ms,
            query,
            variables,
        )
        .await?;
        let (mut issues, has_next_page, end_cursor) = match kind {
            AutomationLinearReadKind::Refresh => (
                vec![normalize_linear_issue(&response).map_err(|error| error.to_string())?],
                false,
                None,
            ),
            AutomationLinearReadKind::Candidates | AutomationLinearReadKind::Terminal => {
                let page =
                    normalize_linear_issue_page(&response).map_err(|error| error.to_string())?;
                (page.issues, page.has_next_page, page.end_cursor)
            }
        };
        let selected_states = match kind {
            AutomationLinearReadKind::Candidates => Some(&profile.tracker.active_states),
            AutomationLinearReadKind::Terminal => Some(&profile.tracker.terminal_states),
            AutomationLinearReadKind::Refresh => None,
        };
        if let Some(states) = selected_states {
            issues.retain(|issue| {
                states
                    .iter()
                    .any(|state| state.eq_ignore_ascii_case(&issue.state))
            });
        }
        Ok(AutomationLinearRead {
            status: AutomationLinearReadStatus::Ready,
            issues,
            has_next_page,
            end_cursor,
            next_action: if has_next_page {
                "request the next bounded Linear page".into()
            } else {
                "live Linear read complete".into()
            },
        })
    }

    async fn run_automation_hook(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        store: &AutomationRunStore,
        root: &mut AutomationRootCheckpoint,
        claim_id: &str,
        kind: AutomationHookKind,
        command: Option<&str>,
        timeout_ms: u64,
    ) -> Result<bool, String> {
        let Some(command) = command else {
            store
                .record_hook_receipt(root, claim_id, kind, None, Err("hook is not configured"))
                .map_err(|error| error.to_string())?;
            return Ok(true);
        };
        let worktree = root.claims[claim_id].worktree.clone();
        let argv = vec!["/bin/sh".into(), "-lc".into(), command.into()];
        match self
            .host
            .run_command(
                parent_thread_id,
                repository,
                &argv,
                Some(&worktree),
                timeout_ms,
            )
            .await
        {
            Ok(outcome) => {
                let receipt = store
                    .record_hook_receipt(root, claim_id, kind, Some(command), Ok(&outcome))
                    .map_err(|error| error.to_string())?;
                Ok(receipt.status == AutomationHookStatus::Succeeded)
            }
            Err(error) => {
                store
                    .record_hook_receipt(root, claim_id, kind, Some(command), Err(&error))
                    .map_err(|storage| storage.to_string())?;
                Ok(false)
            }
        }
    }

    async fn cleanup_eligible_automation_claims(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        store: &AutomationRunStore,
        root: &mut AutomationRootCheckpoint,
    ) -> Result<(), String> {
        let eligible = root
            .claims
            .values()
            .filter(|claim| {
                matches!(
                    claim.cleanup.status,
                    AutomationCleanupStatus::Eligible | AutomationCleanupStatus::RetryPending
                )
            })
            .map(|claim| claim.claim_id.clone())
            .collect::<Vec<_>>();
        for claim_id in eligible {
            let claim = root.claims[&claim_id].clone();
            let profile = store
                .load_profile_revision(&claim.profile_digest)
                .map_err(|error| error.to_string())?;
            let hook_succeeded = self
                .run_automation_hook(
                    parent_thread_id,
                    repository,
                    store,
                    root,
                    &claim_id,
                    AutomationHookKind::BeforeRemove,
                    profile.hooks.before_remove.as_deref(),
                    profile.hooks.timeout_ms,
                )
                .await?;
            if !hook_succeeded {
                store
                    .record_cleanup_attempt(root, &claim_id, Err("before_remove hook failed"))
                    .map_err(|error| error.to_string())?;
                continue;
            }
            match self
                .host
                .remove_worktree(parent_thread_id, repository, &claim.worktree)
                .await
            {
                Ok(()) => store
                    .record_cleanup_attempt(root, &claim_id, Ok(()))
                    .map_err(|error| error.to_string())?,
                Err(error) => store
                    .record_cleanup_attempt(root, &claim_id, Err(&error))
                    .map_err(|storage| storage.to_string())?,
            }
        }
        Ok(())
    }

    pub async fn start_automation(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
    ) -> Result<AutomationRootCheckpoint, String> {
        let preview_issue = AutomationIssue {
            id: "production-start".into(),
            identifier: "PRODUCTION-START".into(),
            title: "Production Automation start".into(),
            description: None,
            priority: None,
            state: "live".into(),
            branch_name: None,
            url: None,
            labels: Vec::new(),
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        };
        let validation = self
            .validate_automation(parent_thread_id, profile_path, preview_issue, None)
            .await?;
        if !validation.valid {
            return Err(format!(
                "Automation profile is invalid: {}",
                validation
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity
                            == codex_orchestra_core::AutomationValidationSeverity::Error
                    })
                    .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let profile = validation
            .profile
            .ok_or("valid Automation profile is missing its canonical snapshot")?;
        let profile_digest = validation
            .profile_digest
            .ok_or("valid Automation profile is missing its digest")?;
        let intake = self
            .read_linear_automation_with_profile(
                &profile,
                AutomationLinearReadKind::Candidates,
                None,
                Some(50),
                None,
            )
            .await?;
        if intake.status == AutomationLinearReadStatus::Skipped {
            return Err(
                "Production Automation start requires the configured live Linear credential".into(),
            );
        }
        let repository = self.repository(parent_thread_id).await?;
        let source_revision =
            repository_revision(&repository).map_err(|error| error.to_string())?;
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: &repository,
            owner_thread_id: parent_thread_id,
            source_revision: &source_revision,
            profile: &profile,
            profile_digest: &profile_digest,
        })
        .map_err(|error| error.to_string())?;
        self.automation_shutdown.track(&repository, &root.run_id);
        if root.status == AutomationRootStatus::Suspended {
            root = self
                .reconcile_automation(parent_thread_id, &root.run_id, profile_path, true)
                .await?;
        }
        if profile_digest != root.profile_digest
            && root.profile_revision.pending_digest.as_deref() != Some(&profile_digest)
        {
            store
                .stage_profile_revision(&mut root, &profile, &profile_digest)
                .map_err(|error| error.to_string())?;
        }
        let pending = intake
            .issues
            .into_iter()
            .filter(|issue| !root.claims.values().any(|claim| claim.issue_id == issue.id))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            root.next_action = if intake.has_next_page {
                "request the next bounded Linear candidate page".into()
            } else {
                "wait for an eligible Linear issue".into()
            };
            store.save(&mut root).map_err(|error| error.to_string())?;
            return Ok(root);
        }

        let service = self.clone();
        let parent_thread_id = parent_thread_id.to_owned();
        let profile_path = profile_path.to_owned();
        tokio::spawn(async move {
            for issue in pending {
                if let Err(error) = service
                    .run_automation_fixture_inner(
                        &parent_thread_id,
                        &profile_path,
                        issue,
                        None,
                        AutomationTrackerBackend::Live,
                        None,
                    )
                    .await
                {
                    eprintln!("Production Automation issue execution failed: {error}");
                }
            }
        });
        root.next_action = "dispatch eligible live Linear issues".into();
        store.save(&mut root).map_err(|error| error.to_string())?;
        Ok(root)
    }

    pub async fn run_automation_fixture(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
        fixture_issue: AutomationIssue,
        attempt: Option<u32>,
    ) -> Result<AutomationRootCheckpoint, String> {
        self.run_automation_fixture_inner(
            parent_thread_id,
            profile_path,
            fixture_issue,
            attempt,
            AutomationTrackerBackend::Fixture,
            None,
        )
        .await
    }

    pub async fn start_automation_fixture(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
        fixture_issue: AutomationIssue,
        attempt: Option<u32>,
    ) -> Result<AutomationRootCheckpoint, String> {
        let service = self.clone();
        let parent_thread_id = parent_thread_id.to_owned();
        let profile_path = profile_path.to_owned();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let started = Arc::new(Mutex::new(Some(sender)));
        let background_started = Arc::clone(&started);
        tokio::spawn(async move {
            let result = service
                .run_automation_fixture_inner(
                    &parent_thread_id,
                    &profile_path,
                    fixture_issue,
                    attempt,
                    AutomationTrackerBackend::Fixture,
                    Some(Arc::clone(&background_started)),
                )
                .await;
            let sender = background_started
                .lock()
                .ok()
                .and_then(|mut sender| sender.take());
            if let Some(sender) = sender {
                let _ = sender.send(result);
            } else if let Err(error) = result {
                eprintln!("Automation fixture background execution failed: {error}");
            }
        });
        receiver.await.map_err(|_| {
            "Automation fixture stopped before publishing its first checkpoint".to_owned()
        })?
    }

    async fn run_automation_fixture_inner(
        &self,
        parent_thread_id: &str,
        profile_path: &str,
        fixture_issue: AutomationIssue,
        attempt: Option<u32>,
        tracker_backend: AutomationTrackerBackend,
        started: Option<AutomationStartSignal>,
    ) -> Result<AutomationRootCheckpoint, String> {
        let attempt = attempt.unwrap_or(1);
        if attempt == 0 {
            return Err("Automation attempt must be greater than zero".into());
        }
        let validation = self
            .validate_automation(
                parent_thread_id,
                profile_path,
                fixture_issue.clone(),
                Some(attempt),
            )
            .await?;
        if !validation.valid {
            return Err(format!(
                "Automation profile is invalid: {}",
                validation
                    .diagnostics
                    .iter()
                    .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let profile = validation
            .profile
            .ok_or("valid Automation profile is missing its canonical snapshot")?;
        let profile_digest = validation
            .profile_digest
            .ok_or("valid Automation profile is missing its digest")?;
        let task_prompt = validation
            .preview
            .and_then(|preview| preview.rendered_prompt)
            .ok_or("valid Automation profile is missing its rendered task prompt")?;
        let repository = self.repository(parent_thread_id).await?;
        let source_revision =
            repository_revision(&repository).map_err(|error| error.to_string())?;
        let (store, mut root) = AutomationRunStore::start(AutomationRunStart {
            repository: &repository,
            owner_thread_id: parent_thread_id,
            source_revision: &source_revision,
            profile: &profile,
            profile_digest: &profile_digest,
        })
        .map_err(|error| error.to_string())?;
        self.automation_shutdown.track(&repository, &root.run_id);
        if profile_digest != root.profile_digest
            && root.profile_revision.pending_digest.as_deref() != Some(&profile_digest)
        {
            store
                .stage_profile_revision(&mut root, &profile, &profile_digest)
                .map_err(|error| error.to_string())?;
        } else if profile_digest == root.profile_digest
            && root.profile_revision.status
                != codex_orchestra_core::AutomationProfileRevisionStatus::Active
        {
            store
                .confirm_active_profile(&mut root)
                .map_err(|error| error.to_string())?;
        }
        let existing_claim_id = root
            .claims
            .values()
            .find(|claim| claim.issue_id == fixture_issue.id)
            .map(|claim| claim.claim_id.clone());
        let dispatch_profile = if let Some(claim_id) = existing_claim_id.as_deref() {
            store
                .load_profile_revision(&root.claims[claim_id].profile_digest)
                .map_err(|error| error.to_string())?
        } else {
            profile.clone()
        };
        let tracker_terminal = dispatch_profile
            .tracker
            .terminal_states
            .iter()
            .any(|state| state.eq_ignore_ascii_case(&fixture_issue.state));
        if tracker_terminal {
            if let Some(claim_id) = existing_claim_id {
                store
                    .update_claim(&mut root, &claim_id, |claim| {
                        claim.tracker_state = fixture_issue.state.clone();
                        claim.status = AutomationClaimStatus::Cancelled;
                        claim.retry = None;
                        claim.next_action =
                            "tracker issue became terminal; stop future invocations".into();
                    })
                    .map_err(|error| error.to_string())?;
                root.next_action = root.claims[&claim_id].next_action.clone();
                store.save(&mut root).map_err(|error| error.to_string())?;
                return Ok(root);
            }
            store.cancel(&mut root).map_err(|error| error.to_string())?;
            return Err(format!(
                "fixture issue `{}` is already terminal and cannot start an Automation claim",
                fixture_issue.identifier
            ));
        }
        validate_fixture_eligibility(&dispatch_profile, &fixture_issue)?;
        let coordination = store
            .coordinate_fixture(
                &mut root,
                &profile,
                std::slice::from_ref(&fixture_issue),
                attempt,
            )
            .map_err(|error| error.to_string())?;
        let (claim_id, is_new_claim, dispatch_reason) =
            if let Some(claim_id) = coordination.dispatched_claim_ids.into_iter().next() {
                (claim_id, true, "initial")
            } else {
                let claim_id = root
                    .claims
                    .values()
                    .find(|claim| claim.issue_id == fixture_issue.id)
                    .map(|claim| claim.claim_id.clone())
                    .ok_or("fixture issue is not dispatchable under current Automation capacity")?;
                let scheduled = store
                    .dispatch_due_claim_work(&mut root, &claim_id, true, unix_epoch_millis())
                    .map_err(|error| error.to_string())?;
                let reason = match scheduled.kind {
                    AutomationRetryKind::Retry => "retry",
                    AutomationRetryKind::Continuation => "continuation",
                };
                (claim_id, false, reason)
            };

        if is_new_claim {
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.task_prompt.clone_from(&task_prompt);
                })
                .map_err(|error| error.to_string())?;
        }
        let claim = root.claims[&claim_id].clone();
        let execution_profile = store
            .load_profile_revision(&claim.profile_digest)
            .map_err(|error| error.to_string())?;
        let execution_profile_digest = claim.profile_digest.clone();
        let execution_task_prompt = if claim.task_prompt.is_empty() {
            task_prompt.clone()
        } else {
            claim.task_prompt.clone()
        };

        let (worktree, issue_handle) = if is_new_claim {
            let requested_worktree = root.claims[&claim_id].worktree.clone();
            let worktree = match self
                .host
                .create_persistent_worktree(
                    parent_thread_id,
                    &repository,
                    &requested_worktree,
                    &source_revision,
                )
                .await
            {
                Ok(path) => path,
                Err(error) => {
                    fail_automation_claim(&store, &mut root, &claim_id, &error)?;
                    return Err(error);
                }
            };
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.worktree = worktree.clone();
                    claim.next_action = "run Issue worktree setup hook".into();
                })
                .map_err(|error| error.to_string())?;

            let setup_succeeded = self
                .run_automation_hook(
                    parent_thread_id,
                    &repository,
                    &store,
                    &mut root,
                    &claim_id,
                    AutomationHookKind::AfterCreate,
                    execution_profile.hooks.after_create.as_deref(),
                    execution_profile.hooks.timeout_ms,
                )
                .await?;
            if !setup_succeeded {
                fail_automation_claim(&store, &mut root, &claim_id, "after_create hook failed")?;
                store
                    .update_claim(&mut root, &claim_id, |claim| {
                        claim.cleanup.status = AutomationCleanupStatus::Eligible;
                        claim.next_action = "remove worktree after setup hook failure".into();
                    })
                    .map_err(|error| error.to_string())?;
                self.cleanup_eligible_automation_claims(
                    parent_thread_id,
                    &repository,
                    &store,
                    &mut root,
                )
                .await?;
                return Err("after_create hook failed".into());
            }

            let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
            let thread_id =
                ThreadId::from_string(parent_thread_id).map_err(|error| error.to_string())?;
            let thread = manager
                .get_thread(thread_id)
                .await
                .map_err(|error| error.to_string())?;
            let config = thread.config_snapshot().await;
            let issue_json =
                serde_json::to_string_pretty(&fixture_issue).map_err(|error| error.to_string())?;
            let bootstrap_prompt = format!(
                "You are the persistent Issue task for `{}`. The native Orchestra runtime will execute the selected typed Workflow under this task after this initialization turn. Retain the issue context below and return exactly {{\"ready\":true}}.\n\n{}\n\nIssue snapshot:\n{}",
                fixture_issue.identifier, execution_task_prompt, issue_json
            );
            let issue_handle = match self
                .host
                .spawn(SpawnRequest {
                    parent_thread_id: parent_thread_id.into(),
                    task_name: format!("automation_{}", safe_task_name(&fixture_issue.identifier)),
                    prompt: bootstrap_prompt,
                    skill_context: String::new(),
                    cwd: worktree.clone(),
                    model: config.model.clone(),
                    reasoning_effort: config
                        .reasoning_effort
                        .clone()
                        .map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
                        .and_then(|value| value.as_str().map(str::to_owned)),
                    service_tier: config.service_tier.clone(),
                    fork_turns: ForkTurns::None,
                    allow_delegation: true,
                    minimum_descendant_depth: 1,
                })
                .await
            {
                Ok(handle) => handle,
                Err(error) => {
                    fail_automation_claim(&store, &mut root, &claim_id, &error)?;
                    return Err(error);
                }
            };
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.issue_task = Some(issue_handle.clone());
                    claim.next_action = "wait for Issue task initialization".into();
                })
                .map_err(|error| error.to_string())?;
            let initialized = self.host.wait(&issue_handle).await?;
            if !matches!(initialized.status, AgentStatus::Completed) {
                let error = format!(
                    "Issue task initialization did not complete: {:?}",
                    initialized.status
                );
                fail_automation_claim(&store, &mut root, &claim_id, &error)?;
                let _ = self.host.cancel(&issue_handle).await;
                return Err(error);
            }
            if initialized
                .final_response
                .as_deref()
                .and_then(|response| serde_json::from_str::<Value>(response).ok())
                != Some(json!({"ready": true}))
            {
                let error = "Issue task initialization returned an invalid readiness result";
                fail_automation_claim(&store, &mut root, &claim_id, error)?;
                return Err(error.into());
            }
            (worktree, issue_handle)
        } else {
            let claim = &root.claims[&claim_id];
            let issue_handle = claim
                .issue_task
                .clone()
                .ok_or("retained Automation claim is missing its native Issue task")?;
            (claim.worktree.clone(), issue_handle)
        };

        let workflow_source = std::fs::read_to_string(&execution_profile.orchestra.workflow_path)
            .map_err(|error| error.to_string())?;
        if automation_source_sha256(&workflow_source) != execution_profile.orchestra.workflow_sha256
        {
            store
                .schedule_claim_retry(
                    &mut root,
                    &claim_id,
                    &execution_profile,
                    unix_epoch_millis(),
                    "pinned Automation workflow source changed",
                )
                .map_err(|error| error.to_string())?;
            root.next_action = format!(
                "claim `{claim_id}` is recoverable after restoring its pinned workflow source"
            );
            store.save(&mut root).map_err(|error| error.to_string())?;
            return Ok(root);
        }
        let plan = compile_workflow(&workflow_source).map_err(|error| error.to_string())?;
        let pre_run_succeeded = self
            .run_automation_hook(
                parent_thread_id,
                &repository,
                &store,
                &mut root,
                &claim_id,
                AutomationHookKind::BeforeRun,
                execution_profile.hooks.before_run.as_deref(),
                execution_profile.hooks.timeout_ms,
            )
            .await?;
        if !pre_run_succeeded {
            store
                .schedule_claim_retry(
                    &mut root,
                    &claim_id,
                    &execution_profile,
                    unix_epoch_millis(),
                    "before_run hook failed",
                )
                .map_err(|error| error.to_string())?;
            root.next_action = format!("claim `{claim_id}` is waiting for hook retry");
            store.save(&mut root).map_err(|error| error.to_string())?;
            return Ok(root);
        }
        let fixture_tracker_state = fixture_issue.state.clone();
        let normalized_inputs = json!({
            "issue": fixture_issue,
            "task_prompt": execution_task_prompt,
            "automation": {
                "profileDigest": execution_profile_digest,
                "claimId": claim_id,
                "attempt": root.claims[&claim_id].workflow_invocations.saturating_add(1),
                "reason": dispatch_reason,
            },
        });
        store
            .update_claim(&mut root, &claim_id, |claim| {
                claim.status = AutomationClaimStatus::Running;
                claim.next_action = "execute selected typed Workflow in Issue task".into();
            })
            .map_err(|error| error.to_string())?;
        let observed_store = AutomationRunStore::open(&repository, &root.run_id)
            .map_err(|error| error.to_string())?;
        let observed_claim = claim_id.clone();
        let observed_started = started.clone();
        let outcome = self
            .runtime
            .run_with_inputs_observed(
                &worktree,
                &issue_handle.thread_id,
                plan,
                Some(&normalized_inputs),
                move |checkpoint| {
                    let mut automation =
                        observed_store.load().map_err(|error| error.to_string())?;
                    observed_store
                        .update_claim(&mut automation, &observed_claim, |claim| {
                            claim.workflow_run_id = Some(checkpoint.run_id.clone());
                            claim.workflow_status = Some(checkpoint.status.clone());
                            claim.next_action = "observe Workflow checkpoint".into();
                        })
                        .map_err(|error| error.to_string())?;
                    if let Some(started) = observed_started.as_ref() {
                        let sender = started.lock().ok().and_then(|mut sender| sender.take());
                        if let Some(sender) = sender {
                            let _ = sender.send(Ok(automation.clone()));
                        }
                    }
                    Ok(())
                },
            )
            .await;
        // The observer persists each typed Workflow checkpoint and advances the
        // Automation lease revision. Continue from that authoritative snapshot
        // so post-run hooks and reconciliation do not write through a stale fence.
        root = store.load().map_err(|error| error.to_string())?;
        if root.status != AutomationRootStatus::Running
            || root.claims.get(&claim_id).is_some_and(|claim| {
                matches!(
                    claim.status,
                    AutomationClaimStatus::Suspended | AutomationClaimStatus::Cancelled
                )
            })
        {
            return Ok(root);
        }
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let post_run_succeeded = self
                    .run_automation_hook(
                        parent_thread_id,
                        &repository,
                        &store,
                        &mut root,
                        &claim_id,
                        AutomationHookKind::AfterRun,
                        execution_profile.hooks.after_run.as_deref(),
                        execution_profile.hooks.timeout_ms,
                    )
                    .await?;
                let reason = if post_run_succeeded {
                    error.to_string()
                } else {
                    format!("{error}; after_run hook failed")
                };
                store
                    .schedule_claim_retry(
                        &mut root,
                        &claim_id,
                        &execution_profile,
                        unix_epoch_millis(),
                        &reason,
                    )
                    .map_err(|storage| storage.to_string())?;
                root.next_action = format!("claim `{claim_id}` is waiting for retry");
                store
                    .save(&mut root)
                    .map_err(|storage| storage.to_string())?;
                return Ok(root);
            }
        };
        let workflow = outcome_checkpoint(&outcome);
        self.persist_lifecycle(
            &issue_handle.thread_id,
            workflow,
            OrchestraLifecycleKind::Invoked,
        )
        .await?;
        let post_run_succeeded = self
            .run_automation_hook(
                parent_thread_id,
                &repository,
                &store,
                &mut root,
                &claim_id,
                AutomationHookKind::AfterRun,
                execution_profile.hooks.after_run.as_deref(),
                execution_profile.hooks.timeout_ms,
            )
            .await?;
        if !post_run_succeeded {
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.workflow_run_id = Some(workflow.run_id.clone());
                    claim.workflow_status = Some(workflow.status.clone());
                })
                .map_err(|error| error.to_string())?;
            store
                .schedule_claim_retry(
                    &mut root,
                    &claim_id,
                    &execution_profile,
                    unix_epoch_millis(),
                    "after_run hook failed",
                )
                .map_err(|error| error.to_string())?;
            root.next_action = format!("claim `{claim_id}` is waiting for hook retry");
            store.save(&mut root).map_err(|error| error.to_string())?;
            return Ok(root);
        }
        let mut effect_statuses = Vec::new();
        if matches!(&outcome, RunOutcome::Completed(_)) {
            for effect in &execution_profile.orchestra.effects {
                let receipt = match tracker_backend {
                    AutomationTrackerBackend::Fixture => match effect {
                        AutomationEffect::TrackerComment => {
                            let body = extract_tracker_comment(workflow)?;
                            store.resolve_tracker_comment(
                                &mut root,
                                &claim_id,
                                &execution_profile,
                                &body,
                                AutomationGatePolicy::AutoAccept,
                                |request| execute_fixture_tracker_comment(&repository, request),
                            )
                        }
                        AutomationEffect::TrackerTransition => {
                            let target_state = extract_tracker_transition(workflow)?;
                            store.resolve_tracker_transition(
                                &mut root,
                                &claim_id,
                                &execution_profile,
                                &fixture_tracker_state,
                                &target_state,
                                AutomationGatePolicy::AutoAccept,
                                |request| execute_fixture_tracker_transition(&repository, request),
                            )
                        }
                        AutomationEffect::TrackerLinkPullRequest => {
                            let pull_request_url = extract_tracker_pull_request(workflow)?;
                            store.resolve_tracker_pull_request_link(
                                &mut root,
                                &claim_id,
                                &execution_profile,
                                &pull_request_url,
                                AutomationGatePolicy::AutoAccept,
                                |request| {
                                    execute_fixture_tracker_pull_request(&repository, request)
                                },
                            )
                        }
                    }
                    .map_err(|error| error.to_string())?,
                    AutomationTrackerBackend::Live => {
                        resolve_live_tracker_effect(
                            &store,
                            &mut root,
                            &claim_id,
                            &execution_profile,
                            &fixture_tracker_state,
                            effect,
                            workflow,
                        )
                        .await?
                    }
                };
                effect_statuses.push(receipt.status);
            }
        }
        store
            .update_claim(&mut root, &claim_id, |claim| {
                claim.workflow_run_id = Some(workflow.run_id.clone());
                claim.workflow_status = Some(workflow.status.clone());
            })
            .map_err(|error| error.to_string())?;
        if effect_statuses.contains(&AutomationEffectStatus::WaitingGate) {
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.status = AutomationClaimStatus::Suspended;
                    claim.next_action = "wait for the native Tracker effect gate".into();
                })
                .map_err(|error| error.to_string())?;
        } else if effect_statuses.contains(&AutomationEffectStatus::Rejected) {
            fail_automation_claim(
                &store,
                &mut root,
                &claim_id,
                "Tracker effect was rejected by policy",
            )?;
        } else if effect_statuses.contains(&AutomationEffectStatus::Failed)
            || matches!(&outcome, RunOutcome::Failed(_))
        {
            store
                .schedule_claim_retry(
                    &mut root,
                    &claim_id,
                    &execution_profile,
                    unix_epoch_millis(),
                    "Workflow or Tracker effect failed",
                )
                .map_err(|error| error.to_string())?;
        } else if effect_statuses.iter().any(|status| {
            matches!(
                status,
                AutomationEffectStatus::Ambiguous | AutomationEffectStatus::Executing
            )
        }) {
            store
                .update_claim(&mut root, &claim_id, |claim| {
                    claim.status = AutomationClaimStatus::Suspended;
                    claim.next_action = "reconcile ambiguous Tracker effect before retry".into();
                })
                .map_err(|error| error.to_string())?;
        } else {
            match &outcome {
                RunOutcome::Completed(_) => {
                    let tracker_issue_active = !execution_profile
                        .tracker
                        .terminal_states
                        .iter()
                        .any(|state| {
                            state.eq_ignore_ascii_case(&root.claims[&claim_id].tracker_state)
                        });
                    store
                        .record_completed_invocation(
                            &mut root,
                            &claim_id,
                            &execution_profile,
                            tracker_issue_active,
                            unix_epoch_millis(),
                        )
                        .map_err(|error| error.to_string())?;
                }
                RunOutcome::Paused(_) => {
                    store
                        .update_claim(&mut root, &claim_id, |claim| {
                            claim.status = AutomationClaimStatus::Suspended;
                            claim.next_action = "resume Workflow from checkpoint".into();
                        })
                        .map_err(|error| error.to_string())?;
                }
                RunOutcome::Cancelled(_) => {
                    store
                        .update_claim(&mut root, &claim_id, |claim| {
                            claim.status = AutomationClaimStatus::Cancelled;
                            claim.retry = None;
                            claim.next_action = "claim cancelled".into();
                        })
                        .map_err(|error| error.to_string())?;
                }
                RunOutcome::Failed(_) => unreachable!("failed outcomes schedule a retry"),
            }
        }
        root.next_action = root.claims[&claim_id].next_action.clone();
        store.save(&mut root).map_err(|error| error.to_string())?;
        Ok(root)
    }

    pub async fn cancel_automation(
        &self,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<AutomationRootCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        if root.owner_thread_id != parent_thread_id {
            return Err("Automation Root Run does not belong to the requested task".into());
        }
        for claim in root.claims.values() {
            if let Some(workflow_run_id) = claim.workflow_run_id.as_deref() {
                let checkpoint = self
                    .runtime
                    .cancel(&claim.worktree, workflow_run_id)
                    .await
                    .map_err(|error| error.to_string())?;
                if let Some(issue_task) = claim.issue_task.as_ref() {
                    self.persist_lifecycle(
                        &issue_task.thread_id,
                        &checkpoint,
                        OrchestraLifecycleKind::Cancelled,
                    )
                    .await?;
                }
            }
            if let Some(issue_task) = claim.issue_task.as_ref() {
                let _ = self.host.cancel(issue_task).await;
            }
        }
        store.cancel(&mut root).map_err(|error| error.to_string())?;
        self.automation_shutdown.remove(&repository, run_id);
        Ok(root)
    }

    pub async fn cancel_automation_issue(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        claim_id: &str,
    ) -> Result<AutomationRootCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        store
            .begin_claim_cancellation(&mut root, claim_id)
            .map_err(|error| error.to_string())?;
        let service = self.clone();
        let repository = repository.clone();
        let parent_thread_id = parent_thread_id.to_owned();
        let run_id = run_id.to_owned();
        let claim_id = claim_id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = service
                .finish_automation_issue_cancellation(
                    &parent_thread_id,
                    &repository,
                    &run_id,
                    &claim_id,
                )
                .await
            {
                eprintln!("Automation issue cancellation failed: {error}");
            }
        });
        Ok(root)
    }

    pub async fn steer_automation_issue(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        claim_id: &str,
        input: &str,
    ) -> Result<(AutomationRootCheckpoint, AutomationSteeringReceipt), String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        let issue_task = root
            .claims
            .get(claim_id)
            .and_then(|claim| claim.issue_task.clone())
            .ok_or_else(|| format!("Automation claim `{claim_id}` has no native Issue task"))?;
        let submitted = store
            .prepare_issue_steering(
                &mut root,
                claim_id,
                parent_thread_id,
                input,
                unix_epoch_millis(),
            )
            .map_err(|error| error.to_string())?;
        let delivery = self.host.send_input(&issue_task, input.trim().into()).await;
        root = store.load().map_err(|error| error.to_string())?;
        let receipt = match delivery {
            Ok(provider_receipt) => store.complete_issue_steering(
                &mut root,
                claim_id,
                submitted.sequence,
                Ok(&provider_receipt),
            ),
            Err(error) => {
                store.complete_issue_steering(&mut root, claim_id, submitted.sequence, Err(&error))
            }
        }
        .map_err(|error| error.to_string())?;
        Ok((root, receipt))
    }

    async fn finish_automation_issue_cancellation(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        run_id: &str,
        claim_id: &str,
    ) -> Result<(), String> {
        let store =
            AutomationRunStore::open(repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        let claim = root
            .claims
            .get(claim_id)
            .cloned()
            .ok_or_else(|| format!("Automation claim `{claim_id}` was not found"))?;
        let mut descendants_cancelled = true;
        if let Some(issue_task) = claim.issue_task.as_ref()
            && self.host.cancel(issue_task).await.is_err()
        {
            descendants_cancelled = false;
        }
        if let Some(workflow_run_id) = claim.workflow_run_id.as_deref() {
            match self.runtime.cancel(&claim.worktree, workflow_run_id).await {
                Ok(checkpoint) => {
                    if let Some(issue_task) = claim.issue_task.as_ref()
                        && self
                            .persist_lifecycle(
                                &issue_task.thread_id,
                                &checkpoint,
                                OrchestraLifecycleKind::Cancelled,
                            )
                            .await
                            .is_err()
                    {
                        descendants_cancelled = false;
                    }
                }
                Err(_) => descendants_cancelled = false,
            }
        }
        root = store.load().map_err(|error| error.to_string())?;
        store
            .complete_claim_cancellation(&mut root, claim_id, descendants_cancelled)
            .map_err(|error| error.to_string())?;
        self.cleanup_eligible_automation_claims(parent_thread_id, &repository, &store, &mut root)
            .await?;
        Ok(())
    }

    pub async fn automation_status(
        &self,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<AutomationRootCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        self.automation_shutdown.track(&repository, run_id);
        let profile = store.load_profile().map_err(|error| error.to_string())?;
        project_claim_liveness(&mut root, &profile, unix_epoch_millis());
        Ok(root)
    }

    pub async fn pause_automation(
        &self,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<AutomationRootCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        self.automation_shutdown.track(&repository, run_id);
        if root.status == AutomationRootStatus::Running {
            store
                .pause(&mut root, "explicit native pause")
                .map_err(|error| error.to_string())?;
        }
        let claims = root.claims.values().cloned().collect::<Vec<_>>();
        for claim in claims {
            let Some(workflow_run_id) = claim.workflow_run_id.as_deref() else {
                continue;
            };
            let checkpoint = self
                .runtime
                .pause(&claim.worktree, workflow_run_id)
                .await
                .map_err(|error| error.to_string())?;
            store
                .update_claim(&mut root, &claim.claim_id, |stored| {
                    stored.workflow_status = Some(checkpoint.status.clone());
                    stored.next_action = "reconcile the paused native Child Run".into();
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(root)
    }

    pub async fn reconcile_automation(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        profile_path: &str,
        resume: bool,
    ) -> Result<AutomationRootCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        authorize_automation_root(&root, parent_thread_id)?;
        self.automation_shutdown.track(&repository, run_id);
        let pinned_profile = store.load_profile().map_err(|error| error.to_string())?;
        let fixture = root
            .claims
            .values()
            .next()
            .map(|claim| AutomationIssue {
                id: claim.issue_id.clone(),
                identifier: claim.issue_identifier.clone(),
                title: claim.issue_title.clone(),
                description: None,
                priority: claim.priority,
                state: claim.tracker_state.clone(),
                branch_name: None,
                url: None,
                labels: pinned_profile.tracker.required_labels.clone(),
                blocked_by: Vec::new(),
                created_at: None,
                updated_at: None,
            })
            .unwrap_or_else(|| AutomationIssue {
                id: "reconcile-profile".into(),
                identifier: "RECONCILE-PROFILE".into(),
                title: "Reconcile Automation profile".into(),
                description: None,
                priority: None,
                state: pinned_profile
                    .tracker
                    .active_states
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "active".into()),
                branch_name: None,
                url: None,
                labels: pinned_profile.tracker.required_labels.clone(),
                blocked_by: Vec::new(),
                created_at: None,
                updated_at: None,
            });
        let validation = self
            .validate_automation(parent_thread_id, profile_path, fixture, None)
            .await?;
        if !validation.valid {
            let diagnostics = validation
                .diagnostics
                .iter()
                .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
                .collect::<Vec<_>>();
            store
                .reject_profile_revision(
                    &mut root,
                    validation.profile_digest.as_deref(),
                    &diagnostics,
                )
                .map_err(|error| error.to_string())?;
            return Ok(root);
        }
        let candidate_profile = validation
            .profile
            .ok_or("valid Automation profile is missing its canonical snapshot")?;
        let candidate_digest = validation
            .profile_digest
            .ok_or("valid Automation profile is missing its digest")?;
        if candidate_digest != root.profile_digest
            && root.profile_revision.pending_digest.as_deref() != Some(&candidate_digest)
        {
            if let Err(error) =
                store.stage_profile_revision(&mut root, &candidate_profile, &candidate_digest)
            {
                if matches!(
                    error,
                    codex_orchestra_core::AutomationRunError::ProfileProjectMismatch
                ) {
                    store
                        .reject_profile_revision(
                            &mut root,
                            Some(&candidate_digest),
                            &["tracker.projectSlug: cannot change for a resident Root Run".into()],
                        )
                        .map_err(|storage| storage.to_string())?;
                    return Ok(root);
                }
                return Err(error.to_string());
            }
        } else if candidate_digest == root.profile_digest
            && root.profile_revision.status
                != codex_orchestra_core::AutomationProfileRevisionStatus::Active
        {
            store
                .confirm_active_profile(&mut root)
                .map_err(|error| error.to_string())?;
        }

        if root.status == AutomationRootStatus::Running {
            store
                .pause(&mut root, "native reconciliation refresh")
                .map_err(|error| error.to_string())?;
        }
        store
            .begin_reconciliation(&mut root)
            .map_err(|error| error.to_string())?;

        let mut tracker_issues = Vec::new();
        for claim in root.claims.values() {
            let claim_profile = store
                .load_profile_revision(&claim.profile_digest)
                .map_err(|error| error.to_string())?;
            let read = self
                .read_linear_automation_with_profile(
                    &claim_profile,
                    AutomationLinearReadKind::Refresh,
                    None,
                    Some(1),
                    Some(&claim.issue_identifier),
                )
                .await?;
            if read.status == AutomationLinearReadStatus::Ready {
                tracker_issues.extend(read.issues);
            }
        }

        let mut observations = Vec::new();
        for claim in root.claims.values() {
            let claim_profile = store
                .load_profile_revision(&claim.profile_digest)
                .map_err(|error| error.to_string())?;
            let issue_task_active = match claim.issue_task.as_ref() {
                Some(handle) => self.host.status(handle).await.is_ok_and(|status| {
                    !matches!(status, AgentStatus::Cancelled | AgentStatus::Failed(_))
                }),
                None => false,
            };
            let mut workflow_status = match claim.workflow_run_id.as_deref() {
                Some(workflow_run_id) => self
                    .runtime
                    .status(&claim.worktree, workflow_run_id)
                    .await
                    .ok()
                    .map(|checkpoint| checkpoint.status),
                None => None,
            };
            let terminal = tracker_issues.iter().any(|issue| {
                issue.id == claim.issue_id
                    && claim_profile
                        .tracker
                        .terminal_states
                        .iter()
                        .any(|state| state.eq_ignore_ascii_case(&issue.state))
            });
            let mut descendants_cancelled = false;
            if terminal {
                if let Some(workflow_run_id) = claim.workflow_run_id.as_deref() {
                    let checkpoint = self
                        .runtime
                        .cancel(&claim.worktree, workflow_run_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    workflow_status = Some(checkpoint.status);
                }
                if let Some(handle) = claim.issue_task.as_ref() {
                    let _ = self.host.cancel(handle).await;
                }
                descendants_cancelled = true;
            }
            observations.push(AutomationClaimReconciliation {
                claim_id: claim.claim_id.clone(),
                issue_task_active,
                descendants_cancelled,
                tracker_terminal: terminal,
                workflow_status,
            });
        }
        if let Err(error) =
            store.reconcile(&mut root, &pinned_profile, &tracker_issues, &observations)
        {
            if !matches!(
                error,
                codex_orchestra_core::AutomationRunError::ReconciliationBlocked(_)
            ) {
                return Err(error.to_string());
            }
            return store.load().map_err(|error| error.to_string());
        }
        self.cleanup_eligible_automation_claims(parent_thread_id, &repository, &store, &mut root)
            .await?;
        if !resume {
            return Ok(root);
        }

        let resumable = root.claims.values().cloned().collect::<Vec<_>>();
        for claim in resumable {
            if claim.workflow_status != Some(RunStatus::WaitingApproval) {
                continue;
            }
            let Some(workflow_run_id) = claim.workflow_run_id.as_deref() else {
                continue;
            };
            let outcome = self
                .runtime
                .resume(&claim.worktree, workflow_run_id)
                .await
                .map_err(|error| error.to_string())?;
            let workflow = outcome_checkpoint(&outcome);
            store
                .update_claim(&mut root, &claim.claim_id, |stored| {
                    stored.workflow_status = Some(workflow.status.clone());
                    stored.status = match &outcome {
                        RunOutcome::Completed(_) => AutomationClaimStatus::Completed,
                        RunOutcome::Paused(_) => AutomationClaimStatus::Suspended,
                        RunOutcome::Failed(_) => AutomationClaimStatus::Failed,
                        RunOutcome::Cancelled(_) => AutomationClaimStatus::Cancelled,
                    };
                    stored.next_action = "native Child Run resumed from retained checkpoint".into();
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(root)
    }

    pub async fn read_automation_queue(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        category: AutomationQueueCategory,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<AutomationQueuePage, String> {
        let repository = self.repository(parent_thread_id).await?;
        let store =
            AutomationRunStore::open(&repository, run_id).map_err(|error| error.to_string())?;
        let mut root = store.load().map_err(|error| error.to_string())?;
        if root.owner_thread_id != parent_thread_id {
            return Err("Automation Root Run does not belong to the requested task".into());
        }
        let profile = store.load_profile().map_err(|error| error.to_string())?;
        project_claim_liveness(&mut root, &profile, unix_epoch_millis());
        Ok(store.queue_page(
            &root,
            category,
            offset.unwrap_or_default(),
            limit.unwrap_or(25),
        ))
    }

    pub async fn run(
        &self,
        parent_thread_id: &str,
        workflow_path: &str,
        inputs: Option<&Value>,
    ) -> Result<RunOutcome, String> {
        let repository = self.repository(parent_thread_id).await?;
        reject_existing_root_run(&repository, parent_thread_id)?;
        let plan = self.validate(parent_thread_id, workflow_path).await?;
        let outcome = self
            .runtime
            .run_with_inputs(&repository, parent_thread_id, plan, inputs)
            .await
            .map_err(|error| error.to_string())?;
        self.persist_lifecycle(
            parent_thread_id,
            outcome_checkpoint(&outcome),
            OrchestraLifecycleKind::Invoked,
        )
        .await?;
        Ok(outcome)
    }

    pub async fn resume(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        approval_decision: Option<&str>,
        inputs: Option<&Value>,
    ) -> Result<RunOutcome, String> {
        let repository = self.repository(parent_thread_id).await?;
        self.status(parent_thread_id, run_id).await?;
        let outcome = self
            .runtime
            .resume_with_approval_and_inputs(&repository, run_id, approval_decision, inputs)
            .await
            .map_err(|error| error.to_string())?;
        self.persist_lifecycle(
            parent_thread_id,
            outcome_checkpoint(&outcome),
            OrchestraLifecycleKind::Resumed,
        )
        .await?;
        Ok(outcome)
    }

    pub async fn status(
        &self,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<RunCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        let checkpoint = self
            .runtime
            .status(&repository, run_id)
            .await
            .map_err(|error| error.to_string())?;
        if checkpoint.parent_thread_id != parent_thread_id {
            return Err("run does not belong to the requested task".into());
        }
        Ok(checkpoint)
    }

    pub async fn cancel(
        &self,
        parent_thread_id: &str,
        run_id: &str,
    ) -> Result<RunCheckpoint, String> {
        let repository = self.repository(parent_thread_id).await?;
        self.status(parent_thread_id, run_id).await?;
        let checkpoint = self
            .runtime
            .cancel(&repository, run_id)
            .await
            .map_err(|error| error.to_string())?;
        self.persist_lifecycle(
            parent_thread_id,
            &checkpoint,
            OrchestraLifecycleKind::Cancelled,
        )
        .await?;
        Ok(checkpoint)
    }

    pub async fn query(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        selector: ExecutionSelector,
        budget: ExecutionQueryBudget,
    ) -> Result<ExecutionQueryResult, String> {
        let repository = self.repository(parent_thread_id).await?;
        self.queries
            .query(&repository, parent_thread_id, run_id, selector, budget)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn digest(
        &self,
        parent_thread_id: &str,
        run_id: &str,
        max_bytes: usize,
    ) -> Result<RunDigest, String> {
        let repository = self.repository(parent_thread_id).await?;
        self.queries
            .digest(&repository, parent_thread_id, run_id, max_bytes)
            .map_err(|error| error.to_string())
    }

    /// Return the bounded digest for this task's active root run, if one exists.
    ///
    /// The task-local Codex projection selects the run; the shared query service
    /// still owns checkpoint authorization, canonical hashing, prioritization,
    /// and byte limits.
    pub async fn active_run_digest(
        &self,
        parent_thread_id: &str,
        max_bytes: usize,
    ) -> Result<Option<RunDigest>, String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id =
            ThreadId::from_string(parent_thread_id).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        let Some(state_db) = thread.state_db() else {
            return Ok(None);
        };
        let Some(snapshot) = state_db
            .orchestra_task_snapshot(thread_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let projection = snapshot.projection.projection;
        if projection.parent_thread_id != parent_thread_id
            || !is_nonterminal_rollout_status(projection.status)
        {
            return Ok(None);
        }
        self.digest(parent_thread_id, &projection.run_id, max_bytes)
            .await
            .map(Some)
    }

    async fn repository(&self, parent_thread_id: &str) -> Result<PathBuf, String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id =
            ThreadId::from_string(parent_thread_id).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(thread.config_snapshot().await.cwd().as_path().to_path_buf())
    }

    async fn persist_lifecycle(
        &self,
        parent_thread_id: &str,
        checkpoint: &RunCheckpoint,
        kind: OrchestraLifecycleKind,
    ) -> Result<(), String> {
        let manager = self.manager.upgrade().ok_or("thread manager dropped")?;
        let thread_id =
            ThreadId::from_string(parent_thread_id).map_err(|error| error.to_string())?;
        let thread = manager
            .get_thread(thread_id)
            .await
            .map_err(|error| error.to_string())?;
        let state_db = thread.state_db();
        let mut previous = if let Some(state_db) = state_db.as_ref() {
            state_db
                .orchestra_task_snapshot(thread_id)
                .await
                .map_err(|error| error.to_string())?
                .map(|snapshot| snapshot.projection)
        } else {
            None
        };
        if previous.is_none() {
            let history = thread
                .load_history(/*include_archived*/ true)
                .await
                .map_err(|error| error.to_string())?;
            let mut recovered = history
                .items
                .iter()
                .filter_map(|item| match item {
                    RolloutItem::Orchestra(event) => Some(event),
                    _ => None,
                })
                .max_by_key(|event| event.sequence)
                .cloned();
            if let (Some(state_db), Some(event)) = (state_db.as_ref(), recovered.as_ref()) {
                state_db
                    .apply_orchestra_event(thread_id, event)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            previous = recovered.take();
        }
        let sequence = previous
            .as_ref()
            .map_or(1, |event| event.sequence.saturating_add(1));
        let revision = previous
            .as_ref()
            .filter(|event| event.run_id == checkpoint.run_id)
            .map_or(1, |event| event.revision.saturating_add(1));
        let event = OrchestraRolloutItem {
            schema_version: 1,
            event_id: format!("{}:{revision}", checkpoint.run_id),
            run_id: checkpoint.run_id.clone(),
            sequence,
            revision,
            kind,
            projection: project_checkpoint(checkpoint),
        };
        // Canonical JSONL wins the barrier before the rebuildable SQLite view.
        thread
            .append_rollout_items(&[RolloutItem::Orchestra(event.clone())])
            .await
            .map_err(|error| error.to_string())?;
        if let Some(state_db) = state_db {
            state_db
                .apply_orchestra_event(thread_id, &event)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn outcome_checkpoint(outcome: &RunOutcome) -> &RunCheckpoint {
    match outcome {
        RunOutcome::Completed(checkpoint)
        | RunOutcome::Paused(checkpoint)
        | RunOutcome::Failed(checkpoint)
        | RunOutcome::Cancelled(checkpoint) => checkpoint,
    }
}

fn validate_linear_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|error| error.to_string())?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.linear.app")
        || url.path() != "/graphql"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("live Linear endpoint must be exactly `https://api.linear.app/graphql`".into());
    }
    Ok(())
}

fn linear_credential(profile: &AutomationProfile) -> Option<String> {
    match profile.tracker.credential.kind {
        AutomationSecretKind::Environment => std::env::var(&profile.tracker.credential.reference)
            .ok()
            .filter(|value| !value.trim().is_empty()),
        AutomationSecretKind::InlineDigest => None,
    }
}

fn linear_project_issues_query() -> String {
    format!(
        "query OrchestraProjectIssues($projectId: String!, $first: Int!, $after: String) {{ project(id: $projectId) {{ issues(first: $first, after: $after, orderBy: updatedAt) {{ nodes {{ {LINEAR_ISSUE_FIELDS} }} pageInfo {{ hasNextPage endCursor }} }} }} }}"
    )
}

fn linear_issue_query() -> String {
    format!(
        "query OrchestraIssueRefresh($issueId: String!) {{ issue(id: $issueId) {{ {LINEAR_ISSUE_FIELDS} }} }}"
    )
}

fn linear_mutation_context_query() -> &'static str {
    "query OrchestraIssueMutationContext($issueId: String!, $projectId: String!) { issue(id: $issueId) { id state { name } project { id } team { states(first: 50) { nodes { id name } } } attachments(first: 50) { nodes { id url } } } project(id: $projectId) { id } }"
}

fn linear_transition_mutation() -> &'static str {
    "mutation OrchestraTransitionIssue($issueId: String!, $stateId: String!) { issueUpdate(id: $issueId, input: { stateId: $stateId }) { success issue { id state { name } } } }"
}

fn linear_comment_mutation() -> &'static str {
    "mutation OrchestraCommentIssue($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success comment { id body } } }"
}

fn linear_pull_request_mutation() -> &'static str {
    "mutation OrchestraLinkPullRequest($issueId: String!, $url: String!) { attachmentCreate(input: { issueId: $issueId, url: $url, title: \"Pull request\" }) { success attachment { id url } } }"
}

#[derive(Debug, Eq, PartialEq)]
enum LinearTransitionDecision {
    AlreadyApplied,
    Apply { state_id: String },
}

fn linear_transition_decision(
    profile: &AutomationProfile,
    request: &AutomationTrackerTransitionRequest,
    context: &Value,
) -> Result<LinearTransitionDecision, String> {
    let current_state = context
        .pointer("/data/issue/state/name")
        .and_then(Value::as_str)
        .ok_or("refreshed Linear Issue has no workflow state")?;
    if current_state.eq_ignore_ascii_case(&request.target_state) {
        return Ok(LinearTransitionDecision::AlreadyApplied);
    }
    if profile
        .tracker
        .terminal_states
        .iter()
        .any(|state| state.eq_ignore_ascii_case(current_state))
    {
        return Err("refreshed Linear Issue became terminal before the transition".into());
    }
    if !current_state.eq_ignore_ascii_case(&request.expected_state) {
        return Err(format!(
            "Linear Issue state changed from `{}` to `{current_state}` before the transition",
            request.expected_state
        ));
    }
    let state_id = context
        .pointer("/data/issue/team/states/nodes")
        .and_then(Value::as_array)
        .and_then(|states| {
            states.iter().find(|state| {
                state
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case(&request.target_state))
            })
        })
        .and_then(|state| state.get("id"))
        .and_then(Value::as_str)
        .ok_or("transition target is not a workflow state for the refreshed Issue team")?;
    Ok(LinearTransitionDecision::Apply {
        state_id: state_id.into(),
    })
}

async fn execute_live_linear_transition(
    profile: &AutomationProfile,
    credential: &str,
    request: &AutomationTrackerTransitionRequest,
) -> AutomationEffectExecution {
    let context = match linear_mutation_context(profile, credential, &request.issue_id).await {
        Ok(context) => context,
        Err(message) => return AutomationEffectExecution::Failed { message },
    };
    let state_id = match linear_transition_decision(profile, request, &context) {
        Ok(LinearTransitionDecision::AlreadyApplied) => {
            return AutomationEffectExecution::Committed {
                provider_receipt: format!(
                    "linear-transition-already:{}:{}",
                    request.issue_id, request.target_state
                ),
            };
        }
        Ok(LinearTransitionDecision::Apply { state_id }) => state_id,
        Err(message) => return AutomationEffectExecution::Failed { message },
    };
    match execute_linear_read(
        &profile.tracker.endpoint,
        credential,
        profile.codex.read_timeout_ms,
        linear_transition_mutation().into(),
        json!({"issueId": request.issue_id, "stateId": state_id}),
    )
    .await
    {
        Ok(value)
            if value
                .pointer("/data/issueUpdate/success")
                .and_then(Value::as_bool)
                == Some(true)
                && value
                    .pointer("/data/issueUpdate/issue/state/name")
                    .and_then(Value::as_str)
                    .is_some_and(|state| state.eq_ignore_ascii_case(&request.target_state)) =>
        {
            AutomationEffectExecution::Committed {
                provider_receipt: format!(
                    "linear-transition:{}:{}",
                    request.issue_id, request.target_state
                ),
            }
        }
        Ok(_) => AutomationEffectExecution::Ambiguous {
            message: "Linear transition returned no matching durable state".into(),
        },
        Err(message) => AutomationEffectExecution::Ambiguous { message },
    }
}

async fn execute_live_linear_comment(
    profile: &AutomationProfile,
    credential: &str,
    request: &AutomationTrackerCommentRequest,
) -> AutomationEffectExecution {
    match execute_linear_read(
        &profile.tracker.endpoint,
        credential,
        profile.codex.read_timeout_ms,
        linear_comment_mutation().into(),
        json!({"issueId": request.issue_id, "body": request.body}),
    )
    .await
    {
        Ok(value)
            if value
                .pointer("/data/commentCreate/success")
                .and_then(Value::as_bool)
                == Some(true)
                && value
                    .pointer("/data/commentCreate/comment/id")
                    .and_then(Value::as_str)
                    .is_some() =>
        {
            AutomationEffectExecution::Committed {
                provider_receipt: format!(
                    "linear-comment:{}:{}",
                    request.issue_id, request.idempotency_key
                ),
            }
        }
        Ok(_) => AutomationEffectExecution::Ambiguous {
            message: "Linear comment returned no matching durable comment".into(),
        },
        Err(message) => AutomationEffectExecution::Ambiguous { message },
    }
}

async fn execute_live_linear_pull_request_link(
    profile: &AutomationProfile,
    credential: &str,
    request: &AutomationTrackerPullRequestLinkRequest,
) -> AutomationEffectExecution {
    let context = match linear_mutation_context(profile, credential, &request.issue_id).await {
        Ok(context) => context,
        Err(message) => return AutomationEffectExecution::Failed { message },
    };
    let already_linked = linear_pull_request_already_linked(&context, &request.pull_request_url);
    if already_linked {
        return AutomationEffectExecution::Committed {
            provider_receipt: format!("linear-link-already:{}", request.idempotency_key),
        };
    }
    match execute_linear_read(
        &profile.tracker.endpoint,
        credential,
        profile.codex.read_timeout_ms,
        linear_pull_request_mutation().into(),
        json!({"issueId": request.issue_id, "url": request.pull_request_url}),
    )
    .await
    {
        Ok(value)
            if value
                .pointer("/data/attachmentCreate/success")
                .and_then(Value::as_bool)
                == Some(true)
                && value
                    .pointer("/data/attachmentCreate/attachment/url")
                    .and_then(Value::as_str)
                    == Some(request.pull_request_url.as_str()) =>
        {
            let attachment_id = value
                .pointer("/data/attachmentCreate/attachment/id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            AutomationEffectExecution::Committed {
                provider_receipt: format!("linear-attachment:{attachment_id}"),
            }
        }
        Ok(_) => AutomationEffectExecution::Ambiguous {
            message: "Linear pull-request link returned no matching durable attachment".into(),
        },
        Err(message) => AutomationEffectExecution::Ambiguous { message },
    }
}

fn linear_pull_request_already_linked(context: &Value, canonical_url: &str) -> bool {
    context
        .pointer("/data/issue/attachments/nodes")
        .and_then(Value::as_array)
        .is_some_and(|attachments| {
            attachments.iter().any(|attachment| {
                attachment
                    .get("url")
                    .and_then(Value::as_str)
                    .and_then(normalize_pull_request_url)
                    .as_deref()
                    == Some(canonical_url)
            })
        })
}

async fn linear_mutation_context(
    profile: &AutomationProfile,
    credential: &str,
    issue_id: &str,
) -> Result<Value, String> {
    validate_linear_endpoint(&profile.tracker.endpoint)?;
    let context = execute_linear_read(
        &profile.tracker.endpoint,
        credential,
        profile.codex.read_timeout_ms,
        linear_mutation_context_query().into(),
        json!({"issueId": issue_id, "projectId": profile.tracker.project_slug}),
    )
    .await?;
    validate_linear_mutation_scope(&context)?;
    Ok(context)
}

fn validate_linear_mutation_scope(context: &Value) -> Result<(), String> {
    let issue_project = context
        .pointer("/data/issue/project/id")
        .and_then(Value::as_str);
    let requested_project = context.pointer("/data/project/id").and_then(Value::as_str);
    if issue_project.is_none() || issue_project != requested_project {
        return Err("refreshed Linear Issue is outside the configured tracker project".into());
    }
    Ok(())
}

async fn execute_linear_read(
    endpoint: &str,
    credential: &str,
    timeout_ms: u64,
    query: String,
    variables: Value,
) -> Result<Value, String> {
    const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
    let client = build_reqwest_client_with_custom_ca(reqwest::Client::builder())
        .map_err(|error| error.to_string())?;
    let response = HttpClient::new(client)
        .post(endpoint)
        .header("Authorization", credential)
        .timeout(Duration::from_millis(timeout_ms.clamp(100, 30_000)))
        .json(&json!({"query": query, "variables": variables}))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err("Linear response exceeds the 1 MiB limit".into());
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("Linear response exceeds the 1 MiB limit".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if !status.is_success() {
        let message = value
            .pointer("/errors/0/message")
            .and_then(Value::as_str)
            .unwrap_or("Linear read failed");
        return Err(format!(
            "Linear returned HTTP {}: {message}",
            status.as_u16()
        ));
    }
    if let Some(message) = value.pointer("/errors/0/message").and_then(Value::as_str) {
        return Err(format!("Linear GraphQL error: {message}"));
    }
    Ok(value)
}

fn validate_fixture_eligibility(
    profile: &AutomationProfile,
    issue: &AutomationIssue,
) -> Result<(), String> {
    if !profile
        .tracker
        .active_states
        .iter()
        .any(|state| state.eq_ignore_ascii_case(&issue.state))
    {
        return Err(format!(
            "fixture issue `{}` is not in an active Automation state",
            issue.identifier
        ));
    }
    let labels = issue
        .labels
        .iter()
        .map(|label| label.trim().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let missing = profile
        .tracker
        .required_labels
        .iter()
        .filter(|label| !labels.contains(&label.to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "fixture issue `{}` is missing required labels: {}",
            issue.identifier,
            missing.join(", ")
        ));
    }
    let blocked = issue.blocked_by.iter().any(|blocker| {
        blocker.state.as_ref().is_none_or(|state| {
            !profile
                .tracker
                .terminal_states
                .iter()
                .any(|terminal| terminal.eq_ignore_ascii_case(state))
        })
    });
    if blocked {
        return Err(format!(
            "fixture issue `{}` has a nonterminal blocker",
            issue.identifier
        ));
    }
    Ok(())
}

fn authorize_automation_root(
    root: &AutomationRootCheckpoint,
    parent_thread_id: &str,
) -> Result<(), String> {
    if root.owner_thread_id == parent_thread_id {
        Ok(())
    } else {
        Err("Automation Root Run does not belong to the requested task".into())
    }
}

async fn resolve_live_tracker_effect(
    store: &AutomationRunStore,
    root: &mut AutomationRootCheckpoint,
    claim_id: &str,
    profile: &AutomationProfile,
    refreshed_state: &str,
    effect: &AutomationEffect,
    workflow: &RunCheckpoint,
) -> Result<AutomationEffectReceipt, String> {
    let credential = linear_credential(profile)
        .ok_or("referenced Linear credential is unavailable for a live mutation")?;
    match effect {
        AutomationEffect::TrackerComment => {
            let body = extract_tracker_comment(workflow)?;
            let (receipt, request) = store
                .prepare_tracker_comment(
                    root,
                    claim_id,
                    profile,
                    &body,
                    AutomationGatePolicy::AutoAccept,
                )
                .map_err(|error| error.to_string())?;
            let Some(request) = request else {
                return Ok(receipt);
            };
            let execution = execute_live_linear_comment(profile, &credential, &request).await;
            store
                .complete_tracker_effect(root, claim_id, &request.idempotency_key, execution)
                .map_err(|error| error.to_string())
        }
        AutomationEffect::TrackerTransition => {
            let target_state = extract_tracker_transition(workflow)?;
            let (receipt, request) = store
                .prepare_tracker_transition(
                    root,
                    claim_id,
                    profile,
                    refreshed_state,
                    &target_state,
                    AutomationGatePolicy::AutoAccept,
                )
                .map_err(|error| error.to_string())?;
            let Some(request) = request else {
                return Ok(receipt);
            };
            let execution = execute_live_linear_transition(profile, &credential, &request).await;
            store
                .complete_tracker_transition(root, claim_id, &request, execution)
                .map_err(|error| error.to_string())
        }
        AutomationEffect::TrackerLinkPullRequest => {
            let pull_request_url = extract_tracker_pull_request(workflow)?;
            let (receipt, request) = store
                .prepare_tracker_pull_request_link(
                    root,
                    claim_id,
                    profile,
                    &pull_request_url,
                    AutomationGatePolicy::AutoAccept,
                )
                .map_err(|error| error.to_string())?;
            let Some(request) = request else {
                return Ok(receipt);
            };
            let execution =
                execute_live_linear_pull_request_link(profile, &credential, &request).await;
            store
                .complete_tracker_effect(root, claim_id, &request.idempotency_key, execution)
                .map_err(|error| error.to_string())
        }
    }
}

fn extract_tracker_comment(checkpoint: &RunCheckpoint) -> Result<String, String> {
    extract_tracker_effect_output(checkpoint, "tracker_comment", "body")
}

fn extract_tracker_transition(checkpoint: &RunCheckpoint) -> Result<String, String> {
    extract_tracker_effect_output(checkpoint, "tracker_transition", "state")
}

fn extract_tracker_pull_request(checkpoint: &RunCheckpoint) -> Result<String, String> {
    extract_tracker_effect_output(checkpoint, "tracker_pull_request", "url")
}

fn extract_tracker_effect_output(
    checkpoint: &RunCheckpoint,
    output_name: &str,
    field_name: &str,
) -> Result<String, String> {
    let values = checkpoint
        .steps
        .values()
        .filter_map(|step| step.outputs.get(output_name))
        .collect::<Vec<_>>();
    let [value] = values.as_slice() else {
        return Err(format!(
            "completed fixture Workflow must return exactly one `{output_name}` output"
        ));
    };
    let object = value.as_object().ok_or_else(|| {
        format!("`{output_name}` must be an object containing only `{field_name}`")
    })?;
    if object.len() != 1 {
        return Err(format!(
            "`{output_name}` must contain only the claim-scoped `{field_name}` field"
        ));
    }
    object
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("`{output_name}.{field_name}` must be a string"))
}

fn execute_fixture_tracker_comment(
    repository: &Path,
    request: &AutomationTrackerCommentRequest,
) -> AutomationEffectExecution {
    append_fixture_tracker_effect(
        repository,
        "comments.jsonl",
        &request.idempotency_key,
        "comment",
        json!({
            "idempotencyKey": request.idempotency_key,
            "effectId": request.effect_id,
            "claimId": request.claim_id,
            "projectSlug": request.tracker_project_slug,
            "issueId": request.issue_id,
            "body": request.body,
        }),
    )
}

fn execute_fixture_tracker_transition(
    repository: &Path,
    request: &AutomationTrackerTransitionRequest,
) -> AutomationEffectExecution {
    append_fixture_tracker_effect(
        repository,
        "transitions.jsonl",
        &request.idempotency_key,
        "transition",
        json!({
            "idempotencyKey": request.idempotency_key,
            "effectId": request.effect_id,
            "claimId": request.claim_id,
            "projectSlug": request.tracker_project_slug,
            "issueId": request.issue_id,
            "expectedState": request.expected_state,
            "targetState": request.target_state,
        }),
    )
}

fn execute_fixture_tracker_pull_request(
    repository: &Path,
    request: &AutomationTrackerPullRequestLinkRequest,
) -> AutomationEffectExecution {
    append_fixture_tracker_effect(
        repository,
        "pull-requests.jsonl",
        &request.idempotency_key,
        "pull-request",
        json!({
            "idempotencyKey": request.idempotency_key,
            "effectId": request.effect_id,
            "claimId": request.claim_id,
            "projectSlug": request.tracker_project_slug,
            "issueId": request.issue_id,
            "pullRequestUrl": request.pull_request_url,
        }),
    )
}

fn append_fixture_tracker_effect(
    repository: &Path,
    file_name: &str,
    idempotency_key: &str,
    receipt_kind: &str,
    value: Value,
) -> AutomationEffectExecution {
    let root = repository.join(".codex/orchestra/fixture-tracker");
    if let Err(error) = std::fs::create_dir_all(&root) {
        return AutomationEffectExecution::Failed {
            message: error.to_string(),
        };
    }
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(file_name))
    {
        Ok(file) => file,
        Err(error) => {
            return AutomationEffectExecution::Failed {
                message: error.to_string(),
            };
        }
    };
    let line = match serde_json::to_vec(&value) {
        Ok(line) => line,
        Err(error) => {
            return AutomationEffectExecution::Failed {
                message: error.to_string(),
            };
        }
    };
    if let Err(error) = file.write_all(&line).and_then(|_| file.write_all(b"\n")) {
        return AutomationEffectExecution::Ambiguous {
            message: format!("fixture tracker write was interrupted: {error}"),
        };
    }
    if let Err(error) = file.sync_all() {
        return AutomationEffectExecution::Ambiguous {
            message: format!("fixture tracker receipt durability is ambiguous: {error}"),
        };
    }
    AutomationEffectExecution::Committed {
        provider_receipt: format!("fixture-{receipt_kind}:{idempotency_key}"),
    }
}

fn fail_automation_claim(
    store: &AutomationRunStore,
    root: &mut AutomationRootCheckpoint,
    claim_id: &str,
    error: &str,
) -> Result<(), String> {
    store
        .update_claim(root, claim_id, |claim| {
            claim.status = AutomationClaimStatus::Failed;
            claim.next_action = bounded_lifecycle_text(error.into());
        })
        .map_err(|storage| storage.to_string())?;
    root.next_action = format!("inspect failed claim `{claim_id}`");
    store.save(root).map_err(|storage| storage.to_string())
}

fn safe_task_name(identifier: &str) -> String {
    identifier
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn project_claim_liveness(
    root: &mut AutomationRootCheckpoint,
    profile: &AutomationProfile,
    now_ms: u64,
) {
    for claim in root.claims.values_mut() {
        if automation_claim_liveness(claim, profile, now_ms) == AutomationClaimLiveness::Stalled {
            claim.next_action =
                "claim stalled; inspect the retained Issue task and Child Run".into();
        }
    }
}

fn project_checkpoint(checkpoint: &RunCheckpoint) -> CodexOrchestraRunProjection {
    CodexOrchestraRunProjection {
        run_id: checkpoint.run_id.clone(),
        workflow_sha256: checkpoint.workflow_sha256.clone(),
        parent_thread_id: checkpoint.parent_thread_id.clone(),
        source_revision: checkpoint.source_revision.clone(),
        status: match checkpoint.status {
            RunStatus::Pending => CodexOrchestraRunStatus::Pending,
            RunStatus::Running => CodexOrchestraRunStatus::Running,
            RunStatus::WaitingApproval => CodexOrchestraRunStatus::WaitingApproval,
            RunStatus::Completed => CodexOrchestraRunStatus::Completed,
            RunStatus::Failed => CodexOrchestraRunStatus::Failed,
            RunStatus::Cancelled => CodexOrchestraRunStatus::Cancelled,
        },
        promotion: match checkpoint.promotion {
            codex_orchestra_core::PromotionStatus::Pending => {
                CodexOrchestraPromotionStatus::Pending
            }
            codex_orchestra_core::PromotionStatus::Applied => {
                CodexOrchestraPromotionStatus::Applied
            }
            codex_orchestra_core::PromotionStatus::NotRequired => {
                CodexOrchestraPromotionStatus::NotRequired
            }
        },
        steps: checkpoint
            .steps
            .iter()
            .map(|(id, step)| CodexOrchestraStepProjection {
                id: id.clone(),
                status: match step.status {
                    codex_orchestra_core::StepStatus::Pending => CodexOrchestraStepStatus::Pending,
                    codex_orchestra_core::StepStatus::Running => CodexOrchestraStepStatus::Running,
                    codex_orchestra_core::StepStatus::Retrying => {
                        CodexOrchestraStepStatus::Retrying
                    }
                    codex_orchestra_core::StepStatus::WaitingApproval => {
                        CodexOrchestraStepStatus::WaitingApproval
                    }
                    codex_orchestra_core::StepStatus::Completed => {
                        CodexOrchestraStepStatus::Completed
                    }
                    codex_orchestra_core::StepStatus::Failed => CodexOrchestraStepStatus::Failed,
                    codex_orchestra_core::StepStatus::Cancelled => {
                        CodexOrchestraStepStatus::Cancelled
                    }
                },
                attempts: step.attempts,
                rounds: step.rounds,
                output_keys: step.outputs.keys().cloned().collect(),
                final_response: step.final_response.clone().map(bounded_lifecycle_text),
                error: step.error.clone().map(bounded_lifecycle_text),
            })
            .collect(),
        next_action: bounded_lifecycle_text(checkpoint.next_action.clone()),
    }
}

fn bounded_lifecycle_text(mut text: String) -> String {
    const MAX_BYTES: usize = 4096;
    if text.len() <= MAX_BYTES {
        return text;
    }
    let mut end = MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}

pub struct OrchestraTools {
    service: OrchestraService,
}

impl OrchestraTools {
    pub fn new(service: OrchestraService) -> Self {
        Self { service }
    }
}

impl ToolContributor for OrchestraTools {
    fn tools(
        &self,
        _: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let parent_thread_id = thread_store.level_id().to_string();
        [
            Kind::Validate,
            Kind::Run,
            Kind::Resume,
            Kind::Status,
            Kind::Cancel,
            Kind::Query,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(OrchestraTool {
                kind,
                parent_thread_id: parent_thread_id.clone(),
                service: self.service.clone(),
            }) as Arc<dyn ToolExecutor<ToolCall>>
        })
        .collect()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Validate,
    Run,
    Resume,
    Status,
    Cancel,
    Query,
}

struct OrchestraTool {
    kind: Kind,
    parent_thread_id: String,
    service: OrchestraService,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowArgs {
    workflow_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteArgs {
    workflow_path: String,
    inputs: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeArgs {
    run_id: String,
    approval_decision: Option<String>,
    inputs: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    run_id: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QuerySelector {
    Run,
    Steps,
    Outputs,
    Evidence,
    EvidenceContent,
    History,
    Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    run_id: String,
    selector: QuerySelector,
    step_id: Option<String>,
    evidence_id: Option<String>,
    after: Option<String>,
    history_after: Option<HistoryCursor>,
    max_items: Option<usize>,
    max_bytes: Option<usize>,
}

impl ToolExecutor<ToolCall> for OrchestraTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.kind.name())
    }
    fn spec(&self) -> ToolSpec {
        self.kind.spec()
    }
    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

impl OrchestraTool {
    async fn handle_call(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn codex_extension_api::ToolOutput>, FunctionCallError> {
        let value = match self.kind {
            Kind::Validate => {
                let args: WorkflowArgs = parse(&call)?;
                let plan = self
                    .service
                    .validate(&self.parent_thread_id, &args.workflow_path)
                    .await
                    .map_err(to_model)?;
                json!({"valid": true, "plan": plan})
            }
            Kind::Run => {
                let args: ExecuteArgs = parse(&call)?;
                json!(
                    self.service
                        .run(
                            &self.parent_thread_id,
                            &args.workflow_path,
                            args.inputs.as_ref(),
                        )
                        .await
                        .map_err(to_model)?
                )
            }
            Kind::Resume => {
                let args: ResumeArgs = parse(&call)?;
                json!(
                    self.service
                        .resume(
                            &self.parent_thread_id,
                            &args.run_id,
                            args.approval_decision.as_deref(),
                            args.inputs.as_ref(),
                        )
                        .await
                        .map_err(to_model)?
                )
            }
            Kind::Status => {
                let args: RunArgs = parse(&call)?;
                json!(
                    self.service
                        .status(&self.parent_thread_id, &args.run_id)
                        .await
                        .map_err(to_model)?
                )
            }
            Kind::Cancel => {
                let args: RunArgs = parse(&call)?;
                json!(
                    self.service
                        .cancel(&self.parent_thread_id, &args.run_id)
                        .await
                        .map_err(to_model)?
                )
            }
            Kind::Query => {
                let args: QueryArgs = parse(&call)?;
                if matches!(args.selector, QuerySelector::Digest) {
                    let digest = self
                        .service
                        .digest(
                            &self.parent_thread_id,
                            &args.run_id,
                            args.max_bytes.unwrap_or(4096),
                        )
                        .await
                        .map_err(to_model)?;
                    return Ok(Box::new(JsonToolOutput::new(json!({
                        "selector": "digest",
                        "result": digest,
                    }))));
                }
                let selector = match args.selector {
                    QuerySelector::Run => ExecutionSelector::Run,
                    QuerySelector::Steps => ExecutionSelector::Steps { after: args.after },
                    QuerySelector::Outputs => ExecutionSelector::Outputs {
                        step_id: args.step_id,
                        after: args.after,
                    },
                    QuerySelector::Evidence => ExecutionSelector::Evidence {
                        step_id: args.step_id,
                        after: args.after,
                    },
                    QuerySelector::EvidenceContent => ExecutionSelector::EvidenceContent {
                        evidence_id: args.evidence_id.ok_or_else(|| {
                            FunctionCallError::RespondToModel(
                                "evidence_id is required for evidence_content".into(),
                            )
                        })?,
                    },
                    QuerySelector::History => ExecutionSelector::History {
                        after: args.history_after,
                    },
                    QuerySelector::Digest => unreachable!(),
                };
                let defaults = ExecutionQueryBudget::default();
                json!(
                    self.service
                        .query(
                            &self.parent_thread_id,
                            &args.run_id,
                            selector,
                            ExecutionQueryBudget {
                                max_items: args.max_items.unwrap_or(defaults.max_items),
                                max_bytes: args.max_bytes.unwrap_or(defaults.max_bytes),
                            },
                        )
                        .await
                        .map_err(to_model)?
                )
            }
        };
        Ok(Box::new(JsonToolOutput::new(value)))
    }
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Validate => "orchestra_validate",
            Self::Run => "orchestra_run",
            Self::Resume => "orchestra_resume",
            Self::Status => "orchestra_status",
            Self::Cancel => "orchestra_cancel",
            Self::Query => "orchestra_query",
        }
    }
    fn spec(self) -> ToolSpec {
        if matches!(self, Self::Query) {
            let history_after = JsonSchema::object(
                BTreeMap::from([
                    (
                        "sequence".into(),
                        JsonSchema::integer(Some("Last lifecycle sequence.".into())),
                    ),
                    (
                        "item_id".into(),
                        JsonSchema::string(Some("Last lifecycle event id.".into())),
                    ),
                    (
                        "revision".into(),
                        JsonSchema::integer(Some("Last lifecycle revision.".into())),
                    ),
                ]),
                Some(vec!["sequence".into(), "item_id".into(), "revision".into()]),
                Some(false.into()),
            );
            let properties = BTreeMap::from([
                (
                    "run_id".into(),
                    JsonSchema::string(Some("Task-owned Orchestra run id.".into())),
                ),
                (
                    "selector".into(),
                    JsonSchema::string_enum(
                        [
                            "run",
                            "steps",
                            "outputs",
                            "evidence",
                            "evidence_content",
                            "history",
                            "digest",
                        ]
                        .map(|value| Value::String(value.into()))
                        .to_vec(),
                        Some("Fixed bounded projection to read.".into()),
                    ),
                ),
                (
                    "step_id".into(),
                    JsonSchema::string(Some("Optional outputs/evidence step filter.".into())),
                ),
                (
                    "evidence_id".into(),
                    JsonSchema::string(Some(
                        "Opaque identity required only for evidence_content.".into(),
                    )),
                ),
                (
                    "after".into(),
                    JsonSchema::string(Some("Opaque selector-specific page cursor.".into())),
                ),
                ("history_after".into(), history_after),
                (
                    "max_items".into(),
                    JsonSchema::integer(Some(
                        "Requested item cap; server limits still apply.".into(),
                    )),
                ),
                (
                    "max_bytes".into(),
                    JsonSchema::integer(Some(
                        "Requested response byte cap; server limits still apply.".into(),
                    )),
                ),
            ]);
            return ToolSpec::Function(ResponsesApiTool {
                name: self.name().into(),
                description: "Read one fixed, bounded, task-authorized Orchestra projection."
                    .into(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    properties,
                    Some(vec!["run_id".into(), "selector".into()]),
                    Some(false.into()),
                ),
                output_schema: None,
            });
        }
        let (property, description) = match self {
            Self::Validate | Self::Run => (
                "workflow_path",
                "Repository-relative path to a restricted .workflow.ts file.",
            ),
            Self::Resume | Self::Status | Self::Cancel => {
                ("run_id", "Orchestra run id under .codex/orchestra/runs/.")
            }
            Self::Query => unreachable!(),
        };
        let mut properties = BTreeMap::from([(
            property.into(),
            JsonSchema::string(Some(description.into())),
        )]);
        if matches!(self, Self::Resume) {
            properties.insert(
                "approval_decision".into(),
                JsonSchema::string(Some(
                    "Optional decision for the pending approval step.".into(),
                )),
            );
        }
        if matches!(self, Self::Run | Self::Resume) {
            properties.insert(
                "inputs".into(),
                JsonSchema::object(BTreeMap::new(), None, Some(true.into())),
            );
        }
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().into(),
            description: format!(
                "Native Orchestra {} operation using the active thread's V2 control plane.",
                self.name()
            ),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec![property.into()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }
}

fn parse<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?).map_err(to_model)
}
fn to_model(error: impl std::fmt::Display) -> FunctionCallError {
    FunctionCallError::RespondToModel(error.to_string())
}
fn safe_workflow(repository: &Path, relative: &str) -> Result<PathBuf, String> {
    if !relative.ends_with(".workflow.ts") {
        return Err("workflow path must end in .workflow.ts".into());
    }
    let root = repository
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !path.starts_with(root) {
        return Err("workflow path escapes repository".into());
    }
    Ok(path)
}

fn safe_automation_profile(repository: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Automation profile path must stay inside the repository".into());
    }
    if relative.file_name().and_then(|value| value.to_str()) != Some("WORKFLOW.md") {
        return Err("Automation profile path must name WORKFLOW.md".into());
    }
    let root = repository
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = root.join(relative);
    if path.exists() {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(&root) {
            return Err("Automation profile path escapes repository".into());
        }
        return Ok(canonical);
    }
    Ok(path)
}

fn reject_existing_root_run(repository: &Path, parent_thread_id: &str) -> Result<(), String> {
    let runs = repository.join(".codex/orchestra/runs");
    let Ok(entries) = std::fs::read_dir(runs) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let Ok(bytes) = std::fs::read(entry.path().join("state.json")) else {
            continue;
        };
        let Ok(checkpoint) = serde_json::from_slice::<RunCheckpoint>(&bytes) else {
            continue;
        };
        if checkpoint.parent_thread_id == parent_thread_id
            && matches!(
                checkpoint.status,
                RunStatus::Pending | RunStatus::Running | RunStatus::WaitingApproval
            )
        {
            return Err(format!(
                "task already owns nonterminal root run `{}`",
                checkpoint.run_id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_mutation_profile(project_id: &str) -> AutomationProfile {
        serde_json::from_value(json!({
            "tracker": {
                "kind": "linear",
                "endpoint": "https://api.linear.app/graphql",
                "projectSlug": project_id,
                "requiredLabels": [],
                "activeStates": ["Todo", "In Progress"],
                "terminalStates": ["Done", "Cancelled"],
                "credential": {"kind": "environment", "reference": "LINEAR_API_KEY", "digest": "explicit-live-test"}
            },
            "polling": {"intervalMs": 30000},
            "workspace": {"root": ".codex/orchestra/worktrees"},
            "hooks": {"afterCreate": null, "beforeRun": null, "afterRun": null, "beforeRemove": null, "timeoutMs": 60000},
            "agent": {"maxConcurrentAgents": 1, "maxTurns": 1, "maxRetryBackoffMs": 300000, "maxConcurrentAgentsByState": {}},
            "codex": {"approvalPolicy": "on-request", "threadSandbox": "workspace-write", "turnSandboxPolicy": null, "turnTimeoutMs": 3600000, "readTimeoutMs": 5000, "stallTimeoutMs": 300000},
            "orchestra": {"workflowPath": "issue.workflow.ts", "workflowSha256": "explicit-live-test", "workflowName": "issue", "effects": ["tracker.transition", "tracker.link_pull_request"]},
            "promptTemplate": "Explicit live mutation test"
        }))
        .unwrap()
    }

    #[test]
    fn persistent_worktree_reuse_requires_the_exact_recorded_base() {
        let exact = CommandOutcome {
            exit_code: 0,
            stdout: "worktree /workspace/orc-40\0HEAD abc123\0detached\0\0".into(),
            stderr: String::new(),
        };
        let expected_path = Path::new("/workspace/orc-40");
        assert!(persistent_worktree_matches_recorded_base(
            &exact,
            expected_path,
            "abc123"
        ));
        assert!(!persistent_worktree_matches_recorded_base(
            &exact,
            expected_path,
            "def456"
        ));
        assert!(!persistent_worktree_matches_recorded_base(
            &exact,
            Path::new("/workspace/unrelated"),
            "abc123"
        ));
        assert!(!persistent_worktree_matches_recorded_base(
            &CommandOutcome {
                exit_code: 128,
                stdout: "worktree /workspace/orc-40\0HEAD abc123\0\0".into(),
                stderr: "not a worktree".into(),
            },
            expected_path,
            "abc123"
        ));
    }

    #[test]
    fn automation_profile_path_stays_task_scoped_and_preserves_missing_diagnostics() {
        let repository = tempfile::tempdir().unwrap();
        let expected = repository
            .path()
            .canonicalize()
            .unwrap()
            .join("config/WORKFLOW.md");
        assert_eq!(
            safe_automation_profile(repository.path(), "config/WORKFLOW.md").unwrap(),
            expected
        );
        assert!(safe_automation_profile(repository.path(), "../WORKFLOW.md").is_err());
        assert!(safe_automation_profile(repository.path(), "/tmp/WORKFLOW.md").is_err());
        assert!(safe_automation_profile(repository.path(), "workflow.md").is_err());
    }

    #[test]
    fn live_linear_reads_are_pinned_to_the_official_https_endpoint() {
        assert!(validate_linear_endpoint("https://api.linear.app/graphql").is_ok());
        assert!(validate_linear_endpoint("http://api.linear.app/graphql").is_err());
        assert!(validate_linear_endpoint("https://linear.example/graphql").is_err());
        assert!(validate_linear_endpoint("https://api.linear.app/graphql?token=nope").is_err());
        assert!(linear_project_issues_query().starts_with("query "));
        assert!(!linear_project_issues_query().contains("mutation"));
        assert!(!linear_issue_query().contains("mutation"));
        assert!(linear_mutation_context_query().starts_with("query "));
        assert!(linear_transition_mutation().starts_with("mutation "));
        assert!(linear_transition_mutation().contains("issueUpdate"));
        assert!(linear_pull_request_mutation().starts_with("mutation "));
        assert!(linear_pull_request_mutation().contains("attachmentCreate"));
    }

    #[test]
    fn refreshed_linear_state_prevents_conflicts_and_terminal_races() {
        let profile = linear_mutation_profile("project-41");
        let request = AutomationTrackerTransitionRequest {
            effect_id: "effect-41".into(),
            idempotency_key: "idem-41".into(),
            claim_id: "claim-41".into(),
            tracker_project_slug: "project-41".into(),
            issue_id: "issue-41".into(),
            expected_state: "Todo".into(),
            target_state: "Done".into(),
        };
        let context = |state: &str| {
            json!({
                "data": {"issue": {
                    "state": {"name": state},
                    "team": {"states": {"nodes": [
                        {"id": "state-todo", "name": "Todo"},
                        {"id": "state-done", "name": "Done"}
                    ]}}
                }}
            })
        };

        assert_eq!(
            linear_transition_decision(&profile, &request, &context("Todo")),
            Ok(LinearTransitionDecision::Apply {
                state_id: "state-done".into()
            })
        );
        assert_eq!(
            linear_transition_decision(&profile, &request, &context("Done")),
            Ok(LinearTransitionDecision::AlreadyApplied)
        );
        assert!(
            linear_transition_decision(&profile, &request, &context("Cancelled"))
                .unwrap_err()
                .contains("terminal")
        );
        assert!(
            linear_transition_decision(&profile, &request, &context("In Progress"))
                .unwrap_err()
                .contains("changed from")
        );
    }

    #[test]
    fn refreshed_linear_issue_must_remain_in_the_configured_project() {
        assert!(
            validate_linear_mutation_scope(&json!({
                "data": {
                    "issue": {"project": {"id": "project-41"}},
                    "project": {"id": "project-41"}
                }
            }))
            .is_ok()
        );
        assert!(
            validate_linear_mutation_scope(&json!({
                "data": {
                    "issue": {"project": {"id": "another-project"}},
                    "project": {"id": "project-41"}
                }
            }))
            .unwrap_err()
            .contains("outside")
        );
    }

    #[test]
    fn existing_linear_pull_request_attachments_are_compared_canonically() {
        let context = json!({
            "data": {"issue": {"attachments": {"nodes": [
                {"id": "attachment-41", "url": "https://github.com/edgefloor/codex-orchestra/pull/00043/?view=files#top"}
            ]}}}
        });
        assert!(linear_pull_request_already_linked(
            &context,
            "https://github.com/edgefloor/codex-orchestra/pull/43"
        ));
        assert!(!linear_pull_request_already_linked(
            &context,
            "https://github.com/edgefloor/codex-orchestra/pull/44"
        ));
    }

    #[test]
    fn tracker_effect_outputs_are_exactly_one_claim_scoped_value() {
        fn checkpoint(output_name: &str, output: Value) -> RunCheckpoint {
            let mut outputs = serde_json::Map::new();
            outputs.insert(output_name.into(), output);
            serde_json::from_value(json!({
                "schema_version": 1,
                "run_id": "workflow-34",
                "workflow_sha256": "sha",
                "parent_thread_id": "issue-task-34",
                "repository": "/tmp/worktree",
                "source_revision": "abc123",
                "status": "completed",
                "steps": {
                    "comment": {
                        "status": "completed",
                        "attempts": 1,
                        "rounds": 1,
                        "outputs": outputs
                    }
                },
                "next_action": "complete"
            }))
            .unwrap()
        }

        assert_eq!(
            extract_tracker_comment(&checkpoint(
                "tracker_comment",
                json!({"body": "Implemented and verified."})
            ))
            .unwrap(),
            "Implemented and verified."
        );
        assert_eq!(
            extract_tracker_transition(&checkpoint("tracker_transition", json!({"state": "Done"})))
                .unwrap(),
            "Done"
        );
        assert_eq!(
            extract_tracker_pull_request(&checkpoint(
                "tracker_pull_request",
                json!({"url": "https://github.com/edgefloor/codex-orchestra/pull/43"})
            ))
            .unwrap(),
            "https://github.com/edgefloor/codex-orchestra/pull/43"
        );
        assert!(
            extract_tracker_comment(&checkpoint("tracker_comment", json!("not an object")))
                .is_err()
        );
        assert!(
            extract_tracker_transition(&checkpoint(
                "tracker_transition",
                json!({"state": "Done", "issueId": "another-issue"})
            ))
            .is_err()
        );
        assert!(
            extract_tracker_pull_request(&checkpoint(
                "tracker_pull_request",
                json!({"url": "https://github.com/o/r/pull/1", "claimId": "another-claim"})
            ))
            .is_err()
        );

        let mut duplicate = checkpoint("tracker_comment", json!({"body": "first"}));
        duplicate.steps.insert(
            "second".into(),
            serde_json::from_value(json!({
                "status": "completed",
                "attempts": 1,
                "rounds": 1,
                "outputs": { "tracker_comment": {"body": "second"} }
            }))
            .unwrap(),
        );
        assert!(extract_tracker_comment(&duplicate).is_err());
    }

    #[test]
    fn fixture_tracker_comment_returns_a_durable_provider_receipt() {
        let repository = tempfile::tempdir().unwrap();
        let request = AutomationTrackerCommentRequest {
            effect_id: "effect-34".into(),
            idempotency_key: "idem-34".into(),
            claim_id: "claim-34".into(),
            tracker_project_slug: "orchestra".into(),
            issue_id: "issue-34".into(),
            body: "Implemented and verified.".into(),
        };

        assert_eq!(
            execute_fixture_tracker_comment(repository.path(), &request),
            AutomationEffectExecution::Committed {
                provider_receipt: "fixture-comment:idem-34".into()
            }
        );
        let persisted = std::fs::read_to_string(
            repository
                .path()
                .join(".codex/orchestra/fixture-tracker/comments.jsonl"),
        )
        .unwrap();
        let record: Value = serde_json::from_str(persisted.trim()).unwrap();
        assert_eq!(record["claimId"], "claim-34");
        assert_eq!(record["issueId"], "issue-34");
        assert_eq!(record["idempotencyKey"], "idem-34");
        assert_eq!(record["body"], "Implemented and verified.");
    }

    #[test]
    fn fixture_transition_and_pull_request_link_return_durable_provider_receipts() {
        let repository = tempfile::tempdir().unwrap();
        let transition = AutomationTrackerTransitionRequest {
            effect_id: "effect-transition-41".into(),
            idempotency_key: "idem-transition-41".into(),
            claim_id: "claim-41".into(),
            tracker_project_slug: "orchestra".into(),
            issue_id: "issue-41".into(),
            expected_state: "Todo".into(),
            target_state: "Done".into(),
        };
        let pull_request = AutomationTrackerPullRequestLinkRequest {
            effect_id: "effect-pr-41".into(),
            idempotency_key: "idem-pr-41".into(),
            claim_id: "claim-41".into(),
            tracker_project_slug: "orchestra".into(),
            issue_id: "issue-41".into(),
            pull_request_url: "https://github.com/edgefloor/codex-orchestra/pull/43".into(),
        };

        assert_eq!(
            execute_fixture_tracker_transition(repository.path(), &transition),
            AutomationEffectExecution::Committed {
                provider_receipt: "fixture-transition:idem-transition-41".into()
            }
        );
        assert_eq!(
            execute_fixture_tracker_pull_request(repository.path(), &pull_request),
            AutomationEffectExecution::Committed {
                provider_receipt: "fixture-pull-request:idem-pr-41".into()
            }
        );

        let transition_record: Value = serde_json::from_str(
            std::fs::read_to_string(
                repository
                    .path()
                    .join(".codex/orchestra/fixture-tracker/transitions.jsonl"),
            )
            .unwrap()
            .trim(),
        )
        .unwrap();
        assert_eq!(transition_record["claimId"], "claim-41");
        assert_eq!(transition_record["projectSlug"], "orchestra");
        assert_eq!(transition_record["issueId"], "issue-41");
        assert_eq!(transition_record["expectedState"], "Todo");
        assert_eq!(transition_record["targetState"], "Done");

        let pull_request_record: Value = serde_json::from_str(
            std::fs::read_to_string(
                repository
                    .path()
                    .join(".codex/orchestra/fixture-tracker/pull-requests.jsonl"),
            )
            .unwrap()
            .trim(),
        )
        .unwrap();
        assert_eq!(pull_request_record["claimId"], "claim-41");
        assert_eq!(pull_request_record["projectSlug"], "orchestra");
        assert_eq!(pull_request_record["issueId"], "issue-41");
        assert_eq!(
            pull_request_record["pullRequestUrl"],
            "https://github.com/edgefloor/codex-orchestra/pull/43"
        );
    }

    #[tokio::test]
    #[ignore = "mutates a user-selected live Linear Issue; run only with the explicit environment gate"]
    async fn live_linear_transition_and_pull_request_link_are_explicit_opt_in() {
        assert_eq!(
            std::env::var("ORCHESTRA_LINEAR_MUTATION_TEST").as_deref(),
            Ok("1"),
            "set ORCHESTRA_LINEAR_MUTATION_TEST=1 to acknowledge live Linear mutations"
        );
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| panic!("missing required `{name}`"))
        };
        let credential = required("LINEAR_API_KEY");
        let issue_id = required("ORCHESTRA_LINEAR_ISSUE_ID");
        let target_state = required("ORCHESTRA_LINEAR_TARGET_STATE");
        let pull_request_url = required("ORCHESTRA_LINEAR_PULL_REQUEST_URL");
        let profile = linear_mutation_profile(&required("ORCHESTRA_LINEAR_PROJECT_ID"));

        let transition = execute_live_linear_transition(
            &profile,
            &credential,
            &AutomationTrackerTransitionRequest {
                effect_id: "live-transition".into(),
                idempotency_key: "live-transition".into(),
                claim_id: "explicit-live-test".into(),
                tracker_project_slug: profile.tracker.project_slug.clone(),
                issue_id: issue_id.clone(),
                expected_state: required("ORCHESTRA_LINEAR_EXPECTED_STATE"),
                target_state,
            },
        )
        .await;
        assert!(matches!(
            transition,
            AutomationEffectExecution::Committed { .. }
        ));

        let linked = execute_live_linear_pull_request_link(
            &profile,
            &credential,
            &AutomationTrackerPullRequestLinkRequest {
                effect_id: "live-pull-request".into(),
                idempotency_key: "live-pull-request".into(),
                claim_id: "explicit-live-test".into(),
                tracker_project_slug: profile.tracker.project_slug.clone(),
                issue_id,
                pull_request_url,
            },
        )
        .await;
        assert!(matches!(
            linked,
            AutomationEffectExecution::Committed { .. }
        ));
    }

    #[test]
    fn exposes_exact_native_tool_surface() {
        let names = [
            Kind::Validate,
            Kind::Run,
            Kind::Resume,
            Kind::Status,
            Kind::Cancel,
            Kind::Query,
        ]
        .map(Kind::name);
        assert_eq!(
            names,
            [
                "orchestra_validate",
                "orchestra_run",
                "orchestra_resume",
                "orchestra_status",
                "orchestra_cancel",
                "orchestra_query",
            ]
        );
    }

    #[test]
    fn run_and_resume_accept_input_objects_without_changing_other_tool_contracts() {
        for kind in [Kind::Run, Kind::Resume] {
            let ToolSpec::Function(tool) = kind.spec() else {
                panic!()
            };
            let inputs = &tool.parameters.properties.as_ref().unwrap()["inputs"];
            assert!(inputs.additional_properties.is_some());
        }
        let ToolSpec::Function(validate) = Kind::Validate.spec() else {
            panic!()
        };
        assert!(
            !validate
                .parameters
                .properties
                .as_ref()
                .unwrap()
                .contains_key("inputs")
        );
    }

    #[test]
    fn query_tool_exposes_only_fixed_bounded_selectors() {
        let ToolSpec::Function(query) = Kind::Query.spec() else {
            panic!()
        };
        let properties = query.parameters.properties.as_ref().unwrap();
        assert_eq!(
            properties["selector"].enum_values.as_ref().unwrap(),
            &[
                json!("run"),
                json!("steps"),
                json!("outputs"),
                json!("evidence"),
                json!("evidence_content"),
                json!("history"),
                json!("digest"),
            ]
        );
        assert!(properties.contains_key("max_items"));
        assert!(properties.contains_key("max_bytes"));
    }

    #[test]
    fn maps_v2_completion_error_and_cancellation_statuses() {
        assert_eq!(
            map_status(CodexAgentStatus::Completed(Some("done".into()))),
            AgentStatus::Completed
        );
        assert_eq!(
            map_status(CodexAgentStatus::Errored("boom".into())),
            AgentStatus::Failed("boom".into())
        );
        assert_eq!(
            map_status(CodexAgentStatus::Shutdown),
            AgentStatus::Cancelled
        );
    }
}
