//! BLACKVAULT — the cloud agent link.
//!
//! One remote brain, driven from the desktop. A *conversation* on the cloud
//! service is a whole agent session: it owns a sandbox, a tool loop and a live
//! event stream. This module is the only place that talks to it, and it is split
//! in two on purpose:
//!
//!   * [`Cloud`] — the request/response half (create, list, send, read events,
//!     pause/resume a sandbox). Plain HTTP, easy to test and to reason about.
//!   * [`stream_events`] — the live half, a WebSocket that pushes what the agent
//!     is doing as it does it, so the desktop shows a turn being written rather
//!     than a spinner.
//!
//! Everything a caller can see is a small, stable shape ([`AgentEvent`]); the
//! service's own event vocabulary never leaks past `normalise_event`. That is
//! what lets the UI stay the same when the service changes its schema, and it is
//! why `normalise_event` is pure and unit-tested.
//!
//! Two service behaviours are load-bearing and are encoded here rather than left
//! to the caller:
//!
//!   * **A sandbox is rationed.** Creating one is limited per day; *reusing* a
//!     warm one is free and takes seconds. [`StartRequest::sandbox_id`] is
//!     therefore threaded through every start, and a start that has a warm
//!     sandbox reuses it.
//!   * **A client tool call ends the run.** When the agent calls a tool that
//!     lives on this machine, the service emits its own placeholder observation
//!     and stops. The bridge must run the tool and then continue the
//!     conversation; that is why a `finished` status is not, on its own, the end
//!     of a turn.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{BvError, BvResult};

/// The service's public API root. Every conversation, sandbox and settings call
/// lives under it.
pub const CLOUD_BASE: &str = "https://app.all-hands.dev/api/v1";

/// The model the agent starts on when the user has not chosen one. It is the
/// fastest of the free pair and the one the phone build used, so behaviour is
/// consistent across the two clients.
pub const DEFAULT_MODEL: &str = "openhands/deepseek-v4.1-flash";

/// The service refuses `limit` above this on the events endpoint (HTTP 422).
pub const MAX_EVENTS_PAGE: u32 = 100;

/// How many events of history the desktop asks for. Long enough for a real
/// conversation, short enough that the panel opens promptly; paging stops here.
pub const MAX_HISTORY: u32 = 1000;

/// How long a plain API call may take. Listing and status calls are quick; a
/// sandbox that is still booting answers slowly, so this is generous.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================ wire types

/// A tool this machine can run. The cloud registers these as real tools of the
/// conversation, so the agent reaches for them by name exactly as it reaches for
/// its own; when it calls one, the call arrives here and BLACKVAULT runs it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientToolSpec {
    pub name: String,
    pub description: String,
    /// A JSON Schema object. The service rejects anything that is not one, so
    /// this is built by [`ClientToolSpec::new`] rather than set by hand.
    pub parameters: Value,
}

impl ClientToolSpec {
    /// Build a spec from a name, a description and a list of
    /// `(parameter, type, required)` triples.
    pub fn new(name: &str, description: &str, params: &[(&str, &str, bool)]) -> Self {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (param, kind, needed) in params {
            properties.insert(
                (*param).to_string(),
                json!({ "type": kind, "description": format!("{param} ({kind})") }),
            );
            if *needed {
                required.push(Value::String((*param).to_string()));
            }
        }
        ClientToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters: json!({
                "type": "object",
                "properties": properties,
                "required": required,
            }),
        }
    }
}

/// Everything needed to start a conversation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub title: String,
    /// The first message. A conversation cannot be created without one.
    pub prompt: String,
    /// Appended to the agent's own instructions. This is where the BLACKVAULT
    /// identity lives — the service refuses to *replace* its prompt, so the
    /// contract is added instead.
    pub system_message_suffix: Option<String>,
    /// A warm sandbox to host the conversation. Reusing one is free.
    pub sandbox_id: Option<String>,
    pub model: Option<String>,
    pub client_tools: Vec<ClientToolSpec>,
}

/// A conversation as it appears in a list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationBrief {
    pub id: String,
    pub title: String,
    pub sandbox_id: Option<String>,
    pub sandbox_status: Option<String>,
    pub execution_status: Option<String>,
    pub llm_model: Option<String>,
    pub repository: Option<String>,
    pub updated_at: Option<String>,
    /// Set once the sandbox is up; a conversation still starting has none.
    #[serde(default)]
    pub conversation_url: Option<String>,
    /// The rotating key for the agent server. Only present on a detail read.
    #[serde(default)]
    pub session_api_key: Option<String>,
}

impl ConversationBrief {
    /// The agent server's base URL, derived from the conversation URL.
    ///
    /// The service hands out a per-conversation URL; the socket, the interrupt
    /// and the confirmation policy all live on the host in front of it.
    pub fn agent_base(&self) -> Option<String> {
        agent_base_from_url(self.conversation_url.as_deref()?)
    }

    /// The socket URL for this conversation's live events.
    pub fn socket_url(&self) -> Option<String> {
        let base = self.agent_base()?;
        let ws = base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        Some(format!("{ws}/sockets/events/{}", self.id))
    }

