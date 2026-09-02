ALTER TABLE local_project
    ADD COLUMN db_tls_enabled INTEGER NOT NULL DEFAULT 0
    CHECK (db_tls_enabled IN (0, 1));
