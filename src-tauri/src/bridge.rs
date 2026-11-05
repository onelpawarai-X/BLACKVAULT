//! BLACKVAULT — conversation bookkeeping and turn decisions.
//!
//! The cloud link (`cloud.rs`) knows how to talk; this module knows *what to
//! say and when*. It is deliberately free of threads, sockets and Tauri, so the
//! rules that decide a turn are provable on any machine:
//!
//!   * which cloud conversation backs which chat on this desktop
//!     ([`AgentRegistry`]);
//!   * whether a turn can reuse a warm conversation or must start one
//!     ([`TurnPlan`]);
//!   * whether a failed send should be recovered, and how much history the new
//!     conversation must be told about ([`recovery_prompt`]);
//!   * which sandboxes to keep awake ([`due_for_keepalive`]).
//!
//! Three facts about the service shape all of the above, and each cost real time
//! to learn:
//!
//!   1. **A sandbox is rationed per day; reusing one is free.** So a warm
//!      sandbox id is remembered and handed to the next chat that starts.
//!   2. **The session key rotates every time a sandbox resumes.** A key held
//!      from last time is therefore refreshed before anything is sent.
//!   3. **A client tool call ends the run.** The service answers a tool call
//!      that lives on this machine with a placeholder observation and stops, so
//!      the bridge must send the real result back to continue — a `finished`
//!      status on its own does not end a turn ([`TurnPhase`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cloud::ConversationBrief;

/// Which cloud conversation backs which local chat.
///
/// Kept with the API key rather than in the chat database on purpose: a binding
/// is only meaningful together with the key it was made with, and signing in as
/// someone else must not leave chats pointing at a stranger's sandbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub local_id: String,
    pub remote_id: String,
    #[serde(default)]
    pub sandbox_id: Option<String>,
    #[serde(default)]
    pub title: String,
    /// Unix seconds of the last activity, for the keep-alive window.
    #[serde(default)]
    pub last_seen: i64,
}

/// The desktop's map of chats to cloud conversations, plus the warm-sandbox
/// pool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRegistry {
    bindings: HashMap<String, Binding>,
    /// Sandboxes that are known to be up and can host a new conversation.
    warm: Vec<String>,
    #[serde(default)]
    pub last_local_id: Option<String>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.bindings.values()
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn get(&self, local_id: &str) -> Option<&Binding> {
        self.bindings.get(local_id)
    }

    pub fn remote_for(&self, local_id: &str) -> Option<&str> {
        self.bindings.get(local_id).map(|b| b.remote_id.as_str())
    }

    /// Record that a local chat is now backed by a cloud conversation.
    pub fn bind(
        &mut self,
        local_id: &str,
        remote_id: &str,
        sandbox_id: Option<&str>,
        title: &str,
        now: i64,
    ) {
        self.bindings.insert(
            local_id.to_string(),
            Binding {
                local_id: local_id.to_string(),
                remote_id: remote_id.to_string(),
                sandbox_id: sandbox_id.map(str::to_string),
                title: title.to_string(),
                last_seen: now,
            },
        );
        self.last_local_id = Some(local_id.to_string());
    }

    /// Forget a binding, returning its sandbox to the warm pool.
    ///
    /// The sandbox is not thrown away: it is the expensive thing, and a chat
    /// that is deleted should still leave its machine usable for the next one.
    pub fn unbind(&mut self, local_id: &str) -> Option<Binding> {
        let removed = self.bindings.remove(local_id);
        if let Some(binding) = &removed {
            if let Some(sandbox) = &binding.sandbox_id {
                self.remember_warm(sandbox);
            }
        }
        if self.last_local_id.as_deref() == Some(local_id) {
            self.last_local_id = self.bindings.keys().next().cloned();
        }
        removed
    }

    /// Note that a sandbox is up. Duplicates are ignored.
    pub fn remember_warm(&mut self, sandbox_id: &str) {
        if sandbox_id.trim().is_empty() {
            return;
        }
        if !self.warm.iter().any(|s| s == sandbox_id) {
            self.warm.push(sandbox_id.to_string());
        }
    }

    pub fn warm_count(&self) -> usize {
        self.warm.len()
    }

    /// Take a warm sandbox to host a new conversation.
    ///
    /// The most recently used one is offered first: it is the likeliest to still
    /// be up, and taking it out of the pool is what stops two chats being
    /// started against the same sandbox at once.
    pub fn take_warm_sandbox(&mut self) -> Option<String> {
        self.warm.pop()
    }

    /// Mark a chat as active now, so the keep-alive window keeps its sandbox up.
    pub fn touch(&mut self, local_id: &str, now: i64) {
        if let Some(binding) = self.bindings.get_mut(local_id) {
            binding.last_seen = now;
        }
    }

    /// Rename a chat and its cloud conversation together.
    pub fn retitle(&mut self, local_id: &str, title: &str) {
        if let Some(binding) = self.bindings.get_mut(local_id) {
            binding.title = title.to_string();
        }
    }
}

