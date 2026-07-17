//! Narrow native capability exposed only to the pinned Orchestra extension.
//!
//! This wrapper deliberately preserves the active thread's `AgentControl`.
//! It does not expose the control plane itself to extensions.

use crate::agent::control::AgentControl;
use crate::agent::control::SpawnAgentForkMode;
use crate::agent::control::SpawnAgentOptions;
use crate::agent::next_thread_spawn_depth;
use crate::config::Config;
use crate::exec::ExecCapturePolicy;
use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::process_exec_tool_call;
use crate::skills::skills_load_input_from_config;
use crate::windows_sandbox::windows_sandbox_level_from_config;
use codex_core_plugins::PluginsManager;
use codex_core_skills::SkillLoadOutcome;
use codex_core_skills::SkillMetadata;
use codex_core_skills::SkillsService;
use codex_core_skills::injection::ORCHESTRA_LITERAL_TASK_MARKER;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::SandboxPermissions;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SkillScope;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum OrchestraForkTurns {
    None,
    All,
    Last(usize),
}

#[derive(Clone, Debug)]
pub struct OrchestraSpawnRequest {
    pub task_name: String,
    pub prompt: String,
    pub skill_context: String,
    pub cwd: AbsolutePathBuf,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub service_tier: Option<String>,
    pub fork_turns: OrchestraForkTurns,
    pub allow_delegation: bool,
    /// Structural native tasks (such as an Automation Issue task) may reserve
    /// a bounded descendant level without enabling general recursive agents.
    pub minimum_descendant_depth: i32,
}

#[derive(Clone, Debug)]
pub struct OrchestraAgentHandle {
    pub thread_id: ThreadId,
    pub task_path: AgentPath,
}

#[derive(Clone, Debug)]
pub struct OrchestraCommandRequest {
    pub argv: Vec<String>,
    pub cwd: AbsolutePathBuf,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub struct OrchestraCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug)]
pub struct OrchestraSkillRequirement {
    pub name: String,
    pub resources: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct OrchestraResolvedSkill {
    pub requirement: String,
    pub canonical_name: String,
    pub source_kind: OrchestraSkillSourceKind,
    pub source_locator: String,
    pub plugin_id: Option<String>,
    pub instructions: Vec<u8>,
    pub resources: BTreeMap<String, Vec<u8>>,
    pub tool_dependencies: Vec<OrchestraSkillToolDependency>,
}

#[derive(Clone, Copy, Debug)]
pub enum OrchestraSkillSourceKind {
    Admin,
    User,
    Repo,
    System,
}

#[derive(Clone, Debug)]
pub struct OrchestraSkillToolDependency {
    pub kind: String,
    pub value: String,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct OrchestraControl {
    control: AgentControl,
    parent_thread_id: ThreadId,
    parent_source: SessionSource,
    config: Config,
    skills_service: Arc<SkillsService>,
    plugins_manager: Arc<PluginsManager>,
}

impl OrchestraControl {
    pub(crate) fn new(
        control: AgentControl,
        parent_thread_id: ThreadId,
        parent_source: SessionSource,
        config: Config,
        skills_service: Arc<SkillsService>,
        plugins_manager: Arc<PluginsManager>,
    ) -> Self {
        Self {
            control,
            parent_thread_id,
            parent_source,
            config,
            skills_service,
            plugins_manager,
        }
    }

