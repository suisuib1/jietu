use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::core::clipboard::{
    ClipboardHistoryDetail, ClipboardHistoryService, ClipboardHistorySummary,
};

use super::clipboard::ClipboardRuntimeState;

const HISTORY_CHANGED_EVENT: &str = "clipboard-history-changed";
const MAX_PAGE_SIZE: usize = 100;
const MAX_PREVIEW_DIMENSION: u32 = 1_400;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClipboardHistoryImagePreview {
    data_base64: String,
    width: u32,
    height: u32,
}

fn service(state: &State<'_, ClipboardRuntimeState>) -> Result<ClipboardHistoryService, String> {
    state
        .history_service()
        .map_err(|error| {
            eprintln!("Clipboard history runtime access failed: {error}");
            "history_unavailable".to_owned()
        })?
        .ok_or_else(|| "history_unavailable".to_owned())
}

fn page_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_PAGE_SIZE)
}

fn command_error(action: &str, error: impl std::fmt::Display) -> String {
    eprintln!("Clipboard history {action} failed: {error}");
    "history_operation_failed".to_owned()
}

#[tauri::command]
pub(crate) fn clipboard_history_list(
    state: State<'_, ClipboardRuntimeState>,
    offset: usize,
    limit: usize,
) -> Result<Vec<ClipboardHistorySummary>, String> {
    service(&state)?
        .list(offset, page_limit(limit))
        .map_err(|error| command_error("list", error))
}

#[tauri::command]
pub(crate) fn clipboard_history_search(
    state: State<'_, ClipboardRuntimeState>,
    query: String,
    offset: usize,
    limit: usize,
) -> Result<Vec<ClipboardHistorySummary>, String> {
    service(&state)?
        .search(&query, offset, page_limit(limit))
        .map_err(|error| command_error("search", error))
}

#[tauri::command]
pub(crate) fn clipboard_history_get(
    state: State<'_, ClipboardRuntimeState>,
    id: i64,
) -> Result<Option<ClipboardHistoryDetail>, String> {
    service(&state)?
        .get(id)
        .map_err(|error| command_error("get", error))
}

#[tauri::command]
pub(crate) fn clipboard_history_delete(
    app: AppHandle,
    state: State<'_, ClipboardRuntimeState>,
    id: i64,
) -> Result<bool, String> {
    let deleted = service(&state)?
        .delete(id)
        .map_err(|error| command_error("delete", error))?;
    if deleted {
        let _ = app.emit(HISTORY_CHANGED_EVENT, ());
    }
    Ok(deleted)
}

#[tauri::command]
pub(crate) fn clipboard_history_set_favorite(
    app: AppHandle,
    state: State<'_, ClipboardRuntimeState>,
    id: i64,
    favorite: bool,
) -> Result<bool, String> {
    let updated = service(&state)?
        .set_favorite(id, favorite)
        .map_err(|error| command_error("favorite update", error))?;
    if updated {
        let _ = app.emit(HISTORY_CHANGED_EVENT, ());
    }
    Ok(updated)
}

#[tauri::command]
pub(crate) fn clipboard_history_count(
    state: State<'_, ClipboardRuntimeState>,
    query: Option<String>,
) -> Result<usize, String> {
    service(&state)?
        .count(query.as_deref())
        .map_err(|error| command_error("count", error))
}

#[tauri::command]
pub(crate) fn clipboard_history_image_preview(
    state: State<'_, ClipboardRuntimeState>,
    id: i64,
    max_width: u32,
    max_height: u32,
) -> Result<Option<ClipboardHistoryImagePreview>, String> {
    let max_width = max_width.clamp(1, MAX_PREVIEW_DIMENSION);
    let max_height = max_height.clamp(1, MAX_PREVIEW_DIMENSION);
    service(&state)?
        .image_preview(id, max_width, max_height)
        .map(|preview| {
            preview.map(|preview| ClipboardHistoryImagePreview {
                data_base64: BASE64.encode(preview.png),
                width: preview.width,
                height: preview.height,
            })
        })
        .map_err(|error| command_error("image preview", error))
}
