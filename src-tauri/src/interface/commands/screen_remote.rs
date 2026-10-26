use crate::{
    formal::app_state::FormalAppState,
    infrastructure::smart_screen::remote_screen::{
        self, RemoteScreenFrame, RemoteScreenSessionView, RemoteScreenSessions,
    },
    interface::error::CommandErrorDto,
};
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn screen_remote_open(
    state: State<'_, FormalAppState>,
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    screen_id: String,
) -> Result<RemoteScreenSessionView, CommandErrorDto> {
    remote_screen::open(
        &state,
        &sessions,
        window.label(),
        &local_project_id,
        &screen_id,
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_remote_capture(
    state: State<'_, FormalAppState>,
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    session_id: String,
) -> Result<RemoteScreenFrame, CommandErrorDto> {
    remote_screen::capture(
        &state,
        &sessions,
        window.label(),
        &local_project_id,
        &session_id,
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub fn screen_remote_close(
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    session_id: String,
) -> Result<(), CommandErrorDto> {
    remote_screen::close(&sessions, window.label(), &local_project_id, &session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_remote_begin_control(
    state: State<'_, FormalAppState>,
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    session_id: String,
) -> Result<remote_screen::ControlView, CommandErrorDto> {
    remote_screen::begin_control(
        &state,
        &sessions,
        window.label(),
        &local_project_id,
        &session_id,
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_remote_end_control(
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    session_id: String,
) -> Result<(), CommandErrorDto> {
    remote_screen::end_control(&sessions, window.label(), &local_project_id, &session_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_remote_action(
    state: State<'_, FormalAppState>,
    sessions: State<'_, RemoteScreenSessions>,
    window: WebviewWindow,
    local_project_id: String,
    session_id: String,
    input: remote_screen::ControlInput,
) -> Result<remote_screen::ActionReceipt, CommandErrorDto> {
    remote_screen::act(
        &state,
        &sessions,
        window.label(),
        &local_project_id,
        &session_id,
        input,
    )
    .await
    .map_err(Into::into)
}
