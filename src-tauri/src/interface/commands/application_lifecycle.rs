use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Default)]
pub struct ExitConfirmationState {
    confirmed: AtomicBool,
}

impl ExitConfirmationState {
    pub fn confirm(&self) {
        self.confirmed.store(true, Ordering::SeqCst);
    }

    pub fn take_confirmation(&self) -> bool {
        self.confirmed.swap(false, Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationExitImpact {
    pub active_task_count: u32,
    pub queued_task_count: u32,
    pub running_task_count: u32,
    pub wait_timeout_seconds: u32,
}

#[tauri::command]
pub fn confirm_application_exit(app_handle: AppHandle, state: State<'_, ExitConfirmationState>) {
    state.confirm();
    app_handle.exit(0);
}
