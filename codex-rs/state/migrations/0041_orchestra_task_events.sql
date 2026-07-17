CREATE TABLE orchestra_task_events (
    thread_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    event_json TEXT NOT NULL,
    PRIMARY KEY (thread_id, event_id),
    UNIQUE (thread_id, sequence),
    UNIQUE (thread_id, run_id, revision)
);

CREATE INDEX orchestra_task_events_latest
    ON orchestra_task_events(thread_id, sequence DESC);
