//! BLACKVAULT — SQLite persistence.
//!
//! One local database holds everything: settings, salted secret hashes, voice
//! commands, launcher entries, protection rules, the voiceprint template and
//! the security audit log. WAL mode plus a busy timeout keep the background
//! voice thread and the UI thread from fighting over the file.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{BvError, BvResult};
use crate::models::*;
use crate::settings::Settings;

pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> BvResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> BvResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS secrets (
                name  TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS voice_commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                phrase TEXT NOT NULL UNIQUE,
                action TEXT NOT NULL,
                action_type TEXT NOT NULL,
                payload TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                requires_auth INTEGER NOT NULL DEFAULT 0,
                builtin INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                hit_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS launcher_apps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                command TEXT NOT NULL,
                args TEXT,
                cwd TEXT,
                voice_phrase TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                icon_path TEXT,
                launch_count INTEGER NOT NULL DEFAULT 0,
                last_launched TEXT
            );

            CREATE TABLE IF NOT EXISTS protected_apps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                exe_path TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                process_name TEXT NOT NULL,
                require_auth INTEGER NOT NULL DEFAULT 1,
                auto_relock INTEGER NOT NULL DEFAULT 1,
                relock_after_secs INTEGER NOT NULL DEFAULT 30,
                locked INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS protected_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                display_name TEXT NOT NULL,
                encryption TEXT NOT NULL,
                protected INTEGER NOT NULL DEFAULT 1,
                acl_snapshot TEXT,
                last_access TEXT,
                created_at TEXT NOT NULL,
                size_bytes INTEGER
            );

            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at TEXT NOT NULL,
                kind TEXT NOT NULL,
                severity TEXT NOT NULL,
                message TEXT NOT NULL,
                detail TEXT
            );

            CREATE TABLE IF NOT EXISTS voice_identity (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                samples TEXT NOT NULL,
                sample_count INTEGER NOT NULL,
                threshold REAL NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS voice_templates (
                phrase TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                mfcc TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS auth_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                failures INTEGER NOT NULL DEFAULT 0,
                window_start TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_events_at ON events(at DESC);
            "#,
        )?;
        Ok(())
    }

    // ------------------------------------------------------------- settings
    pub fn load_settings(&self) -> BvResult<Settings> {
        let c = self.conn.lock();
        let json: Option<String> = c
            .query_row("SELECT json FROM settings WHERE id = 1", [], |r| r.get(0))
            .optional()?;
        match json {
            Some(j) => Ok(serde_json::from_str::<Settings>(&j)
                .unwrap_or_default()
                .sanitize()),
            None => Ok(Settings::default()),
        }
    }

    pub fn save_settings(&self, s: &Settings) -> BvResult<()> {
        let c = self.conn.lock();
        let json = serde_json::to_string(s)?;
        c.execute(
            "INSERT INTO settings (id, json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            params![json],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------- secrets
    pub fn set_secret(&self, name: &str, value: &str) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO secrets (name, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![name, value, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_secret(&self, name: &str) -> BvResult<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT value FROM secrets WHERE name = ?1", params![name], |r| r.get(0))
            .optional()?)
    }

    pub fn clear_secret(&self, name: &str) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM secrets WHERE name = ?1", params![name])?;
        Ok(())
    }

    // ------------------------------------------------------- voice commands
    pub fn list_commands(&self) -> BvResult<Vec<VoiceCommand>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT id, phrase, action, action_type, payload, enabled, requires_auth, builtin, created_at, hit_count
             FROM voice_commands ORDER BY builtin DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(VoiceCommand {
                id: r.get(0)?,
                phrase: r.get(1)?,
                action: r.get(2)?,
                action_type: r.get(3)?,
                payload: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                requires_auth: r.get::<_, i64>(6)? != 0,
                builtin: r.get::<_, i64>(7)? != 0,
                created_at: r.get(8)?,
                hit_count: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_command(&self, cmd: &VoiceCommand) -> BvResult<VoiceCommand> {
        let c = self.conn.lock();
        if cmd.id > 0 {
            c.execute(
                "UPDATE voice_commands SET phrase=?1, action=?2, action_type=?3, payload=?4,
                 enabled=?5, requires_auth=?6 WHERE id=?7",
                params![
                    cmd.phrase,
                    cmd.action,
                    cmd.action_type,
                    cmd.payload,
                    cmd.enabled as i64,
                    cmd.requires_auth as i64,
                    cmd.id
                ],
            )?;
        } else {
            c.execute(
                "INSERT INTO voice_commands (phrase, action, action_type, payload, enabled, requires_auth, builtin, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(phrase) DO UPDATE SET action=excluded.action, action_type=excluded.action_type,
                   payload=excluded.payload, enabled=excluded.enabled, requires_auth=excluded.requires_auth",
                params![
                    cmd.phrase,
                    cmd.action,
                    cmd.action_type,
                    cmd.payload,
                    cmd.enabled as i64,
                    cmd.requires_auth as i64,
                    cmd.builtin as i64,
                    Utc::now().to_rfc3339()
                ],
            )?;
        }
        let id = c.last_insert_rowid();
        let real_id = if cmd.id > 0 { cmd.id } else { id };
        let mut out = cmd.clone();
        out.id = real_id;
        Ok(out)
    }

    pub fn delete_command(&self, id: i64) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM voice_commands WHERE id = ?1 AND builtin = 0", params![id])?;
        Ok(())
    }

    pub fn bump_command_hits(&self, phrase: &str) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE voice_commands SET hit_count = hit_count + 1 WHERE phrase = ?1",
            params![phrase],
        )?;
        Ok(())
    }

    /// Seed the default command set on first run. Returns true when seeding happened.
    pub fn seed_default_commands(&self) -> BvResult<bool> {
        let existing: i64 = {
            let c = self.conn.lock();
            c.query_row("SELECT COUNT(*) FROM voice_commands", [], |r| r.get(0))?
        };
        if existing > 0 {
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        let defaults: &[(&str, &str, &str, Option<&str>, bool, bool)] = &[
            ("lock this pc", "lock", "lock", None, false, true),
            ("unlock this pc", "unlock", "unlock", None, false, true),
            ("next", "next", "media_next", None, false, true),
            ("previous", "previous", "media_prev", None, false, true),
            ("play", "play", "media_play_pause", None, false, true),
            ("pause", "pause", "media_play_pause", None, false, true),
            ("scroll down", "scroll down", "scroll", Some("down:600"), false, true),
            ("scroll up", "scroll up", "scroll", Some("up:600"), false, true),
            ("click", "click", "mouse_click", None, false, true),
            ("press enter", "enter", "key_press", Some("enter"), false, true),
            ("volume up", "volume up", "volume_up", None, false, true),
            ("volume down", "volume down", "volume_down", None, false, true),
            ("mute", "mute", "volume_mute", None, false, true),
            ("show desktop", "show desktop", "show_desktop", None, false, true),
            ("minimise all", "minimise all", "minimize_all", None, false, true),
            ("take a screenshot", "screenshot", "screenshot", None, false, true),
            ("open chrome", "open chrome", "launch_app", Some("Chrome"), false, true),
            ("open notepad", "open notepad", "launch_app", Some("Notepad"), false, true),
            ("open file explorer", "open file explorer", "launch_app", Some("File Explorer"), false, true),
            ("open settings", "open settings", "open_url", Some("ms-settings:"), false, true),
        ];
        let c = self.conn.lock();
        for (phrase, action, action_type, payload, requires_auth, builtin) in defaults {
            c.execute(
                "INSERT OR IGNORE INTO voice_commands (phrase, action, action_type, payload, enabled, requires_auth, builtin, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
                params![phrase, action, action_type, payload, *requires_auth as i64, *builtin as i64, now],
            )?;
        }
        Ok(true)
    }

    // -------------------------------------------------------- launcher apps
    pub fn list_apps(&self) -> BvResult<Vec<LauncherApp>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT id, name, command, args, cwd, voice_phrase, enabled, icon_path, launch_count, last_launched
             FROM launcher_apps ORDER BY name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(LauncherApp {
                id: r.get(0)?,
                name: r.get(1)?,
                command: r.get(2)?,
                args: r.get(3)?,
                cwd: r.get(4)?,
                voice_phrase: r.get(5)?,
                enabled: r.get::<_, i64>(6)? != 0,
                icon_path: r.get(7)?,
                launch_count: r.get(8)?,
                last_launched: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_app(&self, id: i64) -> BvResult<Option<LauncherApp>> {
        Ok(self.list_apps()?.into_iter().find(|a| a.id == id))
    }

    pub fn upsert_app(&self, app: &LauncherApp) -> BvResult<LauncherApp> {
        let c = self.conn.lock();
        if app.id > 0 {
            c.execute(
                "UPDATE launcher_apps SET name=?1, command=?2, args=?3, cwd=?4, voice_phrase=?5, enabled=?6 WHERE id=?7",
                params![app.name, app.command, app.args, app.cwd, app.voice_phrase, app.enabled as i64, app.id],
            )?;
            let mut out = app.clone();
            return Ok(out_ok(&mut out));
        }
        c.execute(
            "INSERT INTO launcher_apps (name, command, args, cwd, voice_phrase, enabled) VALUES (?1,?2,?3,?4,?5,?6)",
            params![app.name, app.command, app.args, app.cwd, app.voice_phrase, app.enabled as i64],
        )?;
        let id = c.last_insert_rowid();
        let mut out = app.clone();
        out.id = id;
        Ok(out)
    }

    pub fn delete_app(&self, id: i64) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM launcher_apps WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn bump_app_launch(&self, id: i64) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE launcher_apps SET launch_count = launch_count + 1, last_launched = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    // ------------------------------------------------------- protected apps
    pub fn list_protected_apps(&self) -> BvResult<Vec<ProtectedApp>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT id, exe_path, display_name, process_name, require_auth, auto_relock, relock_after_secs, locked, created_at
             FROM protected_apps ORDER BY display_name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ProtectedApp {
                id: r.get(0)?,
                exe_path: r.get(1)?,
                display_name: r.get(2)?,
                process_name: r.get(3)?,
                require_auth: r.get::<_, i64>(4)? != 0,
                auto_relock: r.get::<_, i64>(5)? != 0,
                relock_after_secs: r.get(6)?,
                locked: r.get::<_, i64>(7)? != 0,
                created_at: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert_protected_app(&self, app: &ProtectedApp) -> BvResult<ProtectedApp> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO protected_apps (exe_path, display_name, process_name, require_auth, auto_relock, relock_after_secs, locked, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(exe_path) DO UPDATE SET display_name=excluded.display_name",
            params![
                app.exe_path,
                app.display_name,
                app.process_name,
                app.require_auth as i64,
                app.auto_relock as i64,
                app.relock_after_secs,
                app.locked as i64,
                app.created_at
            ],
        )?;
        let mut out = app.clone();
        out.id = c.last_insert_rowid();
        Ok(out)
    }

    pub fn update_protected_app(&self, app: &ProtectedApp) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE protected_apps SET display_name=?1, require_auth=?2, auto_relock=?3, relock_after_secs=?4, locked=?5 WHERE id=?6",
            params![
                app.display_name,
                app.require_auth as i64,
                app.auto_relock as i64,
                app.relock_after_secs,
                app.locked as i64,
                app.id
            ],
        )?;
        Ok(())
    }

    pub fn set_app_locked(&self, id: i64, locked: bool) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("UPDATE protected_apps SET locked=?1 WHERE id=?2", params![locked as i64, id])?;
        Ok(())
    }

    pub fn delete_protected_app(&self, id: i64) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM protected_apps WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ------------------------------------------------------ protected items
    pub fn list_protected_items(&self) -> BvResult<Vec<ProtectedItem>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT id, path, kind, display_name, encryption, protected, last_access, created_at, size_bytes
             FROM protected_items ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ProtectedItem {
                id: r.get(0)?,
                path: r.get(1)?,
                kind: r.get(2)?,
                display_name: r.get(3)?,
                encryption: r.get(4)?,
                protected: r.get::<_, i64>(5)? != 0,
                last_access: r.get(6)?,
                created_at: r.get(7)?,
                size_bytes: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert_protected_item(&self, item: &ProtectedItem, acl_snapshot: &str) -> BvResult<ProtectedItem> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO protected_items (path, kind, display_name, encryption, protected, acl_snapshot, last_access, created_at, size_bytes)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(path) DO UPDATE SET encryption=excluded.encryption, protected=excluded.protected,
               acl_snapshot=excluded.acl_snapshot, size_bytes=excluded.size_bytes",
            params![
                item.path,
                item.kind,
                item.display_name,
                item.encryption,
                item.protected as i64,
                acl_snapshot,
                item.last_access,
                item.created_at,
                item.size_bytes
            ],
        )?;
        let mut out = item.clone();
        out.id = c.last_insert_rowid();
        Ok(out)
    }

    pub fn acl_snapshot(&self, id: i64) -> BvResult<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row("SELECT acl_snapshot FROM protected_items WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?)
    }

    pub fn set_item_protected(&self, id: i64, protected: bool) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE protected_items SET protected=?1, last_access=?2 WHERE id=?3",
            params![protected as i64, Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn delete_protected_item(&self, id: i64) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM protected_items WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---------------------------------------------------------------- events
    pub fn log_event(&self, kind: &str, severity: &str, message: &str, detail: Option<&str>) -> BvResult<SecurityEvent> {
        let at = Utc::now().to_rfc3339();
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO events (at, kind, severity, message, detail) VALUES (?1,?2,?3,?4,?5)",
            params![at, kind, severity, message, detail],
        )?;
        Ok(SecurityEvent {
            id: c.last_insert_rowid(),
            at,
            kind: kind.into(),
            severity: severity.into(),
            message: message.into(),
            detail: detail.map(|s| s.to_string()),
        })
    }

    pub fn list_events(&self, limit: i64) -> BvResult<Vec<SecurityEvent>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT id, at, kind, severity, message, detail FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(SecurityEvent {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                severity: r.get(3)?,
                message: r.get(4)?,
                detail: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn clear_events(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM events", [])?;
        Ok(())
    }

    pub fn prune_events(&self, keep_days: i64) -> BvResult<()> {
        let cutoff = (Utc::now() - chrono::Duration::days(keep_days.max(1))).to_rfc3339();
        let c = self.conn.lock();
        c.execute("DELETE FROM events WHERE at < ?1", params![cutoff])?;
        Ok(())
    }

    pub fn last_auth_time(&self) -> BvResult<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row(
            "SELECT at FROM events WHERE kind = 'auth_success' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
    }

    // -------------------------------------------------------- voice identity
    pub fn set_voice_identity(&self, samples_json: &str, count: i64, threshold: f32) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO voice_identity (id, samples, sample_count, threshold, updated_at) VALUES (1,?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET samples=excluded.samples, sample_count=excluded.sample_count,
               threshold=excluded.threshold, updated_at=excluded.updated_at",
            params![samples_json, count, threshold, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn voice_identity(&self) -> BvResult<Option<VoiceIdentity>> {
        let c = self.conn.lock();
        Ok(c.query_row(
            "SELECT sample_count, threshold, updated_at FROM voice_identity WHERE id = 1",
            [],
            |r| {
                Ok(VoiceIdentity {
                    enrolled: true,
                    sample_count: r.get(0)?,
                    threshold: r.get(1)?,
                    updated_at: r.get(2)?,
                })
            },
        )
        .optional()?)
    }

    pub fn voice_template(&self) -> BvResult<Option<(Vec<f32>, f32)>> {
        let c = self.conn.lock();
        let row: Option<(String, f32)> = c
            .query_row("SELECT samples, threshold FROM voice_identity WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let c = c;
        let _ = c;
        match row {
            Some((stored, threshold)) => {
                // Templates are DPAPI-sealed; fall back to raw JSON so a
                // database written by an older build still loads.
                let json = crate::crypto::open_string(&stored).unwrap_or(stored);
                Ok(Some((serde_json::from_str(&json)?, threshold)))
            }
            None => Ok(None),
        }
    }

    pub fn clear_voice_identity(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM voice_identity", [])?;
        Ok(())
    }

    // --------------------------------------------------------- voice templates
    /// Store the MFCC template for a phrase. `kind` is `"command"` or
    /// `"passphrase"`, which lets the matcher bias thresholds by intent.
    pub fn set_template(&self, phrase: &str, kind: &str, mfcc: &[Vec<f32>]) -> BvResult<()> {
        let json = serde_json::to_string(mfcc)?;
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO voice_templates (phrase, kind, mfcc, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(phrase) DO UPDATE SET kind=excluded.kind, mfcc=excluded.mfcc, updated_at=excluded.updated_at",
            params![phrase, kind, json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn delete_template(&self, phrase: &str) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM voice_templates WHERE phrase = ?1", params![phrase])?;
        Ok(())
    }

    pub fn clear_templates(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM voice_templates", [])?;
        Ok(())
    }

    pub fn templates(&self) -> BvResult<Vec<(String, String, Vec<Vec<f32>>)>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare("SELECT phrase, kind, mfcc FROM voice_templates")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (phrase, kind, json) = row?;
            match serde_json::from_str::<Vec<Vec<f32>>>(&json) {
                Ok(m) => out.push((phrase, kind, m)),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    // ----------------------------------------------------------- auth state
    pub fn record_failure(&self) -> BvResult<(i64, String)> {
        let c = self.conn.lock();
        let now = Utc::now();
        let row: Option<(i64, String)> = c
            .query_row("SELECT failures, window_start FROM auth_state WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let (mut failures, window_start) = row.unwrap_or((0, now.to_rfc3339()));
        let start = chrono::DateTime::parse_from_rfc3339(&window_start)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(now);
        // Reset the counter once the window has elapsed.
        if (now - start).num_seconds() > 3600 {
            failures = 0;
        }
        failures += 1;
        let ws = if failures == 1 { now.to_rfc3339() } else { window_start };
        c.execute(
            "INSERT INTO auth_state (id, failures, window_start) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET failures=excluded.failures, window_start=excluded.window_start",
            params![failures, ws],
        )?;
        Ok((failures, ws))
    }

    pub fn reset_failures(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM auth_state", [])?;
        Ok(())
    }

    pub fn failure_state(&self) -> BvResult<(i64, String)> {
        let c = self.conn.lock();
        let row: Option<(i64, String)> = c
            .query_row("SELECT failures, window_start FROM auth_state WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        Ok(row.unwrap_or((0, Utc::now().to_rfc3339())))
    }

    pub fn counts(&self) -> BvResult<(i64, i64, i64)> {
        let c = self.conn.lock();
        let apps: i64 = c.query_row("SELECT COUNT(*) FROM protected_apps", [], |r| r.get(0))?;
        let files: i64 = c.query_row("SELECT COUNT(*) FROM protected_items WHERE kind = 'file'", [], |r| r.get(0))?;
        let folders: i64 = c.query_row("SELECT COUNT(*) FROM protected_items WHERE kind = 'folder'", [], |r| r.get(0))?;
        Ok((apps, files, folders))
    }

    pub fn factory_reset(&self) -> BvResult<()> {
        let c = self.conn.lock();
        c.execute_batch(
            "DELETE FROM settings; DELETE FROM secrets; DELETE FROM voice_commands; DELETE FROM launcher_apps;
             DELETE FROM protected_apps; DELETE FROM protected_items; DELETE FROM events; DELETE FROM voice_identity;
             DELETE FROM auth_state;",
        )?;
        Ok(())
    }
}

fn out_ok(app: &mut LauncherApp) -> LauncherApp {
    app.clone()
}

// Silence unused-import warning for BvError on non-Windows test builds.
#[allow(dead_code)]
fn _unused() -> Option<BvError> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn settings_round_trip() {
        let d = db();
        let mut s = d.load_settings().unwrap();
        assert!(!s.setup_complete);
        s.setup_complete = true;
        s.wake_phrase = "open sesame".into();
        d.save_settings(&s).unwrap();
        let back = d.load_settings().unwrap();
        assert!(back.setup_complete);
        assert_eq!(back.wake_phrase, "open sesame");
    }

    #[test]
    fn secrets_round_trip() {
        let d = db();
        assert!(d.get_secret("pin").unwrap().is_none());
        d.set_secret("pin", "hash").unwrap();
        assert_eq!(d.get_secret("pin").unwrap().as_deref(), Some("hash"));
        d.clear_secret("pin").unwrap();
        assert!(d.get_secret("pin").unwrap().is_none());
    }

    #[test]
    fn default_commands_seed_once() {
        let d = db();
        assert!(d.seed_default_commands().unwrap());
        assert!(!d.seed_default_commands().unwrap());
        let cmds = d.list_commands().unwrap();
        assert!(cmds.iter().any(|c| c.phrase == "lock this pc"));
        assert!(cmds.iter().any(|c| c.phrase == "next"));
        assert!(cmds.iter().all(|c| c.builtin));
    }

    #[test]
    fn protected_app_lifecycle() {
        let d = db();
        let app = ProtectedApp {
            id: 0,
            exe_path: "C:\\Windows\\notepad.exe".into(),
            display_name: "Notepad".into(),
            process_name: "notepad".into(),
            require_auth: true,
            auto_relock: true,
            relock_after_secs: 30,
            locked: true,
            created_at: Utc::now().to_rfc3339(),
        };
        let saved = d.insert_protected_app(&app).unwrap();
        assert!(saved.id > 0);
        d.set_app_locked(saved.id, false).unwrap();
        let list = d.list_protected_apps().unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].locked);
        d.delete_protected_app(saved.id).unwrap();
        assert!(d.list_protected_apps().unwrap().is_empty());
    }

    #[test]
    fn events_are_ordered_newest_first() {
        let d = db();
        d.log_event("lock", "info", "first", None).unwrap();
        d.log_event("unlock", "info", "second", None).unwrap();
        let ev = d.list_events(10).unwrap();
        assert_eq!(ev[0].message, "second");
    }

    #[test]
    fn rate_limit_counter_increments() {
        let d = db();
        assert_eq!(d.record_failure().unwrap().0, 1);
        assert_eq!(d.record_failure().unwrap().0, 2);
        d.reset_failures().unwrap();
        assert_eq!(d.failure_state().unwrap().0, 0);
    }
}
