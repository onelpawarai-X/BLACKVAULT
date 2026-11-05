# The desktop agent

BLACKVAULT is a security system first, and an agent second. Nothing in this
document changes that: the agent can *ask* to read a file, and only the person
at the keyboard can *allow* it. Every rule below exists so that a voice, a model
or a remote service can never be the thing that opens the vault.

## 1. What it is

The agent is one brain with two bodies.

- **The cloud body** is a conversation on the OpenHands service. It has a
  sandbox, a shell, a filesystem and a model. It is where long work happens:
  building a project, running tests, searching a repository.
- **The desktop body** is this computer, reached through a fixed catalogue of
  tools that BLACKVAULT runs itself (`src-tauri/src/agent.rs`). It is where the
  user's own work happens: opening an application, typing into the focused
  window, protecting a folder, locking the screen.

A single turn can use both. The agent opens a file on the desktop, writes code
in the sandbox, and reports back — without the user having to move between two
programs.

## 2. The shape of a turn

```
  user speaks / types
        │
  bridge::plan_turn ──▶ reuse a warm conversation
        │               resume a sleeping sandbox
        │               or start a new conversation (reusing a warm sandbox)
        │
  cloud::stream_events ──▶ the agent's reasoning, tool calls and answer
        │                  arrive on a WebSocket as they are produced
        │
  an action names one of OUR tools?
        │
        ├── no  ──▶ show it
        │
        └── yes ──▶ sensitive? ──▶ ask the user, and WAIT for the answer
                        │                    │
                        │              refused ──▶ tell the agent, turn continues
                        │
                        └──▶ run it here ──▶ send the result back ──▶ keep listening
        │
  a status of "finished" with nothing owed ──▶ the turn is over
```

The one line worth remembering: **a tool call does not end a turn.** The service
answers a call for a tool that lives on this machine with a placeholder and
stops its own run. The bridge runs the tool and continues the conversation.
Treating that stop as the end would drop the user's answer on the floor.

## 3. Where each decision lives

| Decision | Module | Why there |
|---|---|---|
| What a tool call means | `agent.rs` (`plan_tool`) | Pure; proven by unit tests on any host |
| Which tools are sensitive | `agent.rs` (`SENSITIVE_TOOLS`) | One explicit list, not scattered checks |
| What a turn must do first | `bridge.rs` (`plan_turn`) | Pure; no threads, no sockets |
| Whether an event ends the turn | `bridge.rs` (`react_to`) | Pure; the rule is testable |
| Running a tool | `runner.rs` (`SystemHost`) | The only place with side effects |
| Talking to the service | `cloud.rs` | One client, one place a key is used |

The split is deliberate. Everything that can be decided without a screen, a
socket or a database is decided in a pure function and tested; the code that
touches the machine is small enough to read in one sitting.

## 4. The authentication gate

A tool is sensitive when running it could read something private, change
something important, or undo a protection the user put in place:

```
run_command, read_file, write_file, list_dir, unlock_pc, release_path
```

When the agent calls one, the runner does not run it. It raises
`AgentToolChallenge` with a one-shot token and **blocks the turn**. The UI shows
an approval card. `agent_answer(token, allow)` is the only way forward.

Three properties fall out of this:

1. **The agent cannot approve itself.** There is no path from the agent's own
   message stream to `answer_approval`.
2. **An approval is for one call.** The token is single-use and is compared
   exactly; a replayed or forged answer is rejected.
3. **A silence is a refusal.** The wait is bounded (120 s) and a timeout is
   treated as "no", never as "yes".

`unlock_pc` is refused outright, whatever the approval: releasing the vault is
done by the person at the lock screen, through `auth.rs`, and by nothing else.

## 5. The cloud key

- Stored in the `secrets` table, sealed with DPAPI under the current user
  (`crypto.rs`). It is never written to a config file, a log or the database in
  plain text.
- The UI is write-only with respect to it: it can be set, replaced or cleared,
  and the pane shows only whether one is present.
- It is sent to the service as `Authorization: Bearer …` and nowhere else.

## 6. Privacy

- **No audio is ever stored.** The agent path adds no recording of any kind.
- **The agent's transcript is not persisted by BLACKVAULT.** A conversation
  lives on the service, which is where the user's account is; the local database
  holds only the mapping from a local chat id to a remote conversation id and a
  sandbox id. That mapping is stored next to the API key because a binding is
  only meaningful together with the key it was made with.
- **The tool catalogue is sent, not the machine.** The service is told which
  capabilities exist; it is never sent a directory listing, a file or a
  screenshot unless the agent asks for one and the user approves.
- **No telemetry.** Unchanged from the rest of the product.

## 7. Failure behaviour

| Failure | Behaviour |
|---|---|
| The socket drops mid-turn | Reconnect with backoff (0.5 s → 8 s), 8 attempts; the UI shows "reconnecting" |
| The send fails (sandbox died) | The conversation's history is recapped into a new conversation and the message is resent — the user keeps their thread |
| The sandbox is asleep | Resumed, then the send is retried once |
| The service is slow to provision | Polled with a stage label, up to 180 s |
| The key is wrong | The account call fails and the pane says so; no sandbox is spent |
| No key at all | The pane says so; voice requests to the agent are refused with a reason |
| The socket goes quiet with no `finished` | After a bounded quiet period with no tool mid-run, the turn is closed by the runner, so the pane cannot stay "running" for ever |

## 8. Streaming and history

Two things a chat must do, and how they are done here:

- **A turn streams as it is produced.** The runner opens the socket *before* it
  sends the message, so no event can be missed in the gap. Every
  `StreamingDeltaEvent` is pushed to the pane the moment it arrives — the model's
  reasoning first, then its answer — and folded into one growing card
  (`src/lib/agentstream.ts`). Nothing waits for the turn to finish.
- **An earlier conversation is replayed.** Opening a chat fetches its recent
  events (`Cloud::history`), normalises them exactly as the live socket does, and
  draws the transcript. A reconnect replays the same way, so the history is not
  lost when the socket drops.

The status of a turn is carried by the event's own `execution_status`, never
inferred from an unrelated key: `running` means working, `finished`/`idle` with
nothing owed means over. A `finished` that arrives while one of our tools is
still mid-run is not the end of the turn.

## 9. What this is not

- It is **not** a bypass of Windows security. The agent runs with the same
  privileges as BLACKVAULT, cannot type into a higher-integrity window (UIPI),
  and cannot do anything the signed-in user could not do by hand.
- It is **not** an autonomous daemon. Nothing runs unless the user sends a
  message, and a sensitive step waits for the user's own answer.
- It is **not** a second security boundary. The vault's factors (`auth.rs`),
  the file seals (`fileprot.rs`) and the app guard (`applock.rs`) are unchanged
  by the agent, and the agent has no command that weakens them.
