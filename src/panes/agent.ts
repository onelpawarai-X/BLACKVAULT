// BLACKVAULT — the Agent pane. This is the command centre for the desktop
// agent: a chat on the left, the live turn in the middle, and the machine it
// can actually drive on the right.
//
// The pane owns no state of its own beyond what is on screen. Everything shown
// comes from the Rust runner through `bridge`: the message list is appended from
// the live event stream, the busy/phase indicators come from `agent_status`, and
// a tool that needs approval arrives as an `agentToolChallenge`.

import * as bridge from "../lib/bridge";
import { clear, el, mount, relTime, svgIcon } from "../lib/dom";
import { ICONS } from "../lib/icons";
import type {
  AgentConversation,
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
  render(host, ctx) {
    const conversations: AgentConversation[] = [];
    const messages: AgentMessage[] = [];
    let state: AgentState | null = null;
    let tools: AgentTool[] = [];
    let active = NEW_CHAT;
    let sending = false;

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

    const stageLine = el("div", { class: "bv-agent-stage" });

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
          stageLine,
          stream,
          composer,
        ),
        side,
      ),
    );
    composer.appendChild(input);
    composer.appendChild(sendBtn);

    // ------------------------------------------------------------ actions
    const send = async (): Promise<void> => {
      const text = input.value.trim();
      if (!text || sending) return;
      input.value = "";
      autoGrow();
      sending = true;
      renderComposer();
      messages.push(localMessage("user", "message", text));
      renderStream();
      try {
        await bridge.agentSend(active, text);
        // The chat now exists on the cloud; refresh the rail so its title and
        // remote id appear.
        if (active === NEW_CHAT) {
          const list = await bridge.agentConversations();
          conversations.splice(0, conversations.length, ...list);
          const newest = conversations[0];
          if (newest) active = newest.localId;
        }
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
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        void send();
      }
    });
    input.addEventListener("input", autoGrow);

    const openChat = async (localId: string): Promise<void> => {
      active = localId;
      messages.splice(0, messages.length);
      renderRail();
      renderStream();
      if (localId === NEW_CHAT) return;
      try {
        const history = await bridge.agentHistory(localId, 120);
        messages.splice(0, messages.length, ...history);
        renderStream();
      } catch (e) {
        ctx.toast("ইতিহাস আনা যায়নি", String(e), "warn");
      }
    };

    const newChat = (): void => {
      active = NEW_CHAT;
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

    function renderStream(): void {
      clear(stream);
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
        body.textContent = text;
        card.appendChild(body);
      }
      if (m.kind === "observation") {
        card.classList.add(m.ok ? "is-ok" : "is-fail");
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
    }

    function renderStage(): void {
      const phase = state?.phase ?? "done";
      stageLine.textContent = state?.busy
        ? state.stage || phaseText(phase)
        : "";
      stageLine.classList.toggle("is-visible", Boolean(state?.busy));
    }

    function renderAll(): void {
      renderRail();
      renderSide();
      renderComposer();
      renderStage();
    }

    // ------------------------------------------------------------ wiring
    const off = bridge.onCoreEvent((e) => {
      switch (e.type) {
        case "agent": {
          state = e.payload;
          renderStage();
          renderSide();
          renderRail();
          break;
        }
        case "agentEvent": {
          const ev = e.payload.event;
          if (e.payload.localId !== active && active !== NEW_CHAT) break;
          if (ev.kind === "delta") break;
          messages.push({ ...ev, localId: e.payload.localId });
          renderStream();
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

    // ------------------------------------------------------------ boot
    void (async () => {
      try {
        const [s, list, toolList] = await Promise.all([
          bridge.agentStatus(),
          bridge.agentConversations(),
          bridge.agentTools(),
        ]);
        state = s;
        conversations.splice(0, conversations.length, ...list);
        tools = toolList;
      } catch (e) {
        ctx.toast("এজেন্ট পাওয়া যায়নি", String(e), "warn");
      }
      renderAll();
      renderStream();
    })();

    // Clean up the listener when the pane is torn down.
    (host as HTMLElement & { __bvDispose?: () => void }).__bvDispose = off;
  },
};

// ============================================================ helpers

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
