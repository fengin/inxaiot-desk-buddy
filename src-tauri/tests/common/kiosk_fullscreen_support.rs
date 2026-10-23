//! 仅供明确授权的实机回归使用；系统栏由窗口状态核实，不由锁定偏好推断。
use regex::Regex;

pub const REQUIRED_SYSTEM_UI_FLAGS: u32 = 0x1006; // FULLSCREEN | HIDE_NAVIGATION | IMMERSIVE_STICKY
const MAIN_COMPONENT: &str =
    r"chat\.xiaoxin\.app/(?:\.MainActivity|chat\.xiaoxin\.app\.MainActivity)";

/// 只接受唯一、可见且有 Surface 的小新主窗口；不能借用系统栏或销毁中旧窗口的 flags。
pub fn main_window_flags(windows: &str) -> Result<u32, &'static str> {
    let headers = Regex::new(r"(?m)^\s*Window #\d+ Window\{[^\r\n]+\}:\s*$").unwrap();
    let main = Regex::new(&format!(
        r"^\s*Window #\d+ Window\{{[0-9a-fA-F]+ u0 {MAIN_COMPONENT}\}}:\s*$"
    ))
    .unwrap();
    let visible = Regex::new(r"\bmViewVisibility=0x0\b").unwrap();
    let surface = Regex::new(r"\bmHasSurface=true\b").unwrap();
    let flags = Regex::new(r"\bmSystemUiVisibility=0x([0-9a-fA-F]+)\b").unwrap();
    let ranges = headers.find_iter(windows).collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for (index, header) in ranges.iter().enumerate() {
        if !main.is_match(header.as_str()) {
            continue;
        }
        let end = ranges
            .get(index + 1)
            .map_or(windows.len(), |next| next.start());
        let body = &windows[header.end()..end];
        if !visible.is_match(body) || !surface.is_match(body) {
            continue;
        }
        if [
            "mRemoved=true",
            "mDestroying=true",
            "isOnScreen=false",
            "isVisible=false",
        ]
        .iter()
        .any(|bad| body.contains(bad))
        {
            continue;
        }
        let values = flags.captures_iter(body).collect::<Vec<_>>();
        if values.len() != 1 {
            return Err("小新主窗口未返回唯一系统栏标记");
        }
        candidates.push(u32::from_str_radix(&values[0][1], 16).map_err(|_| "系统栏标记格式异常")?);
    }
    if candidates.len() != 1 {
        return Err("没有唯一可见的小新主窗口，不能确认全屏");
    }
    Ok(candidates[0])
}

pub fn main_is_resumed(activities: &str) -> bool {
    Regex::new(&format!(r"(?m)^\s*(?:mResumedActivity:|mResumedActivity=|ResumedActivity:|topResumedActivity=)\s*ActivityRecord\{{[^\r\n]*\bu0 {MAIN_COMPONENT} t\d+\}}"))
        .unwrap().is_match(activities)
}

pub fn main_activity_exists(activities: &str) -> bool {
    Regex::new(&format!(
        r"ActivityRecord\{{[^\r\n]*\bu0 {MAIN_COMPONENT}(?:\s|\}})"
    ))
    .unwrap()
    .is_match(activities)
}

pub fn voice_service_exists(services: &str) -> bool {
    Regex::new(r"ServiceRecord\{[^\r\n]*\bu0 chat\.xiaoxin\.app/id\.flutter\.flutter_background_service\.BackgroundService(?:\s|\})")
        .unwrap().is_match(services)
}

/// force-stop 默认 HOME 后，Android 可能立即重新创建主屏；这不是可用于验证冷读的状态。
pub fn system_home_relaunch_confirmed(
    activities: &str,
    resolved_home: &str,
    before: (&str, &str),
    after: (&str, &str),
) -> bool {
    let components = resolved_home
        .lines()
        .map(str::trim)
        .filter(|line| {
            matches!(
                *line,
                "chat.xiaoxin.app/.MainActivity" | "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity"
            )
        })
        .count();
    main_is_resumed(activities)
        && components == 1
        && !before.0.is_empty()
        && !after.0.is_empty()
        && before.0 != after.0
        && !before.1.is_empty()
        && before.1 == after.1
}

