CREATE TABLE local_project_master_key (
    local_project_id TEXT NOT NULL,
    key_version INTEGER NOT NULL CHECK (key_version > 0),
    secret_ref TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (local_project_id, key_version),
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_secret_cleanup (
    secret_ref TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    last_error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_local_secret_cleanup_updated
    ON local_secret_cleanup(updated_at, secret_ref);
