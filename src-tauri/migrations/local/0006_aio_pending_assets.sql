CREATE TABLE local_aio_node (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    mac_normalized TEXT NOT NULL,
    values_json TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (local_project_id, mac_normalized)
);
