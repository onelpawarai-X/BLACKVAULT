//! BLACKVAULT — offline speech recognition.
//!
//! The MFCC/DTW matcher in `voice/` can only compare an utterance against a
//! template that was recorded for it, so a command nobody has trained can never
//! be recognised. That is fine for *identity* (a voiceprint must be enrolled),
//! but it is the wrong tool for *command words*: the user types a phrase in the
//! editor and expects it to work immediately.
//!
//! This module does the word recognition with the speech engine already present
//! on every Windows 10/11 machine — SAPI — through an SRGS grammar built from
//! the user's own command list. Nothing leaves the machine and no model needs to
//! be downloaded. The DTW matcher is kept for the voiceprint check, where a
//! per-user template is exactly what is wanted.
//!
//! On non-Windows hosts the module compiles to a stub so the core stays testable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use parking_lot::Mutex;

/// A phrase the recogniser was asked to listen for, with the text the core
/// wants back when it is heard.
#[derive(Debug, Clone)]
pub struct SpeechPhrase {
    pub phrase: String,
}

/// Called on the recogniser thread whenever a phrase is recognised.
pub type SpeechSink = Arc<dyn Fn(String) + Send + Sync>;

pub struct SpeechEngine {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct Shared {
    running: AtomicBool,
    stop: AtomicBool,
    dictation: AtomicBool,
    phrases: Mutex<Vec<String>>,
    sink: Mutex<Option<SpeechSink>>,
    /// Last failure, for the status card. Empty when healthy.
    last_error: Mutex<Option<String>>,
}

impl SpeechEngine {
    pub fn new() -> Self {
        SpeechEngine {
            shared: Arc::new(Shared {
                running: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                dictation: AtomicBool::new(false),
                phrases: Mutex::new(Vec::new()),
                sink: Mutex::new(None),
                last_error: Mutex::new(None),
            }),
            thread: Mutex::new(None),
        }
    }

    pub fn set_sink(&self, sink: SpeechSink) {
        *self.shared.sink.lock() = Some(sink);
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.last_error.lock().clone()
    }

    /// Is a speech engine usable on this machine? False on non-Windows hosts.
    pub fn available() -> bool {
        imp::available()
    }

    /// Start listening for `phrases`. When `dictation` is set, a free-form
    /// dictation grammar is armed alongside the command list, so the user can
    /// *speak* their secret passphrase on the lock screen. Restarts the listener
    /// when it is already running, so the grammar always matches the current
    /// command list.
    pub fn start(&self, phrases: Vec<String>, dictation: bool) {
        if phrases.is_empty() {
            return;
        }
        *self.shared.phrases.lock() = phrases;
        self.shared.dictation.store(dictation, Ordering::Relaxed);
        self.stop();
        self.shared.stop.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        let handle = std::thread::Builder::new()
            .name("blackvault-speech".into())
            .spawn(move || imp::listen(shared))
            .ok();
        *self.thread.lock() = handle;
        self.shared.running.store(true, Ordering::Relaxed);
    }

