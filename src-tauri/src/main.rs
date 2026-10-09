#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--verify-package") {
        if !(2..=3).contains(&args.len()) {
            std::process::exit(2);
        }
        let result = tokio::runtime::Runtime::new()
            .expect("创建验证运行时")
            .block_on(
            inxaiot_desk_buddy_lib::infrastructure::smart_screen::tool_bundle::verify_distribution(
                args.get(2).map(std::path::Path::new),
            ),
        );
        let success = result.is_ok();
        let report = match result {
            Ok(value) => value,
            Err(error) => serde_json::json!({"successful":false,"error":error.to_string()}),
        };
        if std::fs::write(&args[1], serde_json::to_vec_pretty(&report).unwrap()).is_err() {
            std::process::exit(2);
        }
        std::process::exit(if success { 0 } else { 1 });
    }
    inxaiot_desk_buddy_lib::run();
}
