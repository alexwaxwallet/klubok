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

export interface Hotkey {
  /** Клавиша в записи JMC: `F5`, `Ctrl+NUM7`, `Alt+NUM9`. */
  key: string;
  body: string;
}

export type SessionEvent =
  | { type: "line"; spans: Span[] }
  | { type: "prompt"; spans: Span[] }
  | { type: "room"; vnum: string; name: string; area: string; exits: Exit[] }
  | { type: "info"; text: string }
  | { type: "echo"; text: string }
  | { type: "warning"; text: string }
  | { type: "error"; text: string }
  | { type: "bell" }
  | { type: "hotkeys"; hotkeys: Hotkey[] }
  | ({ type: "map" } & MapView)
  | { type: "woutput"; window: number; spans: Span[] }
  | { type: "wname"; window: number; name: string }
  | { type: "wshow"; window: number; mode: "toggle" | "show" | "hide" }
  | { type: "wclear"; window: number }
  | { type: "disconnected" };

export interface MapExit {
  /** `n`, `e`, `s`, `w`, `u`, `d`. */
  dir: string;
  to?: number;
  hidden?: boolean;
  door?: boolean;
}

export interface MapRoom {
  vnum: number;
  name: string;
  /** Координаты сетки; y растёт на север. */
  x: number;
  y: number;
  note?: string;
  color?: string;
  /** `dt`, `aggro`, `flydt`, `nostep`. */
  flags?: string[];
  exits: MapExit[];
}

/** Карта текущей зоны и уровня. */
export interface MapView {
  current: number;
  zone: number;
  area: string;
  z: number;
  rooms: MapRoom[];
  /** Оставшийся путь `#go`. */
  path: number[];
}

export interface MapImportResult {
  files: number;
  rooms_total: number;
  rooms_added: number;
  synthetic: number;
  duplicates: number;
  unresolved_exits: number;
  errors: string[];
}

export interface BlankLines {
  max_consecutive: number;
  trim_after_prompt: boolean;
  trim_before_prompt: boolean;
}

export interface Profile {
  name: string;
  host: string;
  port: number;
  charset: string;
  /** Имя персонажа для автовхода. */
  character: string;
  blank_lines: BlankLines;
  /** Имя карты (файл в папке данных). Профили одного мира делят одну карту. */
  map: string;
}

export interface ProfileInfo {
  id: string;
  profile: Profile;
  has_password: boolean;
}

export interface ProfileList {
  profiles: ProfileInfo[];
  data_dir: string;
  password_storage: string;
}

export interface ImportResult {
  aliases: number;
  actions: number;
  variables: number;
  highlights: number;
  hotkeys: number;
  substitutions: number;
  /** Шаблоны триггеров входа (кодировка, имя, пароль). */
  login_actions: string[];
  unsupported: [string, number][];
}

export function newProfile(): Profile {
  return {
    name: "",
    host: "bylins.su",
    port: 4000,
    charset: "utf8",
    character: "",
    blank_lines: { max_consecutive: 1, trim_after_prompt: true, trim_before_prompt: true },
    map: "bylins",
  };
}

export function listProfiles(): Promise<ProfileList> {
  return invoke("list_profiles");
}

/** password: undefined — не менять, "" — забыть. Возвращает id профиля. */
export function saveProfile(id: string | null, profile: Profile, password?: string): Promise<string> {
  return invoke("save_profile", { id, profile, password: password ?? null });
}

export function deleteProfile(id: string): Promise<void> {
  return invoke("delete_profile", { id });
}

export function importSet(id: string, bytes: Uint8Array): Promise<ImportResult> {
  return invoke("import_set", { id, bytes: Array.from(bytes) });
}

/** Импорт карт MudMapper: откроется выбор папки. null — выбор отменён. */
export function importMm2(id: string): Promise<MapImportResult | null> {
  return invoke("import_mm2", { id });
}

/** Настройки автоматизации профиля текстом JMC. */
export function getAutomation(id: string): Promise<string> {
  return invoke("get_automation", { id });
}

/** Сохранить настройки; у подключённого профиля они применяются сразу. */
export function setAutomation(id: string, text: string): Promise<void> {
  return invoke("set_automation", { id, text });
}

export function connect(id: string): Promise<void> {
  return invoke("connect", { id });
}

/** raw — отправить как есть, без алиасов и #-команд (для пароля). */
export function send(text: string, raw = false): Promise<void> {
  return invoke("send", { text, raw });
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
