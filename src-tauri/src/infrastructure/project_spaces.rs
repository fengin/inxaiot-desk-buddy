use sqlx::{Executor, MySql, Row};
use crate::core::error::{AppError, AppResult};
use crate::domain::common::project_space::SpaceNode;

/// 空间目录来自当前连接的项目业务库；智能屏按业务项目限定，一体机沿用整库资产范围。
pub async fn read<'e, E: Executor<'e, Database = MySql>>(
    executor: E,
    business: Option<&str>,
) -> AppResult<Vec<SpaceNode>> {
    let rows = sqlx::query("SELECT CAST(id AS CHAR) AS id, CAST(parent_id AS CHAR) AS parent_id, area_name, COALESCE(area_level,0) AS area_level FROM t_project_building WHERE delete_flag='0' AND (? IS NULL OR project_info_id=?) ORDER BY area_level,building_sort,id")
        .bind(business).bind(business).fetch_all(executor).await
        .map_err(|error| AppError::database("读取项目空间目录", &error))?;
    rows.into_iter().map(|row| {
        let parse = |error| AppError::database("解析项目空间目录", &error);
        let level: i32 = row.try_get("area_level").map_err(parse)?;
        Ok(SpaceNode {
            id: row.try_get("id").map_err(parse)?,
            parent_id: row.try_get("parent_id").map_err(parse)?,
            name: row.try_get("area_name").map_err(parse)?,
            kind: match level { 3 => "building", 4 => "floor", 5.. => "area", _ => "other" }.into(),
        })
    }).collect()
}
