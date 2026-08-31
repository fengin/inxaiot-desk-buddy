use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlRow;
use sqlx::{MySql, MySqlPool, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::credential_crypto::{
    CredentialEnvelope, CredentialMetadata, ProjectMasterKey, ReleaseCredentials,
    decrypt_release_credentials, decrypt_release_credentials_legacy, encrypt_release_credentials,
};
use super::error::{FormalError, FormalResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileValues {
    pub env_template: String,
    pub compose_template: String,
    pub platform_host: String,
    pub platform_api_port: u16,
    pub platform_mqtt_host: String,
    pub platform_mqtt_port: u16,
    pub ssh_port: u16,
    pub ssh_timeout_seconds: u32,
    pub aio_data_root: String,
    pub aio_deploy_root: String,
}

#[derive(Clone, Debug)]
pub struct ReleaseProfileWrite {
    pub profile_key: String,
    pub values: ReleaseProfileValues,
    pub credentials: ReleaseCredentials,
    pub expected_version: Option<u64>,
    pub operator_name: String,
    pub instance_id: String,
}

#[derive(Clone, Debug)]
pub struct ReleaseProfileRecord {
    pub profile_key: String,
    pub values: ReleaseProfileValues,
    pub credentials: ReleaseCredentials,
    pub version: u64,
    pub updated_by: String,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct ReleaseProfileRepository {
    pool: MySqlPool,
}

impl ReleaseProfileRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn save(
        &self,
        key: &ProjectMasterKey,
        write: ReleaseProfileWrite,
    ) -> FormalResult<ReleaseProfileRecord> {
        validate_write(&write)?;
        let envelope = encrypt_release_credentials(key, &write.credentials)?;
        self.save_envelope(write, envelope, key).await
    }

    async fn save_envelope(
        &self,
        write: ReleaseProfileWrite,
        envelope: CredentialEnvelope,
        key: &ProjectMasterKey,
    ) -> FormalResult<ReleaseProfileRecord> {
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin release profile transaction failed");
            FormalError::LocalDatabase("开始发布配置事务")
        })?;
        let (old_version, new_version, action) = if let Some(expected_version) =
            write.expected_version
        {
            let result = sqlx::query(
                "UPDATE aio_release_profile SET env_template = ?, compose_template = ?, \
                 platform_host = ?, platform_api_port = ?, platform_mqtt_host = ?, \
                 platform_mqtt_port = ?, ssh_port = ?, ssh_timeout_seconds = ?, \
                 aio_data_root = ?, aio_deploy_root = ?, credential_scheme = ?, \
                 credential_key_version = ?, credential_salt = ?, credential_nonce = ?, \
                 credential_ciphertext = ?, version = version + 1, updated_by = ?, updated_at = ? \
                 WHERE profile_key = ? AND version = ?",
            )
            .bind(&write.values.env_template)
            .bind(&write.values.compose_template)
            .bind(&write.values.platform_host)
            .bind(u32::from(write.values.platform_api_port))
            .bind(&write.values.platform_mqtt_host)
            .bind(u32::from(write.values.platform_mqtt_port))
            .bind(u32::from(write.values.ssh_port))
            .bind(write.values.ssh_timeout_seconds)
            .bind(&write.values.aio_data_root)
            .bind(&write.values.aio_deploy_root)
            .bind(&envelope.scheme)
            .bind(envelope.key_version)
            .bind(envelope.salt.as_slice())
            .bind(envelope.nonce.as_slice())
            .bind(&envelope.ciphertext)
            .bind(&write.operator_name)
            .bind(now)
            .bind(&write.profile_key)
            .bind(expected_version)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("更新发布配置", error))?;
            if result.rows_affected() != 1 {
                return Err(FormalError::Conflict("发布配置已被其他实例更新".into()));
            }
            (Some(expected_version), expected_version + 1, "update")
        } else {
            sqlx::query(
                "INSERT INTO aio_release_profile \
                 (profile_key, env_template, compose_template, platform_host, platform_api_port, \
                  platform_mqtt_host, platform_mqtt_port, ssh_port, ssh_timeout_seconds, \
                  aio_data_root, aio_deploy_root, credential_scheme, credential_key_version, \
                  credential_salt, credential_nonce, credential_ciphertext, version, updated_by, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
            )
            .bind(&write.profile_key)
            .bind(&write.values.env_template)
            .bind(&write.values.compose_template)
            .bind(&write.values.platform_host)
            .bind(u32::from(write.values.platform_api_port))
            .bind(&write.values.platform_mqtt_host)
            .bind(u32::from(write.values.platform_mqtt_port))
            .bind(u32::from(write.values.ssh_port))
            .bind(write.values.ssh_timeout_seconds)
            .bind(&write.values.aio_data_root)
            .bind(&write.values.aio_deploy_root)
            .bind(&envelope.scheme)
            .bind(envelope.key_version)
            .bind(envelope.salt.as_slice())
            .bind(envelope.nonce.as_slice())
            .bind(&envelope.ciphertext)
            .bind(&write.operator_name)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("创建发布配置", error))?;
            (None, 1, "create")
        };
        let changed_fields = serde_json::json!([
            "env_template",
            "compose_template",
            "platform_host",
            "platform_api_port",
            "platform_mqtt_host",
            "platform_mqtt_port",
            "ssh_port",
            "ssh_timeout_seconds",
            "aio_data_root",
            "aio_deploy_root",
            "credentials"
        ]);
        sqlx::query(
            "INSERT INTO audit_event \
             (id, domain_type, object_type, object_key, action, operator_name, instance_id, \
              old_version, new_version, changed_fields_json, created_at) \
             VALUES (?, 'aio', 'aio_release_profile', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(&write.profile_key)
        .bind(action)
        .bind(&write.operator_name)
        .bind(&write.instance_id)
        .bind(old_version)
        .bind(new_version)
        .bind(changed_fields.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("记录发布配置审计", error))?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit release profile transaction failed");
            FormalError::LocalDatabase("提交发布配置事务")
        })?;
        self.get(key, &write.profile_key).await
    }

    pub async fn get(
        &self,
        key: &ProjectMasterKey,
        profile_key: &str,
    ) -> FormalResult<ReleaseProfileRecord> {
        let row = sqlx::query(
            "SELECT profile_key, env_template, compose_template, platform_host, platform_api_port, \
             platform_mqtt_host, platform_mqtt_port, ssh_port, ssh_timeout_seconds, \
             aio_data_root, aio_deploy_root, credential_scheme, credential_key_version, \
             credential_salt, credential_nonce, credential_ciphertext, version, updated_by, updated_at \
             FROM aio_release_profile WHERE profile_key = ?",
        )
        .bind(profile_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("读取发布配置", error))?
        .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        let salt = fixed_array::<16>(
            row.try_get("credential_salt")
                .map_err(|_| FormalError::LocalDatabase("解析发布凭据盐值"))?,
        )?;
        let nonce = fixed_array::<12>(
            row.try_get("credential_nonce")
                .map_err(|_| FormalError::LocalDatabase("解析发布凭据随机数"))?,
        )?;
        let envelope = CredentialEnvelope {
            scheme: row
                .try_get("credential_scheme")
                .map_err(|_| FormalError::LocalDatabase("解析凭据加密格式"))?,
            key_version: row
                .try_get("credential_key_version")
                .map_err(|_| FormalError::LocalDatabase("解析凭据密钥版本"))?,
            salt,
            nonce,
            ciphertext: row
                .try_get("credential_ciphertext")
                .map_err(|_| FormalError::LocalDatabase("解析发布凭据密文"))?,
        };
        Ok(ReleaseProfileRecord {
            profile_key: row
                .try_get("profile_key")
                .map_err(|_| FormalError::LocalDatabase("解析发布配置键"))?,
            values: ReleaseProfileValues {
                env_template: row
                    .try_get("env_template")
                    .map_err(|_| FormalError::LocalDatabase("解析环境模板"))?,
                compose_template: row
                    .try_get("compose_template")
                    .map_err(|_| FormalError::LocalDatabase("解析Compose模板"))?,
                platform_host: row
                    .try_get("platform_host")
                    .map_err(|_| FormalError::LocalDatabase("解析平台主机"))?,
                platform_api_port: row
                    .try_get::<u32, _>("platform_api_port")
                    .map_err(|_| FormalError::LocalDatabase("解析平台API端口"))?
                    as u16,
                platform_mqtt_host: row
                    .try_get("platform_mqtt_host")
                    .map_err(|_| FormalError::LocalDatabase("解析平台MQTT主机"))?,
                platform_mqtt_port: row
                    .try_get::<u32, _>("platform_mqtt_port")
                    .map_err(|_| FormalError::LocalDatabase("解析平台MQTT端口"))?
                    as u16,
                ssh_port: row
                    .try_get::<u32, _>("ssh_port")
                    .map_err(|_| FormalError::LocalDatabase("解析SSH端口"))?
                    as u16,
                ssh_timeout_seconds: row
                    .try_get("ssh_timeout_seconds")
                    .map_err(|_| FormalError::LocalDatabase("解析SSH超时"))?,
                aio_data_root: row
                    .try_get("aio_data_root")
                    .map_err(|_| FormalError::LocalDatabase("解析一体机数据目录"))?,
                aio_deploy_root: row
                    .try_get("aio_deploy_root")
                    .map_err(|_| FormalError::LocalDatabase("解析一体机部署目录"))?,
            },
            credentials: decrypt_release_credentials(key, &envelope)?,
            version: row
                .try_get("version")
                .map_err(|_| FormalError::LocalDatabase("解析发布配置版本"))?,
            updated_by: row
                .try_get("updated_by")
                .map_err(|_| FormalError::LocalDatabase("解析发布配置修改人"))?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|_| FormalError::LocalDatabase("解析发布配置时间"))?,
        })
    }

    pub async fn credential_metadata(
        &self,
        profile_key: &str,
    ) -> FormalResult<Option<CredentialMetadata>> {
        let row = sqlx::query(
            "SELECT credential_scheme, credential_key_version \
             FROM aio_release_profile WHERE profile_key = ?",
        )
        .bind(profile_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("读取发布凭据元数据", error))?;
        row.map(|row| {
            Ok(CredentialMetadata {
                scheme: row
                    .try_get("credential_scheme")
                    .map_err(|_| FormalError::LocalDatabase("解析凭据加密格式"))?,
                key_version: row
                    .try_get("credential_key_version")
                    .map_err(|_| FormalError::LocalDatabase("解析凭据密钥版本"))?,
            })
        })
        .transpose()
    }

    pub async fn migrate_legacy_credentials(
        &self,
        profile_key: &str,
        database_password: &str,
        new_key: &ProjectMasterKey,
        operator_name: &str,
        instance_id: &str,
    ) -> FormalResult<ReleaseProfileRecord> {
        validate_maintenance(profile_key, operator_name, instance_id)?;
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin credential migration failed");
            FormalError::LocalDatabase("开始发布凭据迁移事务")
        })?;
        let row = sqlx::query(
            "SELECT credential_scheme, credential_key_version, credential_salt, \
             credential_nonce, credential_ciphertext, version \
             FROM aio_release_profile WHERE profile_key = ? FOR UPDATE",
        )
        .bind(profile_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| map_error("锁定待迁移发布凭据", error))?
        .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        let old_envelope = envelope_from_row(&row)?;
        let metadata = CredentialMetadata {
            scheme: old_envelope.scheme.clone(),
            key_version: old_envelope.key_version,
        };
        if !metadata.is_legacy() {
            return Err(FormalError::Conflict(
                "发布凭据已由其他实例迁移，请重新加载项目主密钥".into(),
            ));
        }
        let credentials = decrypt_release_credentials_legacy(database_password, &old_envelope)?;
        let new_envelope = encrypt_release_credentials(new_key, &credentials)?;
        let profile_version: u64 = row
            .try_get("version")
            .map_err(|_| FormalError::LocalDatabase("解析发布配置版本"))?;
        update_credential_envelope(&mut transaction, profile_key, &new_envelope, operator_name)
            .await?;
        insert_credential_audit(
            &mut transaction,
            profile_key,
            "credential_key_migrate",
            operator_name,
            instance_id,
            profile_version,
            old_envelope.key_version,
            new_envelope.key_version,
        )
        .await?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit credential migration failed");
            FormalError::LocalDatabase("提交发布凭据迁移事务")
        })?;
        self.get(new_key, profile_key).await
    }

    pub async fn rotate_credentials(
        &self,
        profile_key: &str,
        old_key: &ProjectMasterKey,
        new_key: &ProjectMasterKey,
        operator_name: &str,
        instance_id: &str,
    ) -> FormalResult<ReleaseProfileRecord> {
        validate_maintenance(profile_key, operator_name, instance_id)?;
        if new_key.version() != old_key.version().saturating_add(1) {
            return Err(FormalError::InvalidConfig(
                "项目主密钥必须按连续版本轮换".into(),
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin credential rotation failed");
            FormalError::LocalDatabase("开始发布凭据轮换事务")
        })?;
        let row = sqlx::query(
            "SELECT credential_scheme, credential_key_version, credential_salt, \
             credential_nonce, credential_ciphertext, version \
             FROM aio_release_profile WHERE profile_key = ? FOR UPDATE",
        )
        .bind(profile_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| map_error("锁定待轮换发布凭据", error))?
        .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        let old_envelope = envelope_from_row(&row)?;
        let metadata = CredentialMetadata {
            scheme: old_envelope.scheme.clone(),
            key_version: old_envelope.key_version,
        };
        if !metadata.is_project_key() || metadata.key_version != old_key.version() {
            return Err(FormalError::Conflict(
                "发布凭据密钥版本已变化，请刷新后重试".into(),
            ));
        }
        let credentials = decrypt_release_credentials(old_key, &old_envelope)?;
        let new_envelope = encrypt_release_credentials(new_key, &credentials)?;
        let profile_version: u64 = row
            .try_get("version")
            .map_err(|_| FormalError::LocalDatabase("解析发布配置版本"))?;
        update_credential_envelope(&mut transaction, profile_key, &new_envelope, operator_name)
            .await?;
        insert_credential_audit(
            &mut transaction,
            profile_key,
            "credential_key_rotate",
            operator_name,
            instance_id,
            profile_version,
            old_envelope.key_version,
            new_envelope.key_version,
        )
        .await?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit credential rotation failed");
            FormalError::LocalDatabase("提交发布凭据轮换事务")
        })?;
        self.get(new_key, profile_key).await
    }

    pub async fn delete_test_profile(&self, profile_key: &str) -> FormalResult<()> {
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin profile cleanup failed");
            FormalError::LocalDatabase("开始测试配置清理事务")
        })?;
        sqlx::query(
            "DELETE FROM audit_event WHERE object_type = 'aio_release_profile' AND object_key = ?",
        )
        .bind(profile_key)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("清理发布配置审计", error))?;
        sqlx::query("DELETE FROM aio_release_profile WHERE profile_key = ?")
            .bind(profile_key)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("清理发布配置", error))?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit profile cleanup failed");
            FormalError::LocalDatabase("提交测试配置清理事务")
        })?;
        Ok(())
    }
}

