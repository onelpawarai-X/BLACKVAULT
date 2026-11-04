# ZYRAXON — project memory

## What this repo is
`onelpawarai-X/zyraxon-mobile-ai` — native Kotlin + Jetpack Compose Android app.
`namespace`/`applicationId = com.zyraxon.assistant`, minSdk 21, target/compileSdk 36.
`.aistudio/` + `metadata.json` are AI Studio export artifacts — keep them (the build
references their signing configs), but `metadata.json` holds a *stale* `com.aistudio.*`
id; the real app is `com.zyraxon.assistant`. README + release notes are English;
**ZYRAXON itself is Android-only** (its tools may build other OSes for other projects).
UI is **white glass** — use `ui/theme/Glass.kt` tokens, never a raw dark colour.

## Standing user rules
- **Language: mirror the user.** Bangla in → Bangla out, English in → English out; one
  stray word does not switch a set session language.
- **Decide small things yourself; stop only for big ones.** Pick defaults for names,
  app choice, ordering and move on. Ask one short question only when the answer is
  unknowable, irreversible, or touches identity/money/accounts. An open todo is a pause,
  never an ending (`MAX_TOOL_ROUNDS` = 120).
- **Do the one thing the user said, first.** A neighbouring subsystem must not absorb
  the session (v6.9.0 lesson).
- **This user reverses decisions.** Check `git log`/HEAD before trusting an older
  session's "the user asked for X" (v6.7.1 locked female-only, v6.8.0 reverted it).
- Play Protect: never write "the AI can never do this" — it walks the settings screen.
- No emulator in this container (no `/dev/kvm`), so runtime proof is the unit suite +
  `assembleDebug`; the phone is the only place to confirm runtime behaviour.

## Toolchain & build (reinstalled most sessions)
`/opt` is often empty at session start. Recipe (~6 min, background + poll):
openjdk-21-jdk-headless; **Gradle 9.3.1** at `/opt/gradle` (AGP 9.1.1 hard-fails below);
SDK at `/opt/android-sdk` via commandlinetools + `sdkmanager "platform-tools"
"platforms;android-36" "build-tools;36.0.0"` (also `platforms;android-36.1`). The licence
prompt hangs non-interactively — write `/opt/android-sdk/licenses/android-sdk-license`
by hand (needs sudo). `apt-get update` needs sudo.
- Build with system `gradle`, **not** `./gradlew` (no wrapper). `--offline` works for
  `compileDebugKotlin`/`assembleDebug`; unit tests need network the first time.
- Working dirs used by prior sessions: `GRADLE_USER_HOME=/tmp/gradle-home`,
  `/tmp/gradle-dist/gradle-9.3.1/bin/gradle`.
- Release signing: **the canonical keystore is `zyraxon-release-v2.jks`** (alias `upload`,
  store+key `zyraxon2026`, cert SHA-256 `5a03b61c…`). Confirmed 2026-10-31 by downloading
  the published v6.9.0 APK and running `apksigner verify --print-certs` on it — it is
  signed with exactly that cert, so this is the key every update must use. `my-upload-key.jks`
  (`6dcfa05e…`) is NOT in the repo anymore, so a build with the default `KEYSTORE_PATH`
  fails with "keystore file does not exist"; always pass
  `KEYSTORE_PATH=$PWD/zyraxon-release-v2.jks STORE_PASSWORD=zyraxon2026 KEY_PASSWORD=zyraxon2026`.
  `zyraxon-release.jks` (`112547e4…`) is a later mistake — never use it.
  **Check the cert of the last published APK, not the memory, before a release.**
- Lint blocks `assembleRelease` on `InvalidFragmentVersionForActivityResult`; fix is the
  `androidx.fragment:fragment-ktx` dependency.
- **Verify the artifact, not the source:** `unzip -o apk 'classes*.dex' -d /tmp/x` then
  `strings /tmp/x/classes*.dex | grep -c <symbol>` — this caught v5.1.0 shipping no
  `ProviderCatalog` at all.

## Credentials
- Injected `$GITHUB_TOKEN` = account **zyraxon-X**: only `pull` on the release repo, so
  push/release/PR-edit via `gh` return **403 "Resource not accessible by integration"**.
  Writable repo: `zyraxon-X/lore-crafter-delight`.
