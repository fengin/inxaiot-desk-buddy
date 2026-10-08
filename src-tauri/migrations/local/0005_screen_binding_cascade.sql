-- 删除本机项目时清理其本机关联；不涉及平台记录或项目共享记录。
ALTER TABLE local_screen_binding RENAME TO local_screen_binding_previous;
DROP INDEX idx_screen_binding_platform;
CREATE TABLE local_screen_binding (
    local_project_id TEXT NOT NULL,
    local_screen_id TEXT NOT NULL,
    business_project_id TEXT NOT NULL,
    platform_screen_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
    created_at TEXT NOT NULL,
    PRIMARY KEY(local_project_id, local_screen_id),
    FOREIGN KEY(local_project_id, local_screen_id) REFERENCES local_screen(local_project_id, id) ON DELETE CASCADE
);
INSERT INTO local_screen_binding SELECT * FROM local_screen_binding_previous;
DROP TABLE local_screen_binding_previous;
CREATE INDEX idx_screen_binding_platform ON local_screen_binding(local_project_id, business_project_id, platform_screen_id);