    pub async fn resolve_skills(
        &self,
        cwd: AbsolutePathBuf,
        source_revision: &str,
        requirements: &[OrchestraSkillRequirement],
    ) -> CodexResult<Vec<OrchestraResolvedSkill>> {
        let mut config = self.config.clone();
        config.cwd = cwd;
        let plugins_input = config.plugins_config_input();
        let plugin_outcome = self
            .plugins_manager
            .plugins_for_config(&plugins_input)
            .await;
        let skill_input =
            skills_load_input_from_config(&config, plugin_outcome.effective_plugin_skill_roots())
                .with_plugin_skill_snapshots(
                    self.plugins_manager
                        .plugin_skill_snapshots_for_config(&plugins_input),
                );
        let snapshot = self
            .skills_service
            .snapshot_for_config(&skill_input, None)
            .await;
        let outcome = snapshot.outcome();
        let connector_slugs = if requirements.iter().any(|item| !item.name.contains(':')) {
            crate::connectors::list_accessible_connectors_from_mcp_tools_with_options(
                &config, false,
            )
            .await
            .map_err(|error| {
                CodexErr::InvalidRequest(format!(
                    "failed to validate required skill names against connectors: {error}"
                ))
            })?
            .iter()
            .map(codex_connectors::metadata::connector_mention_slug)
            .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        let mut resolved = Vec::new();
        for requirement in requirements {
            let skill = resolve_required_skill(outcome, &requirement.name, &connector_slugs)?;
            let instructions = snapshot
                .read_skill_text(skill)
                .await
                .map_err(|error| {
                    CodexErr::InvalidRequest(format!(
                        "failed to read required skill `{}`: {error}",
                        requirement.name
                    ))
                })?
                .into_bytes();
            let mut resources = BTreeMap::new();
            for resource in &requirement.resources {
                let bytes = snapshot
                    .read_skill_resource(skill, resource)
                    .await
                    .map_err(|error| {
                        CodexErr::InvalidRequest(format!(
                            "failed to read resource `{resource}` for skill `{}`: {error}",
                            requirement.name
                        ))
                    })?;
                resources.insert(resource.clone(), bytes);
            }
            let canonical_name = skill
                .plugin_id
                .as_ref()
                .map(|plugin| format!("{plugin}:{}", skill.name))
                .unwrap_or_else(|| skill.name.clone());
            let source_kind = match skill.scope {
                SkillScope::Admin => OrchestraSkillSourceKind::Admin,
                SkillScope::User => OrchestraSkillSourceKind::User,
                SkillScope::Repo => OrchestraSkillSourceKind::Repo,
                SkillScope::System => OrchestraSkillSourceKind::System,
            };
            let tool_dependencies = skill
                .dependencies
                .as_ref()
                .map(|dependencies| {
                    dependencies
                        .tools
                        .iter()
                        .map(|tool| OrchestraSkillToolDependency {
                            kind: tool.r#type.clone(),
                            value: tool.value.clone(),
                            description: tool.description.clone(),
                            transport: tool.transport.clone(),
                            command: tool.command.clone(),
                            url: tool.url.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let source_locator = skill_source_locator(skill, &config.cwd, source_revision)?;
            resolved.push(OrchestraResolvedSkill {
                requirement: requirement.name.clone(),
                canonical_name,
                source_kind,
                source_locator,
                plugin_id: skill.plugin_id.clone(),
                instructions,
                resources,
                tool_dependencies,
            });
        }
        Ok(resolved)
    }

    pub async fn spawn(&self, request: OrchestraSpawnRequest) -> CodexResult<OrchestraAgentHandle> {
        let fork_mode = match request.fork_turns {
            OrchestraForkTurns::None => None,
            OrchestraForkTurns::All => Some(SpawnAgentForkMode::FullHistory),
            OrchestraForkTurns::Last(value) if value > 0 => {
                Some(SpawnAgentForkMode::LastNTurns(value))
            }
            OrchestraForkTurns::Last(_) => {
                return Err(CodexErr::InvalidRequest(
                    "fork_turns must be positive".into(),
                ));
            }
        };
        if matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory))
            && (request.model != self.config.model.as_deref().unwrap_or_default()
                || request.reasoning_effort != self.config.model_reasoning_effort)
        {
            return Err(CodexErr::InvalidRequest(
                "full-history Orchestra agents must inherit model and reasoning effort".into(),
            ));
        }
        let mut config = self.config.clone();
        if !matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory)) {
            config.model = Some(request.model);
            config.model_reasoning_effort = request.reasoning_effort;
        }
        if request.service_tier.is_some() {
            config.service_tier = request.service_tier;
        }
        let child_depth = next_thread_spawn_depth(&self.parent_source);
        config.cwd = request.cwd;
        config.include_skill_instructions = false;
        if !request.skill_context.is_empty() {
            config.developer_instructions = Some(orchestra_developer_instructions(
                config.developer_instructions.take(),
                request.skill_context,
            ));
        }
        config.agent_max_depth = effective_agent_max_depth(
            config.agent_max_depth,
            child_depth,
            request.allow_delegation,
            request.minimum_descendant_depth,
        );
        let parent_path = self
            .parent_source
            .get_agent_path()
            .unwrap_or_else(AgentPath::root);
        let task_path = parent_path
            .join(&request.task_name)
            .map_err(CodexErr::InvalidRequest)?;
        let source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: self.parent_thread_id,
            depth: child_depth,
            agent_path: Some(task_path.clone()),
            agent_nickname: None,
            agent_role: None,
        });
        let live = self
            .control
            .spawn_agent_with_metadata(
                config,
                orchestra_initial_input(request.prompt),
                Some(source),
                SpawnAgentOptions {
                    fork_parent_spawn_call_id: fork_mode
                        .as_ref()
                        .map(|_| format!("orchestra:{}", request.task_name)),
                    fork_mode,
                    parent_thread_id: Some(self.parent_thread_id),
                    environments: None,
                },
            )
            .await?;
        Ok(OrchestraAgentHandle {
            thread_id: live.thread_id,
            task_path,
        })
    }

