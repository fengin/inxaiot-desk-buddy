use super::write_context::ScreenWriteContext;
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::{
    model::{ScreenAsset, ScreenFields},
    registration::{field_values, set_field},
    rules::{normalize_mac, valid_mac, validate_fields},
};
use crate::formal::resource_lease_repository::LeaseGrant;
use sqlx::{MySql, MySqlPool, Row, Transaction};

#[derive(Clone, Debug)]
pub struct PlatformScreen {
    pub asset: ScreenAsset,
    pub business: Option<String>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationReceipt {
    pub before: Option<ScreenAsset>,
    pub after: ScreenAsset,
    pub wrote: bool,
}
#[derive(Debug)]
pub enum MutationError {
    Rejected(AppError),
    Uncertain(AppError),
}
impl MutationError {
    pub fn message(&self) -> String {
        match self {
            Self::Rejected(e) | Self::Uncertain(e) => e.to_string(),
        }
    }
}
fn db(error: sqlx::Error) -> AppError {
    AppError::database("读写平台智能屏资料", &error)
}
const SELECT_RECORD: &str = "SELECT CAST(s.id AS CHAR) AS id,s.name,s.ip,COALESCE(s.mac,'') AS mac,s.size,CAST(s.building_id AS CHAR) AS space_id,s.install_address,s.app_version,CAST(s.status AS CHAR) AS status,CAST(b.project_info_id AS CHAR) AS business FROM smart_terminal_screen s LEFT JOIN t_project_building b ON b.id=s.building_id AND b.delete_flag='0' WHERE s.id=? AND s.delete_flag=0";
fn map_record(row: sqlx::mysql::MySqlRow) -> AppResult<PlatformScreen> {
    let size: String = row.try_get("size").map_err(db)?;
    let status: String = row.try_get::<Option<String>, _>("status").map_err(db)?.unwrap_or_default();
    Ok(PlatformScreen {
        business: row.try_get("business").map_err(db)?,
        asset: ScreenAsset {
            id: row.try_get("id").map_err(db)?,
            source: "platform".into(),
            fields: ScreenFields {
                name: row.try_get("name").map_err(db)?,
                ip: row.try_get("ip").map_err(db)?,
                mac: row.try_get("mac").map_err(db)?,
                size: match size.as_str() {
                    "4-inch" => "4",
                    "10-inch" => "10",
                    _ => "unknown",
                }
                .into(),
                space_id: Some(row.try_get("space_id").map_err(db)?),
                location: row.try_get("install_address").map_err(db)?,
            },
            app_version: row.try_get("app_version").map_err(db)?,
            platform_status: match status.as_str() {
                "1" => "online",
                "0" => "offline",
                _ => "unknown",
            }
            .into(),
            ..Default::default()
        },
    })
}
pub async fn record(pool: &MySqlPool, id: &str) -> AppResult<Option<PlatformScreen>> {
    sqlx::query(SELECT_RECORD)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(db)?
        .map(map_record)
        .transpose()
}

pub async fn record_locked(tx:&mut Transaction<'_,MySql>,id:&str)->AppResult<Option<PlatformScreen>>{
    sqlx::query(&format!("{SELECT_RECORD} FOR UPDATE")).bind(id).fetch_optional(&mut **tx).await.map_err(db)?.map(map_record).transpose()
}

pub async fn set_business_value(context:&ScreenWriteContext,grant:&LeaseGrant,id:&str,expected_fields:&ScreenFields,field:&str,expected:Option<&str>,next:&str)->Result<MutationReceipt,MutationError>{
    if grant.resource_type!="smart_screen" || grant.resource_key!=format!("{}:{id}",context.business){return Err(MutationError::Rejected(AppError::Conflict("操作占用不属于当前屏".into())));}
    if !["app_version","status"].contains(&field){return Err(MutationError::Rejected(AppError::InvalidConfig("不支持此平台字段更新".into())));}
    if field=="app_version" && (next.is_empty()||next.chars().count()>32){return Err(MutationError::Rejected(AppError::InvalidConfig("小新版本名称无效".into())));}
    if field=="status" && !["online","offline"].contains(&next){return Err(MutationError::Rejected(AppError::InvalidConfig("在线状态无效".into())));}
    let mut tx=context.write.begin().await.map_err(|e|MutationError::Rejected(db(e)))?;
    guard_lease(&mut tx,&context.shared_schema,grant).await.map_err(MutationError::Rejected)?;
    let record=record_locked(&mut tx,id).await.map_err(MutationError::Rejected)?.ok_or_else(||MutationError::Rejected(AppError::NotFound("平台屏已不存在".into())))?;
    if record.business.as_deref()!=Some(&context.business)||record.asset.fields.ip!=expected_fields.ip||normalize_mac(&record.asset.fields.mac)!=normalize_mac(&expected_fields.mac)||record.asset.fields.size!=expected_fields.size{return Err(MutationError::Rejected(AppError::Conflict("屏的项目、地址或身份已变化，请重新核对".into())));}
    let current=if field=="app_version"{record.asset.app_version.as_deref()}else{Some(record.asset.platform_status.as_str())};
    if current!=expected && current!=Some(next){return Err(MutationError::Rejected(AppError::Conflict("平台当前值已变化，不覆盖未确认的数据".into())));}
    let wrote=current!=Some(next);
    if wrote {
        if field=="app_version"{sqlx::query("UPDATE smart_terminal_screen SET app_version=? WHERE id=? AND delete_flag=0").bind(next).bind(id).execute(&mut *tx).await.map_err(write_error)?;}
        else{sqlx::query("UPDATE smart_terminal_screen SET status=? WHERE id=? AND delete_flag=0").bind(if next=="online"{1i32}else{0i32}).bind(id).execute(&mut *tx).await.map_err(write_error)?;}
    }
    let saved=record_locked(&mut tx,id).await.map_err(MutationError::Uncertain)?.ok_or_else(||MutationError::Uncertain(AppError::Conflict("平台记录回读失败".into())))?.asset;
    tx.commit().await.map_err(|e|MutationError::Uncertain(db(e)))?;
    Ok(MutationReceipt{before:Some(record.asset),after:saved,wrote})
}
pub async fn duplicate_ids(
    pool: &MySqlPool,
    fields: &ScreenFields,
    exclude: Option<&str>,
) -> AppResult<Vec<String>> {
    sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM smart_terminal_screen WHERE delete_flag=0 AND (? IS NULL OR CAST(id AS CHAR)<>?) AND (ip=? OR (?<>'' AND UPPER(REPLACE(REPLACE(COALESCE(mac,''),':',''),'-',''))=?)) ORDER BY id")
        .bind(exclude).bind(exclude).bind(&fields.ip).bind(&fields.mac).bind(normalize_mac(&fields.mac)).fetch_all(pool).await.map_err(db)
}
pub async fn allocate_id(pool: &MySqlPool) -> AppResult<String> {
    // 使用与平台长整型编号相同的时间排序布局；主键约束处理极小概率碰撞。
    let millis: u64 =
        sqlx::query_scalar("SELECT CAST(UNIX_TIMESTAMP(CURRENT_TIMESTAMP(3))*1000 AS UNSIGNED)")
            .fetch_one(pool)
            .await
            .map_err(db)?;
    let elapsed = millis
        .checked_sub(1_288_834_974_657)
        .filter(|v| *v < (1u64 << 41))
        .ok_or_else(|| AppError::Conflict("平台时间不在编号可用范围内".into()))?;
    let id = (elapsed << 22) | u64::from(rand::random::<u32>() & ((1 << 22) - 1));
    if id == 0 {
        return Err(AppError::Conflict("未能分配有效屏编号".into()));
    }
    Ok(id.to_string())
}
pub async fn guard_lease(
    tx: &mut Transaction<'_, MySql>,
    schema: &str,
    grant: &LeaseGrant,
) -> AppResult<()> {
    if schema.is_empty()
        || !schema
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(AppError::InvalidConfig("工作台库名称无效".into()));
    }
    // 同一 MySQL 实例分库：锁住占用行直至业务事务提交，避免检查后被另一工作台接管。
    let query = format!(
        "SELECT fencing_token FROM `{schema}`.resource_lease WHERE resource_type=? AND resource_key=? AND operation_id=? AND owner_instance_id=? AND lease_token=? AND fencing_token=? AND lease_state='active' AND expires_at>UTC_TIMESTAMP(6) FOR UPDATE"
    );
    let matched: Option<u64> = sqlx::query_scalar(&query)
        .bind(&grant.resource_type)
        .bind(&grant.resource_key)
        .bind(&grant.operation_id)
        .bind(&grant.owner_instance_id)
        .bind(&grant.lease_token)
        .bind(grant.fencing_token)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    if matched != Some(grant.fencing_token) {
        return Err(AppError::Conflict(
            "该屏的操作占用已失效，未执行后续写入".into(),
        ));
    }
    Ok(())
}
async fn guard_registry(
    tx: &mut Transaction<'_, MySql>,
    context: &ScreenWriteContext,
    id: &str,
    grants: &[LeaseGrant],
) -> AppResult<()> {
    let key = format!("{}:{id}", context.business);
    if !grants
        .iter()
        .any(|g| g.resource_type == "smart_screen" && g.resource_key == key)
        || !grants
            .iter()
            .any(|g| g.resource_type == "smart_screen_registry" && g.resource_key == context.source)
    {
        return Err(AppError::Conflict(
            "未取得当前屏及登记操作的有效占用".into(),
        ));
    }
    for grant in grants {
        guard_lease(tx, &context.shared_schema, grant).await?;
    }
    Ok(())
}
async fn valid_space(
    tx: &mut Transaction<'_, MySql>,
    business: &str,
    space: Option<&str>,
) -> AppResult<()> {
    let found:Option<String>=sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM t_project_building WHERE id=? AND project_info_id=? AND delete_flag='0' FOR UPDATE").bind(space).bind(business).fetch_optional(&mut **tx).await.map_err(db)?;
    if found.is_none() || found.as_deref() != space {
        return Err(AppError::Conflict("所选空间不再属于当前有效项目".into()));
    }
    Ok(())
}

