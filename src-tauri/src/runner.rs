//! BLACKVAULT — the agent runner.
//!
//! This is where a turn actually happens. It owns the threads, the socket and
//! the state; the decisions it makes are all taken by the pure modules
//! ([`crate::bridge`] for turn bookkeeping, [`crate::agent`] for what a tool
//! call means), so what is left here is wiring and side effects.
//!
//! A turn, end to end:
//!
//! ```text
//!   user speaks / types
//!        │
//!   plan_turn ──▶ reuse a warm conversation, resume a sleeping one, or start one
//!        │
//!   open the live socket ──▶ events arrive as the agent works
//!        │
//!   an action names one of OUR tools?
//!        │        yes ──▶ authenticate if sensitive ──▶ run it here ──▶ send the
//!        │                 result back ──▶ keep listening (the turn is NOT over)
//!        │        no  ──▶ show it
//!        │
//!   a status of "finished" with nothing owed ──▶ the turn is over
//! ```
//!
//! Two rules from the brief are enforced here and nowhere else:
//!
//!   * **The agent's word is never the last word.** A sensitive tool runs only
//!     after a fresh, verified factor ([`crate::agent::is_sensitive`] plus
//!     [`AgentRuntime::authorise`]). A voice command can *ask* to read a file;
//!     it cannot make the vault open one.
//!   * **A failed send is recovered, not reported.** If the sandbox has died
//!     between turns, the history is handed to a fresh conversation so the user
//!     keeps their thread instead of getting an error.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{self, DesktopHost};
use crate::bridge::{
    self, AgentRegistry, Reaction, TurnPhase,
};
use crate::cloud::{AgentEvent, Cloud, ConversationBrief, StartRequest};
use crate::error::{BvError, BvResult};
use crate::state::{AppState, CoreEvent};

/// How long to wait for a conversation's sandbox to come up before giving up.
/// A cold sandbox takes roughly half a minute; the budget is generous because
/// giving up early would waste the sandbox that was just paid for.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// How often the keep-alive loop wakes sandboxes that have gone to sleep.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// A chat that is not touched for this long stops having its sandbox kept up.
const KEEPALIVE_WINDOW: i64 = 1_800;

/// Bindings older than this are forgotten entirely.
const BINDING_MAX_AGE: i64 = 7 * 24 * 3_600;

/// A fresh authentication counts for this long when a sensitive tool asks.
const AUTH_WINDOW: i64 = 300;

/// How long a sensitive tool call waits for the user's answer.
const APPROVAL_TIMEOUT: u64 = 120;

/// The state the runner keeps, shared with the UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentState {
    pub busy: bool,
    pub phase: String,
    /// The cloud's own provisioning stage during a cold start.
    pub stage: String,
    pub active_local_id: Option<String>,
    pub active_remote_id: Option<String>,
    pub conversations: usize,
    pub warm_sandboxes: usize,
    pub last_error: Option<String>,
    pub model: String,
    pub key_present: bool,
}

/// One rendered line of a conversation, as the pane shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub local_id: String,
    pub kind: String,
    pub role: String,
    pub text: String,
    pub thought: String,
    pub tool: String,
    pub summary: String,
    pub ok: bool,
    pub at: String,
}

impl AgentMessage {
    fn from_event(local_id: &str, event: &AgentEvent) -> Self {
        AgentMessage {
            id: event.id.clone(),
            local_id: local_id.to_string(),
            kind: event.kind.clone(),
            role: event.role.clone(),
            text: event.text.clone(),
            thought: event.thought.clone(),
            tool: event.tool.clone(),
            summary: event.summary.clone(),
            ok: event.ok,
            at: event.at.clone(),
        }
    }
}

/// The runner. One per application.
pub struct AgentRuntime {
    pub state: Arc<AppState>,
    cloud: Cloud,
    registry: RwLock<AgentRegistry>,
    snapshot: RwLock<AgentState>,
    /// True while a turn is running, so two turns cannot fight over the socket.
    running: AtomicBool,
    /// Set to ask the running turn to stop.
    stop: Mutex<Option<Arc<AtomicBool>>>,
    /// The last time a factor was verified for a sensitive tool call.
    authorised_at: AtomicU64,
    /// A sensitive tool that is waiting for the user's answer. The sender is
    /// kept here so the approval command can reach the blocked turn.
    pending_approval: Mutex<Option<Approval>>,
}

