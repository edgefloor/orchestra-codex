#[cfg(test)]
use super::MAX_PROJECTION_TEXT_BYTES;
use super::automation_bounded_text;
use super::project_automation_queue_item;
use super::project_run_status;
use codex_app_server_protocol as protocol;
use codex_app_server_protocol::AutomationSteerIssueResponse;
use codex_orchestra_core as core;

const MAX_AUTOMATION_RUN_PROJECTION_CLAIMS: usize = 25;

#[derive(Clone, Copy)]
pub(super) enum AutomationClaimFocus<'a> {
    ClaimId(&'a str),
    IssueId(&'a str),
}

pub(super) fn project_automation_run(
    mut checkpoint: core::AutomationRootCheckpoint,
    focused_claim: Option<AutomationClaimFocus<'_>>,
) -> protocol::AutomationRunProjection {
    let queue_counts = core::automation_queue_counts(&checkpoint);
    let claims_total = checkpoint.claims.len() as u32;
    let mut claims = std::mem::take(&mut checkpoint.claims);
    let focused_claim = focused_claim
        .and_then(|focus| match focus {
            AutomationClaimFocus::ClaimId(claim_id) => claims.get(claim_id),
            AutomationClaimFocus::IssueId(issue_id) => {
                claims.values().find(|claim| claim.issue_id == issue_id)
            }
        })
        .map(|claim| claim.claim_id.clone())
        .and_then(|claim_id| claims.remove(&claim_id));
    let visible_claim_limit =
        MAX_AUTOMATION_RUN_PROJECTION_CLAIMS - usize::from(focused_claim.is_some());
    let projected_claims = claims
        .into_values()
        .take(visible_claim_limit)
        .chain(focused_claim);
    let categories = [
        core::AutomationQueueCategory::Queued,
        core::AutomationQueueCategory::Running,
        core::AutomationQueueCategory::Blocked,
        core::AutomationQueueCategory::WaitingGate,
        core::AutomationQueueCategory::Handoff,
        core::AutomationQueueCategory::Terminal,
    ];
    let queue_total = queue_counts.queued
        + queue_counts.running
        + queue_counts.blocked
        + queue_counts.waiting_gate
        + queue_counts.handoff
        + queue_counts.terminal;
    let mut queue_preview = Vec::new();
    for category in categories {
        if queue_preview.len() == 8 {
            break;
        }
        let remaining = 8 - queue_preview.len();
        queue_preview.extend(
            core::automation_queue_page(&checkpoint, category, 0, remaining as u32)
                .items
                .into_iter()
                .map(project_automation_queue_item),
        );
    }
    protocol::AutomationRunProjection {
        schema_version: checkpoint.schema_version,
        run_id: checkpoint.run_id,
        owner_thread_id: checkpoint.owner_thread_id,
        source_revision: checkpoint.source_revision,
        profile_digest: checkpoint.profile_digest,
        profile_revision: checkpoint.profile_revision.revision,
        profile_revision_status: match checkpoint.profile_revision.status {
            core::AutomationProfileRevisionStatus::Active => {
                protocol::AutomationProfileRevisionStatus::Active
            }
            core::AutomationProfileRevisionStatus::PendingValid => {
                protocol::AutomationProfileRevisionStatus::PendingValid
            }
            core::AutomationProfileRevisionStatus::Rejected => {
                protocol::AutomationProfileRevisionStatus::Rejected
            }
        },
        pending_profile_digest: checkpoint.profile_revision.pending_digest,
        rejected_profile_digest: checkpoint.profile_revision.rejected_digest,
        profile_diagnostics: checkpoint
            .profile_revision
            .diagnostics
            .into_iter()
            .map(automation_bounded_text)
            .collect(),
        tracker_project_slug: checkpoint.tracker_project_slug,
        lease_epoch: checkpoint.lease_epoch,
        revision: checkpoint.revision,
        status: match checkpoint.status {
            core::AutomationRootStatus::Running => protocol::AutomationRootStatus::Running,
            core::AutomationRootStatus::Suspended => protocol::AutomationRootStatus::Suspended,
            core::AutomationRootStatus::Cancelled => protocol::AutomationRootStatus::Cancelled,
            core::AutomationRootStatus::Failed => protocol::AutomationRootStatus::Failed,
        },
        reconciliation: match checkpoint.reconciliation {
            core::AutomationReconciliationStatus::Complete => {
                protocol::AutomationReconciliationStatus::Complete
            }
            core::AutomationReconciliationStatus::Required => {
                protocol::AutomationReconciliationStatus::Required
            }
            core::AutomationReconciliationStatus::InProgress => {
                protocol::AutomationReconciliationStatus::InProgress
            }
            core::AutomationReconciliationStatus::Blocked => {
                protocol::AutomationReconciliationStatus::Blocked
            }
        },
        coordination: protocol::AutomationCoordinationProjection {
            cycle: checkpoint.coordination.cycle,
            scan_revision: checkpoint.coordination.scan_revision,
            input_cursor: checkpoint.coordination.input_cursor.clone(),
            output_cursor: checkpoint.coordination.output_cursor.clone(),
            intake_status: match checkpoint.coordination.intake_status {
                core::AutomationCoordinationIntakeStatus::NotStarted => {
                    protocol::AutomationCoordinationIntakeStatus::NotStarted
                }
                core::AutomationCoordinationIntakeStatus::Ready => {
                    protocol::AutomationCoordinationIntakeStatus::Ready
                }
                core::AutomationCoordinationIntakeStatus::Skipped => {
                    protocol::AutomationCoordinationIntakeStatus::Skipped
                }
            },
            page_digest: checkpoint.coordination.page_digest.clone(),
            started_at_ms: checkpoint.coordination.started_at_ms,
            completed_at_ms: checkpoint.coordination.completed_at_ms,
            error: checkpoint
                .coordination
                .error
                .clone()
                .map(automation_bounded_text),
            next_action: automation_bounded_text(checkpoint.coordination.next_action.clone()),
            dispatch_intent: checkpoint
                .coordination
                .dispatch_intent
                .clone()
                .map(|intent| protocol::AutomationDispatchIntentProjection {
                    intent_id: intent.intent_id,
                    claim_id: intent.claim_id,
                    issue_id: intent.issue_id,
                    kind: match intent.kind {
                        core::AutomationDispatchIntentKind::NewClaim => {
                            protocol::AutomationDispatchIntentKind::NewClaim
                        }
                        core::AutomationDispatchIntentKind::Retry => {
                            protocol::AutomationDispatchIntentKind::Retry
                        }
                        core::AutomationDispatchIntentKind::Continuation => {
                            protocol::AutomationDispatchIntentKind::Continuation
                        }
                    },
                    status: match intent.status {
                        core::AutomationDispatchIntentStatus::Pending => {
                            protocol::AutomationDispatchIntentStatus::Pending
                        }
                        core::AutomationDispatchIntentStatus::Started => {
                            protocol::AutomationDispatchIntentStatus::Started
                        }
                        core::AutomationDispatchIntentStatus::Completed => {
                            protocol::AutomationDispatchIntentStatus::Completed
                        }
                    },
                    attempt: intent.attempt,
                    profile_digest: intent.profile_digest,
                    created_at_ms: intent.created_at_ms,
                    ready_at_ms: intent.ready_at_ms,
                }),
        },
        queue_counts: protocol::AutomationQueueCounts {
            queued: queue_counts.queued,
            running: queue_counts.running,
            blocked: queue_counts.blocked,
            waiting_gate: queue_counts.waiting_gate,
            handoff: queue_counts.handoff,
            terminal: queue_counts.terminal,
        },
        claims_total,
        claims: projected_claims
            .into_iter()
            .map(|claim| protocol::AutomationIssueClaimProjection {
                claim_id: claim.claim_id,
                issue_id: claim.issue_id,
                issue_identifier: claim.issue_identifier,
                issue_title: automation_bounded_text(claim.issue_title),
                issue_url: claim.issue_url,
                tracker_state: claim.tracker_state,
                priority: claim.priority,
                attempt: claim.attempt,
                workflow_invocations: claim.workflow_invocations,
                turns_in_window: claim.turns_in_window,
                continuation_count: claim.continuation_count,
                retry_attempt: claim.retry_attempt,
                scheduled_retry: claim.retry.map(|retry| {
                    protocol::AutomationRetryScheduleProjection {
                        kind: match retry.kind {
                            core::AutomationRetryKind::Retry => {
                                protocol::AutomationRetryKind::Retry
                            }
                            core::AutomationRetryKind::Continuation => {
                                protocol::AutomationRetryKind::Continuation
                            }
                        },
                        ready_at_ms: retry.ready_at_ms,
                        reset_turn_window: retry.reset_turn_window,
                    }
                }),
                last_progress_at_ms: claim.last_progress_at_ms,
                profile_digest: claim.profile_digest,
                profile_revision: claim.profile_revision,
                status: match claim.status {
                    core::AutomationClaimStatus::Claimed => {
                        protocol::AutomationClaimStatus::Claimed
                    }
                    core::AutomationClaimStatus::Running => {
                        protocol::AutomationClaimStatus::Running
                    }
                    core::AutomationClaimStatus::Completed => {
                        protocol::AutomationClaimStatus::Completed
                    }
                    core::AutomationClaimStatus::Suspended => {
                        protocol::AutomationClaimStatus::Suspended
                    }
                    core::AutomationClaimStatus::Cancelled => {
                        protocol::AutomationClaimStatus::Cancelled
                    }
                    core::AutomationClaimStatus::Failed => protocol::AutomationClaimStatus::Failed,
                },
                worktree: claim.worktree.to_string_lossy().into_owned(),
                source_revision: claim.source_revision,
                issue_task: claim
                    .issue_task
                    .map(|task| protocol::OrchestraAgentReference {
                        thread_id: task.thread_id,
                        task_path: task.task_path,
                    }),
                workflow_run_id: claim.workflow_run_id,
                workflow_status: claim.workflow_status.map(project_run_status),
                effects: claim
                    .effects
                    .into_iter()
                    .map(|receipt| protocol::AutomationEffectReceiptProjection {
                        effect_id: receipt.effect_id,
                        idempotency_key: receipt.idempotency_key,
                        kind: match receipt.kind {
                            core::AutomationEffect::TrackerComment => {
                                protocol::AutomationEffect::TrackerComment
                            }
                            core::AutomationEffect::TrackerTransition => {
                                protocol::AutomationEffect::TrackerTransition
                            }
                            core::AutomationEffect::TrackerLinkPullRequest => {
                                protocol::AutomationEffect::TrackerLinkPullRequest
                            }
                        },
                        status: match receipt.status {
                            core::AutomationEffectStatus::WaitingGate => {
                                protocol::AutomationEffectStatus::WaitingGate
                            }
                            core::AutomationEffectStatus::Rejected => {
                                protocol::AutomationEffectStatus::Rejected
                            }
                            core::AutomationEffectStatus::Executing => {
                                protocol::AutomationEffectStatus::Executing
                            }
                            core::AutomationEffectStatus::Committed => {
                                protocol::AutomationEffectStatus::Committed
                            }
                            core::AutomationEffectStatus::Failed => {
                                protocol::AutomationEffectStatus::Failed
                            }
                            core::AutomationEffectStatus::Ambiguous => {
                                protocol::AutomationEffectStatus::Ambiguous
                            }
                        },
                        gate_policy: match receipt.gate_policy {
                            core::AutomationGatePolicy::AutoAccept => {
                                protocol::AutomationGatePolicy::AutoAccept
                            }
                            core::AutomationGatePolicy::AutoReject => {
                                protocol::AutomationGatePolicy::AutoReject
                            }
                            core::AutomationGatePolicy::AskHuman => {
                                protocol::AutomationGatePolicy::AskHuman
                            }
                        },
                        request_sha256: receipt.request_sha256,
                        body_preview: automation_bounded_text(receipt.body_preview),
                        provider_receipt: receipt.provider_receipt,
                        failure: receipt.failure.map(automation_bounded_text),
                    })
                    .collect(),
                latest_steering_receipt: claim
                    .steering_receipts
                    .into_iter()
                    .max_by_key(|receipt| receipt.sequence)
                    .map(project_automation_steering_receipt),
                hook_receipts: claim
                    .hook_receipts
                    .into_iter()
                    .map(|receipt| protocol::AutomationHookReceiptProjection {
                        kind: match receipt.kind {
                            core::AutomationHookKind::AfterCreate => {
                                protocol::AutomationHookKind::AfterCreate
                            }
                            core::AutomationHookKind::BeforeRun => {
                                protocol::AutomationHookKind::BeforeRun
                            }
                            core::AutomationHookKind::AfterRun => {
                                protocol::AutomationHookKind::AfterRun
                            }
                            core::AutomationHookKind::BeforeRemove => {
                                protocol::AutomationHookKind::BeforeRemove
                            }
                        },
                        invocation: receipt.invocation,
                        command_sha256: receipt.command_sha256,
                        status: match receipt.status {
                            core::AutomationHookStatus::Succeeded => {
                                protocol::AutomationHookStatus::Succeeded
                            }
                            core::AutomationHookStatus::Failed => {
                                protocol::AutomationHookStatus::Failed
                            }
                            core::AutomationHookStatus::Skipped => {
                                protocol::AutomationHookStatus::Skipped
                            }
                        },
                        exit_code: receipt.exit_code,
                        stdout_preview: automation_bounded_text(receipt.stdout_preview),
                        stderr_preview: automation_bounded_text(receipt.stderr_preview),
                        failure: receipt.failure.map(automation_bounded_text),
                    })
                    .collect(),
                cleanup: protocol::AutomationCleanupProjection {
                    status: match claim.cleanup.status {
                        core::AutomationCleanupStatus::Retained => {
                            protocol::AutomationCleanupStatus::Retained
                        }
                        core::AutomationCleanupStatus::Eligible => {
                            protocol::AutomationCleanupStatus::Eligible
                        }
                        core::AutomationCleanupStatus::RetryPending => {
                            protocol::AutomationCleanupStatus::RetryPending
                        }
                        core::AutomationCleanupStatus::Removed => {
                            protocol::AutomationCleanupStatus::Removed
                        }
                    },
                    attempts: claim.cleanup.attempts,
                    last_failure: claim.cleanup.last_failure.map(automation_bounded_text),
                },
                next_action: automation_bounded_text(claim.next_action),
            })
            .collect(),
        queue_preview_truncated: queue_total as usize > queue_preview.len(),
        queue_preview,
        next_action: automation_bounded_text(checkpoint.next_action),
    }
}

fn project_automation_steering_receipt(
    receipt: core::AutomationSteeringReceipt,
) -> protocol::AutomationSteeringReceipt {
    protocol::AutomationSteeringReceipt {
        sequence: receipt.sequence,
        submitted_at_ms: receipt.submitted_at_ms,
        initiator_thread_id: receipt.initiator_thread_id,
        target_thread_id: receipt.target_thread_id,
        authority: receipt.authority,
        input_sha256: receipt.input_sha256,
        input_preview: receipt.input_preview,
        status: match receipt.status {
            core::AutomationSteeringStatus::Submitted => {
                protocol::AutomationSteeringStatus::Submitted
            }
            core::AutomationSteeringStatus::Delivered => {
                protocol::AutomationSteeringStatus::Delivered
            }
            core::AutomationSteeringStatus::Failed => protocol::AutomationSteeringStatus::Failed,
        },
        provider_receipt: receipt.provider_receipt,
        failure: receipt.failure,
    }
}

pub(super) fn project_automation_steer_issue_response(
    run: core::AutomationRootCheckpoint,
    receipt: core::AutomationSteeringReceipt,
    focused_claim_id: &str,
) -> AutomationSteerIssueResponse {
    AutomationSteerIssueResponse {
        run: project_automation_run(run, Some(AutomationClaimFocus::ClaimId(focused_claim_id))),
        receipt: project_automation_steering_receipt(receipt),
    }
}

#[cfg(test)]
#[path = "automation_projection_tests.rs"]
mod tests;