fn write_error(error: sqlx::Error) -> MutationError {
    if let sqlx::Error::Database(database) = &error {
        let code = database.code().unwrap_or_default();
        if code == "1062" {
            return MutationError::Rejected(AppError::Conflict(
                "平台编号发生冲突，未关联已有记录，请重新检查".into(),
            ));
        }
        return MutationError::Rejected(db(error));
    }
    MutationError::Uncertain(db(error))
}
pub async fn insert(
    context: &ScreenWriteContext,
    grants: &[LeaseGrant],
    id: &str,
    fields: &ScreenFields,
) -> Result<MutationReceipt, MutationError> {
    validate_fields(fields, true).map_err(MutationError::Rejected)?;
    if id
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .is_none()
    {
        return Err(MutationError::Rejected(AppError::InvalidConfig(
            "平台屏编号无效".into(),
        )));
    }
    let mut tx = context
        .write
        .begin()
        .await
        .map_err(|e| MutationError::Rejected(db(e)))?;
    guard_registry(&mut tx, context, id, grants)
        .await
        .map_err(MutationError::Rejected)?;
    valid_space(&mut tx, &context.business, fields.space_id.as_deref())
        .await
        .map_err(MutationError::Rejected)?;
    let duplicate:i64=sqlx::query_scalar("SELECT COUNT(*) FROM smart_terminal_screen WHERE delete_flag=0 AND (ip=? OR (?<>'' AND UPPER(REPLACE(REPLACE(COALESCE(mac,''),':',''),'-',''))=?))")
        .bind(&fields.ip).bind(&fields.mac).bind(normalize_mac(&fields.mac)).fetch_one(&mut *tx).await.map_err(|e|MutationError::Rejected(db(e)))?;
    if duplicate > 0 {
        return Err(MutationError::Rejected(AppError::Conflict(
            "IP 或 MAC 已在平台登记，请先核对重复记录".into(),
        )));
    }
    sqlx::query("INSERT INTO smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,delete_flag) VALUES(?,?,?,?,?,?,?,0)")
        .bind(id).bind(&fields.name).bind(&fields.ip).bind(&fields.mac).bind(if fields.size=="4"{"4-inch"}else{"10-inch"}).bind(&fields.space_id).bind(&fields.location).execute(&mut *tx).await.map_err(write_error)?;
    let row = sqlx::query(SELECT_RECORD)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| MutationError::Uncertain(db(e)))?;
    let created = map_record(row).map_err(MutationError::Uncertain)?.asset;
    tx.commit()
        .await
        .map_err(|e| MutationError::Uncertain(db(e)))?;
    Ok(MutationReceipt {
        before: None,
        after: created,
        wrote: true,
    })
}
pub async fn update(
    context: &ScreenWriteContext,
    grants: &[LeaseGrant],
    id: &str,
    before: &ScreenFields,
    after: &ScreenFields,
    version: Option<(Option<&str>, Option<&str>)>,
    allow_mac_replacement: bool,
) -> Result<MutationReceipt, MutationError> {
    validate_fields(after, true).map_err(MutationError::Rejected)?;
    let mut tx = context
        .write
        .begin()
        .await
        .map_err(|e| MutationError::Rejected(db(e)))?;
    guard_registry(&mut tx, context, id, grants)
        .await
        .map_err(MutationError::Rejected)?;
    let row = sqlx::query(&format!("{SELECT_RECORD} FOR UPDATE"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| MutationError::Rejected(db(e)))?
        .ok_or_else(|| {
            MutationError::Rejected(AppError::Conflict("平台屏不存在或已删除".into()))
        })?;
    let current = map_record(row).map_err(MutationError::Rejected)?;
    if current.business.as_deref() != Some(&context.business) {
        return Err(MutationError::Rejected(AppError::Conflict(
            "平台屏不属于当前业务项目".into(),
        )));
    }
    let baseline = field_values(before);
    let wanted = field_values(after);
    let actual = field_values(&current.asset.fields);
    for key in ["ip", "mac", "size"] {
        let normalize = |value: &str| {
            if key == "mac" {
                normalize_mac(value)
            } else {
                value.to_string()
            }
        };
        if normalize(&actual[key]) != normalize(&baseline[key])
            && normalize(&actual[key]) != normalize(&wanted[key])
        {
            return Err(MutationError::Rejected(AppError::Conflict(
                "平台设备地址、身份或尺寸已变化，请重新预览".into(),
            )));
        }
    }
    let mut next = current.asset.fields.clone();
    for (key, value) in &wanted {
        if value != &baseline[key] {
            if actual[key] != baseline[key] && actual[key] != *value {
                return Err(MutationError::Rejected(AppError::Conflict(format!(
                    "平台{}已变化，请重新预览",
                    crate::domain::smart_screen::registration::label(key)
                ))));
            }
            set_field(&mut next, key, value.clone());
        }
    }
    if !allow_mac_replacement
        && valid_mac(&before.mac)
        && normalize_mac(&before.mac) != normalize_mac(&next.mac)
    {
        return Err(MutationError::Rejected(AppError::Conflict(
            "普通资料更新不能替换已有设备身份，请先核实".into(),
        )));
    }
    valid_space(&mut tx, &context.business, next.space_id.as_deref())
        .await
        .map_err(MutationError::Rejected)?;
    let duplicate:i64=sqlx::query_scalar("SELECT COUNT(*) FROM smart_terminal_screen WHERE delete_flag=0 AND id<>? AND (ip=? OR (?<>'' AND UPPER(REPLACE(REPLACE(COALESCE(mac,''),':',''),'-',''))=?))")
        .bind(id).bind(&next.ip).bind(&next.mac).bind(normalize_mac(&next.mac)).fetch_one(&mut *tx).await.map_err(|e|MutationError::Rejected(db(e)))?;
    if duplicate > 0 {
        return Err(MutationError::Rejected(AppError::Conflict(
            "IP 或 MAC 已有其他平台记录".into(),
        )));
    }
    let mut wrote = next != current.asset.fields;
    if wrote {
        sqlx::query("UPDATE smart_terminal_screen SET name=?,ip=?,mac=?,size=?,building_id=?,install_address=? WHERE id=? AND delete_flag=0")
            .bind(&next.name).bind(&next.ip).bind(&next.mac).bind(if next.size=="4"{"4-inch"}else{"10-inch"}).bind(&next.space_id).bind(&next.location).bind(id).execute(&mut *tx).await.map_err(write_error)?;
    }
    if let Some((expected, value)) = version {
        if current.asset.app_version.as_deref() != expected
            && current.asset.app_version.as_deref() != value
        {
            return Err(MutationError::Rejected(AppError::Conflict(
                "平台小新版本已变化，请重新核对合并来源".into(),
            )));
        }
        if value.is_some_and(|v| v.chars().count() > 32) {
            return Err(MutationError::Rejected(AppError::InvalidConfig(
                "小新版本不能超过 32 字符".into(),
            )));
        }
        if current.asset.app_version.as_deref() != value {
            sqlx::query(
                "UPDATE smart_terminal_screen SET app_version=? WHERE id=? AND delete_flag=0",
            )
            .bind(value)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            wrote = true;
        }
    }
    let row = sqlx::query(SELECT_RECORD)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| MutationError::Uncertain(db(e)))?;
    let saved = map_record(row).map_err(MutationError::Uncertain)?.asset;
    tx.commit()
        .await
        .map_err(|e| MutationError::Uncertain(db(e)))?;
    Ok(MutationReceipt {
        before: Some(current.asset),
        after: saved,
        wrote,
    })
}
