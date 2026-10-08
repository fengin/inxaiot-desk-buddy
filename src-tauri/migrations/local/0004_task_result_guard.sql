-- 公共结果保护；各业务在可能产生外部效果前登记，核实并保存完成后解除。
CREATE TABLE local_task_result_guard (
    local_task_id TEXT PRIMARY KEY NOT NULL REFERENCES local_task(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TRIGGER guard_project_delete_pending_result BEFORE DELETE ON local_project
WHEN EXISTS (SELECT 1 FROM local_task_result_guard g JOIN local_task t ON t.id=g.local_task_id WHERE t.local_project_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK'); END;
CREATE TRIGGER guard_project_update_pending_result BEFORE UPDATE ON local_project
WHEN (NEW.platform_url IS NOT OLD.platform_url OR NEW.db_host IS NOT OLD.db_host
 OR NEW.db_port IS NOT OLD.db_port OR NEW.db_user IS NOT OLD.db_user
 OR NEW.db_tls_enabled IS NOT OLD.db_tls_enabled OR NEW.business_db IS NOT OLD.business_db
 OR NEW.workbench_db IS NOT OLD.workbench_db OR NEW.db_password_secret_ref IS NOT OLD.db_password_secret_ref)
 AND EXISTS (SELECT 1 FROM local_task_result_guard g JOIN local_task t ON t.id=g.local_task_id WHERE t.local_project_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK'); END;

CREATE TRIGGER guard_screen_active_edit BEFORE UPDATE ON local_screen
WHEN (NEW.fields_json IS NOT OLD.fields_json OR NEW.removed IS NOT OLD.removed)
 AND EXISTS (SELECT 1 FROM local_task t JOIN local_task_target r ON r.local_task_id=t.id
 WHERE t.local_project_id=OLD.local_project_id AND t.domain_type='smart_screen' AND r.resource_key=OLD.id
 AND (t.state IN ('draft','checking','ready','queued','running','cancelling','finalizing_failed')
 OR EXISTS (SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id)))
BEGIN SELECT RAISE(ABORT, 'SCREEN_ACTIVE_TASK'); END;