fn validate_write(write: &ReleaseProfileWrite) -> FormalResult<()> {
    if write.profile_key.is_empty()
        || write.values.env_template.is_empty()
        || write.values.compose_template.is_empty()
        || write.values.platform_host.is_empty()
        || write.values.platform_api_port == 0
        || write.values.platform_mqtt_host.is_empty()
        || write.values.platform_mqtt_port == 0
        || write.values.ssh_port == 0
        || write.values.aio_data_root.is_empty()
        || write.values.aio_deploy_root.is_empty()
        || write.operator_name.is_empty()
        || write.instance_id.is_empty()
    {
        return Err(FormalError::InvalidConfig("发布配置参数不完整".into()));
    }
    Ok(())
}

fn validate_maintenance(
    profile_key: &str,
    operator_name: &str,
    instance_id: &str,
) -> FormalResult<()> {
    if profile_key.trim().is_empty()
        || operator_name.trim().is_empty()
        || instance_id.trim().is_empty()
    {
        return Err(FormalError::InvalidConfig("发布凭据维护参数不完整".into()));
    }
    Ok(())
}

fn envelope_from_row(row: &MySqlRow) -> FormalResult<CredentialEnvelope> {
    Ok(CredentialEnvelope {
        scheme: row
            .try_get("credential_scheme")
            .map_err(|_| FormalError::LocalDatabase("解析凭据加密格式"))?,
        key_version: row
            .try_get("credential_key_version")
            .map_err(|_| FormalError::LocalDatabase("解析凭据密钥版本"))?,
        salt: fixed_array::<16>(
            row.try_get("credential_salt")
                .map_err(|_| FormalError::LocalDatabase("解析发布凭据盐值"))?,
        )?,
        nonce: fixed_array::<12>(
            row.try_get("credential_nonce")
                .map_err(|_| FormalError::LocalDatabase("解析发布凭据随机数"))?,
        )?,
        ciphertext: row
            .try_get("credential_ciphertext")
            .map_err(|_| FormalError::LocalDatabase("解析发布凭据密文"))?,
    })
}

