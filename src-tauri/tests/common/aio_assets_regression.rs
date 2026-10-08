//! 两台已授权测试机的真实部署验收。通过正式资料服务准备本机清单，不向共享库伪造待部署资产。
use super::*;
use inxaiot_desk_buddy_lib::application::aio_assets::{ListAioNodesQuery, UpdateAioNodeInput};
use inxaiot_desk_buddy_lib::domain::aio::inventory::InventoryValues;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::aio_node_repository::LocalAioRepository;
use inxaiot_desk_buddy_lib::infrastructure::{
    aio_assets_service as assets, aio_edit, project_spaces,
};

pub async fn prepare(
    state: &FormalAppState,
    project: &str,
    pools: &DualMySqlPools,
    config: &TestConfig,
) -> Result<Vec<InventoryValues>, Box<dyn std::error::Error>> {
    let spaces = project_spaces::read(&pools.platform, None).await?;
    let paths = inxaiot_desk_buddy_lib::domain::common::project_space::space_paths(&spaces);
    let choices = spaces
        .iter()
        .filter(|s| s.kind == "floor")
        .filter_map(|s| paths.get(&s.id).map(|p| (s.id.clone(), p.clone())))
        .filter(|(_, path)| {
            !path.contains([',', '"', '\n']) && paths.values().filter(|p| *p == path).count() == 1
        })
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(choices.len(), 2, "需要两个可唯一识别的业务空间");
    let mut expected = Vec::new();
    for (index, node) in real_nodes().iter().enumerate() {
        let value = InventoryValues {
            name: format!("空间回归-{}", node.ip.rsplit('.').next().unwrap()),
            ip: node.ip.clone(),
            mac: node.mac_normalized.clone(),
            building_id: Some(choices[index].0.clone()),
            addr_alias: if index == 0 {
                Some("机房左侧机柜".into())
            } else {
                None
            },
            ..Default::default()
        };
        let preview = if index == 0 {
            assets::preview_aio_node_create(state, project, value.clone()).await?
        } else {
            let path = state.paths.data_dir.join("空间位置实机导入.csv");
            std::fs::write(
                &path,
                format!(
                    "名称,IP,MAC,空间路径,位置\n{},{},{},{},\n",
                    value.name, value.ip, value.mac, choices[index].1
                ),
            )?;
            assets::preview_inventory_import(state, project, &path).await?
        };
        assert_eq!(preview.session.items.len(), 1);
        assert_eq!(
            preview.session.items[0].values.building_id,
            value.building_id
        );
        assert!(preview.session.items[0].errors.is_empty());
        assets::apply_inventory_import(state, project, &preview.session.id).await?;
        expected.push(value);
    }
    // 未部署时编辑名称、IP、空间、位置，再改回真实 IP；全程只能写本机。
    let mut edited = expected[0].clone();
    edited.name = "新增一体机-本机已编辑".into();
    edited.ip = "192.0.2.79".into();
    edited.building_id = Some(choices[1].0.clone());
    edited.addr_alias = Some("本机编辑后的具体位置".into());
    for value in [edited.clone(), {
        edited.ip = expected[0].ip.clone();
        edited.clone()
    }] {
        let before = assets::get_aio_node_detail(state, project, &value.mac).await?;
        assert!(before.platform.is_none());
        aio_edit::update(
            state,
            project,
            UpdateAioNodeInput {
                mac: value.mac.clone(),
                expected_version: before.node.version,
                platform_base: None,
                values: value.clone(),
                force_takeover: false,
            },
        )
        .await?;
        let actual = assets::get_aio_node_detail(state, project, &value.mac).await?;
        assert_eq!(actual.node.name, value.name);
        assert_eq!(actual.node.ip, value.ip);
        assert_eq!(actual.node.building_id, value.building_id);
        assert_eq!(
            actual.node.location,
            value.addr_alias.clone().unwrap_or_default()
        );
    }
    expected[0] = edited;
    let local = LocalAioRepository::new(state.local_store.pool().clone())
        .list(project)
        .await?;
    assert_eq!(local.len(), 2);
    let registered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM op_edge_aio_server WHERE UPPER(REPLACE(REPLACE(mac,':',''),'-','')) IN ('000C293BB933','000C290B71F4')")
        .fetch_one(&pools.platform).await?;
    assert_eq!(registered, 0, "部署前不能登记平台");
    let shared: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM aio_node")
        .fetch_one(&pools.workbench)
        .await?;
    assert_eq!(shared, 0, "待实施资产不能保存到共享库");
    // 第二份真实本机数据目录连接同一业务库，验证看不到第一台电脑的待实施清单。
    let other = state_at(&state.paths.data_dir.join("other-client")).await;
    let adapter = Stage75Adapter::new(&other);
    let other_project = adapter
        .create_project(project_input(config, "实机回归第二客户端"))
        .await?
        .project
        .id;
    adapter.switch_project(&other_project).await?;
    adapter
        .login_project(&other_project, login_request(config))
        .await?;
    let page = assets::list_aio_nodes(
        &other,
        &other_project,
        ListAioNodesQuery {
            search: None,
            state: None,
            page: 1,
            page_size: 100,
        },
    )
    .await?;
    assert!(
        !page
            .items
            .iter()
            .any(|n| expected.iter().any(|v| v.mac == n.mac_normalized))
    );
    other.task_queue.shutdown(Duration::from_secs(5)).await;
    other.runtime_registry.close_all().await;
    other.local_store.close().await;
    println!("AIO_ASSETS_REAL 本机新增、CSV空间路径导入、本机四字段编辑、其他客户端隔离：通过");
    Ok(expected)
}

