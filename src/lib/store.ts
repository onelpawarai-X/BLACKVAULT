// BLACKVAULT — a tiny observable store. Panes subscribe; the event bridge
// feeds it. Keeps the UI a pure function of state.

import type {
  EngineStatus,
  HelloStatus,
  MicStatus,
  SecurityEvent,
  Settings,
  VoiceCommand,
  VoiceStatus,
} from "./types";

type Listener = () => void;

export interface AppState {
  status: EngineStatus | null;
  voice: VoiceStatus | null;
  mic: MicStatus | null;
  hello: HelloStatus | null;
  settings: Settings | null;
  commands: VoiceCommand[];
  events: SecurityEvent[];
  heard: string;
  heardFinal: boolean;
  lastCommand: string | null;
  booted: boolean;
  version: string;
  route: string;
  toasts: { id: number; title: string; body: string; severity: string }[];
}

const initial: AppState = {
  status: null,
  voice: null,
  mic: null,
  hello: null,
  settings: null,
  commands: [],
  events: [],
  heard: "",
  heardFinal: false,
  lastCommand: null,
  booted: false,
  version: "",
  route: "dashboard",
  toasts: [],
};

let state: AppState = { ...initial };
const listeners = new Set<Listener>();
let toastSeq = 1;

export function getState(): AppState {
  return state;
}

export function setState(patch: Partial<AppState>): void {
  state = { ...state, ...patch };
  for (const l of listeners) l();
}

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function pushToast(title: string, body = "", severity = "info"): void {
  const id = toastSeq++;
  setState({ toasts: [...state.toasts, { id, title, body, severity }] });
  setTimeout(() => {
    setState({ toasts: state.toasts.filter((t) => t.id !== id) });
  }, 4000);
}

export function navigate(route: string): void {
  if (route === state.route) return;
  setState({ route });
}

/** Merge a partial patch into the current status object. */
export function patchStatus(patch: Partial<EngineStatus>): void {
  if (!state.status) return;
  setState({ status: { ...state.status, ...patch } });
}

export function prependEvent(ev: SecurityEvent): void {
  setState({ events: [ev, ...state.events].slice(0, 400) });
}
