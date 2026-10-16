// Разбор и сборка текста настроек в формате JMC для редактора.
//
// Rust сохраняет настройки по одной команде в строке (`#alias {имя} {тело} {группа}`),
// поэтому здесь достаточно простого разбора: имя команды и аргументы в фигурных скобках.

export interface Kind {
  /** Ключ вкладки. */
  id: string;
  title: string;
  /** Команда, которой строка записывается. */
  command: string;
  /** Подписи полей (аргументов в фигурных скобках). */
  fields: string[];
  /** Значения полей для новой строки. */
  defaults: string[];
  /** Поле, которое шире остальных (тело команды). */
  wide: number;
}

export const KINDS: Kind[] = [
  {
    id: "alias",
    title: "Алиасы",
    command: "alias",
    fields: ["Алиас", "Команды", "Группа"],
    defaults: ["", "", "default"],
    wide: 1,
  },
  {
    id: "action",
    title: "Триггеры",
    command: "action",
    fields: ["Шаблон", "Команды", "Приоритет", "Группа"],
    defaults: ["", "", "5", "default"],
    wide: 1,
  },
  {
    id: "variable",
    title: "Переменные",
    command: "variable",
    fields: ["Имя", "Значение"],
    defaults: ["", ""],
    wide: 1,
  },
  {
    id: "hot",
    title: "Клавиши",
    command: "hot",
    fields: ["Клавиша", "Команды", "Группа"],
    defaults: ["", "", "default"],
    wide: 1,
  },
  {
    id: "highlight",
    title: "Подсветка",
    command: "highlight",
    fields: ["Цвет", "Шаблон", "Группа"],
    defaults: ["light red", "", "default"],
    wide: 1,
  },
  {
    id: "substitute",
    title: "Замены",
    command: "substitute",
    fields: ["Шаблон", "Замена", "Группа"],
    defaults: ["", "", "default"],
    wide: 0,
  },
  {
    id: "gag",
    title: "Скрытие",
    command: "gag",
    fields: ["Шаблон", "Группа"],
    defaults: ["", "default"],
    wide: 0,
  },
  {
    id: "merge",
    title: "Склейка",
    command: "merge",
    fields: ["Начало", "Конец (шаблон или число строк)", "Разделитель", "Группа"],
    defaults: ["", "", " ", "default"],
    wide: 0,
  },
];

/** Строка, которую редактор показывает полями. */
export interface Item {
  key: number;
  kind: string;
  values: string[];
  /** Для триггера: тип перед шаблоном (`TEXT`, `RAW`…), сохраняется как был. */
  prefix: string;
}

/** Всё содержимое: разобранные строки и остальные (группы, настройки JMC) как есть. */
export interface Parsed {
  items: Item[];
  other: string;
}

let nextKey = 1;

/** Синонимы команд JMC → вкладка. */
const COMMAND_KIND: Record<string, string> = {
  alias: "alias",
  ali: "alias",
  al: "alias",
  action: "action",
  act: "action",
  variable: "variable",
  var: "variable",
  hot: "hot",
  hotkey: "hot",
  highlight: "highlight",
  high: "highlight",
  substitute: "substitute",
  sub: "substitute",
  subs: "substitute",
  gag: "gag",
  merge: "merge",
};

/** Аргументы: `{в скобках}` (с вложенными скобками) или слово до пробела. */
export function parseArgs(text: string): { value: string; braced: boolean }[] {
  const out: { value: string; braced: boolean }[] = [];
  let i = 0;
  while (i < text.length) {
    while (i < text.length && /\s/.test(text[i])) i++;
    if (i >= text.length) break;
    if (text[i] === "{") {
      let depth = 1;
      let j = i + 1;
      for (; j < text.length; j++) {
        if (text[j] === "{") depth++;
        else if (text[j] === "}" && --depth === 0) break;
      }
      out.push({ value: text.slice(i + 1, j), braced: true });
      i = j + 1;
    } else {
      let j = i;
      while (j < text.length && !/\s/.test(text[j]) && text[j] !== "{") j++;
      out.push({ value: text.slice(i, j), braced: false });
      i = j;
    }
  }
  return out;
}

export function parse(text: string): Parsed {
  const items: Item[] = [];
  const other: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const m = /^\s*#(\w+)\s*(.*)$/.exec(line);
    const kindId = m ? COMMAND_KIND[m[1].toLowerCase()] : undefined;
    const kind = KINDS.find((k) => k.id === kindId);
    if (!m || !kind) {
      if (line.trim() !== "") other.push(line);
      continue;
    }
    let args = parseArgs(m[2]);
    let prefix = "";
    if (kind.id === "action" && args.length > 0 && !args[0].braced) {
      prefix = args[0].value;
      args = args.slice(1);
    }
    const values = kind.fields.map((_, i) => args[i]?.value ?? kind.defaults[i]);
    items.push({ key: nextKey++, kind: kind.id, values, prefix });
  }
  return { items, other: other.join("\n") };
}

export function newItem(kind: Kind): Item {
  return { key: nextKey++, kind: kind.id, values: [...kind.defaults], prefix: "" };
}

/** Скобки в значении должны быть парными, иначе строка развалится при чтении. */
export function balanced(value: string): boolean {
  let depth = 0;
  for (const c of value) {
    if (c === "{") depth++;
    else if (c === "}" && --depth < 0) return false;
  }
  return depth === 0;
}

export function itemLine(item: Item): string {
  const kind = KINDS.find((k) => k.id === item.kind)!;
  const args = item.values.map((v) => `{${v}}`).join(" ");
  const prefix = item.prefix ? `${item.prefix} ` : "";
  return `#${kind.command} ${prefix}${args}`;
}

export function build(parsed: Parsed): string {
  const lines: string[] = [];
  for (const kind of KINDS) {
    for (const item of parsed.items) {
      // Пустой первый столбец (имя, шаблон) — недописанная строка, не сохраняем.
      if (item.kind === kind.id && item.values[0].trim() !== "") lines.push(itemLine(item));
    }
  }
  if (parsed.other.trim() !== "") lines.push(parsed.other.trim());
  return lines.join("\n") + "\n";
}
