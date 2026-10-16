// Связь с Rust-частью: типы событий и команды.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Кусок текста одного стиля. fg/bg: "c0"…"c15" (цвета темы) или "#rrggbb". */
export interface Span {
  text: string;
  fg?: string;
  bg?: string;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface Exit {
  dir: string;
  vnum: string;
}

export type SessionEvent =
  | { type: "line"; spans: Span[] }
  | { type: "prompt"; spans: Span[] }
  | { type: "room"; vnum: string; name: string; area: string; exits: Exit[] }
  | { type: "info"; text: string }
  | { type: "error"; text: string }
  | { type: "disconnected" };

export function connect(host: string, port: number, charset: string): Promise<void> {
  return invoke("connect", { host, port, charset });
}

export function send(text: string): Promise<void> {
  return invoke("send", { text });
}

export function disconnect(): Promise<void> {
  return invoke("disconnect");
}

/** Подписка на пачки событий сессии. */
export function onSession(handler: (batch: SessionEvent[]) => void): Promise<UnlistenFn> {
  return listen<SessionEvent[]>("session", (event) => handler(event.payload));
}

/** Текст строки без стилей. */
export function plainText(spans: Span[]): string {
  return spans.map((s) => s.text).join("");
}