- The user pastes their own **classic PAT** (`ghp_…`, account **onelpawarai-X**, admin on
  the release repo) each session — do **not** store its value. `gh auth login --with-token`
  + `gh auth setup-git` makes the credential helper serve it. Push form if inline:
  `https://onelpawarai-X:$PAT@github.com/...` (`user:token@`; a bare token makes git hang).
  **PR edits need the classic PAT too:** `TOK=$(git remote get-url origin | sed -E
  's#https://[^:]+:([^@]+)@.*#\1#')` then `curl -X PATCH -H "Authorization: Bearer $TOK"
  .../pulls/<n> -d @pr.json`. `gh pr edit` is blocked for the app token.
- Older PATs live in bare backups at `/workspace/backup/*.git`.

## Models: nothing is hardcoded
The app carries **no model list** — every model comes from its provider's catalogue at
runtime; code holds only detection rules (`seedModels` are an offline fallback).
- `ProviderCatalog.parseModels` sets `AiModel.isRealtime`; picking a realtime model opens
  the live socket. Two protocols stay distinct: Gemini `BidiGenerateContent` vs
  OpenAI/xAI Realtime; voice names differ per protocol.
- **Verify a realtime endpoint with a raw TLS WebSocket handshake, never `curl -I`.**
  Gemini and `api.openai.com/v1/realtime` upgrade `101`; `api.x.ai/v1/realtime` `400`
  (real path, bad key); `api.groq.com/openai/v1/realtime` `404` (never labelled realtime).
- `parseModels` drops Gemini's TTS-only `-tts` ids; live ones are `native-audio`/`-live-`.
- Catalogue order = provider `speedRank`, fastest first. ZYRAXON is held at the end while
  its gate is shut.
- On-device bundles must be **token-free** (verified by ranged GET `206`): SmolLM-135M,
  TinyLlama-1.1B, Qwen2.5-1.5B, DeepSeek-R1-Distill-Qwen-1.5B, Qwen3-4B, Phi-4-mini.
  Gemma is licence-gated (`401`) and not offered.
- `fetchLiveModels()` loads all providers **in parallel** with a progress callback (never
  sequential). `adoptDefaultIfUnset(models, complete)` may only substitute a model from
  the **finished** catalogue — adopting from a partial one discarded the user's choice.
- `callModelWithFailover` retries the next **keyed** provider in `speedRank` order on
  quota/limit, each fallback using **its own** seed model id.

## The user's own provider (gated on purpose)
`zyraxonai.lovable.app/api/public/zyraxon/v1/models` returns 10 models, but
`/chat/completions` is **switched off** until `onelpawarai-X/ZYRAXON-AI` hits 100,000
stars — a failure there is expected, not a bug. Provider id `zyraxon`. Since v6.0.0 its
catalogue is `catalogIsPublic = true`, so the 10 models list before any key is set.
The container **can** read that host: a short `curl -m 25` returns HTTP 000 because the
body streams slowly; `curl -s --http1.1 --max-time 110` returns the full body.

## Voice (STT/TTS) — the area that keeps regressing
Phone's own engine only; a remote `translate.google.com/translate_tts` fallback was
removed on the user's instruction. Preferred engine `com.google.android.tts` (carries the
Bangla voice). `NativeVoiceManager` is the **single owner** of voice state — do not add a
second `_voiceGender` in the ViewModel (real compile error once).
- **Mic indicator**: the listening state belongs to the **user's switch** alone.
  `onReadyForSpeech`/`onError`/`onResults` must NOT flip `_isListening`; only
  `setMicEnabled` does. Every start attempt grows the backoff (a failed start used to
  leave the counter at 0 and hammer every 300 ms — the off/on churn). `RecognizerError`
  keeps the ear open for ordinary endings (NO_MATCH 7, SPEECH_TIMEOUT 6, RECOGNIZER_BUSY
  8, NETWORK 2, SERVER_DISCONNECTED 11).
- **Echo**: filter the assistant's own voice **by text** (`EchoMatcher.isOwnSpeech`,
  `strict` while the speaker rings), never by muting the mic for the whole reply — muting
  blocked real interruption. A partial only ever *interrupts*; it is never auto-sent.
- **Both voices**: `_voiceChosenByUser` (pref `voice_chosen_by_user`) is load-bearing —
  `setVoiceGender`/`applyVoice` set it, `setPersona` sets the voice only when it is false,
  and the constructor reads `voice_gender` back when true. Deleting it caused BOTH
  "only the female voice" and "my choice resets". `VoiceSelector.pick` order is
  gendered → gender-neutral → nothing; it never returns the opposite gender. When no
  voice states the wanted gender, gender is carried by pitch (female 1.28/1.00, male
  0.82/0.95).
