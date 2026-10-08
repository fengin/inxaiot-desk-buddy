-- 智能屏资料与过程只保存在本机；平台缓存、草稿和实测分开。
CREATE TABLE local_screen_context (
    local_project_id TEXT PRIMARY KEY NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    business_project_id TEXT,
    data_source_id TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE local_screen (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    fields_json TEXT NOT NULL CHECK(json_valid(fields_json)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    removed INTEGER NOT NULL DEFAULT 0 CHECK(removed IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, id)
);

CREATE TABLE local_screen_binding (
    local_project_id TEXT NOT NULL,
    local_screen_id TEXT NOT NULL,
    business_project_id TEXT NOT NULL,
    platform_screen_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
    created_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, local_screen_id),
    FOREIGN KEY(local_project_id, local_screen_id) REFERENCES local_screen(local_project_id, id)
);
CREATE INDEX idx_screen_binding_platform ON local_screen_binding(local_project_id, business_project_id, platform_screen_id);

CREATE TABLE local_screen_platform_cache (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    business_project_id TEXT NOT NULL,
    platform_screen_id TEXT NOT NULL,
    asset_json TEXT NOT NULL CHECK(json_valid(asset_json)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    read_batch TEXT NOT NULL,
    read_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, business_project_id, platform_screen_id)
);

CREATE TABLE local_screen_draft (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    platform_screen_id TEXT NOT NULL,
    business_project_id TEXT NOT NULL,
    base_json TEXT NOT NULL CHECK(json_valid(base_json)),
    values_json TEXT NOT NULL CHECK(json_valid(values_json)),
    base_revision INTEGER NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, business_project_id, platform_screen_id)
);

CREATE TABLE local_screen_observation (
    id TEXT PRIMARY KEY NOT NULL,
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    screen_id TEXT NOT NULL,
    operation_type TEXT NOT NULL,
    observed_ip TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    task_id TEXT,
    result_json TEXT NOT NULL CHECK(json_valid(result_json))
);
CREATE INDEX idx_screen_observation_latest ON local_screen_observation(local_project_id, screen_id, observed_at DESC, id DESC);

CREATE TABLE local_project_space_cache (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    business_project_id TEXT NOT NULL,
    nodes_json TEXT NOT NULL CHECK(json_valid(nodes_json)),
    complete INTEGER NOT NULL CHECK(complete IN (0, 1)),
    read_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, business_project_id)
);

CREATE TABLE local_screen_write_intent (
    request_id TEXT PRIMARY KEY NOT NULL,
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    business_project_id TEXT NOT NULL,
    screen_id TEXT NOT NULL,
    platform_screen_id TEXT NOT NULL,
    operation_type TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    state TEXT NOT NULL CHECK(state IN ('prepared','submitted','confirmed','conflict','not_applied')),
    result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_screen_intent_pending ON local_screen_write_intent(local_project_id, screen_id, state);

CREATE TABLE local_screen_ignored_pair (
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    pair_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, pair_key)
);

-- 平台写入结果未确认时，不允许更换项目连接或删除恢复依据。
CREATE TRIGGER guard_screen_project_update
BEFORE UPDATE ON local_project
WHEN (NEW.platform_url IS NOT OLD.platform_url OR NEW.db_host IS NOT OLD.db_host
   OR NEW.db_port IS NOT OLD.db_port OR NEW.db_user IS NOT OLD.db_user
   OR NEW.db_tls_enabled IS NOT OLD.db_tls_enabled OR NEW.business_db IS NOT OLD.business_db
   OR NEW.workbench_db IS NOT OLD.workbench_db OR NEW.db_password_secret_ref IS NOT OLD.db_password_secret_ref)
 AND EXISTS (SELECT 1 FROM local_screen_write_intent WHERE local_project_id = OLD.id AND state IN ('prepared','submitted'))
BEGIN SELECT RAISE(ABORT, 'PENDING_SCREEN_WRITE'); END;
CREATE TRIGGER guard_screen_project_delete
BEFORE DELETE ON local_project
WHEN EXISTS (SELECT 1 FROM local_screen_write_intent WHERE local_project_id = OLD.id AND state IN ('prepared','submitted'))
BEGIN SELECT RAISE(ABORT, 'PENDING_SCREEN_WRITE'); END;
