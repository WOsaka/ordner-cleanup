CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE snapshots (
    id INTEGER PRIMARY KEY,
    root_key TEXT NOT NULL,
    root_path TEXT NOT NULL,
    taken_at TEXT NOT NULL,
    scan_finished_at TEXT,
    tool_version TEXT NOT NULL,
    metrics_version INTEGER NOT NULL,
    config_fp TEXT NOT NULL,
    template TEXT,
    profile TEXT,
    scan_errors INTEGER NOT NULL
);
CREATE INDEX snapshots_root ON snapshots(root_key, taken_at);

CREATE TABLE folder_metrics (
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    folder TEXT NOT NULL,
    score INTEGER NOT NULL,
    metrics TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, folder)
);