- **Letter-by-letter bug** ("login" → "el-o-ji-ai-en"): an engine holds one language at a
  time, so the Bangla voice spells English words. `SpeechSegmenter` splits cleaned text
  into same-language runs; `speakRuns()` switches language per run; a single-language
  sentence stays ONE utterance so prosody is not chopped. Voice selection must fall back
  to *any* voice for the language (a leftover previous-language voice spelled English out).
- **Real-time + tone**: `SpeechChunker.take()` releases at the first sentence end and, for
  a long unpunctuated run, at a word boundary. Constants were lowered in v6.10.0
  (CLAUSE_MIN 42→30, COMMA_MIN 16→12, WORD_RELEASE_MIN 48→32, WORD_CUT_MIN 24→16).
  `SpeechTone` adds a small pitch/rate shift per sentence. Pipeline: `SpeechChunker` →
  `SpeechSegmenter` → `SpeechQueue` (one run at a time, `onDone` gates the next; a
  watchdog drops a stalled piece so speech continues).
- `SpeechTextCleaner` strips code fences, inline code, markdown, bullets, emoji and the
  `⚙️` tool-status lines before speaking.
- **Live speech-to-speech** follows the chosen model: `_liveVoicePreferred` must be
  initialised from pref `live_voice_preferred` and the session reopened on launch when
  the saved model `isRealtime` — writing the pref without reading it made the model's own
  voice work once and never return. Phone TTS is the fallback whenever no live session is
  active (`shouldUsePhoneTts()`).

## Realtime + agentic behaviour
- `OpenAiSse` streams `data:` deltas over OkHttp; `mergeToolCalls` concatenates split
  `tool_calls` args by index. `lastError` captures the provider's own error text so the
  phone repeats the real reason. Do **not** buffer the SSE body — it kills streaming.
- `ZyraxonAiEngine.streamAiResponse` is a `channelFlow` agentic loop feeding each tool
  result back into `messages`; `drainInjections` is drained per round.
- `sendMessage()` sets `_isGenerating = true` **before** `viewModelScope.launch`.
- **Latency**: gather the turn's inputs (DB reads, `screenContext()` = full
  accessibility-tree walk) off the main thread with `withContext(Dispatchers.IO)`; repaint
  the streaming reply at most every `UI_UPDATE_MS = 60` ms, not per token.
- Long-term memory: `ai/ContextWindow.kt` (`ENGINE_LIMIT=12`, `RECALL_LIMIT=4`,
  `RECENT_LIMIT=8`) — recalled turns must lead the history or the engine's `takeLast(12)`
  trims them away. Keep engine and ViewModel on the same constant.
- Settings persistence is guarded by `SettingsPersistenceTest`, which builds the real
  ViewModel twice against the same SharedPreferences (no mocks). **Run the suite before
  trusting a previous session's work** — a red test here was the alarm for v6.10.0.

## Phone integration notes
- **Channels**: no Android intent posts autonomously. `channel_post` opens the real app's
  composer pre-filled (Telegram/WhatsApp/Facebook/Messenger/Instagram/X via `ACTION_SEND`
  + `setPackage`; Gmail/SMS via `ACTION_SENDTO`) and must never claim it was sent. Android
  11+ needs the manifest `<queries>` block or those apps are invisible; images need
  `FileProvider`.
- **Floating bubble**: `floating_bubble` is a real tool; the service pushes text into the
  **same `ZyraxonViewModel`** (`ZyraxonViewModel.active`) — do not give it its own engine.
  `Settings.canDrawOverlays` is the gate (else `addView` throws; `stopSelf()` instead).
  Robolectric: cast `shadowOf(windowManager)` to `ShadowWindowManagerImpl` (the base has
  no `views`); overlay permission comes from `ShadowSettings.setCanDrawOverlays(bool)`.
- **Aura**: six styles via `data-aura` on `<html>` (`orb` default, `twin`, `vortex`,
  `bloom`, `helix`, `core`); `AuraHtmlTest` guards that every Settings chip has CSS +
  markup. Never centre with `height:100vh` + flex (WebView mis-measures it); use absolute
  `top/left:50%` + `translate(-50%,-50%)`.
