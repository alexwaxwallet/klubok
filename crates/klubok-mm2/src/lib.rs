//! Чтение карт старого маппера MudMapper (Windows, Delphi) — файлов `.mm2`.
//!
//! Файл `.mm2` — это несколько двоичных потоков компонентов Delphi («TPF0»), записанных подряд,
//! по одному на клетку карты. Каждый поток — объект `TMudRoom2`: либо комната, либо «труба»
//! (`Pipe = True`) — соединительная клетка, которой маппер дотягивает выход до далёкой комнаты.
//!
//! Модуль [`dfm`] — универсальный разбор TPF0 в дерево. Поверх него [`load_mm2`] строит простую
//! модель этого крейта: [`Mm2Zone`] с комнатами [`Mm2Room`] и выходами [`Mm2Exit`].
//!
//! Как устроена карта (выяснено по реальным файлам, см. тест `survey`):
//! - Одна зона — один файл; имя файла (без `.mm2`) — имя зоны, на него ссылаются переходы
//!   между зонами (`AutoMaps.Strings`).
//! - Клетки лежат на сетке. В файле свойство `X` — **строка** (растёт на юг), `Y` — **столбец**
//!   (растёт на восток), `L` — уровень (растёт вверх). В модели крейта это `x` (восток),
//!   `y` (юг) и `level`.
//! - Куда ведёт выход, в файле не записано: цель — соседняя клетка в этом направлении. Если там
//!   труба — идём по трубам дальше, до первой комнаты.
//! - Выход в другую зону — строка `AutoMaps.Strings` вида `Зона|на север|Tag[|команды]`.

pub mod convert;
pub mod dfm;

use std::collections::HashMap;

use dfm::{Component, DfmError, Value};
use thiserror::Error;

/// Класс клетки карты в файлах MudMapper.
pub const ROOM_CLASS: &str = "TMudRoom2";

/// Ошибка загрузки карты.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum Mm2Error {
    /// Поток компонентов повреждён.
    #[error("ошибка разбора .mm2: {0}")]
    Dfm(#[from] DfmError),
}

/// Зона — содержимое одного `.mm2`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mm2Zone {
    /// Комнаты в порядке записи в файле.
    pub rooms: Vec<Mm2Room>,
    /// Клетки-трубы (только для рисования; на выходы они уже учтены).
    pub pipes: Vec<Mm2Pipe>,
    /// `DX`/`DY`/`DL` — одинаковы у всех клеток файла. Похоже на сдвиг вида
    /// (куда была прокручена карта при сохранении); точный смысл неизвестен.
    pub view_offset: Option<(i32, i32, i32)>,
    /// Компоненты других классов (не `TMudRoom2`) — пропущены; здесь их классы.
    pub skipped_classes: Vec<String>,
}

/// Комната карты.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mm2Room {
    /// `Tag` — номер комнаты внутри файла (1, 2, 3…), уникален в пределах зоны.
    /// На него ссылаются переходы из других зон. `None`, если в файле его нет.
    pub tag: Option<i32>,
    /// Номер комнаты на сервере (vnum) — число в квадратных скобках в конце названия.
    pub vnum: Option<u32>,
    /// Название без `[vnum]`.
    pub name: String,
    /// Название как в файле (`Hint`), например `Заброшенная тропинка [90001]`.
    pub raw_name: String,
    /// Описание (строки разделены `\r\n`, как пришли с сервера). Пустое, если не сохранено.
    pub description: String,
    /// Столбец на сетке, растёт на восток (в файле — `Y`).
    pub x: i32,
    /// Строка на сетке, растёт на юг (в файле — `X`).
    pub y: i32,
    /// Уровень, растёт вверх (в файле — `L`).
    pub level: i32,
    /// Выходы.
    pub exits: Vec<Mm2Exit>,
    /// Строка выходов как её показал сервер (`Exits`), например `С(В)Ю`: буквы С/Ю/В/З,
    /// `^` — вверх, `v` — вниз, в скобках — закрытая дверь.
    pub game_exits: String,
    /// Цвет клетки (`BColor`), если задан.
    pub color: Option<Mm2Color>,
    /// Заметка игрока (`Note`), часто команды: «г помогу», «лезть нора».
    pub note: String,
    /// Номер значка (`ImageIndex`): 1 — ДТ, 3 — агро-мобы, остальные — значки игрока.
    pub icon: Option<i32>,
    /// `Agro` — в комнате агрессивные мобы.
    pub aggro: bool,
    /// `DT` — смертельная ловушка.
    pub death_trap: bool,
    /// `FlyDT` — ловушка, если не летать (предположительно).
    pub fly_death_trap: bool,
    /// `NoStep` — сюда не ходить (не прокладывать путь), предположительно.
    pub no_step: bool,
    /// `Checked` — отметка игрока (встречается редко, смысл неясен).
    pub checked: bool,
    /// `QuickList` — имя закладки для быстрого перехода.
    pub quick_name: Option<String>,
    /// Переходы в другие зоны (`AutoMaps.Strings`).
    pub zone_links: Vec<Mm2ZoneLink>,
    /// Свойства, которые модель не знает (на будущее; в картах владельца их нет).
    pub extra: Vec<(String, Value)>,
}