    /// True while the sandbox is up and the socket will accept.
    pub fn is_running(&self) -> bool {
        self.sandbox_status.as_deref() == Some("RUNNING")
    }

    /// True while the sandbox is asleep. A resume is free and takes seconds.
    pub fn is_paused(&self) -> bool {
        self.sandbox_status.as_deref() == Some("PAUSED")
    }
}

/// A conversation start, before its sandbox exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTask {
    /// The start-task id, used to poll until the conversation is ready.
    pub id: String,
    pub app_conversation_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub status: Option<String>,
    pub detail: Option<String>,
}

impl StartTask {
    pub fn is_ready(&self) -> bool {
        self.app_conversation_id
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.status.as_deref(), Some("ERROR") | Some("FAILED"))
    }

    /// A stage label in the user's language, for the boot loader.
    pub fn stage_label(&self) -> &'static str {
        match self.status.as_deref() {
            Some("SETTING_UP_SKILLS") => "সেটআপ হচ্ছে…",
            Some("STARTING_CONVERSATION") => "কনভার্সেশন চালু হচ্ছে…",
            Some("PREPARING_REPOSITORY") => "রিপোজিটরি প্রস্তুত হচ্ছে…",
            Some("WORKING") => "কাজ শুরু হচ্ছে…",
            _ => "অপেক্ষা করা হচ্ছে…",
        }
    }
}

// ============================================================ event shapes

/// One thing the agent did, in a shape the desktop can render.
///
/// This is the whole UI contract: `kind` picks the card, the rest fills it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub id: String,
    /// `message`, `action`, `observation`, `usage`, `status`, `error` or `other`.
    pub kind: String,
    /// `user` or `assistant` for a message.
    #[serde(default)]
    pub role: String,
    /// Message text, observation text or a streamed delta.
    #[serde(default)]
    pub text: String,
    /// The agent's private reasoning, when the service exposes it.
    #[serde(default)]
    pub thought: String,
    /// The tool name for an action or an observation.
    #[serde(default)]
    pub tool: String,
    /// A one-line description of an action.
    #[serde(default)]
    pub summary: String,
    /// False when an observation reports a failure.
    #[serde(default)]
    pub ok: bool,
    /// The arguments of a tool call, as the service sent them.
    ///
    /// A call for one of our tools arrives here and is run on this machine, so
    /// its arguments have to survive normalisation: the service puts them in the
    /// action event's `action` object (and repeats them as a JSON string in
    /// `tool_call.arguments`), not in the summary. Dropping them means every
    /// desktop tool call is attempted with no arguments and fails.
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub at: String,
}

impl AgentEvent {
    fn base(id: &str, kind: &str, at: &str) -> Self {
        AgentEvent {
            id: id.to_string(),
            kind: kind.to_string(),
            ok: true,
            at: at.to_string(),
            ..Default::default()
        }
    }
}

/// Flatten an LLM content field (`[{type:"text",text:"…"}]`, or a bare string)
/// into plain text. Images become a placeholder rather than being dropped, so a
/// turn never looks like it silently lost something.
pub fn blocks_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(block_text).collect(),
        Value::Null => String::new(),
        other => other.as_str().unwrap_or_default().to_string(),
    }
}

