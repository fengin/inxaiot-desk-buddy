use super::*;

fn decoded_arguments(script: &str) -> String {
    let mut result = String::new();
    assert!(script.starts_with("set -e\n") && script.ends_with("exit\n"));
    for line in script.lines().skip(1).filter(|line| *line != "exit") {
        let value = line
            .strip_prefix("input text ")
            .expect("only a fixed input command");
        let bytes = value.as_bytes();
        let mut pos = 0;
        let mut argument = String::new();
        while pos < bytes.len() {
            assert_eq!(bytes[pos], b'\'', "unquoted shell syntax is forbidden");
            pos += 1;
            while pos < bytes.len() && bytes[pos] != b'\'' {
                argument.push(bytes[pos] as char);
                pos += 1;
            }
            assert!(pos < bytes.len());
            pos += 1;
            if pos < bytes.len() {
                assert_eq!(&bytes[pos..pos + 2], b"\\'");
                argument.push('\'');
                pos += 2;
            }
        }
        // 与Android input对同一参数的%s处理规则一致，各命令输出依次追加。
        result.push_str(&argument.replace("%s", " "));
    }
    result
}

#[test]
fn text_encoder_preserves_spaces_quotes_percent_pairs_and_shell_metacharacters() {
    let printable: String = (0x20u8..=0x7e).map(char::from).collect();
    for value in [
        "user@example.test".to_string(),
        "space  in  pass".to_string(),
        "a'%s\"$(`id`);&|<>\\%ss%s%".to_string(),
        printable,
        "%s".repeat(64),
    ] {
        let action = ControlAction::Text {
            text: value.clone(),
        };
        let script = encode_action(&action).unwrap();
        assert_eq!(decoded_arguments(&script), value);
    }
}

#[test]
fn unsupported_text_is_rejected_before_a_script_exists_without_echoing_input() {
    for text in [
        "".to_string(),
        "a".repeat(129),
        "中文".to_string(),
        "🙂".to_string(),
        "test\nexit".to_string(),
        "test\rnext".to_string(),
        "test\tvalue".to_string(),
        "test\0data".to_string(),
    ] {
        assert!(encode_action(&ControlAction::Text { text }).is_err());
    }
}