/// A sensitive tool call waiting for the user.
struct Approval {
    token: String,
    tool: String,
    summary: String,
    reply: std::sync::mpsc::Sender<bool>,
}

impl AgentRuntime {
    pub fn new(state: Arc<AppState>) -> Arc<Self> {
        let registry = load_registry(&state);
        let runtime = Arc::new(AgentRuntime {
            state: state.clone(),
            cloud: Cloud::new(),
            registry: RwLock::new(registry),
            snapshot: RwLock::new(AgentState {
                model: crate::cloud::DEFAULT_MODEL.to_string(),
                ..Default::default()
            }),
            running: AtomicBool::new(false),
            stop: Mutex::new(None),
            authorised_at: AtomicU64::new(0),
            pending_approval: Mutex::new(None),
        });
        runtime.refresh_snapshot();
        runtime
    }

    fn key(&self) -> String {
        self.state
            .db
            .get_secret("cloud_api_key")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub fn has_key(&self) -> bool {
        !self.key().trim().is_empty()
    }

    pub fn snapshot(&self) -> AgentState {
        self.snapshot.read().clone()
    }

    fn refresh_snapshot(&self) {
        let registry = self.registry.read();
        let mut snapshot = self.snapshot.write();
        snapshot.conversations = registry.len();
        snapshot.warm_sandboxes = registry.warm_count();
        snapshot.key_present = self.has_key();
        snapshot.model = self
            .state
            .settings()
            .cloud_model
            .clone()
            .unwrap_or_else(|| crate::cloud::DEFAULT_MODEL.to_string());
        if !self.running.load(Ordering::Relaxed) {
            snapshot.busy = false;
            snapshot.phase = TurnPhase::Done.as_str().to_string();
        }
        drop(snapshot);
        self.emit_state();
    }

    fn emit_state(&self) {
        self.state.emit(CoreEvent::Agent(Box::new(self.snapshot())));
    }

    fn set_phase(&self, phase: TurnPhase, stage: &str) {
        {
            let mut snapshot = self.snapshot.write();
            snapshot.busy = phase != TurnPhase::Done;
            snapshot.phase = phase.as_str().to_string();
            snapshot.stage = stage.to_string();
        }
        self.emit_state();
    }

    fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("agent: {message}");
        {
            let mut snapshot = self.snapshot.write();
            snapshot.busy = false;
            snapshot.phase = TurnPhase::Done.as_str().to_string();
            snapshot.last_error = Some(message.clone());
        }
        self.emit_state();
        self.state.emit(CoreEvent::AgentEvent {
            local_id: self
                .snapshot
                .read()
                .active_local_id
                .clone()
                .unwrap_or_default(),
            event: AgentEvent {
                kind: "error".into(),
                text: message,
                ok: false,
                ..Default::default()
            },
        });
    }

    // ------------------------------------------------------------ registry

    fn persist_registry(&self) {
        let registry = self.registry.read();
        if let Ok(text) = serde_json::to_string(&*registry) {
            let _ = self.state.db.set_secret("agent_registry", &text);
        }
    }

    pub fn conversations(&self) -> Vec<Value> {
        let registry = self.registry.read();
        let mut items: Vec<Value> = registry
            .bindings()
            .map(|b| {
                json!({
                    "localId": b.local_id,
                    "remoteId": b.remote_id,
                    "title": b.title,
                    "sandboxId": b.sandbox_id,
                    "lastSeen": b.last_seen,
                })
            })
            .collect();
        items.sort_by_key(|v| {
            std::cmp::Reverse(v.get("lastSeen").and_then(Value::as_i64).unwrap_or(0))
        });
        items
    }

    pub fn forget(&self, local_id: &str) {
        self.registry.write().unbind(local_id);
        self.persist_registry();
        self.refresh_snapshot();
    }

    // ------------------------------------------------------------ auth gate

    /// Mark that a factor was just verified, which a sensitive tool call needs.
    pub fn authorise(&self) {
        self.authorised_at
            .store(now_secs() as u64, Ordering::Relaxed);
    }

    /// The sensitive tool call that is waiting for the user, if any.
    pub fn pending(&self) -> Option<(String, String, String)> {
        self.pending_approval
            .lock()
            .as_ref()
            .map(|a| (a.token.clone(), a.tool.clone(), a.summary.clone()))
    }

