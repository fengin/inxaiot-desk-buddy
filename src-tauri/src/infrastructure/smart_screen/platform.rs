use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::model::{BusinessProject, ScreenAsset, ScreenFields, SpaceNode};
use sqlx::{Database, Encode, MySql, MySqlPool, QueryBuilder, Row, Transaction, Type};
use std::{collections::HashSet, future::Future};

const ALIVE_ID_BATCH_SIZE: usize = 500;

trait AliveScreenLookup {
    fn fetch_alive(
        &mut self,
        ids: &[String],
    ) -> impl Future<Output = AppResult<Vec<String>>> + Send;
}

struct MySqlAliveScreenLookup<'tx, 'connection>(&'tx mut Transaction<'connection, MySql>);

impl AliveScreenLookup for MySqlAliveScreenLookup<'_, '_> {
    async fn fetch_alive(&mut self, ids: &[String]) -> AppResult<Vec<String>> {
        alive_id_query::<MySql>(ids)
            .build_query_scalar()
            .fetch_all(&mut **self.0)
            .await
            .map_err(db)
    }
}

fn alive_id_query<'ids, DB>(ids: &'ids [String]) -> QueryBuilder<'ids, DB>
where
    DB: Database,
    &'ids str: Encode<'ids, DB> + Type<DB>,
    <DB as Database>::Arguments<'ids>: Default,
{
    // 这里只核对设备主表，不能按当前项目或有效空间过滤仍存活的记录。
    let mut query = QueryBuilder::new(
        "SELECT CAST(id AS CHAR) FROM smart_terminal_screen WHERE delete_flag=0 AND id IN (",
    );
    let mut bindings = query.separated(",");
    for id in ids {
        bindings.push_bind(id.as_str());
    }
    bindings.push_unseparated(")");
    query
}

async fn deleted_known_ids<L: AliveScreenLookup>(
    known_ids: &[String],
    returned: &[ScreenAsset],
    lookup: &mut L,
) -> AppResult<Vec<String>> {
    let returned = returned
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let remaining = known_ids
        .iter()
        .filter(|id| !returned.contains(id.as_str()) && seen.insert(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut alive = HashSet::new();
    for chunk in remaining.chunks(ALIVE_ID_BATCH_SIZE) {
        alive.extend(lookup.fetch_alive(chunk).await?);
    }
    // 所有分块均成功后才形成删除结论；失败直接返回，调用方不会替换完整缓存。
    Ok(remaining
        .into_iter()
        .filter(|id| !alive.contains(id))
        .collect())
}

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
            let status: String = r
                .try_get::<Option<String>, _>("platform_status")
                .map_err(db)?
                .unwrap_or_default();
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
    // 不在当前空间范围内可能只是移动了空间；只有记录本身删除才清理本机资料。
    let deleted =
        deleted_known_ids(known_ids, &assets, &mut MySqlAliveScreenLookup(&mut tx)).await?;
    tx.commit().await.map_err(db)?;
    Ok((assets, spaces, deleted))
}

#[cfg(test)]
mod tests;
