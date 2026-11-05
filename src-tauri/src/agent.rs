//! BLACKVAULT — the desktop tool catalogue.
//!
//! This is the list of things the agent may do *on this machine*. It is the
//! desktop counterpart of the phone's device tools: the cloud agent is handed
//! these schemas when a conversation starts, calls them by name, and the call is
//! executed here — on the real machine, with the real screen, the real files and
//! the real terminal.
//!
//! The module is deliberately split into a **pure** half and a **host** half:
//!
//!   * the catalogue, the argument validation and the [`ToolPlan`] a call turns
//!     into are pure and fully unit-tested, so the decision "what is the agent
//!     asking for?" is proven on any machine;
//!   * [`execute`] takes a [`DesktopHost`], so the same plan can be run against
//!     the real system or against a test double.
//!
//! Two rules are enforced here rather than in the UI, because a rule that lives
//! in the UI is a rule the agent can talk its way around:
//!
//!   1. **A sensitive tool never runs without a verified factor.** Reading a
//!      file, running a command and releasing a protected item are gated on a
//!      fresh authentication ([`is_sensitive`]). The agent may *ask*; only the
//!      person may *authorise*.
//!   2. **Unknown tools are refused, not guessed at.** A name that is not in the
//!      catalogue is an error the agent can read and correct, never a silent
//!      no-op that looks like success.

use serde_json::{json, Value};

use crate::cloud::ClientToolSpec;
use crate::error::{BvError, BvResult};

/// One tool the desktop can run.
#[derive(Debug, Clone, Copy)]
pub struct DesktopTool {
    pub name: &'static str,
    pub description: &'static str,
    /// `(parameter, json type, required)`
    pub params: &'static [(&'static str, &'static str, bool)],
}

const fn tool(
    name: &'static str,
    description: &'static str,
    params: &'static [(&'static str, &'static str, bool)],
) -> DesktopTool {
    DesktopTool {
        name,
        description,
        params,
    }
}

/// The whole catalogue.
pub const DESKTOP_TOOLS: &[DesktopTool] = &[
    tool(
        "bv_open_app",
        "Open an application on this computer by name (e.g. Chrome, Notepad, Visual Studio Code). \
         Use bv_list_apps first if you are unsure of the name.",
        &[("app", "string", true)],
    ),
    tool(
        "bv_close_app",
        "Close a running application on this computer by name.",
        &[("app", "string", true)],
    ),
    tool(
        "bv_list_apps",
        "List the applications installed on this computer that BLACKVAULT can launch.",
        &[],
    ),
    tool(
        "bv_open_path",
        "Open a file or folder on this computer with its default application.",
        &[("path", "string", true)],
    ),
    tool(
        "bv_run_command",
        "Run a shell command on this computer and return its output and exit code. \
         Use this for anything the other tools do not cover.",
        &[("command", "string", true)],
    ),
    tool(
        "bv_read_file",
        "Read a text file on this computer. Returns the contents, truncated if very large.",
        &[("path", "string", true)],
    ),
    tool(
        "bv_write_file",
        "Create or overwrite a text file on this computer.",
        &[("path", "string", true), ("content", "string", true)],
    ),
    tool(
        "bv_list_dir",
        "List the entries of a folder on this computer.",
        &[("path", "string", true)],
    ),
    tool(
        "bv_press_keys",
        "Press a key or key combination, e.g. \"ctrl+s\", \"alt+tab\", \"enter\", \"media_next\".",
        &[("keys", "string", true)],
    ),
    tool(
        "bv_type_text",
        "Type text into whatever window currently has focus.",
        &[("text", "string", true)],
    ),
    tool(
        "bv_click",
        "Click the mouse. Give x and y to click a point, or omit them to click where the pointer is.",
        &[("x", "integer", false), ("y", "integer", false)],
    ),
    tool(
        "bv_scroll",
        "Scroll the focused window. direction is \"up\", \"down\", \"top\" or \"bottom\".",
        &[("direction", "string", true), ("amount", "integer", false)],
    ),
    tool(
        "bv_screenshot",
        "Take a screenshot of this computer's screen and return the file path.",
        &[],
    ),
    tool(
        "bv_read_screen",
        "Describe what is currently on screen: the active window title and a screenshot path.",
        &[],
    ),
    tool(
        "bv_lock_pc",
        "Lock this computer with BLACKVAULT's full-screen privacy lock.",
        &[],
    ),
    tool(
        "bv_unlock_pc",
        "Release BLACKVAULT's lock. Requires the user to authenticate.",
        &[],
    ),
    tool(
        "bv_protect_path",
        "Protect a file or folder so it needs authentication to open. \
         Set folder to true for a directory.",
        &[("path", "string", true), ("folder", "boolean", false)],
    ),
    tool(
        "bv_release_path",
        "Remove protection from a file or folder. Requires the user to authenticate.",
        &[("path", "string", true)],
    ),
    tool(
        "bv_list_protected",
        "List the files and folders that are currently protected.",
        &[],
    ),
    tool(
        "bv_speak",
        "Speak text aloud through BLACKVAULT's voice.",
        &[("text", "string", true)],
    ),
    tool(
        "bv_system_info",
        "Report this computer's platform, host name, user name and BLACKVAULT version.",
        &[],
    ),
    tool(
        "bv_notify",
        "Show a desktop notification.",
        &[("title", "string", true), ("body", "string", true)],
    ),
    tool(
        "bv_window_action",
        "Arrange windows. action is \"minimize_all\", \"show_desktop\" or \"switch\".",
        &[("action", "string", true)],
    ),
];