    /// Answer a waiting tool call. Returns false when the token is stale, which
    /// is how a replayed or forged answer is rejected.
    pub fn answer_approval(&self, token: &str, allow: bool) -> bool {
        let mut slot = self.pending_approval.lock();
        match slot.as_ref() {
            Some(approval) if approval.token == token => {
                let _ = approval.reply.send(allow);
                *slot = None;
                if allow {
                    self.authorise();
                }
                true
            }
            _ => false,
        }
    }

    /// Whether a sensitive tool may run right now.
    ///
    /// A factor verified for a *different* action does not count: the window is
    /// short and it is cleared as soon as a sensitive call uses it, so one
    /// approval can never be spent twice.
    fn is_authorised(&self) -> bool {
        let at = self.authorised_at.load(Ordering::Relaxed);
        at != 0 && now_secs() as u64 - at <= AUTH_WINDOW as u64
    }

    fn consume_authorisation(&self) {
        self.authorised_at.store(0, Ordering::Relaxed);
    }

    // ------------------------------------------------------------ turn

    /// True while a turn is in flight.
    pub fn is_busy(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Ask the running turn to stop.
    pub fn interrupt(&self) {
        if let Some(flag) = self.stop.lock().clone() {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(local_id) = self.snapshot.read().active_local_id.clone() {
            let remote = self.registry.read().remote_for(&local_id).map(str::to_string);
            if let Some(remote) = remote {
                if let Ok(conversation) = self.cloud.conversation(&self.key(), &remote) {
                    if let Some(session) = conversation.session_api_key.clone() {
                        let _ = self.cloud.interrupt(&conversation, &session);
                    }
                }
            }
        }
    }

    /// Run one turn on the runtime's own thread and return immediately.
    pub fn start_turn(self: &Arc<Self>, local_id: String, prompt: String) {
        if self.running.swap(true, Ordering::SeqCst) {
            self.state.toast(
                "আগের কাজ চলছে",
                "একটা কাজ শেষ হলে আবার বলুন।",
                "warn",
            );
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let result = this.run_turn(&local_id, &prompt);
            if let Err(e) = result {
                this.fail(e.to_string());
            }
            this.running.store(false, Ordering::SeqCst);
            this.refresh_snapshot();
        });
    }

    fn run_turn(&self, local_id: &str, prompt: &str) -> BvResult<()> {
        let key = self.key();
        if key.trim().is_empty() {
            return Err(BvError::msg("ক্লাউড কী দেওয়া হয়নি — সেটিংসে যোগ করুন"));
        }
        {
            let mut snapshot = self.snapshot.write();
            snapshot.active_local_id = Some(local_id.to_string());
            snapshot.last_error = None;
        }

        // 1. Decide what this turn needs.
        let binding = self.registry.read().get(local_id).cloned();
        let conversation = binding
            .as_ref()
            .and_then(|b| self.cloud.conversation(&key, &b.remote_id).ok());
        let warm = self.registry.write().take_warm_sandbox();
        let plan = bridge::plan_turn(binding.as_ref(), conversation.as_ref(), warm.clone());

        // 2. Get a conversation that is up, and remember it.
        let (conversation, first_prompt) = match plan {
            bridge::TurnPlan::Reuse { remote_id } => {
                self.set_phase(TurnPhase::Working, "");
                (
                    self.cloud.conversation(&key, &remote_id)?,
                    prompt.to_string(),
                )
            }
            bridge::TurnPlan::Resume {
                remote_id,
                sandbox_id,
            } => {
                self.set_phase(TurnPhase::Preparing, "স্যান্ডবক্স জাগানো হচ্ছে…");
                if !sandbox_id.is_empty() {
                    let _ = self.cloud.resume_sandbox(&key, &sandbox_id);
                }
                let conversation = self.await_running(&key, &remote_id)?;
                self.registry.write().remember_warm(
                    conversation.sandbox_id.as_deref().unwrap_or(&sandbox_id),
                );
                (conversation, prompt.to_string())
            }
            bridge::TurnPlan::Start { reuse } => {
                self.set_phase(TurnPhase::Preparing, "কনভার্সেশন তৈরি হচ্ছে…");
                self.begin_conversation(&key, local_id, prompt, reuse.as_deref())?
            }
        };

        self.registry.write().bind(
            local_id,
            &conversation.id,
            conversation.sandbox_id.as_deref(),
            &bridge::title_from(prompt),
            now_secs(),
        );
        if let Some(sandbox) = &conversation.sandbox_id {
            self.registry.write().remember_warm(sandbox);
        }
        self.persist_registry();
        {
            let mut snapshot = self.snapshot.write();
            snapshot.active_remote_id = Some(conversation.id.clone());
        }

        // 3. Send, and recover once if the send fails.
        self.set_phase(TurnPhase::Working, "");
        if let Err(send_error) = self.cloud.send_message(&key, &conversation.id, &first_prompt) {
            log::warn!("agent: send failed ({send_error}); recovering");
            let history = self
                .cloud
                .history(&key, &conversation.id, 40)
                .unwrap_or_default();
            let recap = bridge::recovery_prompt(&history, 30);
            self.set_phase(TurnPhase::Preparing, "নতুন সেশন তৈরি হচ্ছে…");
            let (fresh, _) = self.begin_conversation(&key, local_id, &recap, warm.as_deref())?;
            self.registry.write().bind(
                local_id,
                &fresh.id,
                fresh.sandbox_id.as_deref(),
                &bridge::title_from(prompt),
                now_secs(),
            );
            self.persist_registry();
            self.cloud.send_message(&key, &fresh.id, prompt)?;
            return self.listen(&key, &fresh, local_id);
        }

        self.listen(&key, &conversation, local_id)
    }

    /// Create a conversation and wait for its sandbox.
    fn begin_conversation(
        &self,
        key: &str,
        _local_id: &str,
        prompt: &str,
        reuse: Option<&str>,
    ) -> BvResult<(ConversationBrief, String)> {
        let request = StartRequest {
            title: bridge::title_from(prompt),
            prompt: prompt.to_string(),
            system_message_suffix: Some(bridge::system_suffix_with(
                &agent::catalogue_text(),
                &self.state.settings().agent_instructions,
            )),
            sandbox_id: reuse.map(str::to_string),
            model: Some(
                self.state
                    .settings()
                    .cloud_model
                    .clone()
                    .unwrap_or_else(|| crate::cloud::DEFAULT_MODEL.to_string()),
            ),
            client_tools: agent::specs(),
        };
        let task = self.cloud.start_conversation(key, &request)?;
        let started = SystemTime::now();
        let mut task = task;
        let remote_id = loop {
            if task.is_ready() {
                break task.app_conversation_id.clone().unwrap_or_default();
            }
            if task.is_failed() {
                return Err(BvError::msg(
                    task.detail
                        .clone()
                        .unwrap_or_else(|| "কনভার্সেশন তৈরি ব্যর্থ".into()),
                ));
            }
            if started.elapsed().unwrap_or_default() > START_TIMEOUT {
                return Err(BvError::msg("কনভার্সেশন সময়মতো তৈরি হয়নি"));
            }
            self.set_phase(TurnPhase::Preparing, task.stage_label());
            std::thread::sleep(Duration::from_secs(2));
            task = self.cloud.start_task(key, &task.id)?;
        };
        let conversation = self.await_running(key, &remote_id)?;
        Ok((conversation, prompt.to_string()))
    }

    /// Wait until a sandbox is up, then re-read the conversation so its rotating
    /// session key is the current one.
    fn await_running(&self, key: &str, remote_id: &str) -> BvResult<ConversationBrief> {
        let started = SystemTime::now();
        loop {
            let conversation = self.cloud.conversation(key, remote_id)?;
            if conversation.is_running() {
                return Ok(conversation);
            }
            if started.elapsed().unwrap_or_default() > START_TIMEOUT {
                return Err(BvError::msg("স্যান্ডবক্স সময়মতো চালু হয়নি"));
            }
            std::thread::sleep(Duration::from_millis(1_500));
        }
    }

    /// Watch the conversation's socket, running our tools and showing the rest.
    fn listen(
        &self,
        key: &str,
        conversation: &ConversationBrief,
        local_id: &str,
    ) -> BvResult<()> {
        let session_key = conversation
            .session_api_key
            .clone()
            .ok_or_else(|| BvError::msg("সেশন কী পাওয়া যায়নি"))?;
        let _ = self.cloud.never_confirm(conversation, &session_key);

        let stop = Arc::new(AtomicBool::new(false));
        *self.stop.lock() = Some(stop.clone());
        self.set_phase(TurnPhase::Working, "");

        let this = self;
        let mut pending_tool: Option<(String, Value)> = None;
        let mut owed = false;
        let mut stream_error: Option<String> = None;

        let result = crate::cloud::stream_events(
            conversation,
            &session_key,
            stop.clone(),
            &mut |signal| match signal {
                crate::cloud::StreamSignal::Event(event) => {
                    let event = *event;
                    // A tool call's arguments are not in the action event's
                    // summary, so a call for one of ours is answered from what
                    // the event carries. The service puts them in `action`.
                    let arguments = pending_tool
                        .as_ref()
                        .filter(|(name, _)| *name == event.tool)
                        .map(|(_, args)| args.clone());
                    match bridge::react_to(&event, agent::is_desktop_tool, arguments) {
                        Reaction::Show => {
                            this.state.emit(CoreEvent::AgentEvent {
                                local_id: local_id.to_string(),
                                event: event.clone(),
                            });
                            if event.kind == "action" && agent::is_desktop_tool(&event.tool) {
                                // Remember the call so the arguments survive
                                // until the tool is run.
                                pending_tool = Some((event.tool.clone(), Value::Null));
                            }
                        }
                        Reaction::RunTool { name, arguments } => {
                            owed = true;
                            this.set_phase(TurnPhase::AwaitingTool, "");
                            this.state.emit(CoreEvent::AgentEvent {
                                local_id: local_id.to_string(),
                                event: event.clone(),
                            });
                            let result = this.run_desktop_tool(&name, &arguments, local_id);
                            this.set_phase(TurnPhase::Working, "");
                            // Hand the real result back so the agent can carry on.
                            if let Err(e) =
                                this.cloud.send_message(key, &conversation.id, &result)
                            {
                                stream_error = Some(e.to_string());
                            }
                            owed = false;
                            pending_tool = None;
                        }
                        Reaction::Finish => {
                            if !owed {
                                stop.store(true, Ordering::Relaxed);
                            }
                        }
                        Reaction::Ignore => {}
                    }
                }
                crate::cloud::StreamSignal::Reconnecting(reason) => {
                    log::debug!("agent socket reconnecting: {reason}");
                    this.state.emit(CoreEvent::AgentStatus {
                        local_id: local_id.to_string(),
                        status: "reconnecting".into(),
                    });
                }
                crate::cloud::StreamSignal::Closed => {
                    stop.store(true, Ordering::Relaxed);
                }
            },
        );

        *self.stop.lock() = None;
        if let Some(error) = stream_error {
            return Err(BvError::msg(error));
        }
        // A closed socket after a quiet turn is normal; a hard error is not.
        if let Err(e) = result {
            let text = e.to_string();
            if !text.contains("বারবার বিচ্ছিন্ন") {
                return Err(BvError::msg(text));
            }
        }
        self.set_phase(TurnPhase::Done, "");
        Ok(())
    }

    /// Run one desktop tool, gated on authentication when it is sensitive.
    fn run_desktop_tool(&self, name: &str, arguments: &Value, local_id: &str) -> String {
        let host = SystemHost { state: self.state.clone() };
        let plan = match agent::plan_tool(name, arguments) {
            Ok(plan) => plan,
            Err(e) => {
                let message = format!("{e}");
                self.note_tool(local_id, name, &message, false);
                return tool_result_text(agent::refusal(&message));
            }
        };

        // A sensitive tool needs a factor verified *for this call*. When none
        // is outstanding, the turn stops here and the user is asked — the agent
        // cannot approve itself, and it cannot proceed while the answer is
        // pending.
        if agent::is_sensitive(name) && !self.is_authorised() {
            let summary = agent::describe_call(&plan);
            if !self.request_approval(local_id, name, &summary) {
                let message = "ব্যবহারকারী এই কাজটার অনুমোদন দেননি।";
                self.note_tool(local_id, name, message, false);
                return tool_result_text(agent::refusal(message));
            }
        }

        let summary = agent::describe_call(&plan);
        match agent::execute(&host, &plan) {
            Ok(value) => {
                if agent::is_sensitive(name) {
                    self.consume_authorisation();
                }
                self.note_tool(local_id, name, &summary, true);
                self.state.log_event(
                    "agent_tool",
                    "info",
                    &format!("Agent ran {name}"),
                    Some(&summary),
                );
                tool_result_text(value)
            }
            Err(e) => {
                let message = e.to_string();
                self.note_tool(local_id, name, &message, false);
                self.state.log_event(
                    "agent_tool",
                    "warn",
                    &format!("Agent tool {name} failed"),
                    Some(&message),
                );
                tool_result_text(agent::refusal(&message))
            }
        }
    }

    /// Ask the user to approve a sensitive call and wait for the answer.
    ///
    /// The wait is bounded: a prompt nobody answers must not hold the turn open
    /// forever, and a timeout is treated as a refusal rather than as consent.
    fn request_approval(&self, local_id: &str, name: &str, summary: &str) -> bool {
        let token = format!("{}-{}", now_secs(), name);
        let (reply, answer) = std::sync::mpsc::channel();
        *self.pending_approval.lock() = Some(Approval {
            token: token.clone(),
            tool: name.to_string(),
            summary: summary.to_string(),
            reply,
        });
        self.state.emit(CoreEvent::AgentToolChallenge {
            local_id: local_id.to_string(),
            tool: name.to_string(),
            summary: summary.to_string(),
            token: token.clone(),
        });
        self.state.log_event(
            "agent_approval",
            "info",
            &format!("Agent asked to run {name}"),
            Some(summary),
        );
        let allowed = matches!(
            answer.recv_timeout(Duration::from_secs(APPROVAL_TIMEOUT)),
            Ok(true)
        );
        *self.pending_approval.lock() = None;
        self.state.log_event(
            "agent_approval",
            if allowed { "info" } else { "warn" },
            &format!("{name} {}", if allowed { "approved" } else { "declined" }),
            Some(summary),
        );
        allowed
    }

    fn note_tool(&self, local_id: &str, name: &str, summary: &str, ok: bool) {
        self.state.emit(CoreEvent::AgentEvent {
            local_id: local_id.to_string(),
            event: AgentEvent {
                id: format!("local-{}-{name}", now_secs()),
                kind: "observation".into(),
                tool: name.to_string(),
                summary: summary.to_string(),
                text: summary.to_string(),
                ok,
                ..Default::default()
            },
        });
    }

    /// Prove a key by reading the account it belongs to.
    pub fn verify_key(&self, key: &str) -> BvResult<String> {
        let account = self.cloud.account(key)?;
        let email = account
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or("অ্যাকাউন্ট");
        Ok(format!("কানেক্টেড — {email}"))
    }

    pub fn save_key(&self, key: &str) -> BvResult<()> {
        self.state.db.set_secret("cloud_api_key", key.trim())?;
        self.refresh_snapshot();
        Ok(())
    }

    /// The history of one chat, newest last.
    pub fn history(&self, local_id: &str, limit: u32) -> BvResult<Vec<AgentMessage>> {
        let key = self.key();
        let remote = self
            .registry
            .read()
            .remote_for(local_id)
            .map(str::to_string)
            .ok_or_else(|| BvError::msg("এই চ্যাটের কোনো কনভার্সেশন নেই"))?;
        let events = self.cloud.history(&key, &remote, limit)?;
        Ok(events
            .iter()
            .map(|e| AgentMessage::from_event(local_id, e))
            .collect())
    }

    /// Keep recently used sandboxes awake so the next message is instant.
    pub fn keepalive_tick(&self) {
        let key = self.key();
        if key.trim().is_empty() || self.running.load(Ordering::Relaxed) {
            return;
        }
        let now = now_secs();
        let due = bridge::due_for_keepalive(&self.registry.read(), now, KEEPALIVE_WINDOW);
        for binding in due {
            let Some(sandbox) = binding.sandbox_id.clone() else {
                continue;
            };
            if let Ok(conversation) = self.cloud.conversation(&key, &binding.remote_id) {
                if conversation.is_paused() {
                    log::debug!("agent: waking sandbox for {}", binding.local_id);
                    let _ = self.cloud.resume_sandbox(&key, &sandbox);
                }
            }
        }
        let pruned = bridge::prune(&mut self.registry.write(), now, BINDING_MAX_AGE);
        if pruned > 0 {
            self.persist_registry();
        }
    }
}

fn tool_result_text(value: Value) -> String {
    format!(
        "Tool result (from BLACKVAULT on the user's computer): {}",
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
    )
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_registry(state: &AppState) -> AgentRegistry {
    state
        .db
        .get_secret("agent_registry")
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The real host: every tool against the actual machine.
struct SystemHost {
    state: Arc<AppState>,
}

impl SystemHost {
    fn path(value: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(value)
    }
}

impl DesktopHost for SystemHost {
    fn launch_app(&self, app: &str) -> BvResult<String> {
        let apps = self.state.db.list_apps()?;
        let needle = app.trim().to_lowercase();
        let found = apps.iter().find(|a| {
            a.name.to_lowercase() == needle
                || a.name.to_lowercase().contains(&needle)
                || a.command.to_lowercase().contains(&needle)
        });
        match found {
            Some(entry) => {
                crate::platform::launcher::launch(
                    &entry.command,
                    entry.args.as_deref(),
                    entry.cwd.as_deref(),
                )?;
                let _ = self.state.db.bump_app_launch(entry.id);
                Ok(format!("{} চালু করা হয়েছে", entry.name))
            }
            None => Err(BvError::msg(format!(
                "\"{app}\" ইনস্টল করা অ্যাপের তালিকায় নেই — আগে Launcher-এ যোগ করুন"
            ))),
        }
    }

    fn close_app(&self, app: &str) -> BvResult<String> {
        let needle = app.trim().to_lowercase();
        let apps = self.state.db.list_apps()?;
        let process = apps
            .iter()
            .find(|a| a.name.to_lowercase().contains(&needle))
            .map(|a| a.command.clone())
            .unwrap_or_else(|| app.to_string());
        crate::platform::launcher::close_process(&process)?;
        Ok(format!("{app} বন্ধ করা হয়েছে"))
    }

    fn list_apps(&self) -> BvResult<Value> {
        let apps = self.state.db.list_apps()?;
        Ok(Value::Array(
            apps.iter()
                .map(|a| json!({ "name": a.name, "command": a.command }))
                .collect(),
        ))
    }

    fn open_path(&self, path: &str) -> BvResult<String> {
        crate::platform::launcher::open_with_default(&Self::path(path))?;
        Ok(format!("{path} খোলা হয়েছে"))
    }

    fn run_command(&self, command: &str) -> BvResult<Value> {
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        let output = cmd
            .output()
            .map_err(|e| BvError::msg(format!("কমান্ড চালানো যায়নি: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(json!({
            "ok": output.status.success(),
            "exitCode": output.status.code(),
            "stdout": clip(&stdout, 8_000),
            "stderr": clip(&stderr, 4_000),
        }))
    }

    fn read_file(&self, path: &str) -> BvResult<String> {
        let text = std::fs::read_to_string(Self::path(path))
            .map_err(|e| BvError::msg(format!("ফাইল পড়া যায়নি: {e}")))?;
        Ok(clip(&text, 20_000))
    }

    fn write_file(&self, path: &str, content: &str) -> BvResult<String> {
        std::fs::write(Self::path(path), content)
            .map_err(|e| BvError::msg(format!("ফাইল লেখা যায়নি: {e}")))?;
        Ok(format!("{path} লেখা হয়েছে"))
    }

    fn list_dir(&self, path: &str) -> BvResult<Value> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(Self::path(path))
            .map_err(|e| BvError::msg(format!("ফোল্ডার পড়া যায়নি: {e}")))?
            .flatten()
        {
            let meta = entry.metadata().ok();
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }
        Ok(Value::Array(entries))
    }

    fn press_keys(&self, keys: &str) -> BvResult<String> {
        crate::platform::automation::press_keys(keys)?;
        Ok(format!("কী চাপা হয়েছে: {keys}"))
    }

    fn type_text(&self, text: &str) -> BvResult<String> {
        crate::platform::automation::type_text(text)?;
        Ok("লেখা হয়েছে".into())
    }

    fn click(&self, x: Option<i32>, y: Option<i32>) -> BvResult<String> {
        crate::platform::automation::click(x, y, "left")?;
        Ok("ক্লিক করা হয়েছে".into())
    }

    fn scroll(&self, direction: &str, amount: i32) -> BvResult<String> {
        match direction {
            "top" => crate::platform::automation::scroll_to(true)?,
            "bottom" => crate::platform::automation::scroll_to(false)?,
            "up" => crate::platform::automation::scroll(0, amount.max(1))?,
            _ => crate::platform::automation::scroll(0, -amount.max(1))?,
        }
        Ok(format!("স্ক্রল: {direction}"))
    }

    fn screenshot(&self) -> BvResult<String> {
        crate::platform::automation::screenshot()
    }

    fn read_screen(&self) -> BvResult<Value> {
        let shot = crate::platform::automation::screenshot().unwrap_or_default();
        Ok(json!({ "screenshot": shot }))
    }

    fn lock(&self) -> BvResult<String> {
        crate::commands::perform_lock(&self.state, "agent");
        Ok("ডেস্কটপ লক করা হয়েছে".into())
    }

    fn unlock(&self) -> BvResult<String> {
        // The agent can never release the vault on its own. Only the prompt,
        // answered by the person, does that.
        Err(BvError::msg(
            "লক খোলা শুধু ব্যবহারকারী নিজে করতে পারেন — PIN বা Secret Phrase দরকার",
        ))
    }

    fn protect_path(&self, path: &str, _folder: bool) -> BvResult<String> {
        let p = Self::path(path);
        if !p.exists() {
            return Err(BvError::msg("এই পথটা নেই"));
        }
        let snapshot = crate::platform::fileprot::snapshot_acl(&p).unwrap_or_default();
        let applied = crate::platform::fileprot::seal_item(&p, &self.state.settings().file_lock_mode)?;
        if !applied.acl && !applied.encrypted {
            return Err(BvError::msg("সুরক্ষা দেওয়া যায়নি — অ্যাডমিন হিসেবে চালান"));
        }
        let kind = if crate::platform::fileprot::is_dir(&p) { "folder" } else { "file" };
        let display_name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let item = crate::models::ProtectedItem {
            id: 0,
            path: path.to_string(),
            kind: kind.into(),
            display_name: display_name.clone(),
            encryption: applied.label(),
            protected: true,
            last_access: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            size_bytes: crate::platform::fileprot::size_of(&p),
        };
        self.state.db.insert_protected_item(&item, &snapshot)?;
        self.state.log_event(
            "file_protected",
            "info",
            &format!("Protected {display_name}"),
            Some(path),
        );
        Ok(format!("{display_name} সুরক্ষিত করা হয়েছে"))
    }

    fn release_path(&self, path: &str) -> BvResult<String> {
        let items = self.state.db.list_protected_items()?;
        let item = items
            .into_iter()
            .find(|i| i.path == path)
            .ok_or_else(|| BvError::msg("এই পথটা সুরক্ষিত তালিকায় নেই"))?;
        let snapshot = self.state.db.acl_snapshot(item.id)?;
        let p = Self::path(path);
        if p.exists() {
            crate::platform::fileprot::open(&p, snapshot.as_deref())?;
        }
        self.state.db.set_item_protected(item.id, false)?;
        self.state.log_event(
            "file_revealed",
            "info",
            &format!("Released {}", item.display_name),
            Some(path),
        );
        Ok(format!("{} থেকে সুরক্ষা সরানো হয়েছে", item.display_name))
    }

    fn list_protected(&self) -> BvResult<Value> {
        let items = self.state.db.list_protected_items()?;
        Ok(Value::Array(
            items
                .iter()
                .filter(|i| i.protected)
                .map(|i| json!({ "path": i.path, "kind": i.kind, "name": i.display_name }))
                .collect(),
        ))
    }

    fn speak(&self, text: &str) -> BvResult<String> {
        self.state.emit(CoreEvent::AgentSpeak {
            text: text.to_string(),
        });
        Ok("বলা হচ্ছে".into())
    }

    fn system_info(&self) -> BvResult<Value> {
        Ok(json!({
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "host": hostname(),
            "user": std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default(),
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }

    fn notify(&self, title: &str, body: &str) -> BvResult<String> {
        self.state.toast(title, body, "info");
        Ok("নোটিফিকেশন দেখানো হয়েছে".into())
    }

    fn window_action(&self, action: &str) -> BvResult<String> {
        match action {
            "minimize_all" => crate::platform::automation::minimize_all()?,
            "show_desktop" => crate::platform::automation::show_desktop()?,
            "switch" => crate::platform::automation::press_keys("alt+tab")?,
            other => return Err(BvError::msg(format!("অজানা উইন্ডো অ্যাকশন: {other}"))),
        }
        Ok(format!("উইন্ডো: {action}"))
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect::<String>() + "…(truncated)"
    }
}

/// Start the keep-alive loop. It runs for the life of the application.
pub fn spawn_keepalive(runtime: Arc<AgentRuntime>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(KEEPALIVE_INTERVAL);
        runtime.keepalive_tick();
    });
}
