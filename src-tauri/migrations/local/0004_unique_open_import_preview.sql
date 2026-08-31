UPDATE local_aio_import_session AS current
SET state = 'discarded'
WHERE current.state = 'preview'
  AND EXISTS (
    SELECT 1
    FROM local_aio_import_session AS newer
    WHERE newer.local_project_id = current.local_project_id
      AND newer.state = 'preview'
      AND (
        newer.updated_at > current.updated_at
        OR (newer.updated_at = current.updated_at AND newer.id > current.id)
      )
  );

CREATE UNIQUE INDEX uq_local_aio_import_open_project
ON local_aio_import_session(local_project_id)
WHERE state = 'preview';
