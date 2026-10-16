// Горячие клавиши в записи JMC (`F5`, `Ctrl+NUM7`, `Alt+NUM9`) → проверка нажатия.
//
// Сравниваем по event.code — физической клавише. Так клавиши не зависят от раскладки
// (русская/английская) и от NumLock.

/** Клавиши JMC → KeyboardEvent.code. Буквы и цифры разбираются отдельно. */
const KEY_CODES: Record<string, string[]> = {
  NUM0: ["Numpad0"],
  NUM1: ["Numpad1"],
  NUM2: ["Numpad2"],
  NUM3: ["Numpad3"],
  NUM4: ["Numpad4"],
  NUM5: ["Numpad5"],
  NUM6: ["Numpad6"],
  NUM7: ["Numpad7"],
  NUM8: ["Numpad8"],
  NUM9: ["Numpad9"],
  NUMDEL: ["NumpadDecimal"],
  ADD: ["NumpadAdd"],
  MIN: ["NumpadSubtract"],
  MUL: ["NumpadMultiply"],
  DIV: ["NumpadDivide"],
  // Обычный Enter отправляет команду, поэтому «RETURN» без модификаторов — это Enter на цифровом блоке.
  RETURN: ["NumpadEnter"],
  ENTER: ["NumpadEnter"],
  TAB: ["Tab"],
  "`": ["Backquote"],
  ESC: ["Escape"],
  SPACE: ["Space"],
  BACK: ["Backspace"],
  INS: ["Insert"],
  INSERT: ["Insert"],
  DEL: ["Delete"],
  DELETE: ["Delete"],
  HOME: ["Home"],
  END: ["End"],
  PGUP: ["PageUp"],
  PRIOR: ["PageUp"],
  PGDN: ["PageDown"],
  NEXT: ["PageDown"],
  UP: ["ArrowUp"],
  DOWN: ["ArrowDown"],
  LEFT: ["ArrowLeft"],
  RIGHT: ["ArrowRight"],
};

export interface KeyMatcher {
  codes: string[];
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}

/** Разбирает запись JMC. `null` — непонятная клавиша. */
export function parseKey(spec: string): KeyMatcher | null {
  const parts = spec.split("+").map((p) => p.trim());
  // «Ctrl++» и подобное не встречается; последний кусок — сама клавиша.
  const key = parts.pop()?.toUpperCase() ?? "";
  const mods = parts.map((p) => p.toLowerCase());
  const matcher: KeyMatcher = {
    codes: [],
    ctrl: mods.includes("ctrl"),
    alt: mods.includes("alt"),
    shift: mods.includes("shift"),
  };
  if (KEY_CODES[key]) {
    matcher.codes = [...KEY_CODES[key]];
    // С модификатором (Ctrl+RETURN) подходит любой Enter.
    if (key === "RETURN" || key === "ENTER") {
      if (matcher.ctrl || matcher.alt || matcher.shift) matcher.codes.push("Enter");
    }
  } else if (/^F([1-9]|1[0-2])$/.test(key)) {
    matcher.codes = [key];
  } else if (/^[A-Z]$/.test(key)) {
    matcher.codes = [`Key${key}`];
  } else if (/^[0-9]$/.test(key)) {
    matcher.codes = [`Digit${key}`];
  } else {
    return null;
  }
  return matcher;
}

export function matches(m: KeyMatcher, e: KeyboardEvent): boolean {
  return (
    m.codes.includes(e.code) && m.ctrl === e.ctrlKey && m.alt === e.altKey && m.shift === e.shiftKey
  );
}