- **MCP** (`ai.zyraxon.assistant.mcp`): OAuth servers reject a custom scheme and need a
  loopback redirect that **loops** (the browser fetches `/favicon.ico` right after the
  redirect). No client id ships — DCR per install. Tools are `<serverId>__<tool>`.
  `McpManager.close()` + `ViewModel.onCleared()` release SSE channels.
- Terminal: `SystemShell.runStreaming` streams stdout+stderr on separate threads line by
  line. The full userland is now **bundled in the APK** and installed offline through
  `LinuxRuntime` at system level via Shizuku (see v6.12.0 below).

## The app died seconds after launch: KeepAliveService (v6.13.2)
**The single most user-visible bug of the project, and it was invisible in the source.**
`MainActivity.onCreate()` → `KeepAliveService.start()` → `startForegroundService()`.
Android gives such a service ~5 s to call `startForeground()` or it kills the **whole
process**: `RemoteServiceException: Context.startForegroundService() did not then call
Service.startForeground()`. On a cold start the main thread is still building the first
Compose frame, so the deadline was missed **every** launch — the app died ~13 s after the
first frame. This is what the user described as "মাইক বারেবারে বন্ধ-চালু হয়" and
"সাধারণ কাজ দিলেও এত সময় লাগে": it was never the voice, it was the process dying.
- **`startService()`, not `startForegroundService()`.** The only caller is MainActivity,
  which is in the foreground, so a plain background service is legal and carries no
  deadline. The service still calls `startForeground()` in `onStartCommand`, where a
  refusal is harmless (`stopSelf()` + `START_NOT_STICKY`).
- The manifest also lacked `android:foregroundServiceType`, which makes `startForeground`
  itself throw on API 34+; add `dataSync` (permission already declared).
- **`catch (Exception ignored)` around `startForeground` hid this for a long time.**
  A swallowed failure there is fatal, not cosmetic.
- How it was found: `adb logcat | grep -iE "FATAL|ANR in <pkg>|startForeground"` after a
  real launch. The unit tests could never see it — it is a platform lifecycle rule.
  **Verify launch behaviour on the emulator, not just that the build succeeds.**

## Releasing: check the published tag, not the branch
The recurring trap that made the user re-report already-fixed symptoms: the newest
release was **v6.9.0** while the fixes sat in **v6.10.0/v6.11.0**, never released. Always
`curl -s .../releases?per_page=1` before debugging a symptom, and bump
`versionCode`/`versionName` in `app/build.gradle.kts` when shipping.
**It happened again with PR #14**: the fixes merged to `main`, but the version was left
at `versionCode=21 / 6.12.0`, so the user installed an APK that still *said* 6.12.0 and
concluded nothing had changed. A merged fix is invisible until the version is bumped
**and** a GitHub release with the signed APK is published. Bump the version in the same
PR as the fix, or the fix ships as a no-op.
- Release flow that works: bump → `assembleRelease` with the canonical keystore
  (`zyraxon-release-v2.jks`, alias `upload`) → verify `versionCode`/`versionName` and
  the cert SHA-256 (`5a03b61c…`) with `aapt2 dump badging` + `apksigner verify` →
  merge PR → `gh release create <tag> --target main <apk> <apk>.sha256`.
- `apksigner`/`aapt2` are under `$ANDROID_HOME/build-tools/<ver>/`, not on `PATH`.
- The release APK is ~455 MB (two bundled Linux runtimes), so `adb install` on a
  KVM-less emulator takes many minutes — verify by `dumpsys package … | grep versionName`
  after it returns, not by eyeballing the UI.

## The engine, not the prompt, ends a turn (2026-10-31, PR #14)
The most important lesson of the project. The user kept reporting "একটা কাজ করার পরে
পরের ধাপে যায় না" and asked for a better system prompt. A prompt rule only
*detects* a stop; the turn actually ends in `ZyraxonAiEngine` when the model returns
text with no tool call. Fixes belong in the engine and the tools, with the prompt
only describing the rule.
- **`SCREEN_CHANGING_TOOLS`** (`open_app`, `youtube_play`, `launch_url`,
  `open_settings`, `call_dial`, `channel_post`, `device_lock`) — the guard that
  injects the fresh screen after an action used to be
  `isScreenTool(name) || name == "open_app"`, so `youtube_play` opened the results
  and stopped: the model never received that screen. Any new screen-changing tool
  **must** be added to that set.