fn block_text(block: &Value) -> String {
    match block {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("text") {
                text.clone()
            } else if map.get("image_url").is_some() || map.get("type").and_then(Value::as_str) == Some("image") {
                "[image]".to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// Turn one raw service event into an [`AgentEvent`].
///
/// Unknown kinds become `other` rather than an error: the service adds event
/// types over time and a new one must never break the stream.
pub fn normalise_event(event: &Value) -> AgentEvent {
    let kind = event.get("kind").and_then(Value::as_str).unwrap_or("Event");
    let source = event.get("source").and_then(Value::as_str).unwrap_or("");
    let id = event.get("id").and_then(Value::as_str).unwrap_or_default();
    let at = event
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match kind {
        "MessageEvent" => {
            let llm = event.get("llm_message").cloned().unwrap_or(Value::Null);
            let role = llm
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if source == "agent" || source == "assistant" {
                        "assistant".to_string()
                    } else {
                        "user".to_string()
                    }
                });
            let mut ev = AgentEvent::base(id, "message", at);
            ev.role = if role == "user" { "user".into() } else { "assistant".into() };
            ev.text = blocks_text(llm.get("content").unwrap_or(&Value::Null));
            ev.thought = blocks_text(llm.get("reasoning_content").unwrap_or(&Value::Null));
            ev
        }
        "StreamingDeltaEvent" => {
            let mut ev = AgentEvent::base(id, "delta", at);
            ev.thought = blocks_text(event.get("reasoning_content").unwrap_or(&Value::Null));
            ev.text = blocks_text(event.get("content").unwrap_or(&Value::Null));
            ev
        }
        "ActionEvent" => {
            let mut ev = AgentEvent::base(id, "action", at);
            ev.tool = event
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ev.summary = action_summary(event);
            ev.thought = blocks_text(event.get("thought").unwrap_or(&Value::Null));
            ev.arguments = action_arguments(event);
            ev
        }
        "ObservationEvent" => {
            let observation = event.get("observation").cloned().unwrap_or(Value::Null);
            let is_error = observation
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut ev = AgentEvent::base(id, "observation", at);
            ev.tool = event
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ev.ok = !is_error;
            ev.text = blocks_text(observation.get("content").unwrap_or(&Value::Null));
            ev
        }
        "ConversationStateUpdateEvent" => {
            let key = event.get("key").and_then(Value::as_str).unwrap_or("");
            let value = event.get("value").cloned().unwrap_or(Value::Null);
            // The turn's own liveness, kept as its own kind. It must not be
            // folded into `status`: only a *transition* to finished ends a turn,
            // and a `status` card is a transcript line the user reads.
            if key == "execution_status" {
                let mut ev = AgentEvent::base(id, "exec_status", at);
                ev.text = value.as_str().unwrap_or_default().to_string();
                return ev;
            }
            if key == "stats" {
                let mut ev = AgentEvent::base(id, "usage", at);
                let metrics = value
                    .get("usage_to_metrics")
                    .and_then(|v| v.get("agent"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let tokens = metrics
                    .get("accumulated_token_usage")
                    .and_then(|v| v.get("prompt_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cost = metrics
                    .get("accumulated_cost")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                ev.text = format!("{tokens} tokens · ${cost:.4}");
                return ev;
            }
            // Every other key the service pushes (`full_state`,
            // `last_user_message_id`, …) is bookkeeping whose value is not a
            // status. Showing it as a card put raw keys in the transcript, so it
            // is dropped instead.
            AgentEvent::base(id, "other", at)
        }
        "AgentStateChangedObservation" => {
            let mut ev = AgentEvent::base(id, "status", at);
            ev.text = event
                .get("agent_state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ev
        }
        "ErrorEvent" | "ConversationErrorEvent" | "AgentErrorEvent" | "ServerErrorEvent" => {
            let mut ev = AgentEvent::base(id, "error", at);
            ev.ok = false;
            ev.text = event
                .get("detail")
                .or_else(|| event.get("error"))
                .or_else(|| event.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            ev
        }
        _ => AgentEvent::base(id, "other", at),
    }
}

/// The arguments of a tool call, as the service sent them.
///
/// The action's own fields are the arguments (`{"command": "ls"}`); the service
/// also mirrors them as a JSON string in `tool_call.arguments`, which is used as
/// a fallback when the action is not an object. A call that arrives with no
/// arguments at all becomes `{}` rather than `null`, so a tool with no
/// parameters still parses.
pub fn action_arguments(event: &Value) -> Value {
    if let Some(action) = event.get("action") {
        if action.is_object() {
            return action.clone();
        }
        if let Some(text) = action.as_str() {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if parsed.is_object() {
                    return parsed;
                }
            }
        }
    }
    if let Some(text) = event
        .get("tool_call")
        .and_then(|c| c.get("arguments"))
        .and_then(Value::as_str)
    {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() {
                return parsed;
            }
        }
    }
    json!({})
}

/// A one-line description of an action, taken from whichever field the tool
/// actually used.
pub fn action_summary(event: &Value) -> String {
    let tool = event
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let action = event.get("action").cloned().unwrap_or(Value::Null);
    let Some(map) = action.as_object() else {
        return tool.to_string();
    };
    for key in ["command", "path", "file_path", "query", "url", "text", "app"] {
        if let Some(Value::String(value)) = map.get(key) {
            if !value.is_empty() {
                let trimmed: String = value.chars().take(200).collect();
                return format!("{tool}: {trimmed}");
            }
        }
    }
    tool.to_string()
}

/// Kinds the desktop renders. Everything else is dropped before it reaches the
/// UI, so the stream stays small.
pub fn is_renderable(event: &AgentEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "message" | "action" | "observation" | "usage" | "status" | "error" | "delta" | "exec_status"
    )
}

/// Derive an agent-server base from a conversation URL.
///
/// The service returns `https://<host>/api/conversations/<id>`; the socket and
/// the control endpoints sit on `https://<host>`.
pub fn agent_base_from_url(url: &str) -> Option<String> {
    let (base, _) = url.split_once("/api/conversations")?;
    if base.is_empty() {
        None
    } else {
        Some(base.trim_end_matches('/').to_string())
    }
}

// ============================================================ the client

/// The request/response half of the cloud link.
pub struct Cloud {
    agent: ureq::Agent,
    base: String,
}

impl Default for Cloud {
    fn default() -> Self {
        Self::new()
    }
}

impl Cloud {
    pub fn new() -> Self {
        Cloud {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(HTTP_TIMEOUT)
                .timeout_write(HTTP_TIMEOUT)
                .build(),
            base: CLOUD_BASE.to_string(),
        }
    }

    /// Point the client at a different service. Used by tests and by anyone
    /// running a self-hosted copy.
    pub fn with_base(base: &str) -> Self {
        let mut cloud = Cloud::new();
        cloud.base = base.trim_end_matches('/').to_string();
        cloud
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// One call. A non-2xx status becomes a [`BvError::Message`] carrying the
    /// service's own words, which are the useful part when a key is wrong.
    fn call(&self, key: &str, method: &str, path: &str, body: Option<&Value>) -> BvResult<Value> {
        if key.trim().is_empty() {
            return Err(BvError::msg(
                "ক্লাউড কী দেওয়া হয়নি — সেটিংসে যোগ করুন",
            ));
        }
        let url = self.url(path);
        let request = self
            .agent
            .request(method, &url)
            .set("Authorization", &format!("Bearer {}", key.trim()))
            .set("Accept", "application/json");

        let result = match body {
            Some(value) => request
                .set("Content-Type", "application/json")
                .send_string(&value.to_string()),
            None => request.call(),
        };

        match result {
            Ok(response) => read_json(response),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                let short: String = detail.chars().take(400).collect();
                if code == 401 || code == 403 {
                    Err(BvError::msg("ক্লাউড কী গ্রহণ করা হয়নি"))
                } else {
                    Err(BvError::msg(format!(
                        "ক্লাউড ত্রুটি (HTTP {code}): {}",
                        short.trim()
                    )))
                }
            }
            Err(ureq::Error::Transport(t)) => Err(BvError::msg(format!("সংযোগ ব্যর্থ: {t}"))),
        }
    }

    /// Prove a key by reading the account it belongs to. The cheapest honest
    /// check: no sandbox, no quota.
    pub fn account(&self, key: &str) -> BvResult<Value> {
        self.call(key, "GET", "/users/me", None)
    }

    /// Recent conversations, newest first.
    pub fn list_conversations(&self, key: &str, limit: u32) -> BvResult<Vec<ConversationBrief>> {
        let value = self.call(
            key,
            "GET",
            &format!("/app-conversations/search?limit={limit}"),
            None,
        )?;
        Ok(parse_conversation_list(&value))
    }

    /// One conversation, including its session key and socket host.
    pub fn conversation(&self, key: &str, id: &str) -> BvResult<ConversationBrief> {
        let value = self.call(key, "GET", &format!("/app-conversations?ids={id}"), None)?;
        parse_conversation_list(&value)
            .into_iter()
            .next()
            .ok_or_else(|| BvError::msg("কনভার্সেশন পাওয়া যায়নি"))
    }

    /// Start a conversation. Returns the start task; poll it until it is ready.
    pub fn start_conversation(&self, key: &str, request: &StartRequest) -> BvResult<StartTask> {
        let mut body = json!({
            "title": request.title,
            "initial_message": {
                "role": "user",
                "content": [{ "type": "text", "text": request.prompt }],
            },
        });
        if let Some(suffix) = &request.system_message_suffix {
            if !suffix.trim().is_empty() {
                body["system_message_suffix"] = json!(suffix);
            }
        }
        if let Some(sandbox) = &request.sandbox_id {
            if !sandbox.trim().is_empty() {
                body["sandbox_id"] = json!(sandbox);
            }
        }
        if let Some(model) = &request.model {
            if !model.trim().is_empty() {
                body["llm_model"] = json!(model);
            }
        }
        if !request.client_tools.is_empty() {
            body["client_tools"] = serde_json::to_value(&request.client_tools)?;
        }
        let value = self.call(key, "POST", "/app-conversations", Some(&body))?;
        Ok(parse_start_task(&value))
    }

    /// Poll a start task. `Ok(None)` means it is still working.
    pub fn start_task(&self, key: &str, task_id: &str) -> BvResult<StartTask> {
        let value = self.call(
            key,
            "GET",
            &format!("/app-conversations/start-tasks?ids={task_id}"),
            None,
        )?;
        Ok(parse_start_task(&value))
    }

    /// Queue a message. The account endpoint is used rather than the agent
    /// server's because this one also wakes a paused sandbox.
    pub fn send_message(&self, key: &str, id: &str, text: &str) -> BvResult<()> {
        let body = json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
            "run": true,
        });
        self.call(
            key,
            "POST",
            &format!("/app-conversations/{id}/send-message"),
            Some(&body),
        )?;
        Ok(())
    }

    /// How many events one page may ask for. The service refuses `limit` above
    /// `MAX_EVENTS_PAGE` with a 422, and the caller used to turn that into an
    /// empty transcript — an old chat then looked like it had never happened.
    pub fn page_size(remaining: u32) -> u32 {
        remaining.clamp(1, MAX_EVENTS_PAGE)
    }

    /// Up to `limit` newest events, walking the service's pages.
    ///
    /// A page holds at most `MAX_EVENTS_PAGE` events and the service will not
    /// answer a larger `limit`, so a chat longer than one page only comes back
    /// whole if its `next_page_id` (an offset) is followed. Without this the
    /// transcript silently stopped at the first page.
    pub fn events_window(&self, key: &str, id: &str, limit: u32) -> BvResult<Vec<Value>> {
        let limit = limit.max(1);
        let mut out: Vec<Value> = Vec::new();
        let mut page: Option<u64> = None;
        while (out.len() as u32) < limit {
            let want = Self::page_size(limit - out.len() as u32);
            let mut url = format!(
                "/conversation/{id}/events/search?limit={want}&sort_order=TIMESTAMP_DESC"
            );
            if let Some(p) = page {
                url.push_str(&format!("&page_id={p}"));
            }
            let value = self.call(key, "GET", &url, None)?;
            let batch = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if batch.is_empty() {
                break;
            }
            out.extend(batch);
            match value.get("next_page_id").and_then(Value::as_u64) {
                Some(next) => page = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// History, already normalised and in reading order.
    ///
    /// Only the kinds that belong in a transcript are kept. Liveness signals —
    /// `exec_status`, and the `status` lines that carry ids and state keys — are
    /// dropped: they mean something live but nothing in a scrollback, and left
    /// in they showed up as stray cards.
    pub fn history(&self, key: &str, id: &str, limit: u32) -> BvResult<Vec<AgentEvent>> {
        let mut items = self.events_window(key, id, limit.min(MAX_HISTORY))?;
        items.reverse();
        Ok(items
            .iter()
            .map(normalise_event)
            .filter(|e| matches!(e.kind.as_str(), "message" | "action" | "observation" | "usage" | "error"))
            .collect())
    }

    pub fn resume_sandbox(&self, key: &str, sandbox_id: &str) -> BvResult<()> {
        self.call(
            key,
            "POST",
            &format!("/sandboxes/{sandbox_id}/resume"),
            Some(&json!({})),
        )?;
        Ok(())
    }

    pub fn pause_sandbox(&self, key: &str, sandbox_id: &str) -> BvResult<()> {
        self.call(
            key,
            "POST",
            &format!("/sandboxes/{sandbox_id}/pause"),
            Some(&json!({})),
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, key: &str, id: &str) -> BvResult<()> {
        self.call(key, "DELETE", &format!("/app-conversations/{id}"), None)?;
        Ok(())
    }

    pub fn rename_conversation(&self, key: &str, id: &str, title: &str) -> BvResult<()> {
        self.call(
            key,
            "PATCH",
            &format!("/app-conversations/{id}"),
            Some(&json!({ "title": title })),
        )?;
        Ok(())
    }

    /// Stop the running turn but keep the conversation and its history.
    pub fn interrupt(&self, conversation: &ConversationBrief, session_key: &str) -> BvResult<()> {
        let base = conversation
            .agent_base()
            .ok_or_else(|| BvError::msg("কনভার্সেশনের ঠিকানা নেই"))?;
        let url = format!("{base}/api/conversations/{}/interrupt", conversation.id);
        match self
            .agent
            .post(&url)
            .set("X-Session-API-Key", session_key)
            .set("Content-Type", "application/json")
            .send_string(&json!({}).to_string())
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, _)) => {
                Err(BvError::msg(format!("থামানো যায়নি (HTTP {code})")))
            }
            Err(ureq::Error::Transport(t)) => Err(BvError::msg(format!("সংযোগ ব্যর্থ: {t}"))),
        }
    }

    /// Turn off the service's approval prompts for a conversation.
    ///
    /// The service defaults to asking before risky actions. A person driving
    /// their own machine should not be interrupted by a dialog for every
    /// command — BLACKVAULT's own guard is the thing that decides what is
    /// allowed, and it does so before a tool ever runs.
    pub fn never_confirm(&self, conversation: &ConversationBrief, session_key: &str) -> BvResult<()> {
        let base = conversation
            .agent_base()
            .ok_or_else(|| BvError::msg("কনভার্সেশনের ঠিকানা নেই"))?;
        let url = format!(
            "{base}/api/conversations/{}/confirmation_policy",
            conversation.id
        );
        match self
            .agent
            .post(&url)
            .set("X-Session-API-Key", session_key)
            .set("Content-Type", "application/json")
            .send_string(&json!({ "policy": { "kind": "NeverConfirm" } }).to_string())
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, _)) => {
                Err(BvError::msg(format!("নীতি সেট হয়নি (HTTP {code})")))
            }
            Err(ureq::Error::Transport(t)) => Err(BvError::msg(format!("সংযোগ ব্যর্থ: {t}"))),
        }
    }
}