#[test]
fn allowed_keys_and_coordinates_generate_only_fixed_commands() {
    for (key, code) in [
        (ControlKey::Backspace, 67),
        (ControlKey::Back, 4),
        (ControlKey::WakeUp, 224),
    ] {
        assert_eq!(
            &*encode_action(&ControlAction::Key { key }).unwrap(),
            &format!("set -e\ninput keyevent {code}\nexit\n")
        );
    }
    assert_eq!(
        &*encode_action(&ControlAction::Tap { x: 7, y: 9 }).unwrap(),
        "set -e\ninput tap 7 9\nexit\n"
    );
    assert!(encode_action(&ControlAction::Tap { x: 4096, y: 0 }).is_err());
    assert!(
        serde_json::from_str::<ControlAction>(r#"{"kind":"shell","command":"anything"}"#).is_err()
    );
    assert!(serde_json::from_str::<ControlAction>(r#"{"kind":"key","key":"home"}"#).is_err());
    assert!(serde_json::from_str::<ControlAction>(r#"{"kind":"key","key":"enter"}"#).is_err());
}

fn probe(size: &str, orientations: &str, activity: &str) -> String {
    format!(
        "__REMOTE_SDK__\n29\n__REMOTE_SERIAL__\nfixture-123\n__REMOTE_MAC__\n26:da:35:7d:85:b5\n__REMOTE_SIZE__\n{size}\n__REMOTE_INPUT__\n{orientations}\n__REMOTE_POWER__\nmWakefulness=Awake\n__REMOTE_ACTIVITY__\n{activity}\n__REMOTE_WINDOW__\nmCurrentFocus=Window{{abc u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}}\n"
    )
}

#[test]
fn device_probe_accepts_observed_android_8_and_10_rotation_formats() {
    let ten = parse_facts(&probe(
        "Physical size: 800x1280",
        "SurfaceOrientation: 1",
        " mResumedActivity: ActivityRecord{aa u0 chat.xiaoxin.app/.MainActivity t7}",
    ))
    .unwrap();
    assert_eq!((ten.width, ten.height), (1280, 800));
    assert_eq!(ten.orientation, 1);
    assert_eq!(ten.android_sdk, 29);
    assert_eq!(ten.awake, Some(true));
    assert_eq!(ten.foreground_package, "chat.xiaoxin.app");
    assert_eq!(ten.serial.as_deref(), Some("fixture-123"));
    let four=parse_facts(&probe("Physical size: 480x480","SurfaceOrientation: 0","topResumedActivity=ActivityRecord{aa u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity t7}")).unwrap();
    assert_eq!((four.width, four.height), (480, 480));
    assert_eq!(four.orientation, 0);
    let override_size = parse_facts(&probe(
        "Physical size: 800x1280\nOverride size: 400x640",
        "SurfaceOrientation: 3",
        "mResumedActivity: ActivityRecord{aa u0 chat.xiaoxin.app/.MainActivity t7}",
    ))
    .unwrap();
    assert_eq!((override_size.width, override_size.height), (640, 400));
}

#[test]
fn android_sdk_selects_one_explicit_text_behavior_without_fallback() {
    assert_eq!(text_input_behavior(27).unwrap(), "insert");
    assert_eq!(text_input_behavior(28).unwrap(), "insert");
    assert_eq!(text_input_behavior(29).unwrap(), "replace");
    assert_eq!(text_input_behavior(35).unwrap(), "replace");
    for sdk in [0, 23, 101] {
        assert!(text_input_behavior(sdk).is_err());
    }
    let backspace = ControlAction::Key {
        key: ControlKey::Backspace,
    };
    let clear = ControlAction::Key {
        key: ControlKey::ClearText,
    };
    assert!(validate_action_for_sdk(&backspace, 27).is_ok());
    assert!(validate_action_for_sdk(&backspace, 29).is_err());
    assert!(validate_action_for_sdk(&clear, 27).is_err());
    assert!(validate_action_for_sdk(&clear, 29).is_ok());
    assert!(encode_action(&clear).is_err());
}

#[test]
fn ambiguous_display_or_foreground_is_rejected_instead_of_guessing() {
    let activity = "mResumedActivity: ActivityRecord{aa u0 chat.xiaoxin.app/.MainActivity t7}";
    assert!(
        parse_facts(&probe(
            "Physical size: 800x1280",
            "SurfaceOrientation: 0\nSurfaceOrientation: 1",
            activity
        ))
        .is_err()
    );
    assert!(parse_facts(&probe("Physical size: 800x1280", "", activity)).is_err());
    let absent = parse_facts(&probe(
        "Physical size: 800x1280",
        "SurfaceOrientation: 0",
        "nothing resumed",
    ))
    .unwrap();
    assert!(absent.foreground_package.is_empty());
    assert!(initial_control_mode(&absent).is_err());
    let ambiguous = parse_facts(&probe(
        "Physical size: 800x1280",
        "SurfaceOrientation: 0",
        &format!(
            "{activity}\nmResumedActivity: ActivityRecord{{bb u0 another.app/.MainActivity t9}}"
        ),
    ))
    .unwrap();
    assert!(ambiguous.foreground_package.is_empty());
    assert!(initial_control_mode(&ambiguous).is_err());
}

#[test]
fn only_explicit_sleep_allows_wake_only_without_a_foreground_activity() {
    let raw = probe(
        "Physical size: 480x480",
        "SurfaceOrientation: 0",
        "mResumedActivity: null\nResumedActivity: ActivityRecord{old u0 chat.xiaoxin.app/.MainActivity}",
    );
    let asleep = parse_facts(
        &raw.replace("mWakefulness=Awake", "mWakefulness=Asleep")
            .replace(
                "mCurrentFocus=Window{abc u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}",
                "mCurrentFocus=null",
            ),
    )
    .unwrap();
    assert_eq!(asleep.serial.as_deref(), Some("fixture-123"));
    assert_eq!(asleep.awake, Some(false));
    assert!(asleep.foreground_package.is_empty());
    assert_eq!(initial_control_mode(&asleep).unwrap(), "wake_only");
    for state in [
        "Dreaming",
        "Dozing",
        "Unknown",
        "Awake\nmWakefulness=Asleep",
    ] {
        let facts =
            parse_facts(&raw.replace("mWakefulness=Awake", &format!("mWakefulness={state}")))
                .unwrap();
        assert_eq!(facts.awake, None);
        assert!(initial_control_mode(&facts).is_err());
    }
}

#[test]
fn full_control_requires_current_activity_and_current_window_not_history_or_overlay() {
    let raw = probe(
        "Physical size: 480x480",
        "SurfaceOrientation: 0",
        "mResumedActivity: ActivityRecord{now u0 chat.xiaoxin.app/.MainActivity}",
    );
    assert_eq!(
        initial_control_mode(&parse_facts(&raw).unwrap()).unwrap(),
        "full"
    );
    for window in [
        "null",
        "Window{sys u0 com.android.systemui/com.android.systemui.Keyguard}",
        "Window{abc u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}\nmCurrentFocus=null",
    ] {
        let facts = parse_facts(&raw.replace(
            "Window{abc u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}",
            window,
        ))
        .unwrap();
        assert!(facts.foreground_package.is_empty());
        assert!(initial_control_mode(&facts).is_err());
    }
    let null_top = raw.replace(
        "mResumedActivity:",
        "topResumedActivity: null\nmResumedActivity:",
    );
    assert!(
        parse_facts(&null_top)
            .unwrap()
            .foreground_package
            .is_empty()
    );
}

#[test]
fn android_10_full_window_dump_reads_focus_from_display_section() {
    let raw = probe(
        "Physical size: 800x1280",
        "SurfaceOrientation: 1",
        "mResumedActivity: ActivityRecord{now u0 chat.xiaoxin.app/.MainActivity t7}",
    );
    let prefix = raw.split_once("__REMOTE_WINDOW__").unwrap().0;
    // Android10固件的windows分项不包含当前focus，不能据旧Activity记录猜测。
    let windows_only = "WINDOW MANAGER WINDOWS (dumpsys window windows)\n  Window #0 Window{window u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}\n";
    let incomplete = parse_facts(&format!("{prefix}__REMOTE_WINDOW__\n{windows_only}")).unwrap();
    assert!(incomplete.foreground_package.is_empty());
    assert!(initial_control_mode(&incomplete).is_err());
    let display = "WINDOW MANAGER DISPLAY CONTENTS (dumpsys window displays)\n  Display: mDisplayId=0\n    mCurrentFocus=Window{42f935e u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}\n";
    let complete = parse_facts(&format!(
        "{prefix}__REMOTE_WINDOW__\n{display}{windows_only}"
    ))
    .unwrap();
    assert_eq!(complete.foreground_package, "chat.xiaoxin.app");
    assert_eq!(initial_control_mode(&complete).unwrap(), "full");
}

#[test]
fn serial_identity_requires_a_stable_non_placeholder_value() {
    for value in ["", "unknown", "0", "with newline\nvalue", "secret=value"] {
        assert!(trusted_serial(value).is_none());
    }
    assert_eq!(
        trusted_serial(" A47A5CK00524 \n").as_deref(),
        Some("A47A5CK00524")
    );
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
async fn stdin_transport_does_not_place_text_in_process_arguments_or_return_tool_output() {
    let executable = std::env::current_exe().unwrap();
    let script = b"private-password-in-stdin-only";
    super::super::transport::run_stdin(
        &executable,
        &child_arguments("stdin_fixture_ok"),
        script,
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let error = super::super::transport::run_stdin(
        &executable,
        &child_arguments("stdin_fixture_error"),
        script,
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .err()
    .unwrap();
    assert!(!error.to_string().contains("private-password"));
}

#[test]
#[ignore = "仅由stdin隔离子进程测试调用"]
fn stdin_fixture_ok() {
    use std::io::Read;
    assert!(!std::env::args().any(|v| v.contains("private-password")));
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"private-password-in-stdin-only");
}
#[test]
#[ignore = "仅由stdin错误脱敏隔离子进程测试调用"]
fn stdin_fixture_error() {
    use std::io::{Read, Write};
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    std::io::stderr().write_all(&bytes).unwrap();
    std::process::exit(1);
}
