// BLACKVAULT — the Agent pane. This is the command centre for the desktop
// agent: a chat on the left, the live turn in the middle, and the machine it
// can actually drive on the right.
//
// The pane owns no state of its own beyond what is on screen. Everything shown
// comes from the Rust runner through `bridge`: the message list is appended from
// the live event stream, the busy/phase indicators come from `agent_status`, and
// a tool that needs approval arrives as an `agentToolChallenge`.

import * as bridge from "../lib/bridge";
import { mergeChunk } from "../lib/agentstream";
import { clear, el, mount, relTime, svgIcon } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { renderMarkdown } from "../lib/markdown";
import type {
  AgentConversation,
  AgentEvent,
  AgentMessage,
  AgentState,
  AgentTool,
} from "../lib/types";
import { paneHead, type Pane } from "../ui/pane";

/** The id used for a chat that has not been created yet. */
const NEW_CHAT = "new";

export const agentPane: Pane = {
  id: "agent",
  title: "Agent",
  // The agent looks after itself: it holds the live chat and listens to the
  // core directly, so the shell must never rebuild it under the user.
  selfUpdating: true,
  render(host, ctx) {
    const conversations: AgentConversation[] = [];
    const messages: AgentMessage[] = [];
    let state: AgentState | null = null;
    let tools: AgentTool[] = [];
    let active = NEW_CHAT;
    let sending = false;
    let hasKey = false;

    // ------------------------------------------------------------ layout
    const rail = el("div", { class: "bv-agent-rail" });
    const stream = el("div", { class: "bv-agent-stream" });
    const side = el("div", { class: "bv-agent-side" });
    const composer = el("div", { class: "bv-agent-composer" });
    const input = el("textarea", {
      class: "bv-agent-input",
      rows: "1",
      placeholder: "কিছু লিখুন… (Enter পাঠাবে, Shift+Enter নতুন লাইন)",
      spellcheck: false,
    }) as HTMLTextAreaElement;
    const sendBtn = el("button", {
      class: "bv-agent-send",
      type: "button",
      title: "পাঠান",
    }) as HTMLButtonElement;
    sendBtn.appendChild(svgIcon(ICONS.send, 18));

    const micBtn = el("button", {
      class: "bv-agent-mic",
      type: "button",
      title: "চাপুন, তারপর কমান্ড বলুন",
    }) as HTMLButtonElement;
    micBtn.appendChild(svgIcon(ICONS.mic, 18));

    const stopBtn = el("button", {
      class: "bv-agent-stop",
      type: "button",
      title: "থামান",
    }) as HTMLButtonElement;
    stopBtn.appendChild(svgIcon(ICONS.stop, 15));

    const banner = el("div", { class: "bv-agent-banner" });
    const stageLine = el("div", { class: "bv-agent-stage" });
    const keyCard = el("div", { class: "bv-agent-key" });

    const autoGrow = (): void => {
      input.style.height = "auto";
      input.style.height = `${Math.min(input.scrollHeight, 180)}px`;
    };

    const header = paneHead(
      "Agent",
      "BLACKVAULT-এর নিজের AI এজেন্ট — ক্লাউডে বানায়, এই কম্পিউটারে চালায়",
    );

    mount(
      host,
      header,
      el(
        "div",
        { class: "bv-agent" },
        rail,
        el(
          "div",
          { class: "bv-agent-main" },
          banner,
          stageLine,
          stream,
          keyCard,
          composer,
        ),
        side,
      ),
    );
    composer.appendChild(input);
    composer.appendChild(micBtn);
    composer.appendChild(stopBtn);
    composer.appendChild(sendBtn);

    // ------------------------------------------------------------ actions
    const send = async (): Promise<void> => {
      const text = input.value.trim();
      if (!text || sending) return;
      // A brand-new chat gets its own id here, not the shared "new" sentinel.
      // Sending under "new" bound every first message to the same chat, so each
      // new chat overwrote the last one's conversation and "new chat" resumed
      // the old thread.
      const localId = active === NEW_CHAT ? newLocalId() : active;
      input.value = "";
      autoGrow();
      sending = true;
      renderComposer();
      seen.clear();
      messages.push(localMessage("user", "message", text));
      renderStream();
      try {
        await bridge.agentSend(localId, text);
        active = localId;
        // The chat now exists on the cloud; refresh the rail so its title and
        // remote id appear.
        const list = await bridge.agentConversations();
        conversations.splice(0, conversations.length, ...list);
        renderRail();
      } catch (e) {
        ctx.toast("পাঠানো যায়নি", String(e), "critical");
        messages.push(localMessage("system", "error", String(e)));
        renderStream();
      } finally {
        sending = false;
        renderComposer();
      }
    };

    sendBtn.addEventListener("click", () => void send());
    stopBtn.addEventListener("click", async () => {
      stopBtn.disabled = true;
      try {
        await bridge.agentInterrupt();
      } catch (e) {
        ctx.toast("থামানো যায়নি", String(e), "warn");
      }
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        void send();
      }
    });
    input.addEventListener("input", autoGrow);

    // Dictation. The mic button turns the message box into a voice-typed field:
    // every recognised utterance is appended to the text area and nothing is
    // matched against the command list, so saying "lock this pc" while writing a
    // message writes it down instead of locking the desktop.
    let dictating = false;
    micBtn.addEventListener("click", async () => {
      try {
        const status = dictating ? await bridge.dictationStop() : await bridge.dictationStart();
        if (!status.running) {
          ctx.toast("ভয়েস ইঞ্জিন বন্ধ", "Voice প্যান থেকে চালু করুন।", "warn");
          return;
        }
        dictating = !dictating;
      } catch (e) {
        ctx.toast("মাইক চালু করা যায়নি", String(e), "warn");
        return;
      }
      micBtn.classList.toggle("is-listening", dictating);
      micBtn.title = dictating ? "লিখছি… আবার চাপলে থামবে" : "চাপুন, তারপর বলে লিখুন";
      if (dictating) input.focus();
    });

    const openChat = async (localId: string): Promise<void> => {
      active = localId;
      seen.clear();
      messages.splice(0, messages.length);
      renderRail();
      renderStream();
      if (localId === NEW_CHAT) return;
      try {
        const history = await bridge.agentHistory(localId, 1000);
        messages.splice(0, messages.length, ...history);
        renderStream();
      } catch (e) {
        ctx.toast("ইতিহাস আনা যায়নি", String(e), "warn");
      }
    };

    const newChat = (): void => {
      active = NEW_CHAT;
      seen.clear();
      messages.splice(0, messages.length);
      renderRail();
      renderStream();
      input.focus();
    };

    // ------------------------------------------------------------ render
    function renderRail(): void {
      clear(rail);
      const head = el(
        "div",
        { class: "bv-agent-rail-head" },
        el("span", { class: "bv-agent-rail-title" }, "চ্যাট"),
      );
      const plus = el("button", { class: "bv-icon-btn", type: "button", title: "নতুন চ্যাট" });
      plus.appendChild(svgIcon(ICONS.plus, 15));
      plus.addEventListener("click", newChat);
      head.appendChild(plus);
      rail.appendChild(head);

      const list = el("div", { class: "bv-agent-chat-list" });
      list.appendChild(chatRow(NEW_CHAT, "নতুন চ্যাট", null));
      for (const c of conversations) {
        list.appendChild(chatRow(c.localId, c.title || "চ্যাট", c.lastSeen));
      }
      rail.appendChild(list);

      const foot = el("div", { class: "bv-agent-rail-foot" });
      foot.appendChild(
        el(
          "div",
          { class: "bv-agent-keyline" },
          svgIcon(state?.keyPresent ? ICONS.check : ICONS.warning, 13),
          el("span", {}, state?.keyPresent ? "ক্লাউড কানেক্টেড" : "ক্লাউড কী নেই"),
        ),
      );
      foot.appendChild(
        el(
          "div",
          { class: "bv-agent-keyline bv-dim" },
          el("span", {}, `${state?.warmSandboxes ?? 0} স্যান্ডবক্স প্রস্তুত`),
        ),
      );
      rail.appendChild(foot);
    }

    function chatRow(localId: string, title: string, lastSeen: number | null): HTMLElement {
      const row = el("button", {
        class: `bv-agent-chat${localId === active ? " is-active" : ""}`,
        type: "button",
      }) as HTMLButtonElement;
      row.appendChild(svgIcon(ICONS.bubble, 14));
      const body = el("div", { class: "bv-agent-chat-body" });
      body.appendChild(el("div", { class: "bv-agent-chat-title" }, title));
      if (lastSeen) {
        body.appendChild(
          el("div", { class: "bv-agent-chat-time" }, relTime(new Date(lastSeen * 1000).toISOString())),
        );
      }
      row.appendChild(body);
      row.addEventListener("click", () => void openChat(localId));
      if (localId !== NEW_CHAT) {
        const del = el("span", { class: "bv-agent-chat-del", title: "মুছে ফেলুন" });
        del.appendChild(svgIcon(ICONS.trash, 13));
        del.addEventListener("click", async (e) => {
          e.stopPropagation();
          try {
            await bridge.agentForget(localId);
            const index = conversations.findIndex((c) => c.localId === localId);
            if (index >= 0) conversations.splice(index, 1);
            if (active === localId) newChat();
            else renderRail();
          } catch (err) {
            ctx.toast("মুছে ফেলা যায়নি", String(err), "warn");
          }
        });
        row.appendChild(del);
      }
      return row;
    }

    // The answer as it arrives. A delta updates this one node instead of
    // rebuilding the transcript, and the card is dropped when the final message
    // for the turn lands, so nothing is shown twice.
    let streaming: {
      localId: string;
      node: HTMLElement;
      body: HTMLElement;
      text: string;
      thought: string;
      details: HTMLElement;
      thoughtBody: HTMLElement;
    } | null = null;

    // Event ids already shown. A re-joined socket replays the events that
    // arrived while it was down, and the same id must not be drawn twice — that
    // is what made the transcript repeat itself mid-turn.
    const seen = new Set<string>();

    /** True the first time an event id is seen; false for a replay. */
    function firstSight(id: string): boolean {
      if (!id) return true;
      if (seen.has(id)) return false;
      seen.add(id);
      return true;
    }

    function renderStream(): void {
      clear(stream);
      streaming = null;
      if (!messages.length) {
        stream.appendChild(emptyState());
        return;
      }
      for (const m of messages) {
        const node = messageCard(m);
        if (node) stream.appendChild(node);
      }
      stream.scrollTop = stream.scrollHeight;
    }

    function clearStreaming(): void {
      if (!streaming) return;
      streaming.node.remove();
      streaming = null;
    }

    function dropEmptyState(): void {
      stream.querySelector(".bv-agent-empty")?.remove();
    }

    /** Append one finished message without touching the ones already shown. */
    function appendMessage(m: AgentMessage): void {
      const node = messageCard(m);
      if (!node) return;
      dropEmptyState();
      stream.appendChild(node);
      stream.scrollTop = stream.scrollHeight;
    }

    function pushDelta(localId: string, ev: AgentEvent): void {
      // Both halves of a live answer are streamed: the model's reasoning as it
      // thinks, and the answer as it is written. The reasoning is what fills the
      // long pause before any answer text exists — without it the transcript sits
      // still for the whole thinking phase and the reply then lands in one burst.
      const text = ev.text ?? "";
      const thought = ev.thought ?? "";
      if (!text && !thought) return;
      if (!streaming || streaming.localId !== localId) {
        clearStreaming();
        dropEmptyState();
        const card = el("div", { class: "bv-msg bv-msg-assistant bv-msg-kind-delta is-streaming" });
        const head = el("div", { class: "bv-msg-head" });
        head.appendChild(el("span", { class: "bv-msg-who" }, "BLACKVAULT"));
        card.appendChild(head);
        const details = el("details", { class: "bv-msg-thought" });
        details.appendChild(el("summary", {}, "চিন্তা"));
        const thoughtBody = el("div", { class: "bv-msg-thought-body" });
        details.appendChild(thoughtBody);
        details.hidden = true;
        card.appendChild(details);
        const body = el("div", { class: "bv-msg-body" });
        card.appendChild(body);
        stream.appendChild(card);
        streaming = { localId, node: card, body, text: "", thought: "", details, thoughtBody };
      }
      if (thought) {
        streaming.details.hidden = false;
        streaming.thought = mergeChunk(streaming.thought, thought);
        streaming.thoughtBody.textContent = streaming.thought;
      }
      if (text) {
        streaming.text = mergeChunk(streaming.text, text);
        streaming.body.textContent = streaming.text;
      }
      stream.scrollTop = stream.scrollHeight;
    }

    function emptyState(): HTMLElement {
      const wrap = el("div", { class: "bv-agent-empty" });
      wrap.appendChild(svgIcon(ICONS.agent, 40, "bv-agent-empty-icon"));
      wrap.appendChild(el("h2", { class: "bv-agent-empty-title" }, "আমি BLACKVAULT"));
      wrap.appendChild(
        el(
          "p",
          { class: "bv-agent-empty-text" },
          "এই কম্পিউটারটা আমি চালাতে পারি — অ্যাপ খুলি, কমান্ড চালাই, ফাইল লিখি, স্ক্রিন দেখি, " +
            "আর নিজের ক্লাউড স্যান্ডবক্সে কোডও বানাই। নিচে লিখে বলুন, বা ভয়েস কমান্ড দিয়ে ডাকুন।",
        ),
      );
      const hints = el("div", { class: "bv-agent-hints" });
      for (const h of [
        "Chrome খুলে আমার প্রোজেক্টটা দেখাও",
        "ডেস্কটপের স্ক্রিনে কী আছে বলো",
        "আমার Downloads ফোল্ডার পরিষ্কার করার স্ক্রিপ্ট লেখো ও চালাও",
        "এই প্রোজেক্টে একটা বাগ খুঁজে ঠিক করো",
      ]) {
        const chip = el("button", { class: "bv-agent-hint", type: "button" }, h);
        chip.addEventListener("click", () => {
          input.value = h;
          autoGrow();
          input.focus();
        });
        hints.appendChild(chip);
      }
      wrap.appendChild(hints);
      return wrap;
    }

    function messageCard(m: AgentMessage): HTMLElement | null {
      if (m.kind === "delta") return null;
      const card = el("div", { class: `bv-msg bv-msg-${m.role || "system"} bv-msg-kind-${m.kind}` });
      const head = el("div", { class: "bv-msg-head" });
      const who =
        m.kind === "error"
          ? "ত্রুটি"
          : m.role === "user"
            ? "আপনি"
            : m.kind === "action" || m.kind === "observation"
              ? "টুল"
              : "BLACKVAULT";
      head.appendChild(el("span", { class: "bv-msg-who" }, who));
      if (m.tool) head.appendChild(el("span", { class: "bv-msg-tool" }, m.tool));
      if (m.at) head.appendChild(el("span", { class: "bv-msg-time" }, shortTime(m.at)));
      card.appendChild(head);

      if (m.thought && m.kind === "message") {
        const details = el("details", { class: "bv-msg-thought" });
        details.appendChild(el("summary", {}, "চিন্তা"));
        details.appendChild(el("div", { class: "bv-msg-thought-body" }, m.thought));
        card.appendChild(details);
      }

      const text = m.kind === "action" ? m.summary || m.tool : m.text;
      if (text) {
        const body = el("div", { class: "bv-msg-body" });
        // An assistant's prose is Markdown; render it. Tool output, a user's own
        // message and an error are shown verbatim — they are not prose and their
        // characters (paths, diffs, regex) must not be reinterpreted.
        const asMarkdown = m.kind === "message" && m.role === "assistant";
        if (asMarkdown) {
          body.classList.add("is-md");
          body.appendChild(renderMarkdown(text, openLink));
        } else {
          body.textContent = text;
        }
        // A tool's raw output can be thousands of lines. Clamp it so one
        // command cannot swallow the transcript, and let it be expanded.
        const long = m.kind === "observation" && text.length > 600;
        if (long) {
          body.classList.add("is-clamped");
          body.addEventListener("click", () => body.classList.toggle("is-clamped"));
        }
        card.appendChild(body);
      }
      if (m.kind === "observation") {
        card.classList.add(m.ok ? "is-ok" : "is-fail");
      }
      // A finished answer can be taken away in one click; the live card cannot,
      // it is still changing.
      if (m.kind === "message" && m.role === "assistant" && text) {
        const copy = el("button", { class: "bv-msg-copy", type: "button", title: "কপি" });
        copy.appendChild(svgIcon(ICONS.copy, 13));
        copy.addEventListener("click", async () => {
          try {
            await navigator.clipboard.writeText(text);
            copy.classList.add("is-done");
          } catch {
            /* the clipboard can be refused; the button simply does nothing */
          }
        });
        head.appendChild(copy);
      }
      return card;
    }

    function renderSide(): void {
      clear(side);
      side.appendChild(
        el(
          "div",
          { class: "bv-agent-side-card" },
          el(
            "div",
            { class: "bv-agent-side-title" },
            svgIcon(ICONS.cpu, 14),
            el("span", {}, "এজেন্ট"),
          ),
          statusRow("মডেল", state?.model ?? "—"),
          statusRow("অবস্থা", phaseLabel(state)),
          statusRow("চ্যাট", String(state?.conversations ?? conversations.length)),
          statusRow("প্রস্তুত স্যান্ডবক্স", String(state?.warmSandboxes ?? 0)),
          state?.lastError ? el("div", { class: "bv-agent-side-error" }, state.lastError) : null,
        ),
      );

      const toolCard = el(
        "div",
        { class: "bv-agent-side-card" },
        el(
          "div",
          { class: "bv-agent-side-title" },
          svgIcon(ICONS.zap, 14),
          el("span", {}, `টুল (${tools.length})`),
        ),
      );
      const toolList = el("div", { class: "bv-agent-tools" });
      for (const t of tools) {
        const row = el("div", { class: "bv-agent-tool" });
        row.appendChild(el("span", { class: "bv-agent-tool-name" }, t.name));
        if (t.sensitive) {
          row.appendChild(el("span", { class: "bv-agent-tool-badge" }, "অনুমোদন"));
        }
        row.title = t.description;
        toolList.appendChild(row);
      }
      toolCard.appendChild(toolList);
      side.appendChild(toolCard);
    }

    function statusRow(label: string, value: string): HTMLElement {
      return el(
        "div",
        { class: "bv-agent-status-row" },
        el("span", { class: "bv-dim" }, label),
        el("span", {}, value),
      );
    }

    function renderComposer(): void {
      sendBtn.disabled = sending;
      sendBtn.classList.toggle("is-busy", sending);
      // Stop is offered exactly while the agent is working on a turn.
      const busy = Boolean(state?.busy);
      stopBtn.classList.toggle("is-on", busy);
      stopBtn.disabled = !busy;
    }

    function renderStage(): void {
      const phase = state?.phase ?? "done";
      stageLine.textContent = state?.busy ? state.stage || phaseText(phase) : "";
      stageLine.classList.toggle("is-visible", Boolean(state?.busy));
      renderBanner();
    }

    /**
     * The one line that answers "can I send now?".
     *
     * Four states, in priority order: no key (nothing can work), busy (a turn is
     * running), ready (a conversation is warm), idle (ready to start one).
     */
    function renderBanner(): void {
      clear(banner);
      const hasKey = Boolean(state?.keyPresent);
      let dot = "off";
      let label = "ক্লাউড কী নেই";
      let detail = "সেটিংস → Desktop agent-এ কী যোগ করুন";
      if (hasKey && state?.busy) {
        dot = "warn";
        label = state.stage || phaseText(state.phase);
        detail = "কাজ চলছে…";
      } else if (hasKey && (state?.conversations ?? 0) > 0) {
        dot = "on";
        label = "রেডি — এখন মেসেজ দিতে পারবেন";
        detail = "Enter চাপলে বা বললে উত্তর আসবে";
      } else if (hasKey) {
        dot = "on";
        label = "রেডি — নতুন চ্যাট শুরু হবে";
        detail = "প্রথম মেসেজে একটা কনভার্সেশন তৈরি হবে";
      }
      banner.appendChild(el("span", { class: `bv-dot ${dot}` }));
      const body = el("div", { class: "bv-agent-banner-body" });
      body.appendChild(el("div", { class: "bv-agent-banner-label" }, label));
      body.appendChild(el("div", { class: "bv-agent-banner-detail" }, detail));
      banner.appendChild(body);
      banner.classList.toggle("is-busy", Boolean(hasKey && state?.busy));
      banner.classList.toggle("is-ready", Boolean(hasKey && !state?.busy));
    }

    /** The key field, shown in place of the composer until a key is stored. */
    function renderKeyCard(): void {
      clear(keyCard);
      keyCard.classList.toggle("is-shown", !hasKey);
      if (hasKey) return;

      const field = el("input", {
        class: "bv-input",
        type: "password",
        placeholder: "sk-…",
        autocomplete: "off",
        "aria-label": "OpenHands API key",
      }) as HTMLInputElement;
      const save = el(
        "button",
        { class: "bv-btn primary", type: "button" },
        "কী সংরক্ষণ করুন",
      ) as HTMLButtonElement;
      const status = el("div", { class: "bv-agent-key-note" }, "");

      const submit = async (): Promise<void> => {
        const value = field.value.trim();
        if (!value) {
          status.textContent = "একটা কী দিন।";
          return;
        }
        save.disabled = true;
        status.textContent = "যাচাই করা হচ্ছে…";
        try {
          const message = await bridge.agentSetKey(value);
          field.value = "";
          hasKey = true;
          ctx.toast("কানেক্টেড", message);
          state = await bridge.agentStatus();
          renderBanner();
          renderSide();
          renderKeyCard();
        } catch (e) {
          status.textContent = String(e);
          ctx.toast("কী গ্রহণ করা হয়নি", String(e), "critical");
        } finally {
          save.disabled = false;
        }
      };

      save.addEventListener("click", () => void submit());
      field.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          void submit();
        }
      });

      mount(
        keyCard,
        el(
          "div",
          { class: "bv-agent-key-card" },
          el(
            "div",
            { class: "bv-agent-key-head" },
            svgIcon(ICONS.key, 16),
            el("span", {}, "OpenHands API কী দিন"),
          ),
          el(
            "div",
            { class: "bv-agent-key-body" },
            "এজেন্টের জন্য একটা কী দরকার। কী ছাড়া সে উত্তর দিতে পারবে না। " +
              "কী DPAPI দিয়ে সিল করা অবস্থায় এই মেশিনেই থাকে — কোথাও পাঠানো হয় না।",
          ),
          el("div", { class: "bv-agent-key-row" }, field, save),
          status,
        ),
      );
    }

    function renderAll(): void {
      renderRail();
      renderSide();
      renderComposer();
      renderStage();
      renderKeyCard();
    }

    // The very first paint, before any state has arrived.
    renderBanner();

    // ------------------------------------------------------------ wiring
    const off = bridge.onCoreEvent((e) => {
      switch (e.type) {
        case "agent": {
          state = e.payload;
          hasKey = Boolean(state.keyPresent);
          renderStage();
          renderSide();
          renderRail();
          renderKeyCard();
          break;
        }
        case "agentEvent": {
          const ev = e.payload.event;
          if (e.payload.localId !== active && active !== NEW_CHAT) break;
          if (ev.kind === "delta") {
            // The answer, live. Kept as its own card until the final message
            // arrives, then replaced by the durable copy.
            pushDelta(e.payload.localId, ev);
            break;
          }
          // The turn's liveness, not a transcript line: it is never rendered as
          // a card. A `finished` retires the live card in case no final message
          // follows it.
          if (ev.kind === "exec_status") {
            if (ev.text === "finished" || ev.text === "stopped") clearStreaming();
            break;
          }
          if (ev.kind === "message" && ev.role === "assistant") clearStreaming();
          // A replayed event (the socket dropped and was re-joined) must not be
          // appended a second time. Deltas never reach here — they are folded
          // above, not appended.
          if (!firstSight(ev.id)) break;
          messages.push({ ...ev, localId: e.payload.localId });
          appendMessage({ ...ev, localId: e.payload.localId });
          break;
        }
        case "agentToolChallenge": {
          // The approval card *is* the record of this step, so it is appended to
          // the stream directly rather than going through the message list.
          stream.appendChild(
            approvalCard(e.payload.tool, e.payload.summary, e.payload.token),
          );
          stream.scrollTop = stream.scrollHeight;
          break;
        }
        case "agentStatus": {
          ctx.toast("সংযোগ", "লাইভ সংযোগ আবার জোড়া হচ্ছে…", "warn");
          break;
        }
        case "agentSpeak": {
          speak(e.payload.text);
          break;
        }
        case "dictated": {
          // Words heard while dictating. Appended to the box rather than sent,
          // so the user can correct them before pressing send.
          if (!e.payload.text) break;
          const sep = input.value && !input.value.endsWith(" ") ? " " : "";
          input.value += sep + e.payload.text;
          autoGrow();
          input.scrollTop = input.scrollHeight;
          break;
        }
        case "agentTranscript": {
          // The server's own transcript, sent when a turn ends. It is the
          // authority: it holds the first message's reply, which the live stream
          // can miss on a cold start. Replacing the list only when it has
          // something to say keeps a just-sent message from vanishing for the
          // instant before the service records it.
          if (e.payload.localId !== active) break;
          if (!e.payload.messages.length) break;
          messages.splice(0, messages.length, ...e.payload.messages);
          renderStream();
          break;
        }
      }
    });

    /** The card that asks the user to allow or refuse one sensitive tool call. */
    function approvalCard(tool: string, summary: string, token: string): HTMLElement {
      const card = el("div", { class: "bv-approval" });
      card.appendChild(
        el(
          "div",
          { class: "bv-approval-head" },
          svgIcon(ICONS.warning, 15),
          el("span", {}, "অনুমোদন দরকার"),
        ),
      );
      card.appendChild(el("div", { class: "bv-approval-tool" }, tool));
      card.appendChild(el("div", { class: "bv-approval-summary" }, summary));

      const actions = el("div", { class: "bv-approval-actions" });
      const allow = el("button", { class: "bv-btn primary", type: "button" }, "অনুমোদন দিন");
      const deny = el("button", { class: "bv-btn", type: "button" }, "না");
      const settle = async (ok: boolean): Promise<void> => {
        allow.disabled = true;
        deny.disabled = true;
        try {
          await bridge.agentAnswer(token, ok);
          card.classList.add(ok ? "is-allowed" : "is-denied");
          card.replaceChildren(
            el("div", { class: "bv-approval-done" }, ok ? "অনুমোদিত" : "প্রত্যাখ্যাত"),
          );
        } catch (e) {
          ctx.toast("উত্তর পাঠানো যায়নি", String(e), "warn");
          allow.disabled = false;
          deny.disabled = false;
        }
      };
      allow.addEventListener("click", () => void settle(true));
      deny.addEventListener("click", () => void settle(false));
      actions.appendChild(allow);
      actions.appendChild(deny);
      card.appendChild(actions);
      return card;
    }

    /** Speak the agent's answer through the system voice. */
    function speak(text: string): void {
      if (!text || typeof window === "undefined" || !("speechSynthesis" in window)) return;
      try {
        const utterance = new SpeechSynthesisUtterance(text.slice(0, 400));
        utterance.rate = 1.02;
        window.speechSynthesis.speak(utterance);
      } catch {
        /* speech is a convenience; never let it break the pane */
      }
    }

    /** Open a link from an answer in the user's own browser. */
    function openLink(url: string): void {
      void bridge.openExternal(url).catch((e) => ctx.toast("লিংক খোলা যায়নি", String(e), "warn"));
    }

    // ------------------------------------------------------------ boot
    void (async () => {
      try {
        const [s, list, toolList, key] = await Promise.all([
          bridge.agentStatus(),
          bridge.agentConversations(),
          bridge.agentTools(),
          bridge.agentHasKey(),
        ]);
        state = s;
        hasKey = Boolean(key) || Boolean(s.keyPresent);
        conversations.splice(0, conversations.length, ...list);
        tools = toolList;
      } catch (e) {
        ctx.toast("এজেন্ট পাওয়া যায়নি", String(e), "warn");
      }
      renderAll();
      renderStream();
    })();

    // The pane lives for the whole session, so this disposer is not called
    // today; it is kept so a future teardown has something to call.
    (host as HTMLElement & { __bvDispose?: () => void }).__bvDispose = off;
  },
};

// ============================================================ helpers

/** A fresh local chat id. Unique per chat so a new chat never reuses another's. */
function newLocalId(): string {
  return `chat-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function localMessage(role: string, kind: string, text: string): AgentMessage {
  return {
    id: `local-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    localId: "",
    kind: kind as AgentMessage["kind"],
    role,
    text,
    thought: "",
    tool: "",
    summary: "",
    ok: kind !== "error",
    at: new Date().toISOString(),
  };
}

function phaseLabel(state: AgentState | null): string {
  if (!state) return "—";
  if (!state.busy) return "প্রস্তুত";
  return phaseText(state.phase);
}

function phaseText(phase: string): string {
  switch (phase) {
    case "preparing":
      return "প্রস্তুত হচ্ছে…";
    case "working":
      return "কাজ করছে…";
    case "awaiting_tool":
      return "টুল চালাচ্ছে…";
    default:
      return "প্রস্তুত";
  }
}

function shortTime(iso: string): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  return new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