/// 只接受本次命令前后新增、带时间戳且由 ActivityManager 明确报告的当前包拒停日志。
/// 历史相同日志不能证明本轮命令被拒绝，其他包、用户、日志标签也不能作为依据。
pub fn new_protected_package_refusals(before_log: &str, after_log: &str) -> Vec<String> {
    let exact = Regex::new(r"^\s*\d{10,}\.\d{3,9}\s+\d+\s+\d+\s+W\s+ActivityManager\s*:\s*Ignoring request to force stop protected package chat\.xiaoxin\.app u0\s*$").unwrap();
    let prior = before_log
        .lines()
        .map(str::trim)
        .collect::<std::collections::BTreeSet<_>>();
    after_log
        .lines()
        .map(str::trim)
        .filter(|line| exact.is_match(line) && !prior.contains(line))
        .map(str::to_string)
        .collect()
}

pub fn protected_force_stop_refusal_confirmed(
    owner_is_self: bool,
    before: (&str, &str),
    after: (&str, &str),
    new_refusals: &[String],
) -> bool {
    owner_is_self
        && !new_refusals.is_empty()
        && !before.0.is_empty()
        && before.0 == after.0
        && !before.1.is_empty()
        && before.1 == after.1
}

#[cfg(test)]
mod tests {
    use super::*;

    // 根据本轮 Android 10 / 8.1 的真实 dumpsys 结构缩减，去掉设备标识和无关状态。
    const ANDROID10: &str = "Window #0 Window{aa u0 StatusBar}:\n  mViewVisibility=0x0 mHasSurface=true\n  mSystemUiVisibility=0x1706\nWindow #5 Window{bb u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}:\n  mViewVisibility=0x0 mHaveFrame=true mObscured=false\n  mSeq=0 mSystemUiVisibility=0x0\n  mHasSurface=true isReadyForDisplay()=true\n  isOnScreen=true\n  isVisible=true\nWindow #6 Window{cc u0 NavigationBar0}:\n  mSystemUiVisibility=0x1706\n";
    const ANDROID81: &str = "Window #3 Window{aa u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}:\n  mViewVisibility=0x8\n  mSystemUiVisibility=0x0\n  mHasSurface=false\nWindow #4 Window{bb u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity}:\n  mViewVisibility=0x0 mHaveFrame=true mObscured=false\n  mSeq=0 mSystemUiVisibility=0x1706\n  mHasSurface=true mShownPosition=[0,0] isReadyForDisplay()=true\n  isOnScreen=true\n  isVisible=true\n";

    #[test]
    fn does_not_borrow_system_bar_or_hidden_old_main_flags() {
        assert_eq!(main_window_flags(ANDROID10), Ok(0));
        assert_eq!(main_window_flags(ANDROID81), Ok(0x1706));
        assert_eq!(0x1706 & REQUIRED_SYSTEM_UI_FLAGS, REQUIRED_SYSTEM_UI_FLAGS);
        assert_ne!(0x600 & REQUIRED_SYSTEM_UI_FLAGS, REQUIRED_SYSTEM_UI_FLAGS);
    }

    #[test]
    fn missing_wrong_component_ambiguous_and_incomplete_window_are_not_success() {
        for invalid in [
            ANDROID10.replace(
                "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity",
                "other.app/.MainActivity",
            ),
            ANDROID10.replace(
                "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity",
                "chat.xiaoxin.app/.OtherActivity",
            ),
            ANDROID10.replace("mSeq=0 mSystemUiVisibility=0x0", "mSeq=0"),
            ANDROID10.replace(
                "mHasSurface=true isReadyForDisplay()=true",
                "mHasSurface=false",
            ),
            format!("{ANDROID81}{ANDROID81}"),
            String::from("Permission denied"),
        ] {
            assert!(
                main_window_flags(&invalid).is_err(),
                "不完整/歧义窗口不能成功: {invalid}"
            );
        }
    }

