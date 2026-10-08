use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::model::{BusinessProject, ScreenAsset, ScreenFields, SpaceNode};
use sqlx::{MySqlPool, Row};

fn db(error: sqlx::Error) -> AppError {
    AppError::database("读取平台智能屏和空间", &error)
}

pub async fn source_id(pool: &MySqlPool) -> AppResult<String> {
    crate::infrastructure::project_context::database_source_id(pool).await
}
pub async fn projects(pool: &MySqlPool) -> AppResult<Vec<BusinessProject>> {
    let rows=sqlx::query("SELECT CAST(project_info_id AS CHAR) AS id, MAX(CASE WHEN parent_id=0 THEN area_name ELSE NULL END) AS name FROM t_project_building WHERE delete_flag='0' GROUP BY project_info_id ORDER BY project_info_id").fetch_all(pool).await.map_err(db)?;
    rows.into_iter()
        .map(|r| {
            let id: String = r.try_get("id").map_err(db)?;
            Ok(BusinessProject {
                name: r
                    .try_get::<Option<String>, _>("name")
                    .map_err(db)?
                    .unwrap_or_else(|| format!("业务项目 {id}")),
                id,
            })
        })
        .collect()
}
pub async fn read(
    pool: &MySqlPool,
    business: &str,
) -> AppResult<(Vec<ScreenAsset>, Vec<SpaceNode>)> {
    let (assets, spaces, _) = read_with_known_ids(pool, business, &[]).await?;
    Ok((assets, spaces))
}

pub async fn read_with_known_ids(
    pool: &MySqlPool,
    business: &str,
    known_ids: &[String],
) -> AppResult<(Vec<ScreenAsset>, Vec<SpaceNode>, Vec<String>)> {
    // 同一次一致性读取中取得完整目录和资产，不对两个来源分别分页后拼接。
    let mut tx = pool.begin().await.map_err(db)?;
    let spaces = crate::infrastructure::project_spaces::read(&mut *tx, Some(business)).await?;
    let rows=sqlx::query("SELECT CAST(s.id AS CHAR) AS id,s.name,s.ip,COALESCE(s.mac,'') AS mac,s.size,CAST(s.building_id AS CHAR) AS space_id,s.install_address,s.app_version,CAST(s.status AS CHAR) AS platform_status FROM smart_terminal_screen s JOIN t_project_building b ON b.id=s.building_id WHERE s.delete_flag=0 AND b.delete_flag='0' AND b.project_info_id=? ORDER BY s.id")
        .bind(business).fetch_all(&mut *tx).await.map_err(db)?;
    let assets = rows
        .into_iter()
        .map(|r| {
            let status: String = r.try_get::<Option<String>, _>("platform_status").map_err(db)?.unwrap_or_default();
            let size: String = r.try_get("size").map_err(db)?;
            Ok(ScreenAsset {
                id: r.try_get("id").map_err(db)?,
                source: "platform".into(),
                fields: ScreenFields {
                    name: r.try_get("name").map_err(db)?,
                    ip: r.try_get("ip").map_err(db)?,
                    mac: r.try_get("mac").map_err(db)?,
                    size: match size.as_str() {
                        "4-inch" => "4",
                        "10-inch" => "10",
                        _ => "unknown",
                    }
                    .into(),
                    space_id: Some(r.try_get("space_id").map_err(db)?),
                    location: r.try_get("install_address").map_err(db)?,
                },
                app_version: r.try_get("app_version").map_err(db)?,
                platform_status: match status.as_str() {
                    "0" => "offline",
                    "1" => "online",
                    _ => "unknown",
                }
                .into(),
                ..Default::default()
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let mut deleted = Vec::new();
    // 不在当前空间范围内可能只是移动了空间；只有记录本身删除才清理本机资料。
    for id in known_ids {
        let alive: Option<String> = sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM smart_terminal_screen WHERE id=? AND delete_flag=0")
            .bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
        if alive.is_none() { deleted.push(id.clone()); }
    }
    tx.commit().await.map_err(db)?;
    Ok((assets, spaces, deleted))
}