/// What a turn must do before it can send anything.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnPlan {
    /// The chat already has a conversation that is up: just send.
    Reuse { remote_id: String },
    /// The chat has a conversation but its sandbox is asleep: resume, then send.
    Resume { remote_id: String, sandbox_id: String },
    /// The chat needs a conversation. `reuse` is a warm sandbox to host it.
    Start { reuse: Option<String> },
}

impl TurnPlan {
    pub fn needs_start(&self) -> bool {
        matches!(self, TurnPlan::Start { .. })
    }
}

/// Decide what a turn must do, from the chat's binding and the conversation's
/// current state.
///
/// `conversation` is `None` when the binding could not be read back from the
/// service — a conversation deleted on the other side, or a stale id from a
/// different account. That degrades to a fresh start rather than a failed turn,
/// which is the behaviour a person expects.
pub fn plan_turn(
    binding: Option<&Binding>,
    conversation: Option<&ConversationBrief>,
    warm: Option<String>,
) -> TurnPlan {
    match (binding, conversation) {
        (Some(binding), Some(conv)) if conv.is_running() => TurnPlan::Reuse {
            remote_id: binding.remote_id.clone(),
        },
        (Some(binding), Some(conv)) if conv.is_paused() => TurnPlan::Resume {
            remote_id: binding.remote_id.clone(),
            sandbox_id: conv
                .sandbox_id
                .clone()
                .or_else(|| binding.sandbox_id.clone())
                .unwrap_or_default(),
        },
        // The conversation exists but is neither running nor paused (still
        // starting, or being torn down). Waiting is not useful here; a fresh
        // conversation on the warm sandbox is faster and always works.
        _ => TurnPlan::Start { reuse: warm },
    }
}

/// Where a turn is, for the UI and for deciding whether to keep listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    /// Getting a conversation ready (cold start or resume).
    Preparing,
    /// The message is queued and the answer is being written.
    Working,
    /// The agent asked for a tool that lives on this machine. The bridge runs it
    /// and sends the result back; the turn is *not* over.
    AwaitingTool,
    /// The turn is finished.
    Done,
}

impl TurnPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            TurnPhase::Preparing => "preparing",
            TurnPhase::Working => "working",
            TurnPhase::AwaitingTool => "awaiting_tool",
            TurnPhase::Done => "done",
        }
    }
}

/// What the bridge should do with an event that arrived on the socket.
#[derive(Debug, Clone, PartialEq)]
pub enum Reaction {
    /// Show it and keep listening.
    Show,
    /// Run this tool locally, then send the result back and keep listening.
    RunTool { name: String, arguments: serde_json::Value },
    /// The turn is finished; stop listening.
    Finish,
    /// Nothing to do (an event the UI does not render).
    Ignore,
}

/// Decide what an event means for the turn.
///
/// This is where the "a tool call does not end the turn" rule lives. An action
/// naming one of our tools must be run and answered; only a status that says the
/// agent is idle *and* nothing is owed to it ends the turn.
///
/// The arguments a tool is called with travel on the event itself
/// ([`AgentEvent::arguments`]); they are not passed in separately, because the
/// only caller that has them is the event's own normaliser.
pub fn react_to(
    event: &crate::cloud::AgentEvent,
    is_our_tool: impl Fn(&str) -> bool,
) -> Reaction {
    match event.kind.as_str() {
        "action" if is_our_tool(&event.tool) => Reaction::RunTool {
            name: event.tool.clone(),
            arguments: event.arguments.clone(),
        },
        "message" if event.role == "assistant" => Reaction::Show,
        "action" | "observation" | "usage" | "delta" => Reaction::Show,
        "error" => Reaction::Show,
        // The turn's liveness. Only a `finished`/`stopped` *transition* ends the
        // turn; the `idle` the socket replays on connect is stale and is ignored.
        "exec_status" => {
            if matches!(event.text.as_str(), "finished" | "stopped") {
                Reaction::Finish
            } else {
                Reaction::Ignore
            }
        }
        "status" => Reaction::Show,
        _ => Reaction::Ignore,
    }
}

