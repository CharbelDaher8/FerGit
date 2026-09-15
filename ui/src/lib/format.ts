// Text formatting shared by the graph rows and the details panel.

import type { Oid, Upstream } from "./bindings";

function pad2(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/** The abbreviated commit id shown in the UI: the first 8 hex characters. */
export function shortId(id: Oid): string {
  return id.slice(0, 8);
}

/** Compact date and time in the viewer's local time zone, e.g. `2026-09-15 14:03`. */
export function formatLocalTime(seconds: number): string {
  const date = new Date(seconds * 1000);
  return (
    `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ` +
    `${pad2(date.getHours())}:${pad2(date.getMinutes())}`
  );
}

/**
 * A signature's time as its author's clock showed it, with that clock's UTC offset, the way git
 * prints it: `2026-09-15 14:03:22 +0200`.
 */
export function formatSignatureTime(seconds: number, offsetMinutes: number): string {
  const date = new Date((seconds + offsetMinutes * 60) * 1000);
  const sign = offsetMinutes < 0 ? "-" : "+";
  const offset = Math.abs(offsetMinutes);
  return (
    `${date.getUTCFullYear()}-${pad2(date.getUTCMonth() + 1)}-${pad2(date.getUTCDate())} ` +
    `${pad2(date.getUTCHours())}:${pad2(date.getUTCMinutes())}:${pad2(date.getUTCSeconds())} ` +
    `${sign}${pad2(Math.floor(offset / 60))}${pad2(offset % 60)}`
  );
}

/**
 * The compact status shown after a branch badge: `↑2 ↓1` (zero parts omitted, nothing when in
 * sync), or `gone` when the upstream no longer exists. Empty without an upstream.
 */
export function upstreamSuffix(upstream: Upstream | null): string {
  if (upstream === null) return "";
  if (upstream.state.kind === "gone") return "gone";
  const { ahead, behind } = upstream.state;
  if (ahead > 0 && behind > 0) return `↑${ahead} ↓${behind}`;
  if (ahead > 0) return `↑${ahead}`;
  return behind > 0 ? `↓${behind}` : "";
}

/** The upstream status in words, for a tooltip: "2 ahead, 1 behind origin/main". */
export function upstreamTitle(upstream: Upstream): string {
  if (upstream.state.kind === "gone") return `${upstream.name} is gone`;
  const { ahead, behind } = upstream.state;
  if (ahead > 0 && behind > 0) return `${ahead} ahead, ${behind} behind ${upstream.name}`;
  if (ahead > 0) return `${ahead} ahead of ${upstream.name}`;
  if (behind > 0) return `${behind} behind ${upstream.name}`;
  return `up to date with ${upstream.name}`;
}
