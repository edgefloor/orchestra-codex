use super::*;
mod automation_projection;

use automation_projection::AutomationClaimFocus;
use automation_projection::project_automation_run;
use automation_projection::project_automation_steer_issue_response;
use codex_app_server_protocol as protocol;
use codex_app_server_protocol::AutomationCancelIssueParams;
use codex_app_server_protocol::AutomationLinearReadParams;
use codex_app_server_protocol::AutomationLinearReadResponse;
use codex_app_server_protocol::AutomationQueueReadParams;
use codex_app_server_protocol::AutomationQueueReadResponse;
use codex_app_server_protocol::AutomationReconcileParams;
use codex_app_server_protocol::AutomationRunFixtureParams;
use codex_app_server_protocol::AutomationRunParams;
use codex_app_server_protocol::AutomationRunResponse;
use codex_app_server_protocol::AutomationStartParams;
use codex_app_server_protocol::AutomationStatusParams;
use codex_app_server_protocol::AutomationSteerIssueParams;
use codex_app_server_protocol::AutomationSteerIssueResponse;
use codex_app_server_protocol::AutomationValidateParams;
use codex_app_server_protocol::AutomationValidateResponse;
use codex_app_server_protocol::OrchestraInvokeParams;
use codex_app_server_protocol::OrchestraLifecycleKind;
use codex_app_server_protocol::OrchestraPromotionStatus;
use codex_app_server_protocol::OrchestraQueryKind;
use codex_app_server_protocol::OrchestraQueryParams;
use codex_app_server_protocol::OrchestraQueryResponse;
use codex_app_server_protocol::OrchestraReplayEvent;
use codex_app_server_protocol::OrchestraResumeParams;
use codex_app_server_protocol::OrchestraRunDigest;
use codex_app_server_protocol::OrchestraRunParams;
use codex_app_server_protocol::OrchestraRunProjection;
use codex_app_server_protocol::OrchestraRunResponse;
use codex_app_server_protocol::OrchestraRunStatus;
use codex_app_server_protocol::OrchestraStepKind;
use codex_app_server_protocol::OrchestraStepProjection;
use codex_app_server_protocol::OrchestraStepStatus;
use codex_app_server_protocol::OrchestraTaskReplay;
use codex_app_server_protocol::OrchestraValidateParams;
use codex_app_server_protocol::OrchestraValidateResponse;
use codex_app_server_protocol::OrchestraWorkflowPlan;
use codex_app_server_protocol::OrchestraWorkflowStep;
use codex_orchestra_core as core;
use codex_orchestra_core::Action;
use codex_orchestra_core::EvidenceAvailability;
use codex_orchestra_core::EvidenceKind;
use codex_orchestra_core::EvidenceProvenance;
use codex_orchestra_core::ExecutionPlan;
use codex_orchestra_core::ExecutionQueryBudget;
use codex_orchestra_core::ExecutionQueryResult;
use codex_orchestra_core::ExecutionSelector;
use codex_orchestra_core::HistoryCursor;
use codex_orchestra_core::PromotionStatus;
use codex_orchestra_core::RunCheckpoint;
use codex_orchestra_core::RunOutcome;
use codex_orchestra_core::RunStatus;
use codex_orchestra_core::StepStatus;
use codex_orchestra_extension::AutomationLinearReadKind as CoreLinearReadKind;
use codex_orchestra_extension::AutomationLinearReadStatus as CoreLinearReadStatus;
use codex_orchestra_extension::OrchestraService;

const MAX_PROJECTION_TEXT_BYTES: usize = 4096;

#[derive(Clone)]
pub(crate) struct OrchestraRequestProcessor {
    service: OrchestraService,
}

impl OrchestraRequestProcessor {
    pub(crate) fn new(thread_manager: &Arc<ThreadManager>) -> Self {
        Self {
            service: OrchestraService::new(Arc::downgrade(thread_manager)),
        }
    }

    pub(crate) async fn validate(
        &self,
        params: OrchestraValidateParams,
    ) -> Result<OrchestraValidateResponse, JSONRPCErrorError> {
        let plan = self
            .service
            .validate(&params.thread_id, &params.workflow_path)
            .await
            .map_err(orchestra_error)?;
        Ok(OrchestraValidateResponse {
            valid: true,
            plan: project_plan(plan),
        })
    }

    pub(crate) async fn validate_automation(
        &self,
        params: AutomationValidateParams,
    ) -> Result<AutomationValidateResponse, JSONRPCErrorError> {
        let issue = core_automation_issue(params.fixture_issue);
        self.service
            .validate_automation(
                &params.thread_id,
                &params.profile_path,
                issue,
                params.attempt,
            )
            .await
            .map(project_automation_validation)
            .map_err(orchestra_error)
    }

