//! Карта целиком: все известные комнаты и операции над ними.

use std::collections::BTreeMap;

use crate::model::{Direction, Exit, Room};

/// Расстояние по оси `x` между «островами» — группами комнат, которые
/// не удалось привязать к уже известным комнатам (см. [`Map::update_from_msdp`]).
pub const ISLAND_GAP: i32 = 5;

/// Данные о комнате из MSDP `ROOM` в простом виде (разбор MSDP — не здесь).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomInfo {
    pub vnum: u32,
    pub name: String,
    pub area: String,
    pub zone: u32,
    pub terrain: String,
    /// Видимые выходы: направление и комната, куда он ведёт (если известна).
    pub exits: Vec<(Direction, Option<u32>)>,
    /// MSDP `FROM_ROOM` — комната, где персонаж был до этого.
    pub from_room: Option<u32>,
    /// MSDP `FROM_DIRECTION` — выход ТЕКУЩЕЙ комнаты, ведущий в `from_room`
    /// («откуда пришёл»), а не направление, в котором двигался игрок.
    pub from_direction: Option<Direction>,
}

/// Что произошло при [`Map::update_from_msdp`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateResult {
    /// Комната появилась на карте впервые.
    pub created: bool,
    /// Другие комнаты, стоящие в тех же координатах, что и новая.
    /// Заполняется только для новых комнат; конфликт не мешает добавлению,
    /// разрешать его будет игрок (или будущая автоматика).
    pub conflicts: Vec<u32>,
}

/// Ошибки ручных правок карты.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MapError {
    #[error("комнаты {0} нет на карте")]
    UnknownRoom(u32),
}

/// Все известные комнаты, по номеру (vnum).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Map {
    rooms: BTreeMap<u32, Room>,
}

impl Map {
    /// Пустая карта.
    pub fn new() -> Map {
        Map::default()
    }

    /// Комната по номеру.
    pub fn room(&self, vnum: u32) -> Option<&Room> {
        self.rooms.get(&vnum)
    }

