ALTER TABLE local_task ADD COLUMN name TEXT NOT NULL DEFAULT '';
ALTER TABLE local_task ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
ALTER TABLE local_task ADD COLUMN batch_size INTEGER NOT NULL DEFAULT 1 CHECK (batch_size > 0);
ALTER TABLE local_task ADD COLUMN concurrency INTEGER NOT NULL DEFAULT 1 CHECK (concurrency > 0);
ALTER TABLE local_task ADD COLUMN payload_ref TEXT;
ALTER TABLE local_task ADD COLUMN error_code TEXT;
ALTER TABLE local_task ADD COLUMN message TEXT;

ALTER TABLE local_task_target ADD COLUMN progress_current INTEGER NOT NULL DEFAULT 0 CHECK (progress_current >= 0);
ALTER TABLE local_task_target ADD COLUMN progress_total INTEGER NOT NULL DEFAULT 0 CHECK (progress_total >= 0);
ALTER TABLE local_task_target ADD COLUMN message_code TEXT;
ALTER TABLE local_task_target ADD COLUMN message_params_json TEXT;

CREATE INDEX idx_local_task_priority
    ON local_task(state, priority DESC, created_at, id);