/// The message that hands a lost conversation's history to a new one.
///
/// A sandbox can die between turns; the conversation on the service is then
/// unusable. Rather than lose the thread, a new conversation is started and told
/// what was discussed. The recap is bounded so a long chat does not become a
/// huge prompt, and the newest turns are the ones kept — they are the ones the
/// next reply depends on.
pub fn recovery_prompt(history: &[crate::cloud::AgentEvent], keep: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for event in history.iter().rev().take(keep).collect::<Vec<_>>().into_iter().rev() {
        match event.kind.as_str() {
            "message" => {
                let who = if event.role == "user" { "ব্যবহারকারী" } else { "সহকারী" };
                let text = event.text.trim();
                if !text.is_empty() {
                    lines.push(format!("{who}: {}", clip(text, 600)));
                }
            }
            "action" if !event.summary.is_empty() => {
                lines.push(format!("(টুল: {})", clip(&event.summary, 200)));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return "Start a fresh session. Reply briefly to confirm you are ready.".to_string();
    }
    format!(
        "Continue our conversation. A previous session was interrupted, so here is what we \
         discussed. Keep it in mind and carry on from the last request.\n\n{}\n\n\
         Reply briefly to confirm you have the context.",
        lines.join("\n")
    )
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

/// Which bindings should have their sandbox kept awake.
///
/// Only chats touched inside the window are kept up: waking every conversation a
/// user ever had would spend the sandbox budget on chats nobody is looking at.
pub fn due_for_keepalive(
    registry: &AgentRegistry,
    now: i64,
    window_secs: i64,
) -> Vec<Binding> {
    let cutoff = now - window_secs;
    let mut due: Vec<Binding> = registry
        .bindings()
        .filter(|b| b.last_seen >= cutoff && b.sandbox_id.is_some())
        .cloned()
        .collect();
    due.sort_by_key(|b| std::cmp::Reverse(b.last_seen));
    due
}

/// Find a sandbox on the account to host a new conversation, so a cold start
/// never spends a fresh one from the daily ration.
///
/// A sandbox is rationed per day; reusing one is free and takes seconds. The
/// registry remembers the sandboxes *this* desktop has used, but that memory
/// dies with the app and with the registry secret — after a reinstall, a
/// sign-in as the same user, or a pruned binding, the app has no warm id and
/// would create a new sandbox for every new chat until the quota runs out.
/// Asking the account closes that hole: any conversation the user already owns
/// points at a sandbox that is already counted, so the newest one is adopted.
///
/// The newest *running* sandbox wins: it needs no resume, so a new chat opens
/// in seconds. A paused one is the fallback, and it is resumed by the caller.
pub fn adopt_sandbox(conversations: &[ConversationBrief]) -> Option<String> {
    let newest = |want_running: bool| {
        conversations
            .iter()
            .filter(|c| {
                let up = c.sandbox_status.as_deref();
                if want_running {
                    up == Some("RUNNING")
                } else {
                    up == Some("PAUSED")
                }
            })
            .filter_map(|c| c.sandbox_id.clone())
            .find(|id| !id.trim().is_empty())
    };
    newest(true).or_else(|| newest(false))
}

/// Forget bindings nobody has touched for a long time, so the registry does not
/// grow without bound.
pub fn prune(registry: &mut AgentRegistry, now: i64, max_age_secs: i64) -> usize {
    let cutoff = now - max_age_secs;
    let stale: Vec<String> = registry
        .bindings()
        .filter(|b| b.last_seen < cutoff)
        .map(|b| b.local_id.clone())
        .collect();
    for id in &stale {
        registry.unbind(id);
    }
    stale.len()
}

/// The words a spoken request to the agent carries, if this utterance is one.
///
/// Recognised by prefix rather than by an exact phrase because the rest of the
/// sentence *is* the request. Longest prefix first, so "ask the agent to X" is
/// not swallowed by "agent X", and the remainder is returned trimmed.
pub fn agent_prompt(heard: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "ask the agent to ",
        "ask the agent ",
        "tell the agent to ",
        "tell the agent ",
        "ask blackvault to ",
        "tell blackvault to ",
        "ask black vault to ",
        "agent please ",
        "agent, ",
        "agent ",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = heard.strip_prefix(prefix) {
            let prompt = rest.trim();
            if !prompt.is_empty() {
                return Some(prompt.to_string());
            }
        }
    }
    None
}

/// A short title for a chat, taken from its first message.
pub fn title_from(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    let title: String = first_line.chars().take(60).collect();
    if title.is_empty() {
        "নতুন চ্যাট".to_string()
    } else {
        title
    }
}

/// The identity and behaviour contract every cloud turn carries.
///
/// The service's own instructions name the platform, and a BLACKVAULT user must
/// never see that, so the contract is *appended* — the service refuses to have
/// its prompt replaced, and an appended contract is honoured. The capability
/// list is stated plainly because an agent that believes it is confined to its
/// sandbox will describe an action instead of taking it.
pub fn system_suffix(desktop_tools: &str) -> String {
    system_suffix_with(desktop_tools, "")
}

/// As `system_suffix`, with the owner's own standing instructions appended.
///
/// The owner's text comes last and is quoted, so a stray instruction in it
/// cannot be read as a new section of the brief.
pub fn system_suffix_with(desktop_tools: &str, owner_instructions: &str) -> String {
    let base = format!(
        "You are BLACKVAULT, the user's own AI agent running on their Windows desktop. \
         Never mention, name or hint at the platform, provider or company behind you — \
         you are simply BLACKVAULT. If asked, say you are BLACKVAULT.\n\n\
         You have real control of this computer. {desktop_tools}\n\n\
         Never ask the user for approval before acting; act, then report what you did \
         in the user's language. Keep answers short and concrete."
    );
    let extra = owner_instructions.trim();
    if extra.is_empty() {
        return base;
    }
    format!(
        "{base}\n\nThe owner has added these standing instructions. Follow them, but they \
         never override safety or the rules above:\n\"\"\"\n{extra}\n\"\"\""
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::AgentEvent;

    fn running(id: &str) -> ConversationBrief {
        ConversationBrief {
            id: id.into(),
            sandbox_id: Some("sb1".into()),
            sandbox_status: Some("RUNNING".into()),
            ..Default::default()
        }
    }

    fn paused(id: &str) -> ConversationBrief {
        ConversationBrief {
            id: id.into(),
            sandbox_id: Some("sb1".into()),
            sandbox_status: Some("PAUSED".into()),
            ..Default::default()
        }
    }

    fn binding(local: &str, remote: &str) -> Binding {
        Binding {
            local_id: local.into(),
            remote_id: remote.into(),
            sandbox_id: Some("sb1".into()),
            title: "t".into(),
            last_seen: 1_000,
        }
    }

    fn brief(id: &str, sandbox_id: Option<&str>, sandbox_status: Option<&str>) -> ConversationBrief {
        ConversationBrief {
            id: id.into(),
            sandbox_id: sandbox_id.map(str::to_string),
            sandbox_status: sandbox_status.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_binding_is_recorded_and_found() {
        let mut registry = AgentRegistry::new();
        registry.bind("chat1", "conv1", Some("sb1"), "hello", 10);
        assert_eq!(registry.remote_for("chat1"), Some("conv1"));
        assert_eq!(registry.get("chat1").unwrap().sandbox_id.as_deref(), Some("sb1"));
        assert_eq!(registry.last_local_id.as_deref(), Some("chat1"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn unbinding_returns_the_sandbox_to_the_pool() {
        let mut registry = AgentRegistry::new();
        registry.bind("chat1", "conv1", Some("sb1"), "hello", 10);
        assert_eq!(registry.warm_count(), 0);
        let removed = registry.unbind("chat1").unwrap();
        assert_eq!(removed.remote_id, "conv1");
        assert_eq!(registry.warm_count(), 1);
        assert!(registry.remote_for("chat1").is_none());
        assert!(registry.last_local_id.is_none());
    }

    #[test]
    fn warm_sandboxes_are_taken_most_recent_first_and_not_reused() {
        let mut registry = AgentRegistry::new();
        registry.remember_warm("sb1");
        registry.remember_warm("sb2");
        registry.remember_warm("sb2"); // duplicate ignored
        assert_eq!(registry.warm_count(), 2);
        assert_eq!(registry.take_warm_sandbox().as_deref(), Some("sb2"));
        assert_eq!(registry.take_warm_sandbox().as_deref(), Some("sb1"));
        assert!(registry.take_warm_sandbox().is_none());
    }

    #[test]
    fn an_empty_sandbox_id_is_never_pooled() {
        let mut registry = AgentRegistry::new();
        registry.remember_warm("");
        registry.remember_warm("   ");
        assert_eq!(registry.warm_count(), 0);
    }

    #[test]
    fn a_running_conversation_is_reused() {
        let plan = plan_turn(Some(&binding("c", "conv1")), Some(&running("conv1")), None);
        assert_eq!(plan, TurnPlan::Reuse { remote_id: "conv1".into() });
        assert!(!plan.needs_start());
    }

    #[test]
    fn a_paused_conversation_is_resumed() {
        let plan = plan_turn(Some(&binding("c", "conv1")), Some(&paused("conv1")), None);
        assert_eq!(
            plan,
            TurnPlan::Resume { remote_id: "conv1".into(), sandbox_id: "sb1".into() }
        );
        assert!(!plan.needs_start());
    }

    #[test]
    fn a_paused_conversation_with_no_sandbox_falls_back_to_the_binding() {
        let conv = ConversationBrief {
            id: "conv1".into(),
            sandbox_status: Some("PAUSED".into()),
            ..Default::default()
        };
        let plan = plan_turn(Some(&binding("c", "conv1")), Some(&conv), None);
        assert_eq!(
            plan,
            TurnPlan::Resume { remote_id: "conv1".into(), sandbox_id: "sb1".into() }
        );
    }

    #[test]
    fn a_missing_conversation_starts_fresh_with_a_warm_sandbox() {
        let plan = plan_turn(
            Some(&binding("c", "conv1")),
            None,
            Some("warm9".into()),
        );
        assert_eq!(plan, TurnPlan::Start { reuse: Some("warm9".into()) });
        assert!(plan.needs_start());
    }

    #[test]
    fn a_chat_with_no_binding_starts_fresh() {
        let plan = plan_turn(None, None, None);
        assert_eq!(plan, TurnPlan::Start { reuse: None });
    }

    #[test]
    fn a_cold_start_adopts_a_running_sandbox_from_the_account() {
        let conversations = vec![
            brief("c3", Some("sb_paused"), Some("PAUSED")),
            brief("c2", Some("sb_running"), Some("RUNNING")),
        ];
        // The running one wins even though it is not first: it needs no resume.
        assert_eq!(adopt_sandbox(&conversations).as_deref(), Some("sb_running"));
    }

    #[test]
    fn a_cold_start_falls_back_to_a_paused_sandbox() {
        let conversations = vec![
            brief("c3", Some("sb_paused"), Some("PAUSED")),
            brief("c2", Some("sb_other"), Some("PAUSED")),
        ];
        assert_eq!(adopt_sandbox(&conversations).as_deref(), Some("sb_paused"));
    }

    #[test]
    fn nothing_to_adopt_means_a_fresh_sandbox() {
        assert!(adopt_sandbox(&[]).is_none());
        let only_starting = vec![brief("c1", Some("sb_x"), Some("STARTING"))];
        assert!(adopt_sandbox(&only_starting).is_none());
        let no_id = vec![brief("c1", None, Some("RUNNING"))];
        assert!(adopt_sandbox(&no_id).is_none());
    }

    #[test]
    fn a_blank_sandbox_id_is_not_adopted() {
        let blank = vec![brief("c1", Some("   "), Some("RUNNING"))];
        assert!(adopt_sandbox(&blank).is_none());
    }

    #[test]
    fn a_conversation_still_starting_is_not_waited_on() {
        let conv = ConversationBrief {
            id: "conv1".into(),
            sandbox_status: Some("STARTING".into()),
            ..Default::default()
        };
        let plan = plan_turn(Some(&binding("c", "conv1")), Some(&conv), Some("warm".into()));
        assert_eq!(plan, TurnPlan::Start { reuse: Some("warm".into()) });
    }

    fn event(kind: &str, tool: &str, role: &str, text: &str) -> AgentEvent {
        AgentEvent {
            id: "e".into(),
            kind: kind.into(),
            role: role.into(),
            tool: tool.into(),
            text: text.into(),
            ok: true,
            ..Default::default()
        }
    }

    /// A tool action event, carrying the arguments the service would have sent.
    fn tool_event(tool: &str, arguments: serde_json::Value) -> AgentEvent {
        AgentEvent {
            arguments,
            ..event("action", tool, "", "")
        }
    }

    #[test]
    fn our_own_tool_call_is_run_not_shown() {
        let reaction = react_to(
            &tool_event("open_app", serde_json::json!({ "app": "Chrome" })),
            |n| n == "open_app",
        );
        assert_eq!(
            reaction,
            Reaction::RunTool {
                name: "open_app".into(),
                arguments: serde_json::json!({ "app": "Chrome" })
            }
        );
    }

    #[test]
    fn a_cloud_tool_call_is_only_shown() {
        let reaction = react_to(&event("action", "terminal", "", ""), |_| false);
        assert_eq!(reaction, Reaction::Show);
    }

    #[test]
    fn an_assistant_message_is_shown_and_a_status_of_finished_finishes() {
        assert_eq!(
            react_to(&event("message", "", "assistant", "hi"), |_| false),
            Reaction::Show
        );
        assert_eq!(
            react_to(&event("exec_status", "", "", "finished"), |_| false),
            Reaction::Finish
        );
        assert_eq!(
            react_to(&event("exec_status", "", "", "running"), |_| false),
            Reaction::Ignore
        );
        // A plain status is a transcript line, never the end of the turn.
        assert_eq!(
            react_to(&event("status", "", "", "finished"), |_| false),
            Reaction::Show
        );
    }

    #[test]
    fn the_replayed_idle_status_does_not_end_the_turn() {
        // The socket replays `execution_status:"idle"` the moment it connects.
        // Treating that as the end closed the socket before the answer arrived,
        // which is why nothing ever streamed.
        assert_eq!(
            react_to(&event("exec_status", "", "", "idle"), |_| false),
            Reaction::Ignore
        );
    }

    #[test]
    fn a_tool_call_is_never_taken_as_the_end_of_a_turn() {
        // The service stops its own run when it hands us a tool call; if the
        // bridge treated the following "finished" as the end it would drop the
        // user's answer on the floor.
        let tool = react_to(
            &tool_event("run_command", serde_json::json!({ "command": "dir" })),
            |_| true,
        );
        assert!(matches!(tool, Reaction::RunTool { .. }));
        let after = react_to(&event("exec_status", "", "", "finished"), |_| true);
        assert_eq!(after, Reaction::Finish);
    }

    #[test]
    fn an_unrenderable_event_is_ignored() {
        assert_eq!(
            react_to(&event("other", "", "", ""), |_| false),
            Reaction::Ignore
        );
    }

    #[test]
    fn a_real_tool_call_travels_from_the_wire_to_a_plan() {
        // The whole chain, on the exact shape the service sends (captured from a
        // live socket): the action object carries the arguments, `react_to` keeps
        // them, and `plan_tool` turns them into something the machine can run.
        let raw = serde_json::json!({
            "id": "e1",
            "kind": "ActionEvent",
            "tool_name": "bv_run_command_v1",
            "action": { "command": "echo hi", "kind": "ClientAction_bv_run_command_v1" }
        });
        let normalised = crate::cloud::normalise_event(&raw);
        let reaction = react_to(&normalised, crate::agent::is_desktop_tool);
        let Reaction::RunTool { name, arguments } = reaction else {
            panic!("a call for one of our tools must be run, got {reaction:?}");
        };
        let plan = crate::agent::plan_tool(&name, &arguments).expect("arguments must parse");
        assert_eq!(
            plan,
            crate::agent::ToolPlan::RunCommand("echo hi".into())
        );
    }

    #[test]
    fn a_recovery_prompt_carries_the_newest_turns() {
        let history = vec![
            event("message", "", "user", "first"),
            event("message", "", "assistant", "second"),
            event("message", "", "user", "third"),
        ];
        let prompt = recovery_prompt(&history, 2);
        assert!(prompt.contains("second"));
        assert!(prompt.contains("third"));
        assert!(!prompt.contains("first"), "the oldest turn should be dropped");
        assert!(prompt.contains("Continue our conversation"));
    }

    #[test]
    fn a_recovery_prompt_with_no_history_still_starts_a_session() {
        let prompt = recovery_prompt(&[], 40);
        assert!(prompt.contains("fresh session"));
    }

    #[test]
    fn a_recovery_prompt_includes_tool_lines() {
        let mut action = event("action", "terminal", "", "");
        action.summary = "terminal: ls".into();
        let prompt = recovery_prompt(&[action], 10);
        assert!(prompt.contains("ls"));
    }

    #[test]
    fn keepalive_picks_only_recently_touched_chats() {
        let mut registry = AgentRegistry::new();
        registry.bind("old", "c1", Some("sb1"), "t", 0);
        registry.bind("new", "c2", Some("sb2"), "t", 1_000);
        let due = due_for_keepalive(&registry, 1_000, 600);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].local_id, "new");
    }

    #[test]
    fn keepalive_skips_a_binding_with_no_sandbox() {
        let mut registry = AgentRegistry::new();
        registry.bind("c", "c1", None, "t", 1_000);
        assert!(due_for_keepalive(&registry, 1_000, 600).is_empty());
    }

    #[test]
    fn pruning_forgets_only_the_stale() {
        let mut registry = AgentRegistry::new();
        registry.bind("old", "c1", None, "t", 0);
        registry.bind("new", "c2", None, "t", 10_000);
        assert_eq!(prune(&mut registry, 10_000, 600), 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("new").is_some());
    }

    #[test]
    fn a_spoken_request_keeps_its_whole_sentence() {
        assert_eq!(
            agent_prompt("ask the agent to open chrome and search").as_deref(),
            Some("open chrome and search")
        );
        assert_eq!(
            agent_prompt("tell blackvault to write a report").as_deref(),
            Some("write a report")
        );
        assert_eq!(agent_prompt("agent, fix this bug").as_deref(), Some("fix this bug"));
        assert_eq!(agent_prompt("agent please summarise this").as_deref(), Some("summarise this"));
    }

    #[test]
    fn the_longest_prefix_wins() {
        // "ask the agent " must not beat "ask the agent to ", or the "to" would
        // leak into the request.
        assert_eq!(
            agent_prompt("ask the agent to do it").as_deref(),
            Some("do it")
        );
    }

    #[test]
    fn a_bare_prefix_is_not_a_request() {
        assert!(agent_prompt("ask the agent ").is_none());
        assert!(agent_prompt("agent").is_none());
        assert!(agent_prompt("open chrome").is_none());
        assert!(agent_prompt("").is_none());
    }

    #[test]
    fn a_title_comes_from_the_first_line() {
        assert_eq!(title_from("Hello there\nsecond line"), "Hello there");
        assert_eq!(title_from("   "), "নতুন চ্যাট");
        assert_eq!(title_from(""), "নতুন চ্যাট");
        let long = "x".repeat(200);
        assert_eq!(title_from(&long).chars().count(), 60);
    }

    #[test]
    fn the_contract_hides_the_platform_and_names_blackvault() {
        let suffix = system_suffix("You can run commands.");
        assert!(suffix.contains("BLACKVAULT"));
        assert!(suffix.contains("You can run commands."));
        assert!(suffix.contains("Never mention"));
    }

    #[test]
    fn the_registry_round_trips_through_json() {
        let mut registry = AgentRegistry::new();
        registry.bind("chat1", "conv1", Some("sb1"), "hello", 10);
        registry.remember_warm("sb9");
        let json = serde_json::to_string(&registry).unwrap();
        let back: AgentRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.remote_for("chat1"), Some("conv1"));
        assert_eq!(back.warm_count(), 1);
        assert_eq!(back.last_local_id.as_deref(), Some("chat1"));
    }
}