pub async fn verify_registered(
    state: &FormalAppState,
    project: &str,
    pools: &DualMySqlPools,
    expected: &[InventoryValues],
) -> AppResult<()> {
    for value in expected {
        let detail = assets::get_aio_node_detail(state, project, &value.mac).await?;
        let platform = detail.platform.expect("部署后已注册平台");
        assert_eq!(platform.name, value.name);
        assert_eq!(platform.ip, value.ip);
        assert_eq!(platform.building_id, value.building_id);
        assert_eq!(
            platform.addr_alias.unwrap_or_default(),
            value.addr_alias.clone().unwrap_or_default()
        );
        assert_eq!(
            detail.node.location,
            value.addr_alias.clone().unwrap_or_default()
        );
        assert!(!detail.node.space_path.is_empty());
    }
    let page = assets::list_aio_nodes(
        state,
        project,
        ListAioNodesQuery {
            search: None,
            state: None,
            page: 1,
            page_size: 100,
        },
    )
    .await?;
    for value in expected {
        assert_eq!(
            page.items
                .iter()
                .filter(|n| n.mac_normalized == value.mac)
                .count(),
            1
        );
    }
    assert!(
        LocalAioRepository::new(state.local_store.pool().clone())
            .list(project)
            .await?
            .is_empty()
    );
    let shared: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM aio_node WHERE platform_aio_id IS NOT NULL")
            .fetch_one(&pools.workbench)
            .await
            .map_err(|e| {
                inxaiot_desk_buddy_lib::core::error::AppError::database("核对部署后关联", &e)
            })?;
    assert_eq!(shared, 2);
    println!(
        "AIO_ASSETS_REAL 平台空间ID、具体位置（含留空）、本机待实施清单退场、共享部署关联：通过"
    );
    Ok(())
}