fn read_json(response: ureq::Response) -> BvResult<Value> {
    let text = response
        .into_string()
        .map_err(|e| BvError::msg(format!("উত্তর পড়া যায়নি: {e}")))?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| BvError::msg(format!("উত্তর বোঝা যায়নি: {e}")))
}

/// Read a conversation record. The service returns a bare array for an id
/// lookup and an object with `items` for a search, so both shapes are accepted.
pub fn parse_conversation_list(value: &Value) -> Vec<ConversationBrief> {
    let items = if let Some(array) = value.as_array() {
        array.clone()
    } else {
        value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    items
        .iter()
        .filter(|item| item.is_object())
        .map(parse_conversation)
        .collect()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_conversation(value: &Value) -> ConversationBrief {
    ConversationBrief {
        id: string_field(value, "id").unwrap_or_default(),
        title: string_field(value, "title").unwrap_or_default(),
        sandbox_id: string_field(value, "sandbox_id"),
        sandbox_status: string_field(value, "sandbox_status"),
        execution_status: string_field(value, "execution_status"),
        llm_model: string_field(value, "llm_model"),
        repository: string_field(value, "selected_repository"),
        updated_at: string_field(value, "updated_at"),
        conversation_url: string_field(value, "conversation_url"),
        session_api_key: string_field(value, "session_api_key"),
    }
}

/// Read a start task. A list lookup wraps it, a create returns it directly.
pub fn parse_start_task(value: &Value) -> StartTask {
    let item = value
        .as_array()
        .and_then(|a| a.first())
        .or_else(|| value.get("items").and_then(|i| i.as_array()).and_then(|a| a.first()))
        .unwrap_or(value);
    StartTask {
        id: string_field(item, "id").unwrap_or_default(),
        app_conversation_id: string_field(item, "app_conversation_id"),
        sandbox_id: string_field(item, "sandbox_id"),
        status: string_field(item, "status"),
        detail: string_field(item, "detail"),
    }
}

// ============================================================ live stream

/// Something the stream can report that is not an event.
#[derive(Debug, Clone)]
pub enum StreamSignal {
    /// The socket is open and subscribed. Reported once per connection so a
    /// caller can act only when events are guaranteed to be received.
    Connected,
    Event(Box<AgentEvent>),
    /// The socket was read and had nothing to say. A chance for the caller to
    /// notice that a turn has gone quiet and stop waiting for it.
    Idle,
    /// The socket dropped; a reconnect is being attempted.
    Reconnecting(String),
    /// The socket closed for good.
    Closed,
}

/// Open the conversation's live event socket and report events until `stop` is
/// set or the turn ends.
///
/// The socket is the reason a turn feels instant: the agent's reasoning, its
/// tool calls and its answer arrive as they are produced instead of being polled
/// for. A dropped socket is retried with a backoff, because losing the socket
/// mid-turn would otherwise freeze the screen while the work carried on.
///
/// The service replays conversation state on connect; that replay is filtered
/// out here so the UI does not redraw history it already has.
pub fn stream_events(
    conversation: &ConversationBrief,
    session_key: &str,
    stop: Arc<AtomicBool>,
    on_signal: &mut dyn FnMut(StreamSignal),
) -> BvResult<()> {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::protocol::Message;

    let url = conversation
        .socket_url()
        .ok_or_else(|| BvError::msg("কনভার্সেশনের সকেট ঠিকানা নেই"))?;

    let mut attempt = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| BvError::msg(format!("সকেট অনুরোধ তৈরি হয়নি: {e}")))?;
        request.headers_mut().insert(
            "X-Session-API-Key",
            session_key
                .parse()
                .map_err(|_| BvError::msg("সেশন কী সঠিক নয়"))?,
        );

        let connected = tungstenite::connect(request);
        let mut socket = match connected {
            Ok((socket, _)) => socket,
            Err(e) => {
                attempt += 1;
                if attempt > 8 {
                    on_signal(StreamSignal::Closed);
                    return Err(BvError::msg(format!("লাইভ সংযোগ ব্যর্থ: {e}")));
                }
                on_signal(StreamSignal::Reconnecting(e.to_string()));
                std::thread::sleep(backoff(attempt));
                continue;
            }
        };

        // A read must not block forever: with no timeout the loop would sit in
        // `read()` after the turn ended and never see `stop`, which is what left
        // the UI showing "running" long after the answer had arrived. A short
        // timeout turns an idle socket into a chance to re-check `stop`.
        if let Err(e) = set_read_timeout(&mut socket, SOCKET_POLL) {
            log::debug!("agent socket: could not set read timeout: {e}");
        }

        // The socket is open and subscribed: anything sent now is guaranteed to
        // be seen.
        on_signal(StreamSignal::Connected);

        attempt = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                on_signal(StreamSignal::Closed);
                return Ok(());
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
                    // The connect-time replay is state, not news.
                    if kind == "ConversationStateUpdateEvent" && value.get("key").is_none() {
                        continue;
                    }
                    let event = normalise_event(&value);
                    if is_renderable(&event) {
                        on_signal(StreamSignal::Event(Box::new(event)));
                    }
                }
                Ok(Message::Binary(bytes)) => {
                    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                        continue;
                    };
                    let event = normalise_event(&value);
                    if is_renderable(&event) {
                        on_signal(StreamSignal::Event(Box::new(event)));
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // An idle socket, not a dead one. Report it so the caller can
                    // notice a quiet turn, then loop back and re-check `stop`.
                    on_signal(StreamSignal::Idle);
                    continue;
                }
                Err(_) => break,
            }
        }

        if stop.load(Ordering::Relaxed) {
            on_signal(StreamSignal::Closed);
            return Ok(());
        }
        attempt += 1;
        if attempt > 8 {
            on_signal(StreamSignal::Closed);
            return Err(BvError::msg("লাইভ সংযোগ বারবার বিচ্ছিন্ন হয়েছে"));
        }
        on_signal(StreamSignal::Reconnecting("সংযোগ আবার জোড়া হচ্ছে".into()));
        std::thread::sleep(backoff(attempt));
    }

    on_signal(StreamSignal::Closed);
    Ok(())
}

