ALTER TABLE resource_lease
    ADD COLUMN lease_state VARCHAR(16) NOT NULL DEFAULT 'active' AFTER fencing_token;

CREATE INDEX idx_lease_state_expiry
    ON resource_lease(lease_state, expires_at);

