use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::operation::{ScreenPlan, ScreenResults};
use crate::infrastructure::local_sqlite::screen_repository::now;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

fn db(error: sqlx::Error) -> AppError {
    AppError::database("保存智能屏任务依据和结果", &error)
}
pub fn plan_json(plan: &ScreenPlan) -> AppResult<(String, String)> {
    let json = serde_json::to_string(plan)
        .map_err(|_| AppError::InvalidConfig("智能屏操作参数无法保存".into()))?;
    let hash = hex::encode(Sha256::digest(json.as_bytes()));
    Ok((json, hash))
}
pub async fn save_plan(pool: &SqlitePool, id: &str, plan: &ScreenPlan) -> AppResult<String> {
    let (json, hash) = plan_json(plan)?;
    sqlx::query("INSERT INTO local_screen_task_data(local_task_id,local_project_id,plan_json,plan_sha256,result_json,updated_at) VALUES(?,?,?,?,?,?)")
        .bind(id).bind(&plan.project_id).bind(json).bind(&hash).bind(serde_json::json!({"targets":{},"finished":false}).to_string()).bind(now()).execute(pool).await.map_err(db)?;
    Ok(hash)
}
pub async fn read_plan(
    pool: &SqlitePool,
    project: &str,
    id: &str,
) -> AppResult<(ScreenPlan, String)> {
    let row=sqlx::query("SELECT plan_json,plan_sha256 FROM local_screen_task_data WHERE local_project_id=? AND local_task_id=?").bind(project).bind(id).fetch_optional(pool).await.map_err(db)?.ok_or_else(||AppError::NotFound("操作检查或任务依据不存在，请重新检查".into()))?;
    let json: String = row.try_get("plan_json").map_err(db)?;
    let hash: String = row.try_get("plan_sha256").map_err(db)?;
    if hex::encode(Sha256::digest(json.as_bytes())) != hash {
        return Err(AppError::Conflict(
            "智能屏任务依据已改变，不能继续执行".into(),
        ));
    }
    let plan: ScreenPlan = serde_json::from_str(&json)
        .map_err(|_| AppError::Conflict("智能屏任务依据格式异常".into()))?;
    if plan.project_id != project {
        return Err(AppError::Conflict("任务不属于当前项目".into()));
    }
    Ok((plan, hash))
}
pub async fn read_preflight_plan(
    pool: &SqlitePool,
    project: &str,
    id: &str,
) -> AppResult<(ScreenPlan, String)> {
    read_plan(pool, project, id).await.map_err(|error| match error {
        AppError::NotFound(_) => AppError::NotFound(
            crate::infrastructure::local_sqlite::task_repository::CLEARED_PREFLIGHT_MESSAGE.into(),
        ),
        error => error,
    })
}

pub async fn discard_unqueued_plan(pool: &SqlitePool, project: &str, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM local_screen_task_data WHERE local_project_id=? AND local_task_id=? AND NOT EXISTS(SELECT 1 FROM local_task WHERE id=?)")
        .bind(project).bind(id).bind(id).execute(pool).await.map_err(db)?;
    Ok(())
}

pub async fn read_results(pool: &SqlitePool, project: &str, id: &str) -> AppResult<ScreenResults> {
    let json:String=sqlx::query_scalar("SELECT result_json FROM local_screen_task_data WHERE local_project_id=? AND local_task_id=?").bind(project).bind(id).fetch_one(pool).await.map_err(db)?;
    serde_json::from_str(&json).map_err(|_| AppError::Conflict("智能屏任务结果格式异常".into()))
}
pub async fn save_results(
    pool: &SqlitePool,
    project: &str,
    id: &str,
    results: &ScreenResults,
) -> AppResult<()> {
    let json = serde_json::to_string(results)
        .map_err(|_| AppError::InvalidConfig("智能屏结果无法保存".into()))?;
    let changed=sqlx::query("UPDATE local_screen_task_data SET result_json=?,updated_at=? WHERE local_project_id=? AND local_task_id=?").bind(json).bind(now()).bind(project).bind(id).execute(pool).await.map_err(db)?;
    if changed.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "智能屏任务已被清理，不能继续保存".into(),
        ));
    }
    Ok(())
}

pub async fn save_target(
    pool: &SqlitePool,
    project: &str,
    id: &str,
    result: &crate::domain::smart_screen::model::ScreenTargetResult,
) -> AppResult<()> {
    let path = format!(
        "$.targets.{}",
        serde_json::to_string(&result.screen_id)
            .map_err(|_| AppError::InvalidConfig("屏编号无法保存".into()))?
    );
    let json = serde_json::to_string(result)
        .map_err(|_| AppError::InvalidConfig("屏结果无法保存".into()))?;
    let changed=sqlx::query("UPDATE local_screen_task_data SET result_json=json_set(result_json,?,json(?)),updated_at=? WHERE local_project_id=? AND local_task_id=?")
        .bind(path).bind(json).bind(now()).bind(project).bind(id).execute(pool).await.map_err(db)?;
    if changed.rows_affected() != 1 {
        return Err(AppError::NotFound("本机任务已不存在".into()));
    }
    Ok(())
}