    pub async fn status(&self, handle: &OrchestraAgentHandle) -> AgentStatus {
        self.control.get_status(handle.thread_id).await
    }

    pub async fn wait(&self, handle: &OrchestraAgentHandle) -> CodexResult<AgentStatus> {
        let mut receiver = self.control.subscribe_status(handle.thread_id).await?;
        loop {
            let status = receiver.borrow().clone();
            if !matches!(
                status,
                AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Interrupted
            ) {
                return Ok(status);
            }
            receiver
                .changed()
                .await
                .map_err(|_| CodexErr::InternalAgentDied)?;
        }
    }

    pub async fn cancel(&self, handle: &OrchestraAgentHandle) -> CodexResult<()> {
        self.control
            .interrupt_agent(handle.thread_id)
            .await
            .map(|_| ())
    }

    pub async fn run_command(
        &self,
        request: OrchestraCommandRequest,
    ) -> CodexResult<OrchestraCommandOutput> {
        let windows_level = windows_sandbox_level_from_config(&self.config);
        let mut env = HashMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), path);
        }
        let params = ExecParams {
            command: request.argv,
            cwd: request.cwd,
            expiration: ExecExpiration::from(request.timeout_ms),
            capture_policy: ExecCapturePolicy::ShellTool,
            env,
            network: None,
            network_environment_id: None,
            sandbox_permissions: SandboxPermissions::UseDefault,
            windows_sandbox_level: windows_level,
            windows_sandbox_private_desktop: self
                .config
                .permissions
                .windows_sandbox_private_desktop,
            justification: None,
            arg0: None,
        };
        let output = process_exec_tool_call(
            params,
            self.config.permissions.permission_profile(),
            &self.config.cwd,
            &self.config.effective_workspace_roots(),
            &self.config.codex_linux_sandbox_exe,
            self.config.features.use_legacy_landlock(),
            None,
        )
        .await?;
        Ok(OrchestraCommandOutput {
            exit_code: output.exit_code,
            stdout: output.stdout.text,
            stderr: output.stderr.text,
        })
    }
}

fn effective_agent_max_depth(
    configured: i32,
    child_depth: i32,
    allow_delegation: bool,
    minimum_descendant_depth: i32,
) -> i32 {
    if !allow_delegation {
        child_depth
    } else if minimum_descendant_depth > 0 {
        configured.max(child_depth.saturating_add(minimum_descendant_depth))
    } else {
        configured
    }
}

