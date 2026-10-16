// Горячие клавиши в записи JMC (`F5`, `Ctrl+NUM7`, `Alt+NUM9`).
//
// И запись из настроек, и нажатие приводятся к одному виду — «каноническому имени»
// вроде `Ctrl+NUM7` — и сравниваются как строки.
//
// Цифровой блок распознаётся по event.location === 3 (NUMPAD), а не только по event.code:
// в Windows при выключенном NumLock клавиши цифрового блока приходят как стрелки,
// Home, End и т.д., и надёжнее всего — место клавиши плюс то, что она нажала.

const NUMPAD = 3; // KeyboardEvent.DOM_KEY_LOCATION_NUMPAD

/** Клавиша цифрового блока по event.key (NumLock включён или выключен). */
const NUMPAD_BY_KEY: Record<string, string> = {
  "0": "NUM0",
  Insert: "NUM0",
  "1": "NUM1",
  End: "NUM1",
  "2": "NUM2",
  ArrowDown: "NUM2",
  "3": "NUM3",
  PageDown: "NUM3",
  "4": "NUM4",
  ArrowLeft: "NUM4",
  "5": "NUM5",
  Clear: "NUM5",
  Unidentified: "NUM5",
  "6": "NUM6",
  ArrowRight: "NUM6",
  "7": "NUM7",
  Home: "NUM7",
  "8": "NUM8",
  ArrowUp: "NUM8",
  "9": "NUM9",
  PageUp: "NUM9",
  ".": "NUMDEL",
  ",": "NUMDEL",
  Delete: "NUMDEL",
  Decimal: "NUMDEL",
  Separator: "NUMDEL",
  "+": "ADD",
  "-": "MIN",
  "*": "MUL",
  "/": "DIV",
  Enter: "RETURN",
};

/** Клавиша цифрового блока по event.code (запасной путь). */
const NUMPAD_BY_CODE: Record<string, string> = {
  Numpad0: "NUM0",
  Numpad1: "NUM1",
  Numpad2: "NUM2",
  Numpad3: "NUM3",
  Numpad4: "NUM4",
  Numpad5: "NUM5",
  Numpad6: "NUM6",
  Numpad7: "NUM7",
  Numpad8: "NUM8",
  Numpad9: "NUM9",
  NumpadDecimal: "NUMDEL",
  NumpadComma: "NUMDEL",
  NumpadAdd: "ADD",
  NumpadSubtract: "MIN",
  NumpadMultiply: "MUL",
  NumpadDivide: "DIV",
  NumpadEnter: "RETURN",
};

/** Остальные клавиши по event.code. */
const OTHER_BY_CODE: Record<string, string> = {
  Tab: "TAB",
  Backquote: "`",
  Escape: "ESC",
  Space: "SPACE",
  Backspace: "BACK",
  Insert: "INSERT",
  Delete: "DELETE",
  Home: "HOME",
  End: "END",
  PageUp: "PGUP",
  PageDown: "PGDN",
  ArrowUp: "UP",
  ArrowDown: "DOWN",
  ArrowLeft: "LEFT",
  ArrowRight: "RIGHT",
  Enter: "ENTER",
  Pause: "PAUSE",
};

/** Синонимы в записи JMC → каноническое имя. */
const ALIASES: Record<string, string> = {
  ENTER: "ENTER",
  RETURN: "RETURN",
  INS: "INSERT",
  DEL: "DELETE",
  PRIOR: "PGUP",
  NEXT: "PGDN",
  PAGEUP: "PGUP",
  PAGEDOWN: "PGDN",
  DECIMAL: "NUMDEL",
  SUBTRACT: "MIN",
  MULTIPLY: "MUL",
  DIVIDE: "DIV",
  BACKSPACE: "BACK",
  ESCAPE: "ESC",
};

function canonical(key: string, ctrl: boolean, alt: boolean, shift: boolean): string {
  return `${ctrl ? "Ctrl+" : ""}${alt ? "Alt+" : ""}${shift ? "Shift+" : ""}${key}`;
}

/** Запись из настроек → каноническое имя. `null` — пустая запись. */
export function normalizeSpec(spec: string): string | null {
  const parts = spec.split("+").map((p) => p.trim());
  let key = (parts.pop() ?? "").toUpperCase();
  if (key === "" && spec.endsWith("+")) key = "+"; // запись вида «Ctrl++» — маловероятно
  if (key === "") return null;
  key = ALIASES[key] ?? key;
  const mods = parts.map((p) => p.toLowerCase());
  return canonical(key, mods.includes("ctrl"), mods.includes("alt"), mods.includes("shift"));
}

/** Нажатие → каноническое имя. `null` — одиночный модификатор (Ctrl, Shift…). */
export function eventKeyName(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta", "AltGraph", "NumLock", "CapsLock"].includes(e.key)) {
    return null;
  }
  let key: string | undefined;
  if (e.location === NUMPAD) {
    key = NUMPAD_BY_KEY[e.key] ?? NUMPAD_BY_CODE[e.code];
  }
  key ??= NUMPAD_BY_CODE[e.code];
  key ??= OTHER_BY_CODE[e.code];
  if (!key && /^F([1-9]|1[0-9]|2[0-4])$/.test(e.code)) key = e.code;
  if (!key && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  if (!key && /^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  if (!key) return null;
  // В Windows Shift + клавиша цифрового блока при включённом NumLock «отжимает» Shift —
  // для цифрового блока Shift не учитываем, чтобы привязки не терялись.
  const shift = e.location === NUMPAD ? false : e.shiftKey;
  return canonical(key, e.ctrlKey, e.altKey, shift);
}

/** Подробности нажатия — для #keytest. */
export function describeEvent(e: KeyboardEvent): string {
  const where = ["", "левая", "правая", "цифровой блок"][e.location] ?? String(e.location);
  return `key=${JSON.stringify(e.key)} code=${JSON.stringify(e.code)}${where ? ` (${where})` : ""}`;
}