/// Tools that touch data or the system in a way that must be authorised by the
/// person, not merely requested by the agent.
///
/// The list is short and explicit on purpose: a tool is sensitive if running it
/// could read something private, change something important, or undo a
/// protection the user put in place.
pub const SENSITIVE_TOOLS: &[&str] = &[
    "bv_run_command",
    "bv_read_file",
    "bv_write_file",
    "bv_list_dir",
    "bv_unlock_pc",
    "bv_release_path",
];

/// The schema fingerprint stamped into every tool name.
///
/// The agent server caches a client tool's `parameters` **by name** for the life
/// of its process. Reusing a sandbox whose process was started by an older build
/// therefore rejects every new conversation with
/// `Client tool '<name>' is already registered with a different parameters schema`
/// — a 422 the user only ever sees as "nothing works". Since the name is part of
/// the cached identity, changing this fingerprint changes the name, so a schema
/// change can never collide with a stale cache entry.
///
/// **Bump this whenever any tool's parameters change.** Additive-only changes
/// still count: the server compares the whole `parameters` object.
pub const SCHEMA_VERSION: u32 = 1;

/// The wire name for a catalogue tool: `bv_open_app` becomes `bv_open_app_v1`.
pub fn wire_name(base: &str) -> String {
    format!("{base}_v{SCHEMA_VERSION}")
}

/// Strip a schema-version suffix, so a name that came off the wire maps back to
/// its catalogue entry.
fn base_name(name: &str) -> &str {
    name.rsplit_once("_v")
        .filter(|(_, v)| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .map(|(base, _)| base)
        .unwrap_or(name)
}

/// Find a tool by name. Accepts both the catalogue name and its wire name, so
/// the same lookup serves the UI, the catalogue text and an inbound call.
pub fn find(name: &str) -> Option<&'static DesktopTool> {
    let base = base_name(name);
    DESKTOP_TOOLS.iter().find(|t| t.name == base)
}

pub fn is_desktop_tool(name: &str) -> bool {
    find(name).is_some()
}

pub fn is_sensitive(name: &str) -> bool {
    let base = base_name(name);
    SENSITIVE_TOOLS.contains(&base)
}

/// The catalogue as the cloud's `ClientToolSpec` shape, with wire names.
pub fn specs() -> Vec<ClientToolSpec> {
    DESKTOP_TOOLS
        .iter()
        .map(|t| ClientToolSpec::new(&wire_name(t.name), t.description, t.params))
        .collect()
}

/// The catalogue as OpenAI-style function schemas, for a local model or a UI.
pub fn schemas() -> Value {
    Value::Array(
        DESKTOP_TOOLS
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": wire_name(t.name),
                        "description": t.description,
                        "parameters": ClientToolSpec::new(t.name, t.description, t.params).parameters,
                    }
                })
            })
            .collect(),
    )
}