    pub(crate) async fn run_automation_fixture(
        &self,
        params: AutomationRunFixtureParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .start_automation_fixture(
                &params.thread_id,
                &params.profile_path,
                core_automation_issue(params.fixture_issue),
                params.attempt,
            )
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn start_automation(
        &self,
        params: AutomationStartParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .start_automation(&params.thread_id, &params.profile_path)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn steer_automation_issue(
        &self,
        params: AutomationSteerIssueParams,
    ) -> Result<AutomationSteerIssueResponse, JSONRPCErrorError> {
        let focused_claim_id = params.claim_id.clone();
        self.service
            .steer_automation_issue(
                &params.thread_id,
                &params.run_id,
                &params.claim_id,
                &params.input,
            )
            .await
            .map(|(run, receipt)| {
                project_automation_steer_issue_response(run, receipt, &focused_claim_id)
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn read_linear_automation(
        &self,
        params: AutomationLinearReadParams,
    ) -> Result<AutomationLinearReadResponse, JSONRPCErrorError> {
        let kind = match params.kind {
            protocol::AutomationLinearReadKind::Candidates => CoreLinearReadKind::Candidates,
            protocol::AutomationLinearReadKind::Terminal => CoreLinearReadKind::Terminal,
            protocol::AutomationLinearReadKind::Refresh => CoreLinearReadKind::Refresh,
        };
        self.service
            .read_linear_automation(
                &params.thread_id,
                &params.profile_path,
                kind,
                params.after.as_deref(),
                params.first,
                params.issue_identifier.as_deref(),
            )
            .await
            .map(|result| AutomationLinearReadResponse {
                status: match result.status {
                    CoreLinearReadStatus::Ready => protocol::AutomationLinearReadStatus::Ready,
                    CoreLinearReadStatus::Skipped => protocol::AutomationLinearReadStatus::Skipped,
                },
                issues: result
                    .issues
                    .into_iter()
                    .map(project_automation_issue)
                    .collect(),
                has_next_page: result.has_next_page,
                end_cursor: result.end_cursor,
                next_action: automation_bounded_text(result.next_action),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn read_automation_queue(
        &self,
        params: AutomationQueueReadParams,
    ) -> Result<AutomationQueueReadResponse, JSONRPCErrorError> {
        let category = core_queue_category(params.category);
        self.service
            .read_automation_queue(
                &params.thread_id,
                &params.run_id,
                category,
                params.offset,
                params.limit,
            )
            .await
            .map(|page| AutomationQueueReadResponse {
                category: protocol_queue_category(page.category),
                total: page.total,
                items: page
                    .items
                    .into_iter()
                    .map(project_automation_queue_item)
                    .collect(),
                next_offset: page.next_offset,
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn cancel_automation(
        &self,
        params: AutomationRunParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .cancel_automation(&params.thread_id, &params.run_id)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn cancel_automation_issue(
        &self,
        params: AutomationCancelIssueParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .cancel_automation_issue(&params.thread_id, &params.run_id, &params.claim_id)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn automation_status(
        &self,
        params: AutomationStatusParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .automation_status(&params.thread_id, &params.run_id)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(
                    run,
                    params
                        .focused_issue_id
                        .as_deref()
                        .map(AutomationClaimFocus::IssueId),
                ),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn pause_automation(
        &self,
        params: AutomationRunParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .pause_automation(&params.thread_id, &params.run_id)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn refresh_automation(
        &self,
        params: AutomationReconcileParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .refresh_automation(&params.thread_id, &params.run_id, &params.profile_path)
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn resume_automation(
        &self,
        params: AutomationReconcileParams,
    ) -> Result<AutomationRunResponse, JSONRPCErrorError> {
        self.service
            .reconcile_automation(
                &params.thread_id,
                &params.run_id,
                &params.profile_path,
                true,
            )
            .await
            .map(|run| AutomationRunResponse {
                run: project_automation_run(run, None),
            })
            .map_err(orchestra_error)
    }

    pub(crate) async fn invoke(
        &self,
        params: OrchestraInvokeParams,
    ) -> Result<OrchestraRunResponse, JSONRPCErrorError> {
        let outcome = self
            .service
            .run(
                &params.thread_id,
                &params.workflow_path,
                params.inputs.as_ref(),
            )
            .await
            .map_err(orchestra_error)?;
        Ok(run_response(outcome_checkpoint(outcome)))
    }

    pub(crate) async fn resume(
        &self,
        params: OrchestraResumeParams,
    ) -> Result<OrchestraRunResponse, JSONRPCErrorError> {
        let outcome = self
            .service
            .resume(
                &params.thread_id,
                &params.run_id,
                params.approval_decision.as_deref(),
                params.inputs.as_ref(),
            )
            .await
            .map_err(orchestra_error)?;
        Ok(run_response(outcome_checkpoint(outcome)))
    }

    pub(crate) async fn status(
        &self,
        params: OrchestraRunParams,
    ) -> Result<OrchestraRunResponse, JSONRPCErrorError> {
        self.service
            .status(&params.thread_id, &params.run_id)
            .await
            .map(run_response)
            .map_err(orchestra_error)
    }

    pub(crate) async fn cancel(
        &self,
        params: OrchestraRunParams,
    ) -> Result<OrchestraRunResponse, JSONRPCErrorError> {
        self.service
            .cancel(&params.thread_id, &params.run_id)
            .await
            .map(run_response)
            .map_err(orchestra_error)
    }

    pub(crate) async fn query(
        &self,
        params: OrchestraQueryParams,
    ) -> Result<OrchestraQueryResponse, JSONRPCErrorError> {
        if matches!(params.selector, OrchestraQueryKind::Digest) {
            let digest = self
                .service
                .digest(
                    &params.thread_id,
                    &params.run_id,
                    params.max_bytes.map_or(4096, |value| value as usize),
                )
                .await
                .map_err(orchestra_error)?;
            return Ok(OrchestraQueryResponse {
                result: codex_app_server_protocol::OrchestraQueryResult::Digest(
                    OrchestraRunDigest {
                        run_id: digest.run_id,
                        state_sha256: digest.state_sha256,
                        text: digest.text,
                        omitted_steps: digest.omitted_steps,
                    },
                ),
            });
        }
        let selector = match params.selector {
            OrchestraQueryKind::Run => ExecutionSelector::Run,
            OrchestraQueryKind::Steps => ExecutionSelector::Steps {
                after: params.after,
            },
            OrchestraQueryKind::Outputs => ExecutionSelector::Outputs {
                step_id: params.step_id,
                after: params.after,
            },
            OrchestraQueryKind::Evidence => ExecutionSelector::Evidence {
                step_id: params.step_id,
                after: params.after,
            },
            OrchestraQueryKind::EvidenceContent => ExecutionSelector::EvidenceContent {
                evidence_id: params.evidence_id.ok_or_else(|| {
                    orchestra_error(core::ExecutionQueryError::InvalidIdentity.to_string())
                })?,
            },
            OrchestraQueryKind::History => ExecutionSelector::History {
                after: params.history_after.map(|cursor| HistoryCursor {
                    sequence: cursor.sequence,
                    item_id: cursor.item_id,
                    revision: cursor.revision,
                }),
            },
            OrchestraQueryKind::Digest => unreachable!(),
        };
        let defaults = ExecutionQueryBudget::default();
        let result = self
            .service
            .query(
                &params.thread_id,
                &params.run_id,
                selector,
                ExecutionQueryBudget {
                    max_items: params
                        .max_items
                        .map_or(defaults.max_items, |value| value as usize),
                    max_bytes: params
                        .max_bytes
                        .map_or(defaults.max_bytes, |value| value as usize),
                },
            )
            .await
            .map_err(orchestra_error)?;
        Ok(OrchestraQueryResponse {
            result: project_query_result(result),
        })
    }
}

fn core_automation_issue(issue: protocol::AutomationIssue) -> core::AutomationIssue {
    core::AutomationIssue {
        id: issue.id,
        identifier: issue.identifier,
        title: issue.title,
        description: issue.description,
        priority: issue.priority,
        state: issue.state,
        branch_name: issue.branch_name,
        url: issue.url,
        labels: issue.labels,
        blocked_by: issue
            .blocked_by
            .into_iter()
            .map(|blocker| core::AutomationIssueBlocker {
                id: blocker.id,
                identifier: blocker.identifier,
                state: blocker.state,
            })
            .collect(),
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    }
}

fn project_automation_issue(issue: core::AutomationIssue) -> protocol::AutomationIssue {
    protocol::AutomationIssue {
        id: issue.id,
        identifier: issue.identifier,
        title: issue.title,
        description: issue.description,
        priority: issue.priority,
        state: issue.state,
        branch_name: issue.branch_name,
        url: issue.url,
        labels: issue.labels,
        blocked_by: issue
            .blocked_by
            .into_iter()
            .map(|blocker| protocol::AutomationIssueBlocker {
                id: blocker.id,
                identifier: blocker.identifier,
                state: blocker.state,
            })
            .collect(),
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    }
}

fn core_queue_category(
    category: protocol::AutomationQueueCategory,
) -> core::AutomationQueueCategory {
    match category {
        protocol::AutomationQueueCategory::Queued => core::AutomationQueueCategory::Queued,
        protocol::AutomationQueueCategory::Running => core::AutomationQueueCategory::Running,
        protocol::AutomationQueueCategory::Blocked => core::AutomationQueueCategory::Blocked,
        protocol::AutomationQueueCategory::WaitingGate => {
            core::AutomationQueueCategory::WaitingGate
        }
        protocol::AutomationQueueCategory::Handoff => core::AutomationQueueCategory::Handoff,
        protocol::AutomationQueueCategory::Terminal => core::AutomationQueueCategory::Terminal,
    }
}

fn protocol_queue_category(
    category: core::AutomationQueueCategory,
) -> protocol::AutomationQueueCategory {
    match category {
        core::AutomationQueueCategory::Queued => protocol::AutomationQueueCategory::Queued,
        core::AutomationQueueCategory::Running => protocol::AutomationQueueCategory::Running,
        core::AutomationQueueCategory::Blocked => protocol::AutomationQueueCategory::Blocked,
        core::AutomationQueueCategory::WaitingGate => {
            protocol::AutomationQueueCategory::WaitingGate
        }
        core::AutomationQueueCategory::Handoff => protocol::AutomationQueueCategory::Handoff,
        core::AutomationQueueCategory::Terminal => protocol::AutomationQueueCategory::Terminal,
    }
}

fn project_automation_queue_item(
    item: core::AutomationQueueProjectionItem,
) -> protocol::AutomationQueueItemProjection {
    protocol::AutomationQueueItemProjection {
        issue_id: item.issue_id,
        issue_identifier: item.issue_identifier,
        issue_title: automation_bounded_text(item.issue_title),
        state: item.state,
        priority: item.priority,
        claim_id: item.claim_id,
        category: protocol_queue_category(item.category),
        next_action: automation_bounded_text(item.next_action),
        blocked_by: item
            .blocked_by
            .into_iter()
            .map(|blocker| protocol::AutomationQueueBlockerProjection {
                id: blocker.id.map(automation_bounded_text),
                identifier: blocker.identifier.map(automation_bounded_text),
                state: blocker.state.map(automation_bounded_text),
            })
            .collect(),
    }
}

fn automation_bounded_text(text: String) -> protocol::OrchestraBoundedText {
    let bounded = bounded_text(text);
    protocol::OrchestraBoundedText {
        truncated: bounded.ends_with('…'),
        text: bounded,
    }
}

fn project_automation_validation(
    result: core::AutomationValidationResult,
) -> AutomationValidateResponse {
    AutomationValidateResponse {
        valid: result.valid,
        profile: result.profile.map(project_automation_profile),
        profile_digest: result.profile_digest,
        preview: result
            .preview
            .map(|preview| protocol::AutomationWorkflowPreview {
                rendered_prompt: preview.rendered_prompt,
                workflow: preview.workflow,
                effects: preview
                    .effects
                    .into_iter()
                    .map(project_automation_effect)
                    .collect(),
                inputs: preview
                    .inputs
                    .into_iter()
                    .map(|input| protocol::AutomationWorkflowInput {
                        name: input.name,
                        kind: project_automation_input_kind(input.kind),
                        required: input.required,
                        default: input.default,
                    })
                    .collect(),
                secret_references: preview
                    .secret_references
                    .into_iter()
                    .map(project_automation_secret)
                    .collect(),
            }),
        diagnostics: result
            .diagnostics
            .into_iter()
            .map(|diagnostic| protocol::AutomationDiagnostic {
                severity: match diagnostic.severity {
                    core::AutomationValidationSeverity::Error => {
                        protocol::AutomationValidationSeverity::Error
                    }
                    core::AutomationValidationSeverity::Warning => {
                        protocol::AutomationValidationSeverity::Warning
                    }
                },
                code: project_automation_diagnostic_code(diagnostic.code),
                path: diagnostic.path,
                message: diagnostic.message,
            })
            .collect(),
    }
}

fn project_automation_profile(profile: core::AutomationProfile) -> protocol::AutomationProfile {
    protocol::AutomationProfile {
        tracker: protocol::AutomationTrackerProfile {
            kind: profile.tracker.kind,
            endpoint: profile.tracker.endpoint,
            project_slug: profile.tracker.project_slug,
            required_labels: profile.tracker.required_labels,
            active_states: profile.tracker.active_states,
            terminal_states: profile.tracker.terminal_states,
            credential: project_automation_secret(profile.tracker.credential),
        },
        polling: protocol::AutomationPollingProfile {
            interval_ms: profile.polling.interval_ms,
        },
        workspace: protocol::AutomationWorkspaceProfile {
            root: profile.workspace.root,
        },
        hooks: protocol::AutomationHooksProfile {
            after_create: profile.hooks.after_create,
            before_run: profile.hooks.before_run,
            after_run: profile.hooks.after_run,
            before_remove: profile.hooks.before_remove,
            timeout_ms: profile.hooks.timeout_ms,
        },
        agent: protocol::AutomationAgentProfile {
            max_concurrent_agents: profile.agent.max_concurrent_agents,
            max_turns: profile.agent.max_turns,
            max_retry_backoff_ms: profile.agent.max_retry_backoff_ms,
            max_concurrent_agents_by_state: profile.agent.max_concurrent_agents_by_state,
        },
        codex: protocol::AutomationCodexPolicy {
            approval_policy: profile.codex.approval_policy,
            thread_sandbox: profile.codex.thread_sandbox,
            turn_sandbox_policy: profile.codex.turn_sandbox_policy,
            turn_timeout_ms: profile.codex.turn_timeout_ms,
            read_timeout_ms: profile.codex.read_timeout_ms,
            stall_timeout_ms: profile.codex.stall_timeout_ms,
        },
        orchestra: protocol::AutomationOrchestraProfile {
            workflow_path: profile.orchestra.workflow_path,
            workflow_sha256: profile.orchestra.workflow_sha256,
            workflow_name: profile.orchestra.workflow_name,
            effects: profile
                .orchestra
                .effects
                .into_iter()
                .map(project_automation_effect)
                .collect(),
        },
        prompt_template: profile.prompt_template,
    }
}

fn project_automation_effect(effect: core::AutomationEffect) -> protocol::AutomationEffect {
    match effect {
        core::AutomationEffect::TrackerComment => protocol::AutomationEffect::TrackerComment,
        core::AutomationEffect::TrackerTransition => protocol::AutomationEffect::TrackerTransition,
        core::AutomationEffect::TrackerLinkPullRequest => {
            protocol::AutomationEffect::TrackerLinkPullRequest
        }
    }
}

fn project_automation_secret(
    secret: core::AutomationSecretReference,
) -> protocol::AutomationSecretReference {
    protocol::AutomationSecretReference {
        kind: match secret.kind {
            core::AutomationSecretKind::Environment => protocol::AutomationSecretKind::Environment,
            core::AutomationSecretKind::InlineDigest => {
                protocol::AutomationSecretKind::InlineDigest
            }
        },
        reference: secret.reference,
        digest: secret.digest,
    }
}

fn project_automation_input_kind(kind: core::InputKind) -> protocol::AutomationWorkflowInputKind {
    match kind {
        core::InputKind::String => protocol::AutomationWorkflowInputKind::String,
        core::InputKind::Number => protocol::AutomationWorkflowInputKind::Number,
        core::InputKind::Boolean => protocol::AutomationWorkflowInputKind::Boolean,
        core::InputKind::Object => protocol::AutomationWorkflowInputKind::Object,
        core::InputKind::Array => protocol::AutomationWorkflowInputKind::Array,
        core::InputKind::Json => protocol::AutomationWorkflowInputKind::Json,
    }
}

fn project_automation_diagnostic_code(
    code: core::AutomationDiagnosticCode,
) -> protocol::AutomationDiagnosticCode {
    use core::AutomationDiagnosticCode as Core;
    use protocol::AutomationDiagnosticCode as Wire;
    match code {
        Core::MissingWorkflowFile => Wire::MissingWorkflowFile,
        Core::WorkflowParseError => Wire::WorkflowParseError,
        Core::WorkflowFrontMatterNotAMap => Wire::WorkflowFrontMatterNotAMap,
        Core::MissingField => Wire::MissingField,
        Core::InvalidValue => Wire::InvalidValue,
        Core::UnknownField => Wire::UnknownField,
        Core::UnsupportedTracker => Wire::UnsupportedTracker,
        Core::MissingSecret => Wire::MissingSecret,
        Core::ProhibitedCodexCommand => Wire::ProhibitedCodexCommand,
        Core::PolicyBroadening => Wire::PolicyBroadening,
        Core::UnsafeWorkspaceRoot => Wire::UnsafeWorkspaceRoot,
        Core::MissingOrchestraExtension => Wire::MissingOrchestraExtension,
        Core::UnsupportedEffect => Wire::UnsupportedEffect,
        Core::WorkflowCompileError => Wire::WorkflowCompileError,
        Core::WorkflowInputMissing => Wire::WorkflowInputMissing,
        Core::WorkflowInputIncompatible => Wire::WorkflowInputIncompatible,
        Core::WorkflowInputNeedsDefault => Wire::WorkflowInputNeedsDefault,
        Core::TemplateParseError => Wire::TemplateParseError,
        Core::TemplateRenderError => Wire::TemplateRenderError,
    }
}

fn project_query_result(
    result: ExecutionQueryResult,
) -> codex_app_server_protocol::OrchestraQueryResult {
    match result {
        ExecutionQueryResult::Run(run) => {
            protocol::OrchestraQueryResult::Run(protocol::OrchestraExecutionRunProjection {
                schema_version: run.schema_version,
                run_id: run.run_id,
                workflow_sha256: run.workflow_sha256,
                source_revision: run.source_revision,
                status: project_run_status(run.status),
                promotion: project_promotion(run.promotion),
                step_counts: protocol::OrchestraStepCounts {
                    pending: run.step_counts.pending,
                    running: run.step_counts.running,
                    retrying: run.step_counts.retrying,
                    waiting_approval: run.step_counts.waiting_approval,
                    completed: run.step_counts.completed,
                    failed: run.step_counts.failed,
                    cancelled: run.step_counts.cancelled,
                },
                next_action: project_bounded_text(run.next_action),
            })
        }
        ExecutionQueryResult::Steps(page) => {
            protocol::OrchestraQueryResult::Steps(protocol::OrchestraStepsPage {
                items: page
                    .items
                    .into_iter()
                    .map(|step| protocol::OrchestraExecutionStepProjection {
                        id: step.id,
                        status: project_step_status(step.status),
                        attempts: step.attempts,
                        rounds: step.rounds,
                        agent: step.agent.map(|agent| protocol::OrchestraAgentReference {
                            thread_id: agent.thread_id,
                            task_path: agent.task_path,
                        }),
                        context_sha256: step.context_sha256,
                        approval_decision: step.approval_decision.map(project_bounded_text),
                        error: step.error.map(project_bounded_text),
                        output_count: step.output_count,
                    })
                    .collect(),
                next: page.next,
            })
        }
        ExecutionQueryResult::Outputs(page) => {
            protocol::OrchestraQueryResult::Outputs(protocol::OrchestraOutputsPage {
                items: page
                    .items
                    .into_iter()
                    .map(|output| protocol::OrchestraOutputProjection {
                        step_id: output.step_id,
                        name: output.name,
                        sha256: output.sha256,
                        canonical_bytes: output.canonical_bytes,
                        value: output.value,
                    })
                    .collect(),
                next: page.next,
            })
        }
        ExecutionQueryResult::Evidence(page) => {
            protocol::OrchestraQueryResult::Evidence(protocol::OrchestraEvidencePage {
                items: page
                    .items
                    .into_iter()
                    .map(|evidence| protocol::OrchestraEvidenceReference {
                        evidence_id: evidence.evidence_id,
                        name: evidence.name,
                        kind: match evidence.kind {
                            EvidenceKind::Check => protocol::OrchestraEvidenceKind::Check,
                            EvidenceKind::Change => protocol::OrchestraEvidenceKind::Change,
                            EvidenceKind::Skill => protocol::OrchestraEvidenceKind::Skill,
                            EvidenceKind::Other => protocol::OrchestraEvidenceKind::Other,
                        },
                        provenance: project_evidence_provenance(evidence.provenance),
                        step_id: evidence.step_id,
                        bytes: evidence.bytes,
                        sha256: evidence.sha256,
                        availability: project_evidence_availability(evidence.availability),
                    })
                    .collect(),
                next: page.next,
            })
        }
        ExecutionQueryResult::EvidenceContent(evidence) => {
            protocol::OrchestraQueryResult::EvidenceContent(
                protocol::OrchestraEvidenceContentProjection {
                    evidence_id: evidence.evidence_id,
                    name: evidence.name,
                    kind: match evidence.kind {
                        EvidenceKind::Check => protocol::OrchestraEvidenceKind::Check,
                        EvidenceKind::Change => protocol::OrchestraEvidenceKind::Change,
                        EvidenceKind::Skill => protocol::OrchestraEvidenceKind::Skill,
                        EvidenceKind::Other => protocol::OrchestraEvidenceKind::Other,
                    },
                    provenance: project_evidence_provenance(evidence.provenance),
                    availability: project_evidence_availability(evidence.availability),
                    bytes: evidence.bytes,
                    sha256: evidence.sha256,
                    media_type: evidence.media_type,
                    content: evidence.content,
                },
            )
        }
        ExecutionQueryResult::History(page) => {
            protocol::OrchestraQueryResult::History(protocol::OrchestraHistoryPage {
                items: page
                    .items
                    .into_iter()
                    .map(|record| protocol::OrchestraHistoryRecord {
                        sequence: record.sequence,
                        item_id: record.item_id,
                        revision: record.revision,
                        kind: record.kind,
                        step_id: record.step_id,
                        summary: record.summary,
                    })
                    .collect(),
                next: page.next.map(|cursor| protocol::OrchestraHistoryCursor {
                    sequence: cursor.sequence,
                    item_id: cursor.item_id,
                    revision: cursor.revision,
                }),
            })
        }
    }
}

fn project_bounded_text(
    text: codex_orchestra_core::BoundedText,
) -> codex_app_server_protocol::OrchestraBoundedText {
    codex_app_server_protocol::OrchestraBoundedText {
        text: text.text,
        truncated: text.truncated,
    }
}

fn project_evidence_provenance(
    provenance: EvidenceProvenance,
) -> protocol::OrchestraEvidenceProvenance {
    match provenance {
        EvidenceProvenance::RuntimeCheck => protocol::OrchestraEvidenceProvenance::RuntimeCheck,
        EvidenceProvenance::RuntimeChange => protocol::OrchestraEvidenceProvenance::RuntimeChange,
        EvidenceProvenance::SkillSnapshot => protocol::OrchestraEvidenceProvenance::SkillSnapshot,
        EvidenceProvenance::RuntimeOther => protocol::OrchestraEvidenceProvenance::RuntimeOther,
    }
}

fn project_evidence_availability(
    availability: EvidenceAvailability,
) -> protocol::OrchestraEvidenceAvailability {
    match availability {
        EvidenceAvailability::Available => protocol::OrchestraEvidenceAvailability::Available,
        EvidenceAvailability::ContentTooLarge => {
            protocol::OrchestraEvidenceAvailability::ContentTooLarge
        }
        EvidenceAvailability::Malformed => protocol::OrchestraEvidenceAvailability::Malformed,
    }
}

fn project_run_status(status: RunStatus) -> OrchestraRunStatus {
    match status {
        RunStatus::Pending => OrchestraRunStatus::Pending,
        RunStatus::Running => OrchestraRunStatus::Running,
        RunStatus::WaitingApproval => OrchestraRunStatus::WaitingApproval,
        RunStatus::Completed => OrchestraRunStatus::Completed,
        RunStatus::Failed => OrchestraRunStatus::Failed,
        RunStatus::Cancelled => OrchestraRunStatus::Cancelled,
    }
}

fn project_promotion(status: PromotionStatus) -> OrchestraPromotionStatus {
    match status {
        PromotionStatus::Pending => OrchestraPromotionStatus::Pending,
        PromotionStatus::Applied => OrchestraPromotionStatus::Applied,
        PromotionStatus::NotRequired => OrchestraPromotionStatus::NotRequired,
    }
}

fn project_step_status(status: StepStatus) -> OrchestraStepStatus {
    match status {
        StepStatus::Pending => OrchestraStepStatus::Pending,
        StepStatus::Running => OrchestraStepStatus::Running,
        StepStatus::Retrying => OrchestraStepStatus::Retrying,
        StepStatus::WaitingApproval => OrchestraStepStatus::WaitingApproval,
        StepStatus::Completed => OrchestraStepStatus::Completed,
        StepStatus::Failed => OrchestraStepStatus::Failed,
        StepStatus::Cancelled => OrchestraStepStatus::Cancelled,
    }
}

fn orchestra_error(error: String) -> JSONRPCErrorError {
    invalid_request(format!("orchestra request failed: {error}"))
}

fn outcome_checkpoint(outcome: RunOutcome) -> RunCheckpoint {
    match outcome {
        RunOutcome::Completed(checkpoint)
        | RunOutcome::Paused(checkpoint)
        | RunOutcome::Failed(checkpoint)
        | RunOutcome::Cancelled(checkpoint) => checkpoint,
    }
}

fn run_response(checkpoint: RunCheckpoint) -> OrchestraRunResponse {
    OrchestraRunResponse {
        run: project_checkpoint(checkpoint),
    }
}

fn project_plan(plan: ExecutionPlan) -> OrchestraWorkflowPlan {
    OrchestraWorkflowPlan {
        name: plan.name,
        description: bounded_text(plan.description),
        max_parallel: u32::try_from(plan.max_parallel).unwrap_or(u32::MAX),
        steps: plan
            .steps
            .into_iter()
            .map(|step| OrchestraWorkflowStep {
                id: step.id,
                kind: match step.action {
                    Action::Agent(_) => OrchestraStepKind::Agent,
                    Action::Check(_) => OrchestraStepKind::Check,
                    Action::Approval(_) => OrchestraStepKind::Approval,
                },
                needs: step.needs,
                max_attempts: step.max_attempts,
            })
            .collect(),
    }
}

fn project_checkpoint(checkpoint: RunCheckpoint) -> OrchestraRunProjection {
    OrchestraRunProjection {
        schema_version: checkpoint.schema_version,
        run_id: checkpoint.run_id,
        workflow_sha256: checkpoint.workflow_sha256,
        parent_thread_id: checkpoint.parent_thread_id,
        source_revision: checkpoint.source_revision,
        status: match checkpoint.status {
            RunStatus::Pending => OrchestraRunStatus::Pending,
            RunStatus::Running => OrchestraRunStatus::Running,
            RunStatus::WaitingApproval => OrchestraRunStatus::WaitingApproval,
            RunStatus::Completed => OrchestraRunStatus::Completed,
            RunStatus::Failed => OrchestraRunStatus::Failed,
            RunStatus::Cancelled => OrchestraRunStatus::Cancelled,
        },
        promotion: match checkpoint.promotion {
            PromotionStatus::Pending => OrchestraPromotionStatus::Pending,
            PromotionStatus::Applied => OrchestraPromotionStatus::Applied,
            PromotionStatus::NotRequired => OrchestraPromotionStatus::NotRequired,
        },
        steps: checkpoint
            .steps
            .into_iter()
            .map(|(id, step)| OrchestraStepProjection {
                id,
                status: match step.status {
                    StepStatus::Pending => OrchestraStepStatus::Pending,
                    StepStatus::Running => OrchestraStepStatus::Running,
                    StepStatus::Retrying => OrchestraStepStatus::Retrying,
                    StepStatus::WaitingApproval => OrchestraStepStatus::WaitingApproval,
                    StepStatus::Completed => OrchestraStepStatus::Completed,
                    StepStatus::Failed => OrchestraStepStatus::Failed,
                    StepStatus::Cancelled => OrchestraStepStatus::Cancelled,
                },
                attempts: step.attempts,
                rounds: step.rounds,
                output_keys: step.outputs.into_keys().collect(),
                final_response: step.final_response.map(bounded_text),
                error: step.error.map(bounded_text),
            })
            .collect(),
        next_action: bounded_text(checkpoint.next_action),
    }
}

fn bounded_text(mut text: String) -> String {
    if text.len() <= MAX_PROJECTION_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_PROJECTION_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}

pub(crate) fn project_persisted_snapshot(
    snapshot: codex_state::OrchestraTaskSnapshot,
) -> OrchestraTaskReplay {
    OrchestraTaskReplay {
        latest: project_persisted_event(snapshot.projection),
        events: snapshot
            .replay
            .into_iter()
            .map(project_persisted_event)
            .collect(),
        replay_truncated: snapshot.replay_truncated,
    }
}

fn project_persisted_event(
    event: codex_protocol::protocol::OrchestraRolloutItem,
) -> OrchestraReplayEvent {
    let projection = event.projection;
    OrchestraReplayEvent {
        schema_version: event.schema_version,
        event_id: event.event_id,
        run_id: event.run_id,
        sequence: event.sequence,
        revision: event.revision,
        kind: match event.kind {
            codex_protocol::protocol::OrchestraLifecycleKind::Invoked => {
                OrchestraLifecycleKind::Invoked
            }
            codex_protocol::protocol::OrchestraLifecycleKind::Resumed => {
                OrchestraLifecycleKind::Resumed
            }
            codex_protocol::protocol::OrchestraLifecycleKind::Cancelled => {
                OrchestraLifecycleKind::Cancelled
            }
            codex_protocol::protocol::OrchestraLifecycleKind::Recovered => {
                OrchestraLifecycleKind::Recovered
            }
        },
        projection: OrchestraRunProjection {
            schema_version: event.schema_version,
            run_id: projection.run_id,
            workflow_sha256: projection.workflow_sha256,
            parent_thread_id: projection.parent_thread_id,
            source_revision: projection.source_revision,
            status: match projection.status {
                codex_protocol::protocol::OrchestraRunStatus::Pending => {
                    OrchestraRunStatus::Pending
                }
                codex_protocol::protocol::OrchestraRunStatus::Running => {
                    OrchestraRunStatus::Running
                }
                codex_protocol::protocol::OrchestraRunStatus::WaitingApproval => {
                    OrchestraRunStatus::WaitingApproval
                }
                codex_protocol::protocol::OrchestraRunStatus::Completed => {
                    OrchestraRunStatus::Completed
                }
                codex_protocol::protocol::OrchestraRunStatus::Failed => OrchestraRunStatus::Failed,
                codex_protocol::protocol::OrchestraRunStatus::Cancelled => {
                    OrchestraRunStatus::Cancelled
                }
            },
            promotion: match projection.promotion {
                codex_protocol::protocol::OrchestraPromotionStatus::Pending => {
                    OrchestraPromotionStatus::Pending
                }
                codex_protocol::protocol::OrchestraPromotionStatus::Applied => {
                    OrchestraPromotionStatus::Applied
                }
                codex_protocol::protocol::OrchestraPromotionStatus::NotRequired => {
                    OrchestraPromotionStatus::NotRequired
                }
            },
            steps: projection
                .steps
                .into_iter()
                .map(|step| OrchestraStepProjection {
                    id: step.id,
                    status: match step.status {
                        codex_protocol::protocol::OrchestraStepStatus::Pending => {
                            OrchestraStepStatus::Pending
                        }
                        codex_protocol::protocol::OrchestraStepStatus::Running => {
                            OrchestraStepStatus::Running
                        }
                        codex_protocol::protocol::OrchestraStepStatus::Retrying => {
                            OrchestraStepStatus::Retrying
                        }
                        codex_protocol::protocol::OrchestraStepStatus::WaitingApproval => {
                            OrchestraStepStatus::WaitingApproval
                        }
                        codex_protocol::protocol::OrchestraStepStatus::Completed => {
                            OrchestraStepStatus::Completed
                        }
                        codex_protocol::protocol::OrchestraStepStatus::Failed => {
                            OrchestraStepStatus::Failed
                        }
                        codex_protocol::protocol::OrchestraStepStatus::Cancelled => {
                            OrchestraStepStatus::Cancelled
                        }
                    },
                    attempts: step.attempts,
                    rounds: step.rounds,
                    output_keys: step.output_keys,
                    final_response: step.final_response,
                    error: step.error,
                })
                .collect(),
            next_action: projection.next_action,
        },
    }
}
