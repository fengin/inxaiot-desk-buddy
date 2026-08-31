CREATE TABLE operation_record (
    id CHAR(36) NOT NULL PRIMARY KEY,
    domain_type VARCHAR(32) NOT NULL,
    operation_type VARCHAR(64) NOT NULL,
    operation_name VARCHAR(255) NOT NULL,
    operator_name VARCHAR(128) NOT NULL,
    instance_id VARCHAR(64) NOT NULL,
    state VARCHAR(32) NOT NULL,
    target_count INT UNSIGNED NOT NULL DEFAULT 0,
    success_count INT UNSIGNED NOT NULL DEFAULT 0,
    failure_count INT UNSIGNED NOT NULL DEFAULT 0,
    cancelled_count INT UNSIGNED NOT NULL DEFAULT 0,
    artifact_name VARCHAR(255) NULL,
    artifact_version VARCHAR(128) NULL,
    operation_summary_json JSON NULL,
    started_at DATETIME(6) NOT NULL,
    ended_at DATETIME(6) NULL,
    heartbeat_at DATETIME(6) NOT NULL,
    result_summary VARCHAR(1000) NULL,
    error_code VARCHAR(128) NULL,
    error_summary VARCHAR(1000) NULL,
    retry_of_operation_id CHAR(36) NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    INDEX idx_operation_started (started_at DESC, id),
    INDEX idx_operation_state_heartbeat (state, heartbeat_at),
    INDEX idx_operation_domain_type (domain_type, operation_type, started_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE operation_target_result (
    operation_id CHAR(36) NOT NULL,
    resource_type VARCHAR(32) NOT NULL,
    resource_key VARCHAR(191) NOT NULL,
    result_state VARCHAR(32) NOT NULL,
    before_version VARCHAR(128) NULL,
    after_version VARCHAR(128) NULL,
    result_summary VARCHAR(1000) NULL,
    error_code VARCHAR(128) NULL,
    error_summary VARCHAR(1000) NULL,
    completed_at DATETIME(6) NULL,
    PRIMARY KEY (operation_id, resource_type, resource_key),
    INDEX idx_target_resource (resource_type, resource_key, operation_id),
    CONSTRAINT fk_target_operation
        FOREIGN KEY (operation_id) REFERENCES operation_record(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE resource_lease (
    resource_type VARCHAR(32) NOT NULL,
    resource_key VARCHAR(191) NOT NULL,
    domain_type VARCHAR(32) NOT NULL,
    operation_id CHAR(36) NOT NULL,
    owner_instance_id VARCHAR(64) NOT NULL,
    owner_user VARCHAR(128) NOT NULL,
    lease_token CHAR(36) NOT NULL,
    fencing_token BIGINT UNSIGNED NOT NULL,
    acquired_at DATETIME(6) NOT NULL,
    heartbeat_at DATETIME(6) NOT NULL,
    expires_at DATETIME(6) NOT NULL,
    PRIMARY KEY (resource_type, resource_key),
    INDEX idx_lease_expiry (expires_at),
    INDEX idx_lease_operation (operation_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE audit_event (
    id CHAR(36) NOT NULL PRIMARY KEY,
    domain_type VARCHAR(32) NOT NULL,
    object_type VARCHAR(64) NOT NULL,
    object_key VARCHAR(191) NOT NULL,
    action VARCHAR(32) NOT NULL,
    operator_name VARCHAR(128) NOT NULL,
    instance_id VARCHAR(64) NOT NULL,
    old_version BIGINT UNSIGNED NULL,
    new_version BIGINT UNSIGNED NULL,
    changed_fields_json JSON NOT NULL,
    created_at DATETIME(6) NOT NULL,
    INDEX idx_audit_object (object_type, object_key, created_at DESC),
    INDEX idx_audit_created (created_at DESC, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE aio_release_profile (
    profile_key VARCHAR(32) NOT NULL PRIMARY KEY,
    env_template MEDIUMTEXT NOT NULL,
    compose_template MEDIUMTEXT NOT NULL,
    platform_host VARCHAR(255) NOT NULL,
    platform_api_port INT UNSIGNED NOT NULL,
    platform_mqtt_host VARCHAR(255) NOT NULL,
    platform_mqtt_port INT UNSIGNED NOT NULL,
    ssh_port INT UNSIGNED NOT NULL DEFAULT 22,
    ssh_timeout_seconds INT UNSIGNED NOT NULL DEFAULT 15,
    aio_data_root VARCHAR(1000) NOT NULL,
    aio_deploy_root VARCHAR(1000) NOT NULL,
    credential_scheme VARCHAR(32) NOT NULL,
    credential_key_version INT UNSIGNED NOT NULL,
    credential_salt BINARY(16) NOT NULL,
    credential_nonce BINARY(12) NOT NULL,
    credential_ciphertext LONGBLOB NOT NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    updated_by VARCHAR(128) NOT NULL,
    updated_at DATETIME(6) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE aio_node (
    mac_normalized CHAR(12) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    ip VARCHAR(64) NOT NULL,
    display_mac VARCHAR(32) NOT NULL,
    building_id VARCHAR(64) NULL,
    region_id VARCHAR(64) NULL,
    addr_alias VARCHAR(255) NULL,
    floor VARCHAR(128) NULL,
    location VARCHAR(500) NULL,
    remark VARCHAR(1000) NULL,
    platform_aio_id VARCHAR(64) NULL,
    management_state VARCHAR(32) NOT NULL,
    source VARCHAR(32) NOT NULL,
    last_operation_id CHAR(36) NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_at DATETIME(6) NOT NULL,
    updated_at DATETIME(6) NOT NULL,
    UNIQUE INDEX uk_aio_node_platform_id (platform_aio_id),
    INDEX idx_aio_node_ip (ip),
    INDEX idx_aio_node_management_state (management_state, updated_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE aio_node_service_version (
    mac_normalized CHAR(12) NOT NULL,
    service_name VARCHAR(128) NOT NULL,
    expected_image_name VARCHAR(255) NULL,
    expected_version VARCHAR(128) NULL,
    observed_image_name VARCHAR(255) NULL,
    observed_version VARCHAR(128) NULL,
    observed_at DATETIME(6) NULL,
    source_operation_id CHAR(36) NULL,
    PRIMARY KEY (mac_normalized, service_name),
    INDEX idx_service_source_operation (source_operation_id),
    CONSTRAINT fk_service_aio_node
        FOREIGN KEY (mac_normalized) REFERENCES aio_node(mac_normalized) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

