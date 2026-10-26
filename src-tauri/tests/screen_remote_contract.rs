#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::{AppError, AppResult},
    domain::smart_screen::model::ScreenFields,
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::remote_screen::{self, RemoteScreenSessions, ScreenCapturePort},
        stage75_adapter::Stage75Adapter,
    },
};
use std::{
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(
        &mut output,
        b"IHDR",
        &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
    );
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&[0, 255, 128, 0, 255]).unwrap();
    chunk(&mut output, b"IDAT", &z.finish().unwrap());
    chunk(&mut output, b"IEND", &[]);
    output
}
struct Capture {
    calls: AtomicUsize,
    started: Notify,
    release: Notify,
    wait: bool,
    corrupt: bool,
}
impl Capture {
    fn new(wait: bool, corrupt: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            wait,
            corrupt,
        }
    }
}
impl ScreenCapturePort for Capture {
    async fn capture(&self, ip: &str, cancel: CancellationToken) -> AppResult<Vec<u8>> {
        assert_eq!(ip, "192.0.2.42");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if self.wait {
            tokio::select! { _ = cancel.cancelled() => return Err(AppError::Cancelled), _ = self.release.notified() => {} }
        }
        Ok(if self.corrupt {
            b"not an image; must never reach a log".to_vec()
        } else {
            png()
        })
    }
}
async fn fixture() -> (tempfile::TempDir, Arc<FormalAppState>, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let screen = ScreenRepository::new(state.local_store.pool().clone())
        .save_local(
            &project,
            &ScreenFields {
                name: "远程查看隔离目标".into(),
                ip: "192.0.2.42".into(),
                size: "10".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    (temp, state, project, screen)
}

#[tokio::test]
async fn screenshot_does_not_create_task_log_image_or_mutate_screen() {
    let (_temp, state, project, screen) = fixture().await;
    let sessions = RemoteScreenSessions::default();
    let port = Capture::new(false, false);
    let before = ScreenRepository::new(state.local_store.pool().clone())
        .snapshot(&project)
        .await
        .unwrap()
        .screens;
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let frame =
        remote_screen::capture_with(&state, &sessions, "main", &project, &view.session_id, &port)
            .await
            .unwrap();
    assert_eq!((frame.width, frame.height), (1, 1));
    assert!(!frame.png_base64.is_empty());
    assert!(frame.captured_at.ends_with('Z'));
    assert!(!frame.input_ready, "仅查看的截图不能授权输入");
    assert_eq!(
        before,
        ScreenRepository::new(state.local_store.pool().clone())
            .snapshot(&project)
            .await
            .unwrap()
            .screens
    );
    for table in [
        "local_task",
        "local_screen_task_data",
        "local_screen_observation",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(state.local_store.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    assert_eq!(
        std::fs::read_dir(&state.paths.task_artifacts_dir)
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(&state.paths.task_logs_dir)
            .unwrap()
            .count(),
        0
    );
    remote_screen::close(&sessions, "main", &project, &view.session_id).unwrap();
    assert!(remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        &project,
        &view.session_id,
        &port
    )
    .await
    .is_err());
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    support::close(state).await;
}

#[tokio::test]
async fn wrong_project_window_or_changed_confirmed_ip_cannot_capture() {
    let (_temp, state, project, screen) = fixture().await;
    let sessions = RemoteScreenSessions::default();
    let port = Capture::new(false, false);
    assert!(
        remote_screen::open(&state, &sessions, "main", "other", &screen)
            .await
            .is_err()
    );
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    assert!(remote_screen::capture_with(
        &state,
        &sessions,
        "other",
        &project,
        &view.session_id,
        &port
    )
    .await
    .is_err());
    assert!(remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        "other",
        &view.session_id,
        &port
    )
    .await
    .is_err());
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let item = repo.snapshot(&project).await.unwrap().screens.remove(0);
    let mut fields = item.fields;
    fields.ip = "192.0.2.43".into();
    repo.save_local(&project, &fields, Some(&screen), Some(item.revision))
        .await
        .unwrap();
    assert!(remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        &project,
        &view.session_id,
        &port
    )
    .await
    .is_err());
    assert_eq!(port.calls.load(Ordering::SeqCst), 0);
    support::close(state).await;
}

#[tokio::test]
async fn concurrent_refresh_is_rejected_and_close_cancels_the_inflight_frame() {
    let (_temp, state, project, screen) = fixture().await;
    let sessions = Arc::new(RemoteScreenSessions::default());
    let port = Arc::new(Capture::new(true, false));
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let capturing = {
        let (state, sessions, port, project, id) = (
            state.clone(),
            sessions.clone(),
            port.clone(),
            project.clone(),
            view.session_id.clone(),
        );
        tokio::spawn(async move {
            remote_screen::capture_with(&state, &sessions, "main", &project, &id, &*port).await
        })
    };
    port.started.notified().await;
    assert!(remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        &project,
        &view.session_id,
        &*port
    )
    .await
    .is_err());
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    remote_screen::close(&sessions, "main", &project, &view.session_id).unwrap();
    assert!(matches!(capturing.await.unwrap(), Err(AppError::Cancelled)));
    support::close(state).await;
}

