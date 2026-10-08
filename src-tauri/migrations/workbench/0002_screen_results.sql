-- 只扩展公共结果与共享库归属，不建立第二套屏资产。
CREATE TABLE workbench_data_source (
    singleton_id TINYINT NOT NULL PRIMARY KEY,
    data_source_id VARCHAR(191) NOT NULL,
    platform_schema VARCHAR(64) NOT NULL,
    created_at DATETIME(6) NOT NULL,
    CONSTRAINT ck_workbench_single_source CHECK (singleton_id = 1)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

ALTER TABLE operation_record ADD COLUMN business_project_id VARCHAR(64) NULL,
    ADD INDEX idx_operation_business_project (business_project_id, domain_type, started_at);
ALTER TABLE operation_target_result ADD COLUMN result_detail_json JSON NULL;
ALTER TABLE audit_event ADD COLUMN business_project_id VARCHAR(64) NULL,
    ADD COLUMN request_id CHAR(36) NULL,
    ADD INDEX idx_audit_project_request (business_project_id, request_id);
