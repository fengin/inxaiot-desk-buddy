use super::*;
use std::io::Write;

fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    output.extend_from_slice(&crc.sum().to_be_bytes());
}
fn picture(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut bytes, b"IHDR", &header);
    let mut compressed =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressed.write_all(&[0, 0, 0, 0, 255]).unwrap();
    chunk(&mut bytes, b"IDAT", &compressed.finish().unwrap());
    chunk(&mut bytes, b"IEND", &[]);
    bytes
}
fn target(ip: &str) -> Binding {
    Binding {
        screen: ScreenAsset {
            id: "screen".into(),
            source: "local".into(),
            fields: crate::domain::smart_screen::model::ScreenFields {
                ip: ip.into(),
                size: "4".into(),
                name: "测试屏".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        connection_revision: "same-project".into(),
        scope: None,
    }
}

fn frame_test_session() -> Arc<Session> {
    let sessions = RemoteScreenSessions::default();
    let ticket = sessions.begin_open("frame-tests").unwrap();
    let view = sessions
        .finish_open("frame-tests", &ticket, "project", target("192.0.2.1"))
        .unwrap();
    sessions
        .session("frame-tests", "project", &view.session_id)
        .unwrap()
}
fn frame_stamp(id: &str, generation: u64, captured: Instant) -> FrameStamp {
    FrameStamp {
        id: id.into(),
        width: 480,
        height: 480,
        captured,
        control_task: Some("task".into()),
        serial: Some("fixture".into()),
        generation,
    }
}

#[tokio::test]
async fn input_uses_a_recent_displayed_frame_while_capture_remains_single_flight() {
    let session = frame_test_session();
    {
        let mut frames = session.frame_history().unwrap();
        frames.retain(frame_stamp("displayed", 0, Instant::now()));
        frames.retain(frame_stamp("new-background", 0, Instant::now()));
    }
    let capture = session.capture.try_lock().unwrap();
    let action = session
        .begin_frame_action("displayed", "task", &ControlAction::Tap { x: 3, y: 4 })
        .unwrap();
    assert_eq!(action.stamp.id, "displayed");
    assert!(session.capture.try_lock().is_err(), "截图自身仍必须单飞");
    assert!(session.frame_history().unwrap().input_in_flight);
    assert!(
        session
            .begin_frame_action("new-background", "task", &ControlAction::Tap { x: 0, y: 0 })
            .is_err()
    );
    drop(action);
    assert_eq!(session.input_generation(), 2);
    assert!(!session.frame_history().unwrap().input_in_flight);
    assert!(
        session
            .begin_frame_action("displayed", "task", &ControlAction::Tap { x: 0, y: 0 })
            .is_err()
    );
    drop(capture);
}

#[test]
fn captures_crossing_or_started_during_input_never_become_input_frames() {
    let session = frame_test_session();
    session
        .frame_history()
        .unwrap()
        .retain(frame_stamp("before", 0, Instant::now()));
    let old_capture_generation = session.input_generation();
    let action = session
        .begin_frame_action(
            "before",
            "task",
            &ControlAction::Key {
                key: ControlKey::Back,
            },
        )
        .unwrap();
    let during_capture_generation = session.input_generation();
    assert!(
        !session
            .frame_history()
            .unwrap()
            .accepts_capture(during_capture_generation, true)
    );
    drop(action);
    let frames = session.frame_history().unwrap();
    assert!(!frames.accepts_capture(old_capture_generation, false));
    assert!(!frames.accepts_capture(during_capture_generation, true));
    assert!(frames.accepts_capture(frames.generation, false));
}

#[test]
fn frame_history_is_bounded_and_uses_capture_start_age_without_renewing_old_frames() {
    let session = frame_test_session();
    let too_old = Instant::now() - input::FRAME_MAX_AGE - Duration::from_millis(1);
    session
        .frame_history()
        .unwrap()
        .stamps
        .push_back(frame_stamp("expired", 0, too_old));
    assert!(
        session
            .begin_frame_action(
                "expired",
                "task",
                &ControlAction::Key {
                    key: ControlKey::Back
                }
            )
            .is_err()
    );
    {
        let mut frames = session.frame_history().unwrap();
        for index in 0..MAX_FRAME_STAMPS + 3 {
            frames.retain(frame_stamp(&format!("frame-{index}"), 0, Instant::now()));
        }
        assert_eq!(frames.stamps.len(), MAX_FRAME_STAMPS);
        assert_eq!(frames.stamps.front().unwrap().id, "frame-3");
    }
    assert!(
        session
            .begin_frame_action(
                "frame-0",
                "task",
                &ControlAction::Key {
                    key: ControlKey::Back
                }
            )
            .is_err()
    );
    assert!(
        session
            .begin_frame_action(
                "frame-3",
                "other-task",
                &ControlAction::Key {
                    key: ControlKey::Back
                }
            )
            .is_err()
    );
    let accepted = session
        .begin_frame_action(
            "frame-3",
            "task",
            &ControlAction::Key {
                key: ControlKey::Back,
            },
        )
        .unwrap();
    drop(accepted);
}

#[test]
fn rejected_frame_or_coordinates_do_not_start_an_input_epoch() {
    let session = frame_test_session();
    session
        .frame_history()
        .unwrap()
        .retain(frame_stamp("good", 0, Instant::now()));
    assert!(
        session
            .begin_frame_action("good", "task", &ControlAction::Tap { x: 480, y: 0 })
            .is_err()
    );
    assert!(
        session
            .begin_frame_action(
                "missing",
                "task",
                &ControlAction::Key {
                    key: ControlKey::Back
                }
            )
            .is_err()
    );
    assert_eq!(session.input_generation(), 0);
    assert!(!session.frame_history().unwrap().input_in_flight);
    assert_eq!(session.frame_history().unwrap().stamps.len(), 1);
}

#[test]
fn png_preserves_binary_and_rejects_truncation_corruption_missing_end_and_oversize() {
    let bytes = picture(1, 1);
    assert_eq!(png::dimensions(&bytes).unwrap(), (1, 1));
    for length in [0, 8, 32, bytes.len() - 1, bytes.len() - 12] {
        assert!(png::dimensions(&bytes[..length]).is_err());
    }
    let mut corrupt = bytes.clone();
    corrupt[29] ^= 1;
    assert!(png::dimensions(&corrupt).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(png::dimensions(&trailing).is_err());
    assert!(png::dimensions(&picture(0, 1)).is_err());
    assert!(png::dimensions(&picture(4097, 1)).is_err());
    assert!(png::dimensions(&picture(4096, 4096)).is_err());
    let mut huge = bytes;
    huge.resize(png::MAX_PNG_BYTES + 1, 0);
    assert!(png::dimensions(&huge).is_err());
}

#[test]
fn late_open_and_close_never_replace_or_cancel_the_new_window_session() {
    let sessions = RemoteScreenSessions::default();
    let old = sessions.begin_open("main").unwrap();
    let new = sessions.begin_open("main").unwrap();
    let current = sessions
        .finish_open("main", &new, "project", target("192.0.2.1"))
        .unwrap();
    assert!(matches!(
        sessions.finish_open("main", &old, "project", target("192.0.2.2")),
        Err(AppError::Cancelled)
    ));
    close(&sessions, "main", "project", "old-session").unwrap();
    close(&sessions, "other-window", "project", &current.session_id).unwrap();
    close(&sessions, "main", "other-project", &current.session_id).unwrap();
    assert!(
        sessions
            .session("main", "project", &current.session_id)
            .is_ok()
    );
    sessions.close_owner("main");
    assert!(
        sessions
            .session("main", "project", &current.session_id)
            .is_err()
    );
}

#[test]
fn app_exit_cancels_sessions_and_invalidates_incomplete_opens() {
    let sessions = RemoteScreenSessions::default();
    let first = sessions.begin_open("main").unwrap();
    let view = sessions
        .finish_open("main", &first, "project", target("192.0.2.1"))
        .unwrap();
    let current = sessions
        .session("main", "project", &view.session_id)
        .unwrap();
    let opening = sessions.begin_open("other-window").unwrap();
    sessions.close_all();
    assert!(current.cancel.is_cancelled());
    assert!(
        sessions
            .session("main", "project", &view.session_id)
            .is_err()
    );
    assert!(matches!(
        sessions.finish_open("other-window", &opening, "project", target("192.0.2.2")),
        Err(AppError::Cancelled)
    ));
    let next = sessions.begin_open("other-window").unwrap();
    assert!(
        sessions
            .finish_open("other-window", &next, "project", target("192.0.2.2"))
            .is_ok()
    );
}

#[tokio::test]
async fn capture_is_single_flight_and_replacing_or_closing_cancels_its_token() {
    let sessions = RemoteScreenSessions::default();
    let ticket = sessions.begin_open("main").unwrap();
    let view = sessions
        .finish_open("main", &ticket, "project", target("192.0.2.1"))
        .unwrap();
    let session = sessions
        .session("main", "project", &view.session_id)
        .unwrap();
    let held = session.capture.try_lock().unwrap();
    assert!(session.capture.try_lock().is_err());
    let next_ticket = sessions.begin_open("main").unwrap();
    assert!(session.cancel.is_cancelled());
    drop(held);
    let next = sessions
        .finish_open("main", &next_ticket, "project", target("192.0.2.2"))
        .unwrap();
    close(&sessions, "main", "project", &view.session_id).unwrap();
    let next_session = sessions
        .session("main", "project", &next.session_id)
        .unwrap();
    assert!(!next_session.cancel.is_cancelled());
    close(&sessions, "main", "project", &next.session_id).unwrap();
    assert!(next_session.cancel.is_cancelled());
}

#[test]
fn current_binding_uses_confirmed_identity_and_project_connection() {
    let original = target("192.0.2.1");
    let mut changed = original.clone();
    changed.screen.fields.name = "仅改名".into();
    assert!(same_binding(&original, &changed));
    changed.screen.fields.ip = "192.0.2.2".into();
    assert!(!same_binding(&original, &changed));
    changed = original.clone();
    changed.connection_revision = "other".into();
    assert!(!same_binding(&original, &changed));
    changed = original.clone();
    changed.scope = Some(("other-platform".into(), "source".into()));
    assert!(!same_binding(&original, &changed));
}

fn child_arguments(name: &str) -> Vec<String> {
    let path = module_path!().split_once("::").unwrap().1;
    vec![
        "--exact".into(),
        format!("{path}::{name}"),
        "--ignored".into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ]
}

#[tokio::test]
async fn binary_process_preserves_non_utf8_and_rejects_excess_stdout_or_stderr() {
    let executable = std::env::current_exe().unwrap();
    let output = transport::run_bytes(
        &executable,
        &child_arguments("process_fixture_binary"),
        Duration::from_secs(5),
        CancellationToken::new(),
        65536,
        65536,
    )
    .await
    .unwrap();
    assert!(output.windows(4).any(|bytes| bytes == [0, 255, 128, 0]));
    let error = transport::run_bytes(
        &executable,
        &child_arguments("process_fixture_large"),
        Duration::from_secs(5),
        CancellationToken::new(),
        1024,
        1024,
    )
    .await;
    assert!(matches!(error, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn binary_process_timeout_and_cancel_stop_the_child_without_replaying() {
    let executable = std::env::current_exe().unwrap();
    let started = Instant::now();
    let result = transport::run_bytes(
        &executable,
        &child_arguments("process_fixture_slow"),
        Duration::from_millis(150),
        CancellationToken::new(),
        65536,
        65536,
    )
    .await;
    assert!(matches!(result, Err(AppError::Timeout { .. })));
    assert!(started.elapsed() < Duration::from_secs(4));
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let cancelling = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
    });
    let result = transport::run_bytes(
        &executable,
        &child_arguments("process_fixture_slow"),
        Duration::from_secs(5),
        cancel,
        65536,
        65536,
    )
    .await;
    cancelling.await.unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)));
}

#[test]
#[ignore = "由二进制采集回归以隔离子进程执行"]
fn process_fixture_binary() {
    std::io::stdout().write_all(&[0, 255, 128, 0]).unwrap();
}

#[test]
#[ignore = "由有界输出回归以隔离子进程执行"]
fn process_fixture_large() {
    let _ = std::io::stderr().write_all(&vec![b'e'; 128 * 1024]);
    let _ = std::io::stdout().write_all(&vec![b'o'; 128 * 1024]);
}

#[test]
#[ignore = "由超时取消回归以隔离子进程执行"]
fn process_fixture_slow() {
    std::thread::sleep(Duration::from_secs(5));
}