    /// Все комнаты по возрастанию номера.
    pub fn rooms(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    /// Количество комнат.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Пуста ли карта.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Положить комнату как есть (замещает старую с тем же номером).
    /// Нужно хранилищу при загрузке и импорту.
    pub fn insert_room(&mut self, room: Room) {
        self.rooms.insert(room.vnum, room);
    }

    /// Удалить комнату. Выходы других комнат, ведущие в неё, остаются как есть:
    /// они однонаправленные и принадлежат своим комнатам.
    pub fn remove_room(&mut self, vnum: u32) -> Option<Room> {
        self.rooms.remove(&vnum)
    }

    /// Номера комнат, стоящих в точке `(x, y, z)`.
    pub fn rooms_at(&self, x: i32, y: i32, z: i32) -> Vec<u32> {
        self.rooms
            .values()
            .filter(|room| room.coords() == (x, y, z))
            .map(|room| room.vnum)
            .collect()
    }

    /// Применить данные MSDP `ROOM`: создать или обновить комнату.
    ///
    /// Выходы: список из MSDP — авторитетный для ВИДИМЫХ выходов этой комнаты.
    /// Новые добавляются, исчезнувшие видимые удаляются, а скрытые/ручные
    /// (`hidden == true`) сохраняются. Обратные выходы в соседних комнатах
    /// не создаются никогда.
    ///
    /// Координаты задаются только новой комнате; у существующей не меняются
    /// (её мог передвинуть игрок). Правила раскладки новой комнаты, по порядку:
    /// 1. В `from_room` уже известен выход `d`, ведущий сюда → `from + d`.
    /// 2. Есть `from_direction` (выход отсюда в `from_room`) → `from − from_direction`.
    /// 3. Какая-нибудь известная комната имеет выход `d` сюда → `та + d`.
    /// 4. Отсюда есть выход `d` в уже размещённую комнату → `та − d`.
    /// 5. Иначе это «остров»: `(max_x + ISLAND_GAP, 0, 0)`, на пустой карте — `(0, 0, 0)`.
    ///
    /// Про `FROM_ROOM`/`FROM_DIRECTION` (по исходникам сервера,
    /// `msdp_reporters.cpp` и `RemoveCharFromRoom` в `char_handler.cpp`):
    /// - `FROM_ROOM` — комната, из которой персонаж ушёл в последний раз
    ///   любым способом (шаг, телепорт, возврат). При «смотреть» без движения
    ///   он не меняется, т. е. может быть «старым».
    /// - `FROM_DIRECTION` — направление видимого выхода ТЕКУЩЕЙ комнаты, который
    ///   ведёт в `FROM_ROOM`. Это «откуда пришёл», т. е. обратное к движению:
    ///   шли на восток — придёт `w`. Если такого выхода нет (путь
    ///   односторонний, выход скрытый, телепорт), поле не присылается.
    ///
    /// Поэтому правило 1 надёжнее правила 2: оно опирается на реальный выход
    /// из прошлой комнаты, а не на предположение, что обратный путь прямой.
    pub fn update_from_msdp(&mut self, info: RoomInfo) -> UpdateResult {
        let mut result = UpdateResult::default();

        if !self.rooms.contains_key(&info.vnum) {
            let (x, y, z) = self.place_new_room(&info);
            result.created = true;
            result.conflicts = self.rooms_at(x, y, z);
            let mut room = Room::new(info.vnum);
            room.x = x;
            room.y = y;
            room.z = z;
            self.rooms.insert(info.vnum, room);
        }

        // Комната точно есть: только что создали или она была.
        let Some(room) = self.rooms.get_mut(&info.vnum) else {
            return result;
        };
        room.name = info.name;
        room.area = info.area;
        room.zone = info.zone;
        room.terrain = info.terrain;

        // Убираем видимые выходы, которых больше нет в MSDP.
        room.exits.retain(|dir, exit| {
            exit.hidden || info.exits.iter().any(|(msdp_dir, _)| msdp_dir == dir)
        });
        // Добавляем новые и обновляем, куда ведут старые.
        for (dir, to) in info.exits {
            let exit = room.exits.entry(dir).or_insert_with(|| Exit::to(None));
            // Если MSDP не знает, куда ведёт выход, известное значение не затираем.
            if to.is_some() {
                exit.to = to;
            }
        }

        result
    }

    /// Выбрать координаты для новой комнаты (правила — в [`Map::update_from_msdp`]).
    fn place_new_room(&self, info: &RoomInfo) -> (i32, i32, i32) {
        if let Some(from) = info.from_room.and_then(|vnum| self.rooms.get(&vnum)) {
            // 1. Выход из прошлой комнаты, ведущий сюда.
            for (dir, exit) in &from.exits {
                if exit.to == Some(info.vnum) {
                    return shift(from.coords(), *dir, 1);
                }
            }
            // 2. Выход отсюда в прошлую комнату: мы стоим «напротив» него.
            if let Some(back) = info.from_direction {
                return shift(from.coords(), back, -1);
            }
        }

        // 3. Любая известная комната с выходом сюда.
        for room in self.rooms.values() {
            for (dir, exit) in &room.exits {
                if exit.to == Some(info.vnum) {
                    return shift(room.coords(), *dir, 1);
                }
            }
        }

        // 4. Выход отсюда в известную комнату.
        for (dir, to) in &info.exits {
            if let Some(neighbour) = to.and_then(|vnum| self.rooms.get(&vnum)) {
                return shift(neighbour.coords(), *dir, -1);
            }
        }

        // 5. Остров правее всего, что уже есть.
        match self.rooms.values().map(|room| room.x).max() {
            Some(max_x) => (max_x + ISLAND_GAP, 0, 0),
            None => (0, 0, 0),
        }
    }

    /// Задать заметку игрока.
    pub fn set_note(&mut self, vnum: u32, note: &str) -> Result<(), MapError> {
        let room = self.room_mut(vnum)?;
        room.note = note.to_string();
        Ok(())
    }

    /// Добавить (или заменить) выход вручную. Обратный выход не создаётся.
    /// Чтобы выход пережил обновления из MSDP, передайте `hidden: true`.
    pub fn add_exit(&mut self, vnum: u32, dir: Direction, exit: Exit) -> Result<(), MapError> {
        let room = self.room_mut(vnum)?;
        room.exits.insert(dir, exit);
        Ok(())
    }

    /// Удалить выход вручную. Возвращает удалённый выход, если он был.
    pub fn remove_exit(&mut self, vnum: u32, dir: Direction) -> Result<Option<Exit>, MapError> {
        let room = self.room_mut(vnum)?;
        Ok(room.exits.remove(&dir))
    }

    /// Передвинуть комнату. Возвращает другие комнаты в новой точке (конфликты).
    pub fn move_room(&mut self, vnum: u32, x: i32, y: i32, z: i32) -> Result<Vec<u32>, MapError> {
        let room = self.room_mut(vnum)?;
        room.x = x;
        room.y = y;
        room.z = z;
        let conflicts = self
            .rooms_at(x, y, z)
            .into_iter()
            .filter(|other| *other != vnum)
            .collect();
        Ok(conflicts)
    }

    fn room_mut(&mut self, vnum: u32) -> Result<&mut Room, MapError> {
        self.rooms.get_mut(&vnum).ok_or(MapError::UnknownRoom(vnum))
    }
}

/// Сдвинуть координаты на `steps` шагов в направлении `dir` (`-1` — в обратную сторону).
fn shift(coords: (i32, i32, i32), dir: Direction, steps: i32) -> (i32, i32, i32) {
    let (dx, dy, dz) = dir.offset();
    (
        coords.0 + dx * steps,
        coords.1 + dy * steps,
        coords.2 + dz * steps,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::*;

    fn info(vnum: u32, exits: &[(Direction, u32)]) -> RoomInfo {
        RoomInfo {
            vnum,
            name: format!("Комната {vnum}"),
            area: "Тестовая зона".to_string(),
            zone: vnum / 100,
            terrain: "city".to_string(),
            exits: exits.iter().map(|(d, to)| (*d, Some(*to))).collect(),
            from_room: None,
            from_direction: None,
        }
    }

    fn moved(mut info: RoomInfo, from: u32, from_dir: Option<Direction>) -> RoomInfo {
        info.from_room = Some(from);
        info.from_direction = from_dir;
        info
    }

    #[test]
    fn exits_are_one_way() {
        let mut map = Map::new();
        // В A выход на север в B, а в B выходов нет вовсе.
        map.update_from_msdp(info(100, &[(North, 101)]));
        map.update_from_msdp(moved(info(101, &[]), 100, None));

        assert_eq!(map.room(100).unwrap().exits[&North].to, Some(101));
        assert!(map.room(101).unwrap().exits.is_empty());

        // Ручной выход тоже не порождает обратный.
        map.add_exit(101, East, Exit::to(Some(100))).unwrap();
        assert_eq!(map.room(100).unwrap().exits.len(), 1);
    }

    #[test]
    fn layout_follows_movement() {
        let mut map = Map::new();
        map.update_from_msdp(info(100, &[(East, 101)]));
        // Пошли на восток: в 101 выход «w» ведёт обратно, FROM_DIRECTION = w.
        map.update_from_msdp(moved(
            info(101, &[(West, 100), (North, 102)]),
            100,
            Some(West),
        ));
        map.update_from_msdp(moved(info(102, &[(Up, 103)]), 101, None));
        map.update_from_msdp(moved(info(103, &[]), 102, None));

        assert_eq!(map.room(100).unwrap().coords(), (0, 0, 0));
        assert_eq!(map.room(101).unwrap().coords(), (1, 0, 0));
        assert_eq!(map.room(102).unwrap().coords(), (1, 1, 0));
        assert_eq!(map.room(103).unwrap().coords(), (1, 1, 1));
    }

    #[test]
    fn layout_uses_from_direction_when_previous_room_has_no_exit() {
        let mut map = Map::new();
        // В 100 видимого выхода в 101 нет (например, скрытый), но в 101
        // есть выход на юг в 100 — значит, 101 севернее.
        map.update_from_msdp(info(100, &[]));
        map.update_from_msdp(moved(info(101, &[(South, 100)]), 100, Some(South)));
        assert_eq!(map.room(101).unwrap().coords(), (0, 1, 0));
    }

    #[test]
    fn unrelated_room_becomes_island() {
        let mut map = Map::new();
        map.update_from_msdp(info(100, &[(East, 101)]));
        map.update_from_msdp(moved(info(101, &[]), 100, None));
        // Телепорт в никак не связанную комнату.
        map.update_from_msdp(moved(info(5000, &[]), 101, None));
        assert_eq!(map.room(5000).unwrap().coords(), (1 + ISLAND_GAP, 0, 0));
    }

    #[test]
    fn reports_coordinate_conflicts() {
        let mut map = Map::new();
        // Кольцо, которое «не сходится»: 100 →в 101 →с 102 →з 103, а 103 →ю ведёт в 104, не в 100.
        map.update_from_msdp(info(100, &[(East, 101)]));
        map.update_from_msdp(moved(info(101, &[(North, 102)]), 100, None));
        map.update_from_msdp(moved(info(102, &[(West, 103)]), 101, None));
        map.update_from_msdp(moved(info(103, &[(South, 104)]), 102, None));
        let result = map.update_from_msdp(moved(info(104, &[]), 103, None));
        assert!(result.created);
        assert_eq!(result.conflicts, vec![100]);
        assert_eq!(map.room(104).unwrap().coords(), (0, 0, 0));

        let conflicts = map.move_room(104, 0, -1, 0).unwrap();
        assert!(conflicts.is_empty());
        assert_eq!(map.move_room(104, 1, 0, 0).unwrap(), vec![101]);
    }

    #[test]
    fn msdp_exits_replace_visible_but_keep_hidden() {
        let mut map = Map::new();
        map.update_from_msdp(info(100, &[(North, 101), (East, 102)]));
        map.add_exit(
            100,
            Down,
            Exit {
                to: Some(200),
                door: Some("люк".to_string()),
                hidden: true,
            },
        )
        .unwrap();
        map.set_note(100, "тут продают хлеб").unwrap();

        // Выход на восток пропал, на юг появился.
        map.update_from_msdp(info(100, &[(North, 101), (South, 103)]));
        let room = map.room(100).unwrap();
        let dirs: Vec<Direction> = room.exits.keys().copied().collect();
        assert_eq!(dirs, vec![North, South, Down]);
        assert_eq!(room.exits[&Down].door.as_deref(), Some("люк"));
        assert_eq!(room.note, "тут продают хлеб");
        // Повторное обновление координаты не трогает.
        assert_eq!(room.coords(), (0, 0, 0));
    }

    #[test]
    fn manual_edits_on_unknown_room_fail() {
        let mut map = Map::new();
        assert_eq!(map.set_note(1, "x"), Err(MapError::UnknownRoom(1)));
        assert_eq!(map.remove_exit(1, North), Err(MapError::UnknownRoom(1)));
        map.update_from_msdp(info(1, &[(North, 2)]));
        assert_eq!(map.remove_exit(1, North).unwrap(), Some(Exit::to(Some(2))));
        assert_eq!(map.remove_exit(1, North).unwrap(), None);
    }
}
