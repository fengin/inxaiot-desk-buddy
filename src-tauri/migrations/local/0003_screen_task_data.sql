-- 屏任务的检查依据和结果需要在临时 APK/制品清理后继续保留。
CREATE TABLE local_screen_task_data (
    local_task_id TEXT PRIMARY KEY NOT NULL,
    local_project_id TEXT NOT NULL REFERENCES local_project(id) ON DELETE CASCADE,
    plan_json TEXT NOT NULL CHECK(json_valid(plan_json)),
    plan_sha256 TEXT NOT NULL,
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    updated_at TEXT NOT NULL
);
CREATE TRIGGER cleanup_screen_task_data AFTER DELETE ON local_task
BEGIN DELETE FROM local_screen_task_data WHERE local_task_id=OLD.id; END;