    /// Reload the grammar without dropping the listener for long.
    pub fn set_phrases(&self, phrases: Vec<String>) {
        let changed = {
            let mut guard = self.shared.phrases.lock();
            if *guard == phrases {
                false
            } else {
                *guard = phrases.clone();
                true
            }
        };
        if changed && self.is_running() {
            let dictation = self.shared.dictation.load(Ordering::Relaxed);
            self.start(phrases, dictation);
        }
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.thread.lock().take() {
            let _ = h.join();
        }
    }
}

impl Default for SpeechEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the SRGS grammar document for a phrase list.
///
/// The phrases are user-typed, so they are escaped: the grammar is XML and a
/// stray `&` or `<` must not be able to break out of a `<item>` element.
pub fn build_grammar(phrases: &[String]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <grammar version=\"1.0\" xml:lang=\"en-US\" root=\"blackvault\" \
         xmlns=\"http://www.w3.org/2001/06/grammar\" tag-format=\"semantics/1.0\">\n\
         \x20 <rule id=\"blackvault\" scope=\"public\">\n\x20   <one-of>\n",
    );
    for p in phrases {
        let cleaned = p.trim();
        if cleaned.is_empty() {
            continue;
        }
        out.push_str("      <item>");
        out.push_str(&xml_escape(cleaned));
        out.push_str("</item>\n");
    }
    out.push_str("    </one-of>\n  </rule>\n</grammar>\n");
    out
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    use super::*;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{LPARAM, WAIT_OBJECT_0, WPARAM};
    use windows::Win32::Media::Speech::{
        ISpRecoContext, ISpRecoGrammar, ISpRecoResult, ISpRecognizer, SpSharedRecognizer,
        SPEI_END_SR_STREAM, SPEI_RECOGNITION, SPEVENT, SPLO_STATIC, SPRS_ACTIVE, SPRS_INACTIVE,
        SPRST_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::WaitForSingleObject;

    /// RPC_E_CHANGED_MODE — COM is already initialised on this thread.
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

    /// `SPFEI(id)` — the event-interest bit for a SAPI event id.
    fn spfei(id: i32) -> u64 {
        1u64 << id
    }

    /// An all-zero event buffer; `SPEVENT` does not implement `Default`.
    fn empty_events() -> [SPEVENT; 8] {
        let zero = SPEVENT {
            _bitfield: 0,
            ulStreamNum: 0,
            ullAudioStreamOffset: 0,
            wParam: WPARAM(0),
            lParam: LPARAM(0),
        };
        [zero; 8]
    }

    pub fn available() -> bool {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let created: Result<ISpRecognizer, _> =
                CoCreateInstance(&SpSharedRecognizer, None::<&windows::core::IUnknown>, CLSCTX_ALL);
            if hr.is_ok() {
                CoUninitialize();
            }
            created.is_ok()
        }
    }

    pub fn listen(shared: Arc<Shared>) {
        if let Err(e) = run(shared.clone()) {
            *shared.last_error.lock() = Some(e);
        }
        shared.running.store(false, Ordering::Relaxed);
    }

    fn run(shared: Arc<Shared>) -> Result<(), String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() && hr.0 != RPC_E_CHANGED_MODE {
                return Err(format!("CoInitializeEx failed: {hr:?}"));
            }

            let recognizer: ISpRecognizer = CoCreateInstance(
                &SpSharedRecognizer,
                None::<&windows::core::IUnknown>,
                CLSCTX_ALL,
            )
            .map_err(|e| format!("no speech recogniser is installed: {e}"))?;

            // The shared recogniser drives the user's own input device.
            recognizer
                .SetRecoState(SPRST_ACTIVE)
                .map_err(|e| format!("could not activate the recogniser: {e}"))?;

            let context: ISpRecoContext = recognizer
                .CreateRecoContext()
                .map_err(|e| format!("could not create a recognition context: {e}"))?;

            // Ask for a Win32 event we can wait on, rather than a window message.
            context
                .SetNotifyWin32Event()
                .map_err(|e| format!("could not set the notification event: {e}"))?;
            let event = context.GetNotifyEventHandle();
            if event.is_invalid() {
                return Err("the recogniser did not provide an event handle".into());
            }

            context
                .SetInterest(spfei(SPEI_RECOGNITION.0) | spfei(SPEI_END_SR_STREAM.0), 0)
                .map_err(|e| format!("could not set event interest: {e}"))?;

            let grammar: ISpRecoGrammar = context
                .CreateGrammar(0)
                .map_err(|e| format!("could not create a grammar: {e}"))?;

            let phrases = shared.phrases.lock().clone();
            let path = grammar_path();
            std::fs::write(&path, build_grammar(&phrases))
                .map_err(|e| format!("could not write the grammar file: {e}"))?;
            let wide: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            grammar
                .LoadCmdFromFile(PCWSTR(wide.as_ptr()), SPLO_STATIC)
                .map_err(|e| format!("could not load the grammar: {e}"))?;
            grammar
                .SetRuleState(None, std::ptr::null_mut(), SPRS_ACTIVE)
                .map_err(|e| format!("could not arm the grammar: {e}"))?;