/// Выход из комнаты.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mm2Exit {
    /// Направление: `"n"`, `"e"`, `"s"`, `"w"`, `"u"`, `"d"`.
    pub dir: String,
    /// `true` — выход нарисован на карте (флаг `ChN`/`ChS`/…). `false` — выход есть только
    /// в строке выходов сервера, но маппер его не связал (не исследован).
    pub linked: bool,
    /// Комната-цель в этой же зоне: её `Tag`…
    pub to_tag: Option<i32>,
    /// …и её vnum (если известен).
    pub to_vnum: Option<u32>,
    /// Переход в другую зону, если выход ведёт туда.
    pub to_zone: Option<Mm2ZoneLink>,
    /// Закрытая дверь: направление в скобках в строке выходов сервера.
    pub door: bool,
}

/// Переход в другую зону: строка `Зона|на север|Tag[|команды]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mm2ZoneLink {
    /// Имя зоны = имя файла `.mm2` без расширения.
    pub zone: String,
    /// Направление (`"n"`…), либо слово как в файле, если не распознано.
    pub dir: String,
    /// `Tag` комнаты в той зоне.
    pub to_tag: Option<i32>,
    /// Команды перед переходом (например, `отд;г куда`), может быть пусто.
    pub commands: String,
}

/// Труба — соединительная клетка.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mm2Pipe {
    pub x: i32,
    pub y: i32,
    pub level: i32,
    /// `true` — вертикальная (север–юг), иначе горизонтальная (запад–восток).
    pub vertical: bool,
}

/// Цвет клетки.
#[derive(Debug, Clone, PartialEq)]
pub enum Mm2Color {
    /// Цвет RGB.
    Rgb(u8, u8, u8),
    /// Системный цвет Delphi (`clBtnFace` и т.п.) или неизвестное имя — как есть.
    Named(String),
}

/// Описание направления.
struct Dir {
    /// Код в модели: `"n"`, `"e"`…
    code: &'static str,
    /// Свойство-флаг «выход нарисован»: `ChN`…
    prop: &'static str,
    /// Сдвиг по сетке модели: (x — восток, y — юг, level — вверх).
    step: (i32, i32, i32),
    /// Буква в строке выходов сервера.
    letter: char,
    /// Слово в `AutoMaps.Strings`.
    word: &'static str,
}

const DIRS: [Dir; 6] = [
    Dir {
        code: "n",
        prop: "ChN",
        step: (0, -1, 0),
        letter: 'С',
        word: "на север",
    },
    Dir {
        code: "e",
        prop: "ChE",
        step: (1, 0, 0),
        letter: 'В',
        word: "на восток",
    },
    Dir {
        code: "s",
        prop: "ChS",
        step: (0, 1, 0),
        letter: 'Ю',
        word: "на юг",
    },
    Dir {
        code: "w",
        prop: "ChW",
        step: (-1, 0, 0),
        letter: 'З',
        word: "на запад",
    },
    Dir {
        code: "u",
        prop: "ChU",
        step: (0, 0, 1),
        letter: '^',
        word: "вверх",
    },
    Dir {
        code: "d",
        prop: "ChD",
        step: (0, 0, -1),
        letter: 'v',
        word: "вниз",
    },
];

/// Загружает карту из байтов `.mm2`. Пустой файл — пустая зона.
pub fn load_mm2(bytes: &[u8]) -> Result<Mm2Zone, Mm2Error> {
    let components = dfm::parse_all(bytes)?;
    let mut zone = Mm2Zone::default();

    // Сначала разбираем клетки, потом по сетке ищем цели выходов.
    for c in &components {
        if c.class != ROOM_CLASS {
            zone.skipped_classes.push(c.class.clone());
            continue;
        }
        if zone.view_offset.is_none() {
            zone.view_offset = Some((int(c, "DX"), int(c, "DY"), int(c, "DL")));
        }
        if flag(c, "Pipe") {
            let exits = text(c, "Exits");
            zone.pipes.push(Mm2Pipe {
                x: int(c, "Y"),
                y: int(c, "X"),
                level: int(c, "L"),
                vertical: flag(c, "ChN") || flag(c, "ChS") || exits.contains(['С', 'Ю']),
            });
        } else {
            zone.rooms.push(room_from_component(c));
        }
    }

    resolve_exits(&mut zone);
    Ok(zone)
}