pub async fn edit_registered(
    state: &FormalAppState,
    project: &str,
    pools: &DualMySqlPools,
    expected: &mut [InventoryValues],
) -> Result<(), Box<dyn std::error::Error>> {
    let spaces = project_spaces::read(&pools.platform, None).await?;
    let paths = inxaiot_desk_buddy_lib::domain::common::project_space::space_paths(&spaces);
    let next_space = spaces
        .iter()
        .find(|s| {
            s.kind == "floor"
                && paths.contains_key(&s.id)
                && Some(&s.id) != expected[0].building_id.as_ref()
        })
        .expect("另一业务空间")
        .id
        .clone();
    for value in expected.iter_mut() {
        let detail = assets::get_aio_node_detail(state, project, &value.mac).await?;
        let id = detail.platform.as_ref().unwrap().id.clone();
        let protected: String = sqlx::query_scalar("SELECT SHA2(CONCAT_WS('|',mac,point_x,point_y,account,password,platform_ip,platform_port),256) FROM op_edge_aio_server WHERE id=?").bind(&id).fetch_one(&pools.platform).await?;
        let real_ip = value.ip.clone();
        value.ip = if real_ip.ends_with(".79") {
            "192.0.2.79"
        } else {
            "192.0.2.121"
        }
        .into();
        value.name = format!("aio-test-{}", real_ip.rsplit('.').next().unwrap());
        value.building_id = Some(next_space.clone());
        value.addr_alias = Some(format!(
            "空间回归-{}号测试机",
            real_ip.rsplit('.').next().unwrap()
        ));
        for editing in [value.clone(), {
            value.ip = real_ip;
            value.clone()
        }] {
            let before = assets::get_aio_node_detail(state, project, &value.mac).await?;
            aio_edit::update(
                state,
                project,
                UpdateAioNodeInput {
                    mac: value.mac.clone(),
                    expected_version: before.node.version,
                    platform_base: before.platform,
                    values: editing.clone(),
                    force_takeover: false,
                },
            )
            .await?;
            let after = assets::get_aio_node_detail(state, project, &value.mac).await?;
            assert_eq!(after.node.ip, editing.ip);
            assert_eq!(after.node.name, editing.name);
            assert_eq!(after.node.building_id, editing.building_id);
            assert_eq!(after.node.location, editing.addr_alias.unwrap());
        }
        let after: String = sqlx::query_scalar("SELECT SHA2(CONCAT_WS('|',mac,point_x,point_y,account,password,platform_ip,platform_port),256) FROM op_edge_aio_server WHERE id=?").bind(&id).fetch_one(&pools.platform).await?;
        assert_eq!(protected, after, "编辑不能改写MAC、布点坐标和连接凭据");
    }
    verify_registered(state, project, pools, expected).await?;
    println!("AIO_ASSETS_REAL 已注册名称、IP、空间、位置直接保存平台；MAC、布点和凭据保持：通过");
    Ok(())
}

pub async fn snapshot(config: &TestConfig, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(output) = std::env::var_os("INX_AIO_REAL_EVIDENCE_PATH") else {
        return Ok(());
    };
    let source = include_str!("aio_runtime_inventory.py");
    for node in real_nodes() {
        let session = RusshConnector::default()
            .connect(
                &RemoteTarget {
                    host: node.ip.clone(),
                    port: 22,
                    connect_timeout: Duration::from_secs(15),
                },
                &RemoteAuth::PrivateKey {
                    username: line(&config.description, "一体机ssh用户：").into(),
                    private_key: SecretValue::new(read_private_key(config)?),
                    passphrase: None,
                },
                HostKeyPolicy::Capture,
            )
            .await?;
        let result = session
            .run(
                &ExecRequest {
                    program: "python3".into(),
                    args: vec!["-".into()],
                    env: BTreeMap::new(),
                    stdin: Some(source.as_bytes().to_vec()),
                    total_timeout: Duration::from_secs(30),
                    inactivity_timeout: Duration::from_secs(15),
                },
                &tokio_util::sync::CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await?;
        assert_eq!(result.exit_status, 0);
        let _: Value = serde_json::from_str(&result.stdout)?;
        std::fs::write(
            Path::new(&output).join(format!(
                "{label}-{}.json",
                node.ip.rsplit('.').next().unwrap()
            )),
            result.stdout,
        )?;
        session.disconnect().await?;
    }
    Ok(())
}
