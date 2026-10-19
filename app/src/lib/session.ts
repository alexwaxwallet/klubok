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
  | ({ type: "status" } & CharStatus)
  | { type: "mapfound"; rooms: FoundRoom[] }
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

/** Состояние персонажа по MSDP; `null` — сервер ещё не сообщил. */
export interface CharStatus {
  hp: number | null;
  max_hp: number | null;
  moves: number | null;
  max_moves: number | null;
  /** Только у «маначных» классов. */
  mana: number | null;
  level: number | null;
  exp: number | null;
  gold: number | null;
  bank: number | null;
  group: GroupMember[];
}

export interface GroupMember {
  name: string;
  /** Проценты. */
  health: number;
  moves: number;
  here: boolean;
  /** Секунд до конца запоминания (0 — всё выучено). */
  mem_time: number;
  affects: string;
  role: string;
  position: string;
}

/** Комната соседнего уровня. */
export interface MapGhost {
  vnum: number;
  x: number;
  y: number;
  z: number;
}

/** Карта текущей зоны и уровня. */
export interface MapView {
  current: number;
  /** Комната в центре: игрок или выбранная в поиске. */
  center: number;
  zone: number;
  area: string;
  /** Показанный уровень. */
  z: number;
  /** Уровень игрока. */
  own_z: number;
  rooms: MapRoom[];
  /** Комнаты уровнем выше и ниже (рисуются полупрозрачно). */
  ghosts: MapGhost[];
  /** Оставшийся путь `#go`. */
  path: number[];
}

/** Правка карты мышью. dy — на север, dz — уровни. */
export type MapEdit =
  | { type: "move"; rooms: number[]; dx: number; dy: number; dz: number }
  | { type: "delete"; rooms: number[] }
  | { type: "color"; rooms: number[]; color: string }
  | { type: "flag"; rooms: number[]; flag: string; on: boolean }
  | { type: "level"; dz: number }
  | { type: "find"; query: string }
  | { type: "focus"; vnum: number | null }
  | { type: "note"; vnum: number; note: string };

/** Найденная комната (поиск в панели карты). */
export interface FoundRoom {
  vnum: number;
  name: string;
  area: string;
  note: string;
}

export function mapEdit(edit: MapEdit): Promise<void> {
  return invoke("map_edit", { edit });
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
    blank_lines: {
      max_consecutive: 1,
      trim_after_prompt: true,
      trim_before_prompt: true,
    },
    map: "bylins",
  };
}

export function listProfiles(): Promise<ProfileList> {
  return invoke("list_profiles");
}

/** password: undefined — не менять, "" — забыть. Возвращает id профиля. */
export function saveProfile(
  id: string | null,
  profile: Profile,
  password?: string,
): Promise<string> {
  return invoke("save_profile", { id, profile, password: password ?? null });
}

export function deleteProfile(id: string): Promise<void> {
  return invoke("delete_profile", { id });
}

export function importSet(
  id: string,
  bytes: Uint8Array,
): Promise<ImportResult> {
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
export function onSession(
  handler: (batch: SessionEvent[]) => void,
): Promise<UnlistenFn> {
  return listen<SessionEvent[]>("session", (event) => handler(event.payload));
}

/** Текст строки без стилей. */
export function plainText(spans: Span[]): string {
  return spans.map((s) => s.text).join("");
}