async fn update_credential_envelope(
    transaction: &mut Transaction<'_, MySql>,
    profile_key: &str,
    envelope: &CredentialEnvelope,
    operator_name: &str,
) -> FormalResult<()> {
    let result = sqlx::query(
        "UPDATE aio_release_profile SET credential_scheme = ?, credential_key_version = ?, \
         credential_salt = ?, credential_nonce = ?, credential_ciphertext = ?, \
         updated_by = ?, updated_at = ? WHERE profile_key = ?",
    )
    .bind(&envelope.scheme)
    .bind(envelope.key_version)
    .bind(envelope.salt.as_slice())
    .bind(envelope.nonce.as_slice())
    .bind(&envelope.ciphertext)
    .bind(operator_name)
    .bind(OffsetDateTime::now_utc())
    .bind(profile_key)
    .execute(&mut **transaction)
    .await
    .map_err(|error| map_error("更新发布凭据密文", error))?;
    if result.rows_affected() != 1 {
        return Err(FormalError::Conflict(
            "发布配置在凭据维护期间发生变化".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_credential_audit(
    transaction: &mut Transaction<'_, MySql>,
    profile_key: &str,
    action: &str,
    operator_name: &str,
    instance_id: &str,
    profile_version: u64,
    old_key_version: u32,
    new_key_version: u32,
) -> FormalResult<()> {
    let changed_fields = serde_json::json!({
        "fields": ["credential_scheme", "credential_key_version", "credentials"],
        "oldKeyVersion": old_key_version,
        "newKeyVersion": new_key_version,
    });
    sqlx::query(
        "INSERT INTO audit_event \
         (id, domain_type, object_type, object_key, action, operator_name, instance_id, \
          old_version, new_version, changed_fields_json, created_at) \
         VALUES (?, 'aio', 'aio_release_profile', ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(profile_key)
    .bind(action)
    .bind(operator_name)
    .bind(instance_id)
    .bind(profile_version)
    .bind(profile_version)
    .bind(changed_fields.to_string())
    .bind(OffsetDateTime::now_utc())
    .execute(&mut **transaction)
    .await
    .map_err(|error| map_error("记录发布凭据维护审计", error))?;
    Ok(())
}

fn fixed_array<const N: usize>(value: Vec<u8>) -> FormalResult<[u8; N]> {
    value
        .try_into()
        .map_err(|_| FormalError::LocalDatabase("发布凭据密文字段长度无效"))
}

fn map_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?error, "release profile mysql operation failed");
    FormalError::LocalDatabase(operation)
}
