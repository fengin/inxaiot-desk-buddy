PRAGMA foreign_keys = ON;

CREATE TABLE local_project (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    platform_url TEXT NOT NULL,
    db_host TEXT NOT NULL,
    db_port INTEGER NOT NULL CHECK (db_port BETWEEN 1 AND 65535),
    db_user TEXT NOT NULL,
    db_tls_enabled INTEGER NOT NULL DEFAULT 0 CHECK (db_tls_enabled IN (0, 1)),
    business_db TEXT NOT NULL,
    workbench_db TEXT NOT NULL,
    db_password_secret_ref TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_opened_at TEXT
);

CREATE TABLE local_project_session (
    local_project_id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL,
    token_secret_ref TEXT NOT NULL,
    expires_at TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_preference (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

CREATE TABLE local_task (
    id TEXT PRIMARY KEY NOT NULL,
    local_project_id TEXT NOT NULL,
    remote_operation_record_id TEXT,
    domain_type TEXT NOT NULL,
    operation_type TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    priority INTEGER NOT NULL DEFAULT 0,
    batch_size INTEGER NOT NULL DEFAULT 1 CHECK (batch_size > 0),
    concurrency INTEGER NOT NULL DEFAULT 1 CHECK (concurrency > 0),
    payload_ref TEXT,
    error_code TEXT,
    message TEXT,
    state TEXT NOT NULL,
    sequence INTEGER NOT NULL DEFAULT 0,
    log_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_task_target (
    local_task_id TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_key TEXT NOT NULL,
    state TEXT NOT NULL,
    stage TEXT NOT NULL,
    progress INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    progress_current INTEGER NOT NULL DEFAULT 0 CHECK (progress_current >= 0),
    progress_total INTEGER NOT NULL DEFAULT 0 CHECK (progress_total >= 0),
    message_code TEXT,
    message_params_json TEXT,
    fencing_token INTEGER,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (local_task_id, resource_type, resource_key),
    FOREIGN KEY (local_task_id) REFERENCES local_task(id) ON DELETE CASCADE
);

CREATE TABLE local_task_step (
    id TEXT PRIMARY KEY NOT NULL,
    local_task_id TEXT NOT NULL,
    resource_type TEXT,
    resource_key TEXT,
    step_code TEXT NOT NULL,
    state TEXT NOT NULL,
    error_code TEXT,
    message TEXT,
    started_at TEXT,
    ended_at TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (local_task_id) REFERENCES local_task(id) ON DELETE CASCADE
);

CREATE TABLE local_aio_import_session (
    id TEXT PRIMARY KEY NOT NULL,
    local_project_id TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    state TEXT NOT NULL,
    counts_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_aio_import_item (
    import_session_id TEXT NOT NULL,
    row_number INTEGER NOT NULL,
    mac_normalized TEXT,
    parsed_data_json TEXT NOT NULL,
    classification TEXT NOT NULL,
    conflict_json TEXT,
    selected INTEGER NOT NULL DEFAULT 0 CHECK (selected IN (0, 1)),
    error TEXT,
    PRIMARY KEY (import_session_id, row_number),
    FOREIGN KEY (import_session_id) REFERENCES local_aio_import_session(id) ON DELETE CASCADE
);

CREATE TABLE local_recent_artifact (
    local_project_id TEXT NOT NULL,
    domain_type TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    service TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL,
    used_at TEXT NOT NULL,
    PRIMARY KEY (local_project_id, domain_type, artifact_type, service),
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_host_key (
    local_project_id TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    algorithm TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    PRIMARY KEY (local_project_id, host, port),
    FOREIGN KEY (local_project_id) REFERENCES local_project(id) ON DELETE CASCADE
);

CREATE TABLE local_aio_service_check (
    local_project_id TEXT NOT NULL,
    mac_normalized TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (local_project_id, mac_normalized),
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

CREATE INDEX idx_local_task_project_state
    ON local_task(local_project_id, state, updated_at);
CREATE INDEX idx_local_task_priority
    ON local_task(state, priority DESC, created_at, id);
CREATE INDEX idx_local_task_target_state
    ON local_task_target(local_task_id, state, updated_at);
CREATE INDEX idx_local_import_project_state
    ON local_aio_import_session(local_project_id, state, updated_at);
CREATE INDEX idx_local_import_item_mac
    ON local_aio_import_item(mac_normalized);
CREATE UNIQUE INDEX uq_local_aio_import_open_project
    ON local_aio_import_session(local_project_id)
    WHERE state = 'preview';
CREATE INDEX idx_local_secret_cleanup_updated
    ON local_secret_cleanup(updated_at, secret_ref);

CREATE TRIGGER guard_local_project_update_with_active_task
BEFORE UPDATE ON local_project
WHEN EXISTS (
    SELECT 1 FROM local_task
    WHERE local_project_id = OLD.id
      AND state IN ('draft', 'checking', 'ready', 'queued', 'running', 'cancelling', 'finalizing_failed')
)
BEGIN
    SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK');
END;

CREATE TRIGGER guard_local_project_delete_with_active_task
BEFORE DELETE ON local_project
WHEN EXISTS (
    SELECT 1 FROM local_task
    WHERE local_project_id = OLD.id
      AND state IN ('draft', 'checking', 'ready', 'queued', 'running', 'cancelling', 'finalizing_failed')
)
BEGIN
    SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK');
END;
