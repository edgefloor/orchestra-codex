use crate::ForkTurns;
use crate::SkillRequirement;
use crate::StepOutputs;
use crate::WorktreePolicy;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSkill {
    pub requirement: String,
    pub identity: SkillIdentity,
    pub instructions: Vec<u8>,
    pub resources: BTreeMap<String, Vec<u8>>,
    pub tool_dependencies: Vec<SkillToolDependency>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillIdentity {
    pub canonical_name: String,
    pub source_kind: SkillSourceKind,
    pub source_locator: String,
    pub plugin_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Admin,
    User,
    Repo,
    System,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillToolDependency {
    pub kind: String,
    pub value: String,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct AgentHandle {
    pub thread_id: String,
    pub task_path: String,
    pub parent_thread_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpawnRequest {
    pub parent_thread_id: String,
    pub task_name: String,
    pub prompt: String,
    pub skill_context: String,
    pub cwd: PathBuf,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub fork_turns: ForkTurns,
    pub allow_delegation: bool,
    /// Additional native descendant levels required by a structural task
    /// container. Ordinary workflow agents set this to zero.
    pub minimum_descendant_depth: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentOutcome {
    pub status: AgentStatus,
    pub final_response: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait NativeHost: Send + Sync + 'static {
    async fn resolve_skills(
        &self,
        _parent_thread_id: &str,
        _repository: &Path,
        _source_revision: &str,
        requirements: &[SkillRequirement],
    ) -> Result<Vec<ResolvedSkill>, String> {
        if requirements.is_empty() {
            Ok(Vec::new())
        } else {
            Err("native host does not support skill resolution".into())
        }
    }
    async fn spawn(&self, request: SpawnRequest) -> Result<AgentHandle, String>;
    async fn status(&self, handle: &AgentHandle) -> Result<AgentStatus, String>;
    async fn wait(&self, handle: &AgentHandle) -> Result<AgentOutcome, String>;
    async fn cancel(&self, handle: &AgentHandle) -> Result<(), String>;
    async fn run_command(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        argv: &[String],
        cwd: Option<&Path>,
        timeout_ms: u64,
    ) -> Result<CommandOutcome, String>;
    async fn create_worktree(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        run_id: &str,
        step_id: &str,
        policy: &WorktreePolicy,
        source_revision: &str,
    ) -> Result<PathBuf, String>;
    async fn create_persistent_worktree(
        &self,
        _parent_thread_id: &str,
        _repository: &Path,
        _path: &Path,
        _source_revision: &str,
    ) -> Result<PathBuf, String> {
        Err("native host does not support persistent Automation worktrees".into())
    }
    async fn remove_worktree(
        &self,
        parent_thread_id: &str,
        repository: &Path,
        path: &Path,
    ) -> Result<(), String>;
    async fn request_approval(
        &self,
        parent_thread_id: &str,
        prompt: &str,
        choices: &[String],
    ) -> Result<Option<String>, String>;
    async fn emit_activity(&self, parent_thread_id: &str, message: &str);
    async fn persist_outputs(&self, _run_id: &str, _step_id: &str, _outputs: &StepOutputs) {}
}
