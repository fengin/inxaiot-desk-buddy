CREATE TRIGGER guard_local_project_update_with_active_task
BEFORE UPDATE ON local_project
WHEN EXISTS (
    SELECT 1
    FROM local_task
    WHERE local_project_id = OLD.id
      AND state IN ('draft', 'checking', 'ready', 'queued', 'running', 'cancelling', 'finalizing_failed')
)
BEGIN
    SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK');
END;

CREATE TRIGGER guard_local_project_delete_with_active_task
BEFORE DELETE ON local_project
WHEN EXISTS (
    SELECT 1
    FROM local_task
    WHERE local_project_id = OLD.id
      AND state IN ('draft', 'checking', 'ready', 'queued', 'running', 'cancelling', 'finalizing_failed')
)
BEGIN
    SELECT RAISE(ABORT, 'ACTIVE_PROJECT_TASK');
END;