fn resolve_required_skill<'a>(
    outcome: &'a SkillLoadOutcome,
    requirement: &str,
    connector_slugs: &BTreeSet<String>,
) -> CodexResult<&'a SkillMetadata> {
    if !requirement.contains(':') && connector_slugs.contains(requirement) {
        return Err(CodexErr::InvalidRequest(format!(
            "required skill `{requirement}` collides with a connector; use a qualified name"
        )));
    }
    let matches: Vec<_> = outcome
        .skills
        .iter()
        .filter(|skill| {
            let canonical = skill
                .plugin_id
                .as_ref()
                .map(|plugin| format!("{plugin}:{}", skill.name))
                .unwrap_or_else(|| skill.name.clone());
            if requirement.contains(':') {
                canonical == requirement
            } else {
                skill.name == requirement
            }
        })
        .collect();
    let enabled: Vec<_> = matches
        .iter()
        .copied()
        .filter(|skill| outcome.is_skill_enabled(skill))
        .collect();
    match enabled.as_slice() {
        [skill] => Ok(*skill),
        [] if !matches.is_empty() => Err(CodexErr::InvalidRequest(format!(
            "required skill `{requirement}` is disabled"
        ))),
        [] => Err(CodexErr::InvalidRequest(format!(
            "required skill `{requirement}` is not installed"
        ))),
        _ => Err(CodexErr::InvalidRequest(format!(
            "required skill `{requirement}` is ambiguous; use its qualified name"
        ))),
    }
}

fn skill_source_locator(
    skill: &SkillMetadata,
    cwd: &AbsolutePathBuf,
    source_revision: &str,
) -> CodexResult<String> {
    if skill.scope != SkillScope::Repo {
        return Ok(skill.path_to_skills_md.to_string_lossy().into_owned());
    }
    let relative = skill
        .path_to_skills_md
        .strip_prefix(cwd.as_path())
        .map_err(|_| {
            CodexErr::InvalidRequest(format!(
                "repo skill `{}` is outside the source-revision checkout",
                skill.name
            ))
        })?;
    Ok(format!(
        "git:{source_revision}:{}",
        relative.to_string_lossy()
    ))
}

fn orchestra_initial_input(prompt: String) -> Vec<UserInput> {
    vec![
        UserInput::Text {
            text: prompt,
            text_elements: Vec::new(),
        },
        UserInput::Mention {
            name: "Orchestra literal task".into(),
            path: ORCHESTRA_LITERAL_TASK_MARKER.into(),
        },
    ]
}

