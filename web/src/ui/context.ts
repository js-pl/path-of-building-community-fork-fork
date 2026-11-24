// On the host only: what the user is looking at (the tab and the node,
// item or socket group last selected or hovered), reported to the host for
// the agents' `ui_context` (docs/agent.md, "Live sync"). Reports are ids
// only and debounced, so hovering across the tree sends one message.

import { engine } from "../app";

export type Selection = { node: number } | { item: number } | { slot: string } | { group: number; gem?: number } | null;

const state: { tab: string | null; selection: Selection } = { tab: null, selection: null };
let timer: ReturnType<typeof setTimeout> | undefined;
let sent = "";

export function reportContext(c: { tab?: string; selection?: Selection }) {
  const host = engine.hosted;
  if (!host) return;
  if (c.tab !== undefined && c.tab !== state.tab) {
    state.tab = c.tab;
    state.selection = null;
  }
  if (c.selection !== undefined) state.selection = c.selection;
  clearTimeout(timer);
  timer = setTimeout(() => {
    const msg = JSON.stringify(state);
    if (msg === sent) return;
    sent = msg;
    host.host("context", { ...state }).catch(() => (sent = ""));
  }, 300);
}
