use sqlx::{MySqlPool, Row};
use crate::{core::error::{AppError,AppResult}, domain::{aio::inventory::WorkbenchNodeSnapshot,common::project_space::space_path}, formal::resource_lease_repository::LeaseGrant, infrastructure::{device_api::AioRegistrationPayload,project_spaces}};

#[derive(Clone)]
pub struct RegistrationCompletion { pub pool: MySqlPool, pub shared_schema: String }

/// 既有平台注册接口不接受“空间非零、位置空”。先完成设备登记，再按同一占用补齐空间。
pub fn compatible_payload(input: &AioRegistrationPayload) -> AioRegistrationPayload {
    let mut payload=input.clone();
    if payload.building_id.unwrap_or(0)>0 && payload.addr_alias.as_deref().is_none_or(|value| value.trim().is_empty()) {
        payload.building_id=Some(0);
    }
    payload
}

impl RegistrationCompletion {
    pub async fn confirm(&self, expected: &WorkbenchNodeSnapshot, grant: &LeaseGrant) -> AppResult<String> {
        if self.shared_schema.is_empty() || !self.shared_schema.bytes().all(|c| c.is_ascii_alphanumeric() || c==b'_')
            || grant.resource_type!="aio" || grant.resource_key!=expected.mac_normalized {
            return Err(AppError::InvalidConfig("平台注册核对参数无效".into()));
        }
        let db=|error| AppError::database("核对一体机平台注册资料",&error);
        let mut tx=self.pool.begin().await.map_err(db)?;
        let sql=format!("SELECT fencing_token FROM `{}`.resource_lease WHERE resource_type='aio' AND resource_key=? AND operation_id=? AND owner_instance_id=? AND lease_token=? AND fencing_token=? AND lease_state='active' AND expires_at>UTC_TIMESTAMP(6) FOR UPDATE",self.shared_schema);
        let held:Option<u64>=sqlx::query_scalar(&sql).bind(&grant.resource_key).bind(&grant.operation_id).bind(&grant.owner_instance_id).bind(&grant.lease_token).bind(grant.fencing_token).fetch_optional(&mut *tx).await.map_err(db)?;
        if held.is_none(){return Err(AppError::Conflict("部署已被接手，不能继续保存注册资料".into()));}
        let rows=sqlx::query("SELECT CAST(id AS CHAR) AS id,name,ip,CAST(building_id AS CHAR) AS space,addr_alias FROM op_edge_aio_server WHERE REPLACE(REPLACE(REPLACE(UPPER(mac),':',''),'-',''),' ','')=? FOR UPDATE")
            .bind(&expected.mac_normalized).fetch_all(&mut *tx).await.map_err(db)?;
        if rows.is_empty(){return Err(AppError::NotFound("注册接口已完成，但平台尚未查询到该一体机".into()));}
        if rows.len()!=1{return Err(AppError::Conflict("平台存在重复 MAC，不能确认登记结果".into()));}
        let row=&rows[0]; let id:String=row.try_get("id").map_err(db)?;
        let space:String=row.try_get("space").map_err(db)?; let location:String=row.try_get("addr_alias").map_err(db)?;
        let desired=expected.building_id.as_deref().unwrap_or("0");
        if row.try_get::<String,_>("name").map_err(db)?!=expected.name || row.try_get::<String,_>("ip").map_err(db)?!=expected.ip
            || location!=expected.addr_alias.as_deref().unwrap_or("")
            || expected.platform_aio_id.as_ref().is_some_and(|original| original!=&id)
            || (space!=desired && space!="0") {
            return Err(AppError::Conflict("平台登记资料与本次部署不一致，请在详情中核对；未覆盖平台已有资料".into()));
        }
        if desired!="0" {
            let present:Option<String>=sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM t_project_building WHERE id=? AND delete_flag='0' FOR UPDATE").bind(desired).fetch_optional(&mut *tx).await.map_err(db)?;
            let directory=project_spaces::read(&mut *tx,None).await?;
            if present.is_none() || space_path(&directory,desired).is_none(){return Err(AppError::Conflict("部署所选空间已失效，请核对平台资料".into()));}
        }
        if space!=desired {
            sqlx::query("UPDATE op_edge_aio_server SET building_id=? WHERE id=?").bind(desired).bind(&id).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(|_|AppError::Conflict("平台注册资料保存结果未确认，请刷新列表核对".into()))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_location_is_not_replaced_with_a_space_path() {
        let input=AioRegistrationPayload{name:"node".into(),ip:"192.0.2.1".into(),mac:"001122334455".into(),platform_ip:"192.0.2.2".into(),platform_port:"8055".into(),auth_key:"test".into(),building_id:Some(103),addr_alias:None};
        assert_eq!(compatible_payload(&input).building_id,Some(0));
        assert_eq!(input.building_id,Some(103)); assert!(compatible_payload(&input).addr_alias.is_none());
        let filled=AioRegistrationPayload{addr_alias:Some("门口设备柜".into()),..input};
        assert_eq!(compatible_payload(&filled).building_id,Some(103));
        assert_eq!(compatible_payload(&filled).addr_alias.as_deref(),Some("门口设备柜"));
    }
}