/// Что лежит в клетке сетки.
#[derive(Clone, Copy)]
enum Cell {
    /// Комната: индекс в `zone.rooms`.
    Room(usize),
    Pipe,
}

/// Находит цели выходов: соседняя клетка, через трубы — до комнаты.
fn resolve_exits(zone: &mut Mm2Zone) {
    let mut grid: HashMap<(i32, i32, i32), Cell> = HashMap::new();
    for pipe in &zone.pipes {
        grid.insert((pipe.x, pipe.y, pipe.level), Cell::Pipe);
    }
    // Комнаты кладём после труб: если в клетке и то и другое, важнее комната.
    // При двух комнатах в одной клетке выигрывает первая по файлу.
    for (index, room) in zone.rooms.iter().enumerate() {
        let key = (room.x, room.y, room.level);
        if !matches!(grid.get(&key), Some(Cell::Room(_))) {
            grid.insert(key, Cell::Room(index));
        }
    }

    // Цели считаем отдельно, чтобы не менять комнаты, пока читаем соседей.
    let mut targets: Vec<Vec<(Option<i32>, Option<u32>)>> = Vec::new();
    for room in &zone.rooms {
        let mut row = Vec::new();
        for exit in &room.exits {
            let step = DIRS
                .iter()
                .find(|d| d.code == exit.dir)
                .map(|d| d.step)
                .unwrap_or((0, 0, 0));
            let found = if exit.linked {
                walk(&grid, (room.x, room.y, room.level), step)
            } else {
                None
            };
            row.push(match found {
                Some(i) => (zone.rooms[i].tag, zone.rooms[i].vnum),
                None => (None, None),
            });
        }
        targets.push(row);
    }

    for (room, row) in zone.rooms.iter_mut().zip(targets) {
        for (exit, (tag, vnum)) in room.exits.iter_mut().zip(row) {
            exit.to_tag = tag;
            exit.to_vnum = vnum;
            if tag.is_none() {
                // Соседа нет — возможно, это переход в другую зону.
                exit.to_zone = room.zone_links.iter().find(|l| l.dir == exit.dir).cloned();
            }
        }
    }
}

/// Шагает от клетки `from` в направлении `step`, пропуская трубы. Возвращает индекс комнаты.
fn walk(
    grid: &HashMap<(i32, i32, i32), Cell>,
    from: (i32, i32, i32),
    step: (i32, i32, i32),
) -> Option<usize> {
    if step == (0, 0, 0) {
        return None;
    }
    let mut pos = from;
    // Ограничение на случай «кривой» карты, где трубы тянутся бесконечно.
    for _ in 0..10_000 {
        pos = (pos.0 + step.0, pos.1 + step.1, pos.2 + step.2);
        match grid.get(&pos)? {
            Cell::Room(index) => return Some(*index),
            Cell::Pipe => continue,
        }
    }
    None
}

/// Свойства, которые модель разбирает сама (остальные попадут в `extra`).
const KNOWN: [&str; 26] = [
    "Tag",
    "Exits",
    "Hint",
    "ChN",
    "ChS",
    "ChW",
    "ChE",
    "ChU",
    "ChD",
    "BColor",
    "Description",
    "X",
    "Y",
    "L",
    "DX",
    "DY",
    "DL",
    "Agro",
    "DT",
    "FlyDT",
    "NoStep",
    "Checked",
    "Note",
    "QuickList",
    "AutoMaps.Strings",
    "ImageIndex",
];

fn room_from_component(c: &Component) -> Mm2Room {
    let raw_name = text(c, "Hint");
    let (name, vnum) = split_vnum(&raw_name);
    let game_exits = text(c, "Exits");
    let (game_dirs, doors) = parse_game_exits(&game_exits);

    let zone_links: Vec<Mm2ZoneLink> = match c.prop("AutoMaps.Strings") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(parse_zone_link)
            .collect(),
        _ => Vec::new(),
    };

    // Выходы: сначала связанные на карте, потом — только из строки сервера.
    let mut exits = Vec::new();
    for Dir {
        code: dir, prop, ..
    } in DIRS
    {
        let linked = flag(c, prop);
        if linked || game_dirs.iter().any(|d| d == dir) {
            exits.push(Mm2Exit {
                dir: dir.to_string(),
                linked,
                door: doors.iter().any(|d| d == dir),
                ..Mm2Exit::default()
            });
        }
    }

    Mm2Room {
        tag: c.prop("Tag").and_then(Value::as_i64).map(|v| v as i32),
        vnum,
        name,
        raw_name,
        description: text(c, "Description"),
        x: int(c, "Y"),
        y: int(c, "X"),
        level: int(c, "L"),
        exits,
        game_exits,
        color: c.prop("BColor").and_then(parse_color),
        note: text(c, "Note"),
        icon: c
            .prop("ImageIndex")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        aggro: flag(c, "Agro"),
        death_trap: flag(c, "DT"),
        fly_death_trap: flag(c, "FlyDT"),
        no_step: flag(c, "NoStep"),
        checked: flag(c, "Checked"),
        quick_name: c
            .prop("QuickList")
            .and_then(Value::as_str)
            .map(str::to_string),
        zone_links,
        extra: c
            .props
            .iter()
            .filter(|(n, _)| !KNOWN.iter().any(|k| k.eq_ignore_ascii_case(n)))
            .cloned()
            .collect(),
    }
}

