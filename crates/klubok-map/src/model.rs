//! Основные типы карты: направление, выход, комната.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Направление движения. Порядок вариантов (N, E, S, W, U, D) совпадает
/// с порядком выходов на сервере Былин и задаёт порядок сортировки в `BTreeMap`.
///
/// В JSON и в базе направление хранится как MSDP-ключ: `"n"`, `"e"`, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Direction {
    #[serde(rename = "n")]
    North,
    #[serde(rename = "e")]
    East,
    #[serde(rename = "s")]
    South,
    #[serde(rename = "w")]
    West,
    #[serde(rename = "u")]
    Up,
    #[serde(rename = "d")]
    Down,
}

impl Direction {
    /// Все направления по порядку.
    pub const ALL: [Direction; 6] = [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
        Direction::Up,
        Direction::Down,
    ];

    /// Разбор MSDP-ключа выхода (`n`, `e`, `s`, `w`, `u`, `d`).
    pub fn from_msdp_key(key: &str) -> Option<Direction> {
        match key.trim().to_lowercase().as_str() {
            "n" => Some(Direction::North),
            "e" => Some(Direction::East),
            "s" => Some(Direction::South),
            "w" => Some(Direction::West),
            "u" => Some(Direction::Up),
            "d" => Some(Direction::Down),
            _ => None,
        }
    }

    /// MSDP-ключ направления (обратная операция к [`Direction::from_msdp_key`]).
    pub fn msdp_key(self) -> &'static str {
        match self {
            Direction::North => "n",
            Direction::East => "e",
            Direction::South => "s",
            Direction::West => "w",
            Direction::Up => "u",
            Direction::Down => "d",
        }
    }

    /// Разбор русской команды движения: полной («север») или короткой («с»).
    /// Регистр и пробелы по краям не важны.
    pub fn from_command(command: &str) -> Option<Direction> {
        match command.trim().to_lowercase().as_str() {
            "север" | "с" => Some(Direction::North),
            "восток" | "в" => Some(Direction::East),
            "юг" | "ю" => Some(Direction::South),
            "запад" | "з" => Some(Direction::West),
            "вверх" | "вв" => Some(Direction::Up),
            "вниз" | "вн" => Some(Direction::Down),
            _ => None,
        }
    }

    /// Полная русская команда для движения в этом направлении.
    pub fn command(self) -> &'static str {
        match self {
            Direction::North => "север",
            Direction::East => "восток",
            Direction::South => "юг",
            Direction::West => "запад",
            Direction::Up => "вверх",
            Direction::Down => "вниз",
        }
    }

    /// Противоположное направление. Используется ТОЛЬКО для раскладки
    /// координат — обратные выходы по нему никогда не создаются.
    pub fn opposite(self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::East => Direction::West,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }

    /// Смещение координат `(dx, dy, dz)` при шаге в этом направлении.
    /// Ось `y` растёт на север, `x` — на восток, `z` — вверх.
    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Direction::North => (0, 1, 0),
            Direction::East => (1, 0, 0),
            Direction::South => (0, -1, 0),
            Direction::West => (-1, 0, 0),
            Direction::Up => (0, 0, 1),
            Direction::Down => (0, 0, -1),
        }
    }
}

/// Выход из комнаты в одном направлении.
///
/// Выход **однонаправленный**: он описывает только путь из своей комнаты.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Exit {
    /// Куда ведёт выход. `None` — выход известен, но куда ведёт, неизвестно.
    pub to: Option<u32>,
    /// Название двери (например, «ворота»), если она есть.
    #[serde(default)]
    pub door: Option<String>,
    /// Скрытый выход или выход, добавленный игроком вручную. Сервер такие
    /// выходы в MSDP не присылает, поэтому при обновлении из MSDP они не удаляются.
    #[serde(default)]
    pub hidden: bool,
}

impl Exit {
    /// Обычный видимый выход в известную (или неизвестную) комнату.
    pub fn to(to: Option<u32>) -> Exit {
        Exit {
            to,
            door: None,
            hidden: false,
        }
    }
}

/// Комната на карте.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Room {
    /// Номер комнаты на сервере (уникален).
    pub vnum: u32,
    /// Название комнаты.
    pub name: String,
    /// Название зоны (MSDP `AREA`).
    pub area: String,
    /// Номер зоны (MSDP `ZONE`, обычно `vnum / 100`).
    pub zone: u32,
    /// Тип местности (MSDP `TERRAIN`, например `city`).
    pub terrain: String,
    /// Выходы по направлениям.
    pub exits: BTreeMap<Direction, Exit>,
    /// Координаты для рисования карты (строит клиент, у сервера их нет).
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Заметка игрока.
    #[serde(default)]
    pub note: String,
}

impl Room {
    /// Пустая комната с заданным номером, в точке (0, 0, 0).
    pub fn new(vnum: u32) -> Room {
        Room {
            vnum,
            ..Room::default()
        }
    }

    /// Координаты комнаты одной тройкой.
    pub fn coords(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_msdp_keys_and_commands() {
        for dir in Direction::ALL {
            assert_eq!(Direction::from_msdp_key(dir.msdp_key()), Some(dir));
            assert_eq!(Direction::from_command(dir.command()), Some(dir));
            assert_eq!(dir.opposite().opposite(), dir);
        }
        assert_eq!(Direction::from_command("с"), Some(Direction::North));
        assert_eq!(Direction::from_command(" ВВ "), Some(Direction::Up));
        assert_eq!(Direction::from_command("вн"), Some(Direction::Down));
        assert_eq!(Direction::from_command("з"), Some(Direction::West));
        assert_eq!(Direction::from_command("прыгать"), None);
        assert_eq!(Direction::from_msdp_key("x"), None);
    }
}