fn orchestra_developer_instructions(existing: Option<String>, prompt: String) -> String {
    let orchestra = format!("# Orchestra skill snapshot\n\n{prompt}");
    match existing {
        Some(existing) => format!("{existing}\n\n{orchestra}"),
        None => orchestra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect_explicit_skill_mentions;
    use codex_core_skills::SkillMetadata;
    use codex_core_skills::SkillPolicy;
    use codex_protocol::protocol::SkillScope;
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn skill(name: &str, path: &str, allow_implicit_invocation: bool) -> SkillMetadata {
        SkillMetadata {
            name: name.into(),
            description: format!("{name} description"),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: Some(SkillPolicy {
                allow_implicit_invocation: Some(allow_implicit_invocation),
                products: Vec::new(),
            }),
            path_to_skills_md: AbsolutePathBuf::try_from(PathBuf::from(path)).unwrap(),
            scope: SkillScope::User,
            plugin_id: None,
        }
    }

    fn text_input(text: &str) -> Vec<UserInput> {
        vec![UserInput::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }]
    }

    #[test]
    fn structural_issue_task_reserves_one_native_child_level_only_when_requested() {
        assert_eq!(effective_agent_max_depth(1, 1, true, 0), 1);
        assert_eq!(effective_agent_max_depth(1, 1, true, 1), 2);
        assert_eq!(effective_agent_max_depth(4, 1, true, 1), 4);
        assert_eq!(effective_agent_max_depth(4, 2, false, 0), 2);
    }

    #[test]
    fn orchestra_initial_input_cannot_select_ambient_skills() {
        let input = orchestra_initial_input("Use $wayfinder literally.".into());
        let [
            UserInput::Text {
                text,
                text_elements,
            },
            UserInput::Mention { path, .. },
        ] = input.as_slice()
        else {
            panic!("Orchestra prompt should be submitted as one text input");
        };

        assert_eq!(text, "Use $wayfinder literally.");
        assert_eq!(path, ORCHESTRA_LITERAL_TASK_MARKER);
        assert!(text_elements.is_empty());
        let skills = vec![skill("wayfinder", "/tmp/wayfinder/SKILL.md", false)];
        assert!(
            collect_explicit_skill_mentions(&input, &skills, &HashSet::new(), &HashMap::new(),)
                .is_empty()
        );
        assert_eq!(
            orchestra_developer_instructions(
                Some("Parent policy".into()),
                "Recorded $wayfinder instructions".into(),
            ),
            "Parent policy\n\n# Orchestra skill snapshot\n\nRecorded $wayfinder instructions"
        );
    }

    #[test]
    fn explicit_mention_selects_a_skill_hidden_from_implicit_invocation() {
        let inputs = text_input("Use $wayfinder for this task.");
        let skills = vec![skill(
            "wayfinder",
            "/tmp/wayfinder/SKILL.md",
            /*allow_implicit_invocation*/ false,
        )];

        let selected =
            collect_explicit_skill_mentions(&inputs, &skills, &HashSet::new(), &HashMap::new());

        assert_eq!(selected, skills);
    }

    #[test]
    fn missing_and_ambiguous_plain_skill_mentions_do_not_select_a_skill() {
        let skills = vec![
            skill("wayfinder", "/tmp/one/SKILL.md", false),
            skill("wayfinder", "/tmp/two/SKILL.md", false),
        ];

        for prompt in ["Use $missing.", "Use $wayfinder."] {
            let selected = collect_explicit_skill_mentions(
                &text_input(prompt),
                &skills,
                &HashSet::new(),
                &HashMap::new(),
            );
            assert!(selected.is_empty(), "unexpected selection for {prompt}");
        }
    }

    #[test]
    fn required_skill_resolution_rejects_missing_disabled_and_ambiguous_names() {
        let one = skill("wayfinder", "/tmp/one/SKILL.md", false);
        let two = skill("wayfinder", "/tmp/two/SKILL.md", false);
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = vec![one.clone(), two];
        assert!(
            resolve_required_skill(&outcome, "missing", &BTreeSet::new())
                .unwrap_err()
                .to_string()
                .contains("not installed")
        );
        assert!(
            resolve_required_skill(&outcome, "wayfinder", &BTreeSet::new())
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        outcome.skills.pop();
        outcome.disabled_paths.insert(one.path_to_skills_md.clone());
        assert!(
            resolve_required_skill(&outcome, "wayfinder", &BTreeSet::new())
                .unwrap_err()
                .to_string()
                .contains("disabled")
        );
    }

    #[test]
    fn qualified_plugin_skill_identity_disambiguates_plain_duplicates() {
        let one = skill("review", "/tmp/one/SKILL.md", true);
        let mut two = skill("review", "/tmp/two/SKILL.md", true);
        two.plugin_id = Some("plugin-two".into());
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = vec![one, two.clone()];
        assert_eq!(
            resolve_required_skill(&outcome, "plugin-two:review", &BTreeSet::new()).unwrap(),
            &two
        );
    }

    #[test]
    fn plain_skill_names_reject_connector_collisions() {
        let mut outcome = SkillLoadOutcome::default();
        outcome.skills = vec![skill("calendar", "/tmp/calendar/SKILL.md", false)];
        let connectors = BTreeSet::from(["calendar".to_string()]);
        assert!(
            resolve_required_skill(&outcome, "calendar", &connectors)
                .unwrap_err()
                .to_string()
                .contains("collides with a connector")
        );
    }

    #[test]
    fn repo_skill_locator_is_revision_qualified_and_checkout_independent() {
        let mut repo_skill = skill(
            "review",
            "/tmp/checkout/.agents/skills/review/SKILL.md",
            false,
        );
        repo_skill.scope = SkillScope::Repo;
        let cwd = AbsolutePathBuf::try_from(PathBuf::from("/tmp/checkout")).unwrap();
        assert_eq!(
            skill_source_locator(&repo_skill, &cwd, "abc123").unwrap(),
            "git:abc123:.agents/skills/review/SKILL.md"
        );
    }
}