/// Exponential backoff, capped so a long outage still retries often enough to
/// recover quickly when the network returns.
pub fn backoff(attempt: u32) -> Duration {
    let millis = 500u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(millis.min(8_000))
}

/// How long the socket read may block before the loop re-checks `stop`.
const SOCKET_POLL: Duration = Duration::from_millis(750);

/// Put a read timeout on the socket underneath a websocket.
///
/// Returns an error rather than panicking when the stream is not a plain TCP
/// stream, so a different backend degrades to a blocking read instead of
/// crashing the turn.
fn set_read_timeout(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    timeout: Duration,
) -> std::io::Result<()> {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.get_mut().set_read_timeout(Some(timeout))
        }
        _ => Ok(()),
    }
}

/// Read a whole body to a string. Kept here so the HTTP client has one place
/// that consumes a response.
pub fn read_to_string(mut reader: impl Read) -> std::io::Result<String> {
    let mut buffer = String::new();
    reader.read_to_string(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_base_is_derived_from_the_conversation_url() {
        let base = agent_base_from_url("https://host.example/api/conversations/abc123");
        assert_eq!(base.as_deref(), Some("https://host.example"));
        assert!(agent_base_from_url("https://host.example/nothing").is_none());
    }

    #[test]
    fn socket_url_switches_scheme_and_keeps_the_host() {
        let conv = ConversationBrief {
            id: "abc".into(),
            conversation_url: Some("https://host.example/api/conversations/abc".into()),
            ..Default::default()
        };
        assert_eq!(
            conv.socket_url().as_deref(),
            Some("wss://host.example/sockets/events/abc")
        );
    }

    #[test]
    fn the_events_page_limit_is_clamped_to_what_the_service_accepts() {
        // The service answers 422 for limit > 100, which the app turned into an
        // empty transcript — an old chat looked like it had never happened.
        assert_eq!(Cloud::page_size(120), 100);
        assert_eq!(Cloud::page_size(1000), 100);
        assert_eq!(Cloud::page_size(50), 50);
        assert_eq!(Cloud::page_size(0), 1);
    }

    #[test]
    fn conversation_list_accepts_both_shapes() {
        let bare = json!([{ "id": "a", "title": "one", "sandbox_status": "RUNNING" }]);
        let wrapped = json!({ "items": [{ "id": "b", "title": "two" }] });
        assert_eq!(parse_conversation_list(&bare)[0].id, "a");
        assert_eq!(parse_conversation_list(&wrapped)[0].id, "b");
        assert!(parse_conversation_list(&json!(null)).is_empty());
    }

    #[test]
    fn running_and_paused_are_recognised() {
        let mut conv = ConversationBrief::default();
        conv.sandbox_status = Some("RUNNING".into());
        assert!(conv.is_running() && !conv.is_paused());
        conv.sandbox_status = Some("PAUSED".into());
        assert!(conv.is_paused() && !conv.is_running());
    }

    #[test]
    fn start_task_reads_a_list_and_a_single_object() {
        let list = json!([{ "id": "t1", "app_conversation_id": "c1", "status": "READY" }]);
        let single = json!({ "id": "t2", "status": "WORKING" });
        let from_list = parse_start_task(&list);
        assert_eq!(from_list.app_conversation_id.as_deref(), Some("c1"));
        assert!(from_list.is_ready());
        let from_single = parse_start_task(&single);
        assert_eq!(from_single.id, "t2");
        assert!(!from_single.is_ready());
        assert!(!from_single.is_failed());
    }

    #[test]
    fn failed_start_tasks_are_recognised() {
        for status in ["ERROR", "FAILED"] {
            let task = parse_start_task(&json!({ "status": status }));
            assert!(task.is_failed());
        }
    }

    #[test]
    fn a_message_event_carries_text_and_reasoning() {
        let raw = json!({
            "id": "e1",
            "kind": "MessageEvent",
            "source": "agent",
            "timestamp": "2026-01-01T00:00:00",
            "llm_message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "hello" }],
                "reasoning_content": "thinking"
            }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "message");
        assert_eq!(event.role, "assistant");
        assert_eq!(event.text, "hello");
        assert_eq!(event.thought, "thinking");
    }

    #[test]
    fn a_user_message_stays_user() {
        let raw = json!({
            "id": "e2",
            "kind": "MessageEvent",
            "source": "user",
            "llm_message": { "role": "user", "content": "hi" }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.role, "user");
        assert_eq!(event.text, "hi");
    }

    #[test]
    fn an_action_summary_prefers_the_real_field() {
        let raw = json!({
            "id": "e3",
            "kind": "ActionEvent",
            "tool_name": "terminal",
            "action": { "command": "ls -la" }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "action");
        assert_eq!(event.tool, "terminal");
        assert_eq!(event.summary, "terminal: ls -la");
    }

    #[test]
    fn execution_status_is_its_own_kind_and_renderable() {
        // It has to survive `is_renderable`, or the turn would never be seen to
        // finish; and it must not be a plain `status`, which is what the socket
        // replays as a stale "idle" on connect.
        let raw = json!({
            "id": "e9",
            "kind": "ConversationStateUpdateEvent",
            "key": "execution_status",
            "value": "finished"
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "exec_status");
        assert_eq!(event.text, "finished");
        assert!(is_renderable(&event));
    }

    #[test]
    fn a_plain_status_is_not_an_execution_status() {
        // Bookkeeping keys (`last_user_message_id`, `full_state`, …) are not
        // status lines: they are dropped rather than shown as stray cards.
        for key in ["last_user_message_id", "full_state"] {
            let raw = json!({
                "id": "e10",
                "kind": "ConversationStateUpdateEvent",
                "key": key,
                "value": "abc"
            });
            let event = normalise_event(&raw);
            assert_eq!(event.kind, "other", "key {key} should not be a card");
            assert!(!is_renderable(&event));
        }
    }

    #[test]
    fn a_tool_call_carries_its_arguments() {
        // The arguments live on the action object, not in the summary. Without
        // them every desktop tool call was attempted with nothing and failed.
        let raw = json!({
            "id": "e11",
            "kind": "ActionEvent",
            "tool_name": "bv_run_command_v1",
            "action": { "command": "echo hi", "kind": "ClientAction_bv_run_command_v1" }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.tool, "bv_run_command_v1");
        assert_eq!(event.arguments["command"], "echo hi");
    }

    #[test]
    fn a_tool_call_without_arguments_is_an_empty_object() {
        let raw = json!({
            "id": "e12",
            "kind": "ActionEvent",
            "tool_name": "bv_system_info_v1"
        });
        let event = normalise_event(&raw);
        assert!(event.arguments.is_object());
        assert!(event.arguments.as_object().unwrap().is_empty());
    }

    #[test]
    fn arguments_can_come_from_the_tool_call_string() {
        let raw = json!({
            "id": "e13",
            "kind": "ActionEvent",
            "tool_name": "bv_open_app_v1",
            "tool_call": { "arguments": "{\"app\":\"Chrome\"}" }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.arguments["app"], "Chrome");
    }

    #[test]
    fn an_observation_reports_failure() {
        let raw = json!({
            "id": "e4",
            "kind": "ObservationEvent",
            "tool_name": "terminal",
            "observation": {
                "is_error": true,
                "content": [{ "type": "text", "text": "boom" }]
            }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "observation");
        assert!(!event.ok);
        assert_eq!(event.text, "boom");
    }

    #[test]
    fn a_stats_update_becomes_a_usage_line() {
        let raw = json!({
            "id": "e5",
            "kind": "ConversationStateUpdateEvent",
            "key": "stats",
            "value": {
                "usage_to_metrics": {
                    "agent": {
                        "accumulated_token_usage": { "prompt_tokens": 1234 },
                        "accumulated_cost": 0.5
                    }
                }
            }
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "usage");
        assert!(event.text.contains("1234"));
    }

    #[test]
    fn errors_become_error_events() {
        let raw = json!({
            "id": "e6",
            "kind": "ConversationErrorEvent",
            "detail": "service unavailable"
        });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "error");
        assert!(!event.ok);
        assert_eq!(event.text, "service unavailable");
    }

    #[test]
    fn an_unknown_event_is_kept_as_other() {
        let raw = json!({ "id": "e7", "kind": "SomethingBrandNew" });
        let event = normalise_event(&raw);
        assert_eq!(event.kind, "other");
        assert!(!is_renderable(&event));
    }

    #[test]
    fn image_blocks_become_a_placeholder() {
        let blocks = json!([
            { "type": "text", "text": "look: " },
            { "type": "image", "image_url": "data:..." }
        ]);
        assert_eq!(blocks_text(&blocks), "look: [image]");
    }

    #[test]
    fn client_tool_specs_are_valid_object_schemas() {
        let spec = ClientToolSpec::new("open_app", "Open an app", &[("app", "string", true)]);
        assert_eq!(spec.parameters["type"], "object");
        assert!(spec.parameters["properties"]["app"].is_object());
        assert_eq!(spec.parameters["required"][0], "app");
    }

    /// Live check against the real service. Ignored by default so the suite
    /// stays offline; run with `cargo test -- --ignored` and a real key.
    #[test]
    #[ignore]
    fn live_account_and_conversation_list() {
        let key = std::env::var("OPENHANDS_API_KEY").expect("OPENHANDS_API_KEY not set");
        let cloud = Cloud::new();
        let account = cloud.account(&key).expect("account call failed");
        assert!(account.get("id").is_some() || account.get("email").is_some());
        let list = cloud.list_conversations(&key, 5).expect("list failed");
        println!("live: account ok, {} conversations", list.len());
        for c in list.iter().take(3) {
            println!("  {} :: {} :: {:?}", c.id, c.title, c.sandbox_status);
        }
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert!(backoff(1) < backoff(2));
        assert!(backoff(3) <= Duration::from_secs(8));
        assert_eq!(backoff(9), Duration::from_secs(8));
    }
}