            // On the lock screen a free-form dictation grammar is armed as well,
            // so the spoken passphrase is recognised without being listed.
            if shared.dictation.load(Ordering::Relaxed) {
                if grammar.LoadDictation(None, SPLO_STATIC).is_ok() {
                    let _ = grammar.SetDictationState(SPRS_ACTIVE);
                } else {
                    log::warn!("could not arm the dictation grammar");
                }
            }

            // ---- event loop -------------------------------------------------
            let mut events = empty_events();
            while !shared.stop.load(Ordering::Relaxed) {
                let waited = WaitForSingleObject(event, 300);
                if waited != WAIT_OBJECT_0 {
                    continue;
                }
                let mut fetched: u32 = 0;
                loop {
                    let ok = context
                        .GetEvents(events.len() as u32, events.as_mut_ptr(), &mut fetched)
                        .is_ok();
                    if !ok || fetched == 0 {
                        break;
                    }
                    for ev in events.iter().take(fetched as usize) {
                        let id = ev._bitfield & 0xffff;
                        if id == SPEI_END_SR_STREAM.0 {
                            // The engine is shutting down; let the supervisor
                            // restart us.
                            let _ = grammar.SetRuleState(None, std::ptr::null_mut(), SPRS_INACTIVE);
                            return Ok(());
                        }
                        if id != SPEI_RECOGNITION.0 {
                            continue;
                        }
                        if let Some(text) = result_text(ev) {
                            if !text.is_empty() {
                                let sink = shared.sink.lock().clone();
                                if let Some(sink) = sink {
                                    sink(text);
                                }
                            }
                        }
                    }
                }
            }

            let _ = grammar.SetRuleState(None, std::ptr::null_mut(), SPRS_INACTIVE);
            CoUninitialize();
            Ok(())
        }
    }

    /// Pull the recognised words out of a `SPEI_RECOGNITION` event.
    unsafe fn result_text(ev: &SPEVENT) -> Option<String> {
        if ev.lParam.0 == 0 {
            return None;
        }
        let raw = ev.lParam.0 as *mut core::ffi::c_void;
        let borrowed = windows::core::from_raw_borrowed(&raw)?;
        let result: &ISpRecoResult = borrowed;
        let mut text = PWSTR::null();
        result
            .GetText(0, u32::MAX, true, &mut text, None)
            .ok()?;
        if text.is_null() {
            return None;
        }
        let out = text.to_string().ok();
        CoTaskMemFree(Some(text.0 as *const core::ffi::c_void));
        out
    }

    /// A per-process grammar file under the temporary directory.
    fn grammar_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("blackvault-grammar-{}.xml", std::process::id()))
    }

    // Keep the unused-import linters honest about types we name in signatures.
    #[allow(dead_code)]
    fn _types(_: LPARAM, _: WPARAM) {}
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn available() -> bool {
        false
    }

    pub fn listen(shared: Arc<Shared>) {
        *shared.last_error.lock() = Some("speech recognition is only available on Windows".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_lists_every_phrase() {
        let g = build_grammar(&["lock this pc".into(), "open chrome".into()]);
        assert!(g.contains("<item>lock this pc</item>"));
        assert!(g.contains("<item>open chrome</item>"));
        assert!(g.contains("root=\"blackvault\""));
        assert!(g.contains("scope=\"public\""));
    }

    #[test]
    fn grammar_escapes_xml_metacharacters() {
        let g = build_grammar(&["rock & roll <script>".into()]);
        assert!(g.contains("rock &amp; roll &lt;script&gt;"));
        assert!(!g.contains("<script>"));
    }

    #[test]
    fn grammar_skips_blank_phrases() {
        let g = build_grammar(&["next".into(), "   ".into(), "".into()]);
        assert_eq!(g.matches("<item>").count(), 1);
    }

    #[test]
    fn engine_reports_unavailable_off_windows() {
        // On Windows this is true when a recogniser is installed; off Windows it
        // must always be false so the UI does not promise what it cannot do.
        #[cfg(not(windows))]
        assert!(!SpeechEngine::available());
    }
}
