// Text formatting shared by the graph rows and the details panel.

import type { Oid } from "./bindings";

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