#[tokio::test]
async fn reopening_window_rejects_old_capture_and_keeps_new_frame_read_only() {
    let (_temp, state, project, screen) = fixture().await;
    let sessions = Arc::new(RemoteScreenSessions::default());
    let slow = Arc::new(Capture::new(true, false));
    let old = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let capturing = {
        let (state, sessions, project, id, slow) = (
            state.clone(),
            sessions.clone(),
            project.clone(),
            old.session_id.clone(),
            slow.clone(),
        );
        tokio::spawn(async move {
            remote_screen::capture_with(&state, &sessions, "main", &project, &id, &*slow).await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), slow.started.notified())
        .await
        .unwrap();
    remote_screen::close(&sessions, "main", &project, &old.session_id).unwrap();
    let reopened = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    assert_ne!(old.session_id, reopened.session_id);
    slow.release.notify_one();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), capturing)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let port = Capture::new(false, false);
    let fresh = remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        &project,
        &reopened.session_id,
        &port,
    )
    .await
    .unwrap();
    assert_eq!(fresh.session_id, reopened.session_id);
    assert!(!fresh.input_ready);
    assert!(!fresh.control_active);
    remote_screen::close(&sessions, "main", &project, &reopened.session_id).unwrap();
    support::close(state).await;
}

#[tokio::test]
async fn identity_changed_during_capture_and_invalid_png_never_return_a_frame() {
    let (_temp, state, project, screen) = fixture().await;
    let sessions = Arc::new(RemoteScreenSessions::default());
    let port = Arc::new(Capture::new(true, false));
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let capturing = {
        let (state, sessions, port, project, id) = (
            state.clone(),
            sessions.clone(),
            port.clone(),
            project.clone(),
            view.session_id.clone(),
        );
        tokio::spawn(async move {
            remote_screen::capture_with(&state, &sessions, "main", &project, &id, &*port).await
        })
    };
    port.started.notified().await;
    sqlx::query("UPDATE local_project SET name='改名不改变地址',platform_url='http://changed.invalid' WHERE id=?").bind(&project).execute(state.local_store.pool()).await.unwrap();
    port.release.notify_one();
    assert!(capturing.await.unwrap().is_err());
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let invalid = Capture::new(false, true);
    let error = remote_screen::capture_with(
        &state,
        &sessions,
        "main",
        &project,
        &view.session_id,
        &invalid,
    )
    .await
    .err()
    .unwrap();
    assert!(!error.to_string().contains("must never"));
    support::close(state).await;
}

#[tokio::test]
async fn high_frequency_capture_does_not_read_unrelated_history_drafts_or_platform_assets() {
    let (_temp, state, project, screen) = fixture().await;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    repo.set_scope(&project, "business", "source")
        .await
        .unwrap();
    // JSON本身合法，但不是对应业务结构。全项目snapshot会解析失败，单屏确认查询不能依赖它们。
    sqlx::query("INSERT INTO local_screen_observation(id,local_project_id,screen_id,operation_type,observed_ip,observed_at,result_json) VALUES('unrelated-observation',?,'other-screen','inspect','192.0.2.99','1','{}')")
        .bind(&project).execute(state.local_store.pool()).await.unwrap();
    sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES(?,'business','other-platform','{}','test','1')")
        .bind(&project).execute(state.local_store.pool()).await.unwrap();
    sqlx::query("INSERT INTO local_screen_draft(local_project_id,business_project_id,platform_screen_id,base_json,values_json,base_revision,updated_at) VALUES(?,'business','other-platform','{}','{}',1,'1')")
        .bind(&project).execute(state.local_store.pool()).await.unwrap();
    assert!(repo.snapshot(&project).await.is_err());
    let sessions = RemoteScreenSessions::default();
    let view = remote_screen::open(&state, &sessions, "main", &project, &screen)
        .await
        .unwrap();
    let port = Capture::new(false, false);
    let frame =
        remote_screen::capture_with(&state, &sessions, "main", &project, &view.session_id, &port)
            .await
            .unwrap();
    assert_eq!((frame.width, frame.height), (1, 1));
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    support::close(state).await;
}

#[tokio::test]
async fn narrow_confirmed_asset_matches_visible_local_and_platform_identity_and_aliases() {
    use inxaiot_desk_buddy_lib::domain::smart_screen::model::ScreenAsset;
    let (_temp, state, project, screen) = fixture().await;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let original = repo.asset(&project, &screen).await.unwrap();
    assert_eq!(
        repo.confirmed_asset(&project, &screen).await.unwrap(),
        original
    );
    repo.set_scope(&project, "business", "source")
        .await
        .unwrap();
    let platform = ScreenAsset {
        id: "platform-target".into(),
        source: "platform".into(),
        fields: original.fields.clone(),
        platform_status: "online".into(),
        ..Default::default()
    };
    repo.replace_platform_cache(&project, "business", &[platform], &[])
        .await
        .unwrap();
    sqlx::query("INSERT INTO local_screen_binding(local_project_id,local_screen_id,business_project_id,platform_screen_id,request_id,evidence_json,created_at) VALUES(?,?,'business','platform-target','test-confirmed-alias','{}','1')")
        .bind(&project).bind(&screen).execute(state.local_store.pool()).await.unwrap();
    let expected = repo.asset(&project, "platform-target").await.unwrap();
    assert_eq!(
        repo.confirmed_asset(&project, "platform-target")
            .await
            .unwrap(),
        expected
    );
    assert_eq!(expected.aliases, vec![screen.clone()]);
    assert!(
        repo.confirmed_asset(&project, &screen).await.is_err(),
        "已绑定旧本机ID不能重新作为本机屏返回"
    );
    assert!(repo
        .confirmed_asset("other-project", "platform-target")
        .await
        .is_err());
    sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES(?,'business','unrelated-bad-cache','{}','test','1')")
        .bind(&project).execute(state.local_store.pool()).await.unwrap();
    assert!(repo.snapshot(&project).await.is_err());
    assert_eq!(
        repo.confirmed_asset(&project, "platform-target")
            .await
            .unwrap(),
        expected
    );
    support::close(state).await;
}