/// The catalogue as a flat list for the UI to render.
pub fn tool_list() -> Value {
    Value::Array(
        DESKTOP_TOOLS
            .iter()
            .map(|t| {
                json!({
                    "name": wire_name(t.name),
                    "description": t.description,
                    "sensitive": is_sensitive(t.name),
                    "params": t.params.iter().map(|(name, kind, required)| json!({
                        "name": name, "type": kind, "required": required,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// A short, plain statement of what the agent can do, for its instructions.
///
/// This is written out rather than left implicit because an agent that does not
/// know it has a terminal will describe a command instead of running it.
pub fn catalogue_text() -> String {
    let names: Vec<String> = DESKTOP_TOOLS.iter().map(|t| wire_name(t.name)).collect();
    format!(
        "You are running inside BLACKVAULT on the user's own Windows desktop, not in a sandbox. \
         You have real control of this machine through these tools: {}. \
         They run on the user's computer and return their result to you, so use them to \
         actually do the work instead of explaining how it could be done. \
         Prefer these desktop tools for anything that happens on the user's own computer, \
         and use your own sandbox for building and testing larger projects. \
         Never mention the platform, provider or company behind you — you are BLACKVAULT.",
        names.join(", ")
    )
}

// ============================================================ plans

/// What a tool call is asking for, once it has been validated.
///
/// A call becomes a plan *before* anything runs, so the decision is testable and
/// the executor has nothing left to interpret.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolPlan {
    LaunchApp(String),
    CloseApp(String),
    ListApps,
    OpenPath(String),
    RunCommand(String),
    ReadFile(String),
    WriteFile { path: String, content: String },
    ListDir(String),
    PressKeys(String),
    TypeText(String),
    Click { x: Option<i32>, y: Option<i32> },
    Scroll { direction: String, amount: i32 },
    Screenshot,
    ReadScreen,
    Lock,
    Unlock,
    ProtectPath { path: String, folder: bool },
    ReleasePath(String),
    ListProtected,
    Speak(String),
    SystemInfo,
    Notify { title: String, body: String },
    WindowAction(String),
}

impl ToolPlan {
    /// The tool name this plan came from.
    pub fn tool_name(&self) -> &'static str {
        match self {
            ToolPlan::LaunchApp(_) => "bv_open_app",
            ToolPlan::CloseApp(_) => "bv_close_app",
            ToolPlan::ListApps => "bv_list_apps",
            ToolPlan::OpenPath(_) => "bv_open_path",
            ToolPlan::RunCommand(_) => "bv_run_command",
            ToolPlan::ReadFile(_) => "bv_read_file",
            ToolPlan::WriteFile { .. } => "bv_write_file",
            ToolPlan::ListDir(_) => "bv_list_dir",
            ToolPlan::PressKeys(_) => "bv_press_keys",
            ToolPlan::TypeText(_) => "bv_type_text",
            ToolPlan::Click { .. } => "bv_click",
            ToolPlan::Scroll { .. } => "bv_scroll",
            ToolPlan::Screenshot => "bv_screenshot",
            ToolPlan::ReadScreen => "bv_read_screen",
            ToolPlan::Lock => "bv_lock_pc",
            ToolPlan::Unlock => "bv_unlock_pc",
            ToolPlan::ProtectPath { .. } => "bv_protect_path",
            ToolPlan::ReleasePath(_) => "bv_release_path",
            ToolPlan::ListProtected => "bv_list_protected",
            ToolPlan::Speak(_) => "bv_speak",
            ToolPlan::SystemInfo => "bv_system_info",
            ToolPlan::Notify { .. } => "bv_notify",
            ToolPlan::WindowAction(_) => "bv_window_action",
        }
    }
}

/// Read a required string argument, refusing an empty one.
fn required_str(args: &Value, key: &str, tool: &str) -> BvResult<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| BvError::msg(format!("{tool}: \"{key}\" প্রয়োজন")))
}

fn optional_i32(args: &Value, key: &str) -> Option<i32> {
    args.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64().map(|n| n as i32),
        Value::String(s) => s.trim().parse::<i32>().ok(),
        _ => None,
    })
}

fn optional_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Turn a validated tool call into a plan.
///
/// Everything that can be rejected is rejected here, so a plan that exists is a
/// plan that can run.
pub fn plan_tool(name: &str, args: &Value) -> BvResult<ToolPlan> {
    let args = if args.is_null() { &Value::Null } else { args };
    // A call arrives with the wire name (`bv_open_app_v1`); match on the
    // catalogue name so the plan is version-independent.
    let name = base_name(name);
    let plan = match name {
        "bv_open_app" => ToolPlan::LaunchApp(required_str(args, "app", name)?),
        "bv_close_app" => ToolPlan::CloseApp(required_str(args, "app", name)?),
        "bv_list_apps" => ToolPlan::ListApps,
        "bv_open_path" => ToolPlan::OpenPath(required_str(args, "path", name)?),
        "bv_run_command" => ToolPlan::RunCommand(required_str(args, "command", name)?),
        "bv_read_file" => ToolPlan::ReadFile(required_str(args, "path", name)?),
        "bv_write_file" => ToolPlan::WriteFile {
            path: required_str(args, "path", name)?,
            content: args
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "bv_list_dir" => ToolPlan::ListDir(required_str(args, "path", name)?),
        "bv_press_keys" => ToolPlan::PressKeys(required_str(args, "keys", name)?),
        "bv_type_text" => ToolPlan::TypeText(required_str(args, "text", name)?),
        "bv_click" => ToolPlan::Click {
            x: optional_i32(args, "x"),
            y: optional_i32(args, "y"),
        },
        "bv_scroll" => ToolPlan::Scroll {
            direction: required_str(args, "direction", name)?,
            amount: optional_i32(args, "amount").unwrap_or(3),
        },
        "bv_screenshot" => ToolPlan::Screenshot,
        "bv_read_screen" => ToolPlan::ReadScreen,
        "bv_lock_pc" => ToolPlan::Lock,
        "bv_unlock_pc" => ToolPlan::Unlock,
        "bv_protect_path" => ToolPlan::ProtectPath {
            path: required_str(args, "path", name)?,
            folder: optional_bool(args, "folder"),
        },
        "bv_release_path" => ToolPlan::ReleasePath(required_str(args, "path", name)?),
        "bv_list_protected" => ToolPlan::ListProtected,
        "bv_speak" => ToolPlan::Speak(required_str(args, "text", name)?),
        "bv_system_info" => ToolPlan::SystemInfo,
        "bv_notify" => ToolPlan::Notify {
            title: required_str(args, "title", name)?,
            body: args
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "bv_window_action" => ToolPlan::WindowAction(required_str(args, "action", name)?),
        other => {
            return Err(BvError::msg(format!(
                "unknown tool \"{other}\" — BLACKVAULT has no such capability"
            )))
        }
    };
    Ok(plan)
}

// ============================================================ execution

/// Everything a plan needs from the running system.
///
/// The trait exists so the catalogue's behaviour can be tested without a screen,
/// a filesystem or an authenticated user, and so the real implementation is
/// forced to be an explicit, readable list of side effects.
pub trait DesktopHost {
    fn launch_app(&self, app: &str) -> BvResult<String>;
    fn close_app(&self, app: &str) -> BvResult<String>;
    fn list_apps(&self) -> BvResult<Value>;
    fn open_path(&self, path: &str) -> BvResult<String>;
    fn run_command(&self, command: &str) -> BvResult<Value>;
    fn read_file(&self, path: &str) -> BvResult<String>;
    fn write_file(&self, path: &str, content: &str) -> BvResult<String>;
    fn list_dir(&self, path: &str) -> BvResult<Value>;
    fn press_keys(&self, keys: &str) -> BvResult<String>;
    fn type_text(&self, text: &str) -> BvResult<String>;
    fn click(&self, x: Option<i32>, y: Option<i32>) -> BvResult<String>;
    fn scroll(&self, direction: &str, amount: i32) -> BvResult<String>;
    fn screenshot(&self) -> BvResult<String>;
    fn read_screen(&self) -> BvResult<Value>;
    fn lock(&self) -> BvResult<String>;
    fn unlock(&self) -> BvResult<String>;
    fn protect_path(&self, path: &str, folder: bool) -> BvResult<String>;
    fn release_path(&self, path: &str) -> BvResult<String>;
    fn list_protected(&self) -> BvResult<Value>;
    fn speak(&self, text: &str) -> BvResult<String>;
    fn system_info(&self) -> BvResult<Value>;
    fn notify(&self, title: &str, body: &str) -> BvResult<String>;
    fn window_action(&self, action: &str) -> BvResult<String>;
}

/// Run a plan and return a JSON result for the agent.
pub fn execute(host: &dyn DesktopHost, plan: &ToolPlan) -> BvResult<Value> {
    let text = |s: String| Ok(json!({ "ok": true, "result": s }));
    match plan {
        ToolPlan::LaunchApp(app) => text(host.launch_app(app)?),
        ToolPlan::CloseApp(app) => text(host.close_app(app)?),
        ToolPlan::ListApps => Ok(json!({ "ok": true, "apps": host.list_apps()? })),
        ToolPlan::OpenPath(path) => text(host.open_path(path)?),
        ToolPlan::RunCommand(command) => host.run_command(command),
        ToolPlan::ReadFile(path) => Ok(json!({ "ok": true, "content": host.read_file(path)? })),
        ToolPlan::WriteFile { path, content } => text(host.write_file(path, content)?),
        ToolPlan::ListDir(path) => Ok(json!({ "ok": true, "entries": host.list_dir(path)? })),
        ToolPlan::PressKeys(keys) => text(host.press_keys(keys)?),
        ToolPlan::TypeText(value) => text(host.type_text(value)?),
        ToolPlan::Click { x, y } => text(host.click(*x, *y)?),
        ToolPlan::Scroll { direction, amount } => text(host.scroll(direction, *amount)?),
        ToolPlan::Screenshot => text(host.screenshot()?),
        ToolPlan::ReadScreen => Ok(json!({ "ok": true, "screen": host.read_screen()? })),
        ToolPlan::Lock => text(host.lock()?),
        ToolPlan::Unlock => text(host.unlock()?),
        ToolPlan::ProtectPath { path, folder } => text(host.protect_path(path, *folder)?),
        ToolPlan::ReleasePath(path) => text(host.release_path(path)?),
        ToolPlan::ListProtected => Ok(json!({ "ok": true, "items": host.list_protected()? })),
        ToolPlan::Speak(value) => text(host.speak(value)?),
        ToolPlan::SystemInfo => Ok(json!({ "ok": true, "system": host.system_info()? })),
        ToolPlan::Notify { title, body } => text(host.notify(title, body)?),
        ToolPlan::WindowAction(action) => text(host.window_action(action)?),
    }
}

/// A result the agent can read when a call was refused.
pub fn refusal(reason: &str) -> Value {
    json!({ "ok": false, "error": reason })
}

/// The line the chat shows for a tool call, in the user's language.
pub fn describe_call(plan: &ToolPlan) -> String {
    match plan {
        ToolPlan::LaunchApp(app) => format!("অ্যাপ খোলা হচ্ছে: {app}"),
        ToolPlan::CloseApp(app) => format!("অ্যাপ বন্ধ করা হচ্ছে: {app}"),
        ToolPlan::ListApps => "ইনস্টল করা অ্যাপের তালিকা".into(),
        ToolPlan::OpenPath(path) => format!("খোলা হচ্ছে: {path}"),
        ToolPlan::RunCommand(command) => format!("কমান্ড: {command}"),
        ToolPlan::ReadFile(path) => format!("ফাইল পড়া: {path}"),
        ToolPlan::WriteFile { path, .. } => format!("ফাইল লেখা: {path}"),
        ToolPlan::ListDir(path) => format!("ফোল্ডার: {path}"),
        ToolPlan::PressKeys(keys) => format!("কী: {keys}"),
        ToolPlan::TypeText(text) => format!("লেখা হচ্ছে: {}", truncate(text, 60)),
        ToolPlan::Click { x, y } => match (x, y) {
            (Some(x), Some(y)) => format!("ক্লিক: {x},{y}"),
            _ => "ক্লিক".into(),
        },
        ToolPlan::Scroll { direction, .. } => format!("স্ক্রল: {direction}"),
        ToolPlan::Screenshot => "স্ক্রিনশট".into(),
        ToolPlan::ReadScreen => "স্ক্রিন পড়া".into(),
        ToolPlan::Lock => "ডেস্কটপ লক".into(),
        ToolPlan::Unlock => "আনলক".into(),
        ToolPlan::ProtectPath { path, .. } => format!("সুরক্ষা: {path}"),
        ToolPlan::ReleasePath(path) => format!("সুরক্ষা সরানো: {path}"),
        ToolPlan::ListProtected => "সুরক্ষিত জিনিসের তালিকা".into(),
        ToolPlan::Speak(_) => "বলা হচ্ছে".into(),
        ToolPlan::SystemInfo => "সিস্টেম তথ্য".into(),
        ToolPlan::Notify { title, .. } => format!("নোটিফিকেশন: {title}"),
        ToolPlan::WindowAction(action) => format!("উইন্ডো: {action}"),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn every_catalogue_entry_has_a_distinct_name_and_a_description() {
        let mut seen = std::collections::HashSet::new();
        for t in DESKTOP_TOOLS {
            assert!(seen.insert(t.name), "duplicate tool {}", t.name);
            assert!(!t.description.trim().is_empty(), "{} has no description", t.name);
        }
    }

    #[test]
    fn every_catalogue_entry_can_be_planned() {
        // A tool that cannot be planned is a tool the agent can call but nobody
        // can run — the worst of both.
        for t in DESKTOP_TOOLS {
            let args = json!({
                "app": "Chrome", "path": "C:/x", "command": "dir", "content": "hi",
                "keys": "enter", "text": "hello", "direction": "down",
                "title": "T", "body": "B", "action": "minimize_all",
                "x": 1, "y": 2, "folder": true,
            });
            let plan = plan_tool(t.name, &args)
                .unwrap_or_else(|e| panic!("{} could not be planned: {e}", t.name));
            assert_eq!(plan.tool_name(), t.name);
        }
    }

    #[test]
    fn specs_are_valid_object_schemas() {
        for spec in specs() {
            assert_eq!(spec.parameters["type"], "object");
            assert!(spec.parameters["properties"].is_object());
            assert!(spec.parameters["required"].is_array());
        }
    }

    #[test]
    fn every_wire_name_carries_the_schema_version() {
        // The agent server caches a client tool's parameters by name for the life
        // of its process. A reused sandbox whose process predates a schema change
        // then rejects every conversation with a 422. Stamping the version into
        // the name means a schema change also changes the name, so a stale cache
        // entry can never be hit.
        let suffix = format!("_v{}", SCHEMA_VERSION);
        for spec in specs() {
            assert!(
                spec.name.ends_with(&suffix),
                "{} must carry the schema version so a reused sandbox never collides",
                spec.name
            );
        }
        let schemas = schemas();
        for entry in schemas.as_array().unwrap() {
            let name = entry["function"]["name"].as_str().unwrap();
            assert!(name.ends_with(&suffix), "{name} is missing the schema version");
        }
    }

    #[test]
    fn a_versioned_call_maps_back_to_its_tool() {
        // A call arrives with the wire name; the plan and the sensitivity check
        // must still resolve it, or a bumped version silently disables the tool.
        let call = wire_name("bv_read_file");
        assert_eq!(call, "bv_read_file_v1");
        assert!(is_desktop_tool(&call));
        assert!(is_sensitive(&call), "a sensitive tool must stay sensitive when versioned");
        assert!(matches!(
            plan_tool(&call, &json!({ "path": "C:/x" })).unwrap(),
            ToolPlan::ReadFile(_)
        ));
        // The bare catalogue name keeps working too.
        assert!(is_desktop_tool("bv_read_file"));
        assert!(!is_desktop_tool("bv_read_file_v"));
    }

    #[test]
    fn every_tool_name_is_namespaced_to_avoid_a_server_collision() {
        // The agent server already ships tools called `read_file`, `write_file`,
        // `list_dir` and others. A client tool that reuses one of those names is
        // rejected with a 422 and the *whole conversation* fails to start — the
        // user sees "cannot create a conversation" with no clue why. Keeping a
        // private namespace on every name makes that impossible, including for
        // names the server adds in future.
        for t in DESKTOP_TOOLS {
            assert!(
                t.name.starts_with("bv_"),
                "tool {} is not namespaced; it can collide with a server tool",
                t.name
            );
        }
        let mut names: Vec<&str> = DESKTOP_TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two tools share a name");
    }

    #[test]
    fn sensitive_tools_are_all_real_tools() {
        for name in SENSITIVE_TOOLS {
            assert!(is_desktop_tool(name), "{name} is not in the catalogue");
        }
        assert!(is_sensitive("bv_run_command"));
        assert!(!is_sensitive("bv_open_app"));
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        let error = plan_tool("bv_open_app", &json!({})).unwrap_err().to_string();
        assert!(error.contains("app"), "error should name the missing field: {error}");
        assert!(plan_tool("bv_run_command", &json!({ "command": "   " })).is_err());
    }

    #[test]
    fn an_unknown_tool_is_refused_with_its_name() {
        let error = plan_tool("format_hard_drive", &json!({})).unwrap_err().to_string();
        assert!(error.contains("format_hard_drive"));
        assert!(!is_desktop_tool("format_hard_drive"));
    }

    #[test]
    fn optional_arguments_default_sensibly() {
        assert_eq!(
            plan_tool("bv_click", &json!({})).unwrap(),
            ToolPlan::Click { x: None, y: None }
        );
        assert_eq!(
            plan_tool("bv_click", &json!({ "x": "40", "y": 90 })).unwrap(),
            ToolPlan::Click { x: Some(40), y: Some(90) }
        );
        assert_eq!(
            plan_tool("bv_scroll", &json!({ "direction": "down" })).unwrap(),
            ToolPlan::Scroll { direction: "down".into(), amount: 3 }
        );
        assert_eq!(
            plan_tool("bv_protect_path", &json!({ "path": "C:/x" })).unwrap(),
            ToolPlan::ProtectPath { path: "C:/x".into(), folder: false }
        );
    }

    #[test]
    fn a_write_with_no_content_writes_an_empty_file() {
        let plan = plan_tool("bv_write_file", &json!({ "path": "C:/note.txt" })).unwrap();
        assert_eq!(
            plan,
            ToolPlan::WriteFile { path: "C:/note.txt".into(), content: String::new() }
        );
    }

    #[test]
    fn whitespace_around_a_path_is_trimmed() {
        let plan = plan_tool("bv_open_path", &json!({ "path": "  C:/x  " })).unwrap();
        assert_eq!(plan, ToolPlan::OpenPath("C:/x".into()));
    }

    /// A host that records what it was asked to do and answers a fixed value.
    struct Recorder {
        seen: RefCell<Vec<String>>,
        fail: bool,
    }

    impl Recorder {
        fn new() -> Self {
            Recorder { seen: RefCell::new(Vec::new()), fail: false }
        }
        fn record(&self, what: impl Into<String>) -> BvResult<String> {
            let what = what.into();
            self.seen.borrow_mut().push(what.clone());
            if self.fail {
                Err(BvError::msg("refused"))
            } else {
                Ok(what)
            }
        }
    }

    impl DesktopHost for Recorder {
        fn launch_app(&self, app: &str) -> BvResult<String> { self.record(format!("launch:{app}")) }
        fn close_app(&self, app: &str) -> BvResult<String> { self.record(format!("close:{app}")) }
        fn list_apps(&self) -> BvResult<Value> { Ok(json!(["Chrome"])) }
        fn open_path(&self, path: &str) -> BvResult<String> { self.record(format!("open:{path}")) }
        fn run_command(&self, command: &str) -> BvResult<Value> {
            self.record(format!("run:{command}"))?;
            Ok(json!({ "ok": true, "exitCode": 0, "stdout": "done" }))
        }
        fn read_file(&self, path: &str) -> BvResult<String> { self.record(format!("read:{path}")) }
        fn write_file(&self, path: &str, _c: &str) -> BvResult<String> { self.record(format!("write:{path}")) }
        fn list_dir(&self, path: &str) -> BvResult<Value> {
            self.record(format!("ls:{path}"))?;
            Ok(json!(["a.txt"]))
        }
        fn press_keys(&self, keys: &str) -> BvResult<String> { self.record(format!("keys:{keys}")) }
        fn type_text(&self, text: &str) -> BvResult<String> { self.record(format!("type:{text}")) }
        fn click(&self, x: Option<i32>, y: Option<i32>) -> BvResult<String> {
            self.record(format!("click:{x:?},{y:?}"))
        }
        fn scroll(&self, d: &str, n: i32) -> BvResult<String> { self.record(format!("scroll:{d}:{n}")) }
        fn screenshot(&self) -> BvResult<String> { self.record("shot") }
        fn read_screen(&self) -> BvResult<Value> { Ok(json!({ "title": "Editor" })) }
        fn lock(&self) -> BvResult<String> { self.record("lock") }
        fn unlock(&self) -> BvResult<String> { self.record("unlock") }
        fn protect_path(&self, path: &str, folder: bool) -> BvResult<String> {
            self.record(format!("protect:{path}:{folder}"))
        }
        fn release_path(&self, path: &str) -> BvResult<String> { self.record(format!("release:{path}")) }
        fn list_protected(&self) -> BvResult<Value> { Ok(json!(["C:/secret"])) }
        fn speak(&self, text: &str) -> BvResult<String> { self.record(format!("speak:{text}")) }
        fn system_info(&self) -> BvResult<Value> { Ok(json!({ "os": "windows" })) }
        fn notify(&self, t: &str, _b: &str) -> BvResult<String> { self.record(format!("notify:{t}")) }
        fn window_action(&self, a: &str) -> BvResult<String> { self.record(format!("window:{a}")) }
    }

    #[test]
    fn executing_a_plan_calls_the_host_once_with_the_right_arguments() {
        let host = Recorder::new();
        let plan = plan_tool("bv_open_app", &json!({ "app": "Notepad" })).unwrap();
        let result = execute(&host, &plan).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(host.seen.borrow().as_slice(), ["launch:Notepad"]);
    }

    #[test]
    fn a_command_result_carries_the_output_through() {
        let host = Recorder::new();
        let plan = plan_tool("bv_run_command", &json!({ "command": "echo hi" })).unwrap();
        let result = execute(&host, &plan).unwrap();
        assert_eq!(result["stdout"], "done");
        assert_eq!(result["exitCode"], 0);
    }

    #[test]
    fn a_host_failure_becomes_an_error_the_agent_can_read() {
        let host = Recorder { seen: RefCell::new(Vec::new()), fail: true };
        let plan = plan_tool("bv_screenshot", &json!({})).unwrap();
        assert!(execute(&host, &plan).is_err());
        assert_eq!(refusal("no screen").get("ok"), Some(&Value::Bool(false)));
    }

    #[test]
    fn a_description_is_produced_for_every_plan() {
        for t in DESKTOP_TOOLS {
            let args = json!({
                "app": "Chrome", "path": "C:/x", "command": "dir", "content": "hi",
                "keys": "enter", "text": "hello", "direction": "down",
                "title": "T", "body": "B", "action": "minimize_all",
            });
            let plan = plan_tool(t.name, &args).unwrap();
            assert!(!describe_call(&plan).is_empty(), "{} has no label", t.name);
        }
    }

    #[test]
    fn the_catalogue_text_names_every_tool() {
        let text = catalogue_text();
        for t in DESKTOP_TOOLS {
            assert!(
                text.contains(&wire_name(t.name)),
                "catalogue text omits {}",
                wire_name(t.name)
            );
        }
        assert!(text.contains("BLACKVAULT"));
    }

    #[test]
    fn local_schemas_match_the_catalogue() {
        let schemas = schemas();
        assert_eq!(schemas.as_array().unwrap().len(), DESKTOP_TOOLS.len());
        assert_eq!(schemas[0]["type"], "function");
    }
}
