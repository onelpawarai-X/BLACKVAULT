//! BLACKVAULT — the agent's command surface.
//!
//! The UI talks to the agent only through these commands, exactly as it talks to
//! the rest of the core through `commands.rs`. Keeping them separate keeps the
//! security commands small and reviewable, and makes it obvious that none of the
//! agent's own commands can unlock the vault or change a protection.

use std::sync::Arc;

use tauri::State;

use crate::error::{BvError, BvResult};
use crate::runner::{AgentMessage, AgentRuntime, AgentState};
use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

/// The runner, or an error saying the app is not ready yet.
fn agent(state: &Arc<AppState>) -> BvResult<Arc<AgentRuntime>> {
    state
        .agent
        .lock()
        .clone()
        .ok_or_else(|| BvError::msg("এজেন্ট এখনো চালু হয়নি"))
}

#[tauri::command]
pub fn agent_status(state: St<'_>) -> BvResult<AgentState> {
    Ok(agent(&state)?.snapshot())
}

/// Prove a key and, on success, store it sealed with DPAPI.
#[tauri::command]
pub fn agent_set_key(state: St<'_>, key: String) -> BvResult<String> {
    let runtime = agent(&state)?;
    let message = runtime.verify_key(&key)?;
    runtime.save_key(&key)?;
    state.log_event("agent_key", "info", "Cloud key stored", None);
    Ok(message)
}

#[tauri::command]
pub fn agent_clear_key(state: St<'_>) -> BvResult<()> {
    state.db.clear_secret("cloud_api_key")?;
    state.log_event("agent_key", "warn", "Cloud key removed", None);
    Ok(())
}

/// Whether a key is stored, without revealing it.
#[tauri::command]
pub fn agent_has_key(state: St<'_>) -> bool {
    agent(&state).map(|a| a.has_key()).unwrap_or(false)
}

/// The chats on this desktop and the cloud conversations behind them.
#[tauri::command]
pub fn agent_conversations(state: St<'_>) -> BvResult<Vec<serde_json::Value>> {
    Ok(agent(&state)?.conversations())
}

/// The history of one chat, newest last.
#[tauri::command]
pub fn agent_history(
    state: St<'_>,
    local_id: String,
    limit: Option<u32>,
) -> BvResult<Vec<AgentMessage>> {
    agent(&state)?.history(&local_id, limit.unwrap_or(60))
}

/// Send a message. The turn runs in the background; the answer arrives as
/// events.
#[tauri::command]
pub fn agent_send(state: St<'_>, local_id: String, text: String) -> BvResult<()> {
    let runtime = agent(&state)?;
    if !runtime.has_key() {
        return Err(BvError::msg("ক্লাউড কী দেওয়া হয়নি — সেটিংসে যোগ করুন"));
    }
    runtime.start_turn(local_id, text);
    Ok(())
}

/// Stop the running turn.
#[tauri::command]
pub fn agent_interrupt(state: St<'_>) -> BvResult<()> {
    agent(&state)?.interrupt();
    Ok(())
}

/// Forget a chat. Its sandbox is returned to the warm pool.
#[tauri::command]
pub fn agent_forget(state: St<'_>, local_id: String) -> BvResult<()> {
    agent(&state)?.forget(&local_id);
    Ok(())
}

/// The sensitive tool call that is waiting for the user, if any.
#[tauri::command]
pub fn agent_pending(state: St<'_>) -> Option<(String, String, String)> {
    agent(&state).ok().and_then(|a| a.pending())
}

/// Answer a waiting tool call.
///
/// The token is the one the prompt was raised with, so an answer can only ever
/// release the exact call the user was shown — never a different one, and never
/// a replayed one. This is the only route by which a sensitive tool is allowed
/// to run, and it is reached only from the user's own prompt.
#[tauri::command]
pub fn agent_answer(state: St<'_>, token: String, allow: bool) -> BvResult<()> {
    let runtime = agent(&state)?;
    if !runtime.answer_approval(&token, allow) {
        return Err(BvError::msg("এই অনুমোদনের অনুরোধ আর সক্রিয় নেই"));
    }
    Ok(())
}

/// The tools the agent may use on this machine, for the pane to show.
#[tauri::command]
pub fn agent_tools() -> serde_json::Value {
    crate::agent::tool_list()
}
