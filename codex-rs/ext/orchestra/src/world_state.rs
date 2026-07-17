use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::PreviousWorldStateSection;
use codex_extension_api::RenderedWorldStateFragment;
use codex_extension_api::WorldStateContributionInput;
use codex_extension_api::WorldStateSectionContribution;
use codex_orchestra_core::RunDigest;
use serde_json::json;

use crate::OrchestraService;

const RUN_DIGEST_WORLD_STATE_ID: &str = "orchestra_run_digest";
const RUN_DIGEST_OPEN_TAG: &str = "<orchestra_run_digest>";
const RUN_DIGEST_CLOSE_TAG: &str = "</orchestra_run_digest>";

/// Leaves room for the state identity and fragment envelope while keeping the
/// complete model-visible section below four KiB.
const RUN_DIGEST_QUERY_BYTES: usize = 3_840;

pub(crate) struct OrchestraWorldState {
    service: OrchestraService,
}

impl OrchestraWorldState {
    pub(crate) fn new(service: OrchestraService) -> Self {
        Self { service }
    }
}

impl ContextContributor for OrchestraWorldState {
    fn contribute_world_state<'a>(
        &'a self,
        input: WorldStateContributionInput<'a>,
    ) -> ExtensionFuture<'a, Vec<WorldStateSectionContribution>> {
        Box::pin(async move {
            let parent_thread_id = input.thread_id.to_string();
            match self
                .service
                .active_run_digest(&parent_thread_id, RUN_DIGEST_QUERY_BYTES)
                .await
            {
                Ok(digest) => run_digest_contribution(digest),
                Err(error) => {
                    tracing::warn!(%error, "failed to contribute Orchestra run digest");
                    Vec::new()
                }
            }
        })
    }
}

fn run_digest_contribution(digest: Option<RunDigest>) -> Vec<WorldStateSectionContribution> {
    let Some(digest) = digest else {
        return Vec::new();
    };
    let snapshot = json!({
        "runId": digest.run_id,
        "stateSha256": digest.state_sha256,
    });
    let body = format!("state {}\n{}", digest.state_sha256, digest.text);
    let retained_state = format!("state {}\n", digest.state_sha256);

    vec![WorldStateSectionContribution::new(
        RUN_DIGEST_WORLD_STATE_ID,
        snapshot.clone(),
        move |previous| {
            if matches!(previous, PreviousWorldStateSection::Known(previous) if previous == &snapshot)
            {
                return None;
            }
            Some(RenderedWorldStateFragment::new(
                "developer",
                (RUN_DIGEST_OPEN_TAG, RUN_DIGEST_CLOSE_TAG),
                body.clone(),
            ))
        },
    )
    .with_legacy_matcher(|role, text| {
        role == "developer"
            && text.contains(RUN_DIGEST_OPEN_TAG)
            && text.contains(RUN_DIGEST_CLOSE_TAG)
    })
    .with_retained_fragment_matcher(move |role, text| {
        role == "developer"
            && text.contains(RUN_DIGEST_OPEN_TAG)
            && text.contains(&retained_state)
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(state_sha256: &str, text: &str) -> RunDigest {
        RunDigest {
            run_id: "run-1".into(),
            state_sha256: state_sha256.into(),
            text: text.into(),
            omitted_steps: 0,
        }
    }

    #[test]
    fn omits_section_without_an_active_run() {
        assert!(run_digest_contribution(None).is_empty());
    }

    #[test]
    fn unchanged_digest_is_a_snapshot_noop() {
        let contribution = run_digest_contribution(Some(digest("same", "current")))
            .pop()
            .unwrap();
        assert!(
            contribution
                .render_diff(PreviousWorldStateSection::Known(contribution.snapshot()))
                .is_none()
        );
    }

    #[test]
    fn changed_digest_renders_the_current_replacement() {
        let contribution = run_digest_contribution(Some(digest("new", "current digest")))
            .pop()
            .unwrap();
        let rendered = contribution
            .render_diff(PreviousWorldStateSection::Known(
                &json!({"runId": "run-1", "stateSha256": "old"}),
            ))
            .unwrap();
        assert_eq!(rendered.role(), "developer");
        assert_eq!(
            rendered.markers(),
            (RUN_DIGEST_OPEN_TAG, RUN_DIGEST_CLOSE_TAG)
        );
        assert_eq!(rendered.body(), "state new\ncurrent digest");
    }
}