    #[test]
    fn foreground_and_cold_state_match_actual_records_only() {
        let foreground =
            "mResumedActivity: ActivityRecord{abc u0 chat.xiaoxin.app/.MainActivity t42}";
        assert!(main_is_resumed(foreground));
        assert!(main_activity_exists(foreground));
        assert!(!main_is_resumed(
            &foreground.replace("mResumedActivity:", "lastPausedActivity:")
        ));
        assert!(!main_is_resumed(&foreground.replace(
            "chat.xiaoxin.app/.MainActivity",
            "com.android.settings/.Settings"
        )));
        assert!(!main_activity_exists(
            "Intent { cmp=chat.xiaoxin.app/.MainActivity }"
        ));
        assert!(voice_service_exists("* ServiceRecord{abc u0 chat.xiaoxin.app/id.flutter.flutter_background_service.BackgroundService}"));
        assert!(!voice_service_exists(
            "ACTIVITY MANAGER SERVICES\nPackage query: chat.xiaoxin.app\nBackgroundService"
        ));
        assert!(!voice_service_exists("ServiceRecord{abc u0 other.app/id.flutter.flutter_background_service.BackgroundService}"));
    }

    #[test]
    fn system_home_restart_requires_foreground_home_and_a_new_process_in_same_boot() {
        let activity =
            "mResumedActivity: ActivityRecord{abc u0 chat.xiaoxin.app/.MainActivity t42}";
        let home = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\nchat.xiaoxin.app/.MainActivity";
        assert!(system_home_relaunch_confirmed(
            activity,
            home,
            ("100", "boot-a"),
            ("200", "boot-a")
        ));
        assert!(!system_home_relaunch_confirmed(
            activity,
            home,
            ("100", "boot-a"),
            ("100", "boot-a")
        ));
        assert!(!system_home_relaunch_confirmed(
            activity,
            home,
            ("100", "boot-a"),
            ("200", "boot-b")
        ));
        assert!(!system_home_relaunch_confirmed(
            activity,
            "other.app/.Home",
            ("100", "boot-a"),
            ("200", "boot-a")
        ));
        assert!(!system_home_relaunch_confirmed(
            &activity.replace("mResumedActivity:", "lastPausedActivity:"),
            home,
            ("100", "boot-a"),
            ("200", "boot-a")
        ));
    }

    #[test]
    fn protected_stop_boundary_requires_new_exact_log_same_process_and_self_owner() {
        let old = "1792749000.100  2040  2040 W ActivityManager: Ignoring request to force stop protected package chat.xiaoxin.app u0";
        let fresh = old.replace("9000.100", "9001.200");
        assert!(new_protected_package_refusals(old, old).is_empty());
        let warnings = new_protected_package_refusals(old, &format!("{old}\n{fresh}"));
        assert_eq!(warnings, vec![fresh.clone()]);
        assert!(protected_force_stop_refusal_confirmed(
            true,
            ("2943", "boot-a"),
            ("2943", "boot-a"),
            &warnings
        ));
        assert!(!protected_force_stop_refusal_confirmed(
            false,
            ("2943", "boot-a"),
            ("2943", "boot-a"),
            &warnings
        ));
        assert!(!protected_force_stop_refusal_confirmed(
            true,
            ("2943", "boot-a"),
            ("3000", "boot-a"),
            &warnings
        ));
        assert!(!protected_force_stop_refusal_confirmed(
            true,
            ("2943", "boot-a"),
            ("2943", "boot-b"),
            &warnings
        ));
        assert!(!protected_force_stop_refusal_confirmed(
            true,
            ("2943", "boot-a"),
            ("2943", "boot-a"),
            &[]
        ));
        for invalid in [
            fresh.replace("chat.xiaoxin.app", "other.app"),
            fresh.replace("u0", "u10"),
            fresh.replace("ActivityManager:", "OtherTag:"),
            fresh.replace("W ActivityManager", "I ActivityManager"),
        ] {
            assert!(new_protected_package_refusals(old, &invalid).is_empty());
        }
    }
}