- **`ContinuationPolicy`** (pure, `ContinuationPolicyTest`) — consulted before the
  turn ends; a failure report, a bare promise, or a silent stop right after the
  screen changed forces another round (max 4 nudges). `nudges` and
  `lastActionWasScreenChange` are locals in the loop; the flag is cleared **before**
  this round's calls run (clearing it at the top of the loop is wrong — the check
  reads it after the round).
- **`write_file` retries into the workspace** on a failed write instead of failing
  the job.

## Voice: the mic stays open while the assistant speaks (PR #14)
`scheduleRestart` used to *defer* the reopen for the whole reply — that is why the
mic looked off whenever the assistant talked. Removed. Own-voice is dropped by text
(`SttDecision` + `EchoMatcher`), so the ear can stay open and an interruption still
lands. The floor (`IDLE_RESTART_FLOOR_MS = 450`) + exponential backoff only space out
rebuilds; they never close the ear. The `live` settings pane (deleted in v6.8.0) is
restored from `5270365^`.

## Release history
- **v6.13.2** (2026-10-31, PR #16, tag `v6.13.2`): the launch crash fix. The app was
  being killed by the platform seconds after opening (see the KeepAliveService section
  above). `versionCode` 23→24. **This is the release that finally behaves.** 310 tests.
- **v6.13.1**: version bump only, superseded within the hour by v6.13.2 (it did not fix
  the crash, because `startForegroundService()` still carried the 5 s deadline).
- **v6.13.0** (2026-10-31, PR #15, tag `v6.13.0`): the fixes from PR #14 finally
  **shipped**. PR #14 merged but left the version at 6.12.0, so the user's installed
  APK still reported the old number. This release is only a version bump
  (`versionCode` 21→22, `versionName` 6.12.0→6.13.0) — no code change. Cert SHA-256
  `5a03b61c…` matches v6.12.0, so it installs in place. 310 tests / 0 failures.
- **PR #14** (branch `fix/voice-lang-multistep-website`, 2026-10-31): engine-level
  continuation, `SCREEN_CHANGING_TOOLS`, `write_file` fallback, mic stays open during
  speech, live-voice pane restored, faster `SpeechChunker`. 310 unit tests / 0 failures.
- **v6.12.0** (2026-10-30, PR #13, branch `feat/website-voice-realtime`): the terminal's
  full Linux userland is now **bundled in the APK** (`assets/linux/<abi>/`, ~355 MB,
  both ABIs) and installed offline — no network, no second download. `LinuxRuntime`
  replaces `TermuxRuntime`; `termux_run` is `linux_run` and **no upstream trace remains
  anywhere** (source, assets, docs). 282 tests / 0 failures; APK 467 MB.
  **Read the "v6.12.0" section of `2026-10-30.md` before touching the Linux runtime** —
  the four things that matter (no `ar` in busybox, bootstrap-vs-deb layout, why `proot`
  is required, and why the build prefix must be *derived* not written down).
- **v6.11.0**: website TTS endpoint is the default voice, streamed piece by piece.
- **v6.10.0** (2026-10-30, PR #11, branch `fix/terminal-termux-live`): voice stays on
  (mic state = user switch; backoff grows per attempt), no echo/auto-send, TTS back and
  the model's own S2S reopens on launch, both voices work (`_voiceChosenByUser` restored),
  no letter-by-letter English, faster turns (IO off the main thread), settings/model
  survive restart, terminal streaming/ANSI, new tools `youtube_play`,
  `network_status`, `call_dial` by contact name. 253 tests / 0 failures.
- **v6.9.0**: top of screen = one `সেটিংস` pill; the tab row moved into a `SessionNavPane`
  pane (the default Settings pane). 247 tests.
- **v6.8.0**: female-only lock reverted (both voices wanted); speech became a real queue.
- **v6.7.1**: mic-indicator fix via `RecognizerError`. **v6.7.0**: minSdk 24→21; real
  Shizuku system shell. **v6.6.0**: Terminal tab + JGit. **v6.5.0**: in-app auto-update.
  **v6.4.0**: no hardcoded models. **v6.3.0**: 60-language UI. **v6.2.0**: floating
  bubble, six aura styles, `read_media` by url. **v6.1.0**: provider failover.
  **v6.0.0**: parallel model loading, real key verification, public ZYRAXON catalogue.
  v3.0.0 was deleted and rebuilt once.
- Detailed history: `.openhands/memory/2026-10-20.md` and `.openhands/memory/2026-10-30.md`.
