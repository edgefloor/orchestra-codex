use codex_protocol::ThreadId;
use codex_protocol::protocol::OrchestraRolloutItem;

use super::StateRuntime;

const MAX_REPLAY_EVENTS: i64 = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct OrchestraTaskSnapshot {
    pub projection: OrchestraRolloutItem,
    pub replay: Vec<OrchestraRolloutItem>,
    pub replay_truncated: bool,
}

impl StateRuntime {
    /// Project one canonical rollout revision into the rebuildable task-local
    /// cache. Duplicate event IDs and semantic revisions are idempotent.
    pub async fn apply_orchestra_event(
        &self,
        thread_id: ThreadId,
        event: &OrchestraRolloutItem,
    ) -> anyhow::Result<()> {
        let event_json = serde_json::to_string(event)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO orchestra_task_events
             (thread_id, sequence, event_id, run_id, revision, event_json)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(thread_id, event_id) DO NOTHING",
        )
        .bind(thread_id.to_string())
        .bind(i64::try_from(event.sequence)?)
        .bind(&event.event_id)
        .bind(&event.run_id)
        .bind(i64::try_from(event.revision)?)
        .bind(event_json)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "DELETE FROM orchestra_task_events
             WHERE thread_id = ? AND sequence < (
               SELECT COALESCE(MAX(sequence), 0) - ?
               FROM orchestra_task_events WHERE thread_id = ?
             )",
        )
        .bind(thread_id.to_string())
        .bind(MAX_REPLAY_EVENTS - 1)
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return the latest semantic revision and bounded replay tail for one
    /// task. No child conversation content is copied into this projection.
    pub async fn orchestra_task_snapshot(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<OrchestraTaskSnapshot>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT event_json FROM orchestra_task_events
             WHERE thread_id = ? ORDER BY sequence ASC LIMIT ?",
        )
        .bind(thread_id.to_string())
        .bind(MAX_REPLAY_EVENTS)
        .fetch_all(self.pool.as_ref())
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let replay = rows
            .into_iter()
            .map(|(json,)| serde_json::from_str(&json))
            .collect::<Result<Vec<OrchestraRolloutItem>, _>>()?;
        let replay_truncated = replay.first().is_some_and(|event| event.sequence > 1);
        Ok(replay
            .last()
            .cloned()
            .map(|projection| OrchestraTaskSnapshot {
                projection,
                replay,
                replay_truncated,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::OrchestraLifecycleKind;
    use codex_protocol::protocol::OrchestraPromotionStatus;
    use codex_protocol::protocol::OrchestraRunProjection;
    use codex_protocol::protocol::OrchestraRunStatus;

    fn event(sequence: u64, revision: u64) -> OrchestraRolloutItem {
        OrchestraRolloutItem {
            schema_version: 1,
            event_id: format!("run:{revision}"),
            run_id: "run".into(),
            sequence,
            revision,
            kind: OrchestraLifecycleKind::Resumed,
            projection: OrchestraRunProjection {
                run_id: "run".into(),
                workflow_sha256: "sha".into(),
                parent_thread_id: "parent".into(),
                source_revision: "rev".into(),
                status: OrchestraRunStatus::Running,
                promotion: OrchestraPromotionStatus::Pending,
                steps: Vec::new(),
                next_action: "wait".into(),
            },
        }
    }

    #[tokio::test]
    async fn duplicate_revision_is_idempotent_and_restart_replays_latest() {
        let home =
            std::env::temp_dir().join(format!("codex-orchestra-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let thread_id = ThreadId::new();
        let runtime = StateRuntime::init(home.clone(), "openai".into())
            .await
            .unwrap();
        runtime
            .apply_orchestra_event(thread_id, &event(1, 1))
            .await
            .unwrap();
        runtime
            .apply_orchestra_event(thread_id, &event(1, 1))
            .await
            .unwrap();
        runtime.close().await;

        let runtime = StateRuntime::init(home.clone(), "openai".into())
            .await
            .unwrap();
        let snapshot = runtime
            .orchestra_task_snapshot(thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.projection.revision, 1);
        assert_eq!(snapshot.replay.len(), 1);
        runtime.close().await;
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn replay_tail_is_bounded_and_reports_pruning() {
        let home =
            std::env::temp_dir().join(format!("codex-orchestra-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let thread_id = ThreadId::new();
        let runtime = StateRuntime::init(home.clone(), "openai".into())
            .await
            .unwrap();
        for revision in 1..=70 {
            runtime
                .apply_orchestra_event(thread_id, &event(revision, revision))
                .await
                .unwrap();
        }
        let snapshot = runtime
            .orchestra_task_snapshot(thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.replay.len(), 64);
        assert_eq!(snapshot.replay.first().unwrap().sequence, 7);
        assert_eq!(snapshot.projection.sequence, 70);
        assert!(snapshot.replay_truncated);
        runtime.close().await;
        std::fs::remove_dir_all(home).unwrap();
    }
}