/// `"Заброшенная тропинка [90001]"` → (`"Заброшенная тропинка"`, `Some(90001)`).
pub fn split_vnum(raw: &str) -> (String, Option<u32>) {
    let trimmed = raw.trim_end();
    if let Some(body) = trimmed.strip_suffix(']')
        && let Some(open) = body.rfind('[')
        && let Ok(vnum) = body[open + 1..].trim().parse::<u32>()
    {
        return (body[..open].trim_end().to_string(), Some(vnum));
    }
    (raw.trim().to_string(), None)
}

/// Строка выходов сервера → (направления, направления с закрытой дверью).
/// `С(В)Ю` → (`[n, e, s]`, `[e]`). Латинская `C` считается русской `С` (встречается опечатка).
pub fn parse_game_exits(s: &str) -> (Vec<String>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut doors = Vec::new();
    let mut in_door = false;
    for ch in s.chars() {
        match ch {
            '(' => in_door = true,
            ')' => in_door = false,
            _ => {
                let ch = if ch == 'C' { 'С' } else { ch };
                if let Some(d) = DIRS.iter().find(|d| d.letter == ch) {
                    dirs.push(d.code.to_string());
                    if in_door {
                        doors.push(d.code.to_string());
                    }
                }
            }
        }
    }
    (dirs, doors)
}

/// `"Черное Болото|на восток|36|отд"` → переход в зону.
fn parse_zone_link(s: &str) -> Option<Mm2ZoneLink> {
    let mut parts = s.splitn(4, '|');
    let zone = parts.next()?.to_string();
    let word = parts.next()?.trim().to_string();
    let to_tag = parts.next().and_then(|t| t.trim().parse().ok());
    let commands = parts.next().unwrap_or("").to_string();
    let dir = DIRS
        .iter()
        .find(|d| d.word == word)
        .map(|d| d.code.to_string())
        .unwrap_or(word);
    Some(Mm2ZoneLink {
        zone,
        dir,
        to_tag,
        commands,
    })
}

/// Цвет Delphi: целое `$00BBGGRR` или имя `clRed`.
fn parse_color(v: &Value) -> Option<Mm2Color> {
    match v {
        Value::Int(n) => {
            let n = *n as u32;
            if n >> 24 != 0 {
                // Старший байт ≠ 0 — системный цвет Windows, RGB не знаем.
                return Some(Mm2Color::Named(format!("${n:08X}")));
            }
            Some(Mm2Color::Rgb(n as u8, (n >> 8) as u8, (n >> 16) as u8))
        }
        Value::Ident(name) => Some(named_color(name)),
        _ => None,
    }
}

/// Стандартные цвета Delphi (`Graphics.pas`).
fn named_color(name: &str) -> Mm2Color {
    let rgb = match name.to_ascii_lowercase().as_str() {
        "clblack" => (0, 0, 0),
        "clmaroon" => (128, 0, 0),
        "clgreen" => (0, 128, 0),
        "clolive" => (128, 128, 0),
        "clnavy" => (0, 0, 128),
        "clpurple" => (128, 0, 128),
        "clteal" => (0, 128, 128),
        "clgray" => (128, 128, 128),
        "clsilver" => (192, 192, 192),
        "clred" => (255, 0, 0),
        "cllime" => (0, 255, 0),
        "clyellow" => (255, 255, 0),
        "clblue" => (0, 0, 255),
        "clfuchsia" => (255, 0, 255),
        "claqua" => (0, 255, 255),
        "clwhite" => (255, 255, 255),
        "clmoneygreen" => (192, 220, 192),
        "clskyblue" => (166, 202, 240),
        "clcream" => (255, 251, 240),
        "clmedgray" => (160, 160, 164),
        _ => return Mm2Color::Named(name.to_string()),
    };
    Mm2Color::Rgb(rgb.0, rgb.1, rgb.2)
}

fn int(c: &Component, name: &str) -> i32 {
    c.prop(name).and_then(Value::as_i64).unwrap_or(0) as i32
}

fn flag(c: &Component, name: &str) -> bool {
    c.prop(name).and_then(Value::as_bool).unwrap_or(false)
}

fn text(c: &Component, name: &str) -> String {
    c.prop(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests;
