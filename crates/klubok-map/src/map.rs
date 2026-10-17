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
    /// Комнаты, которые сдвинули, чтобы освободить место новой (их надо пересохранить).
    pub moved: Vec<u32>,
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
            let ((x, y, z), approach) = self.place_new_room(&info);
            result.created = true;
            // Карты бывают «неровными»: по выходам в одну клетку приходят разные комнаты.
            // Тогда раздвигаем зону по ходу движения, чтобы комнаты не слиплись.
            if let Some(dir) = approach
                && !self.zone_rooms_at(info.zone, x, y, z).is_empty()
            {
                result.moved = self.make_room_at(info.zone, (x, y, z), dir);
            }
            result.conflicts = self.zone_rooms_at(info.zone, x, y, z);
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
    /// Второе значение — направление, «откуда пришли» в эту клетку: в эту сторону
    /// раздвигается зона, если клетка занята (`None` — остров, раздвигать некуда).
    fn place_new_room(&self, info: &RoomInfo) -> ((i32, i32, i32), Option<Direction>) {
        if let Some(from) = info.from_room.and_then(|vnum| self.rooms.get(&vnum)) {
            // 1. Выход из прошлой комнаты, ведущий сюда.
            for (dir, exit) in &from.exits {
                if exit.to == Some(info.vnum) {
                    return (shift(from.coords(), *dir, 1), Some(*dir));
                }
            }
            // 2. Выход отсюда в прошлую комнату: мы стоим «напротив» него.
            if let Some(back) = info.from_direction {
                return (shift(from.coords(), back, -1), Some(back.opposite()));
            }
        }

        // 3. Любая известная комната с выходом сюда.
        for room in self.rooms.values() {
            for (dir, exit) in &room.exits {
                if exit.to == Some(info.vnum) {
                    return (shift(room.coords(), *dir, 1), Some(*dir));
                }
            }
        }

        // 4. Выход отсюда в известную комнату.
        for (dir, to) in &info.exits {
            if let Some(neighbour) = to.and_then(|vnum| self.rooms.get(&vnum)) {
                return (shift(neighbour.coords(), *dir, -1), Some(dir.opposite()));
            }
        }

        // 5. Остров правее всего, что уже есть.
        let island = match self.rooms.values().map(|room| room.x).max() {
            Some(max_x) => (max_x + ISLAND_GAP, 0, 0),
            None => (0, 0, 0),
        };
        (island, None)
    }

    /// Комнаты зоны `zone` в точке `(x, y, z)`. Комнаты других зон не мешают:
    /// карта рисуется по зонам.
    pub fn zone_rooms_at(&self, zone: u32, x: i32, y: i32, z: i32) -> Vec<u32> {
        self.rooms
            .values()
            .filter(|room| room.zone == zone && room.coords() == (x, y, z))
            .map(|room| room.vnum)
            .collect()
    }

    /// Освобождает клетку `at`: комнаты зоны, стоящие в ней, и все, что лежит строго
    /// дальше по направлению `dir`, сдвигаются на шаг в ту же сторону. Комнаты на одной
    /// линии с клеткой (поперёк направления) не трогаем — иначе связи с ними станут косыми. При сдвиге по
    /// горизонтали трогаем только этот уровень. Возвращает сдвинутые комнаты.
    pub fn make_room_at(&mut self, zone: u32, at: (i32, i32, i32), dir: Direction) -> Vec<u32> {
        let (dx, dy, dz) = dir.offset();
        let mut moved = Vec::new();
        for room in self.rooms.values_mut() {
            if room.zone != zone {
                continue;
            }
            if dz == 0 && room.z != at.2 {
                continue;
            }
            // Насколько комната «впереди» клетки по направлению сдвига (только одна ось ≠ 0).
            let ahead = (room.x - at.0) * dx + (room.y - at.1) * dy + (room.z - at.2) * dz;
            if ahead > 0 || room.coords() == at {
                room.x += dx;
                room.y += dy;
                room.z += dz;
                moved.push(room.vnum);
            }
        }
        moved
    }

    /// Разводит комнату `vnum` с теми, кто стоит в той же клетке её зоны.
    ///
    /// Направление ищется по соседу, из которого в комнату ведёт выход и который стоит
    /// ровно в шаге от неё (или по выходу из неё в такого соседа). Комната остаётся на месте,
    /// а остальные из клетки — вместе со всем, что дальше в эту сторону, — сдвигаются.
    /// Возвращает сдвинутые комнаты (пусто — нечего разводить или не понятно, куда).
    pub fn separate(&mut self, vnum: u32) -> Vec<u32> {
        let Some(room) = self.rooms.get(&vnum) else {
            return Vec::new();
        };
        let (cell, zone) = (room.coords(), room.zone);
        if self.zone_rooms_at(zone, cell.0, cell.1, cell.2).len() < 2 {
            return Vec::new();
        }
        let dir = self.approach_direction(vnum);
        let Some(dir) = dir else {
            return Vec::new();
        };
        // Вынимаем комнату, раздвигаем клетку и ставим её обратно.
        let Some(room) = self.rooms.remove(&vnum) else {
            return Vec::new();
        };
        let moved = self.make_room_at(zone, cell, dir);
        self.rooms.insert(vnum, room);
        moved
    }

    /// С какой стороны в комнату «входят» по карте: сосед в шаге, связанный выходом.
    fn approach_direction(&self, vnum: u32) -> Option<Direction> {
        let room = self.rooms.get(&vnum)?;
        for other in self.rooms.values() {
            if other.vnum == vnum || other.zone != room.zone {
                continue;
            }
            for (dir, exit) in &other.exits {
                if exit.to == Some(vnum) && shift(other.coords(), *dir, 1) == room.coords() {
                    return Some(*dir);
                }
            }
        }
        for (dir, exit) in &room.exits {
            let neighbour = exit.to.and_then(|to| self.rooms.get(&to))?;
            if neighbour.zone == room.zone && shift(room.coords(), *dir, 1) == neighbour.coords() {
                return Some(dir.opposite());
            }
        }
        None
    }

    /// Цвет клетки (`#rrggbb`; пусто — обычный).
    pub fn set_color(&mut self, vnum: u32, color: &str) -> Result<(), MapError> {
        self.room_mut(vnum)?.color = color.to_string();
        Ok(())
    }

    /// Поставить или снять пометку (`dt`, `aggro`, `nostep`…).
    pub fn set_flag(&mut self, vnum: u32, flag: &str, on: bool) -> Result<(), MapError> {
        let room = self.room_mut(vnum)?;
        room.flags.retain(|f| f != flag);
        if on {
            room.flags.push(flag.to_string());
        }
        Ok(())
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
        // Клетка (0,0) занята комнатой 100 — зона раздвинулась вниз (на юг), 104 встала на место.
        assert!(result.conflicts.is_empty());
        assert_eq!(result.moved, vec![100]);
        assert_eq!(map.room(104).unwrap().coords(), (0, 0, 0));
        assert_eq!(map.room(100).unwrap().coords(), (0, -1, 0));
        // 101 стоит на одной линии с клеткой — её не трогаем.
        assert_eq!(map.room(101).unwrap().coords(), (1, 0, 0));
        assert_eq!(map.room(103).unwrap().coords(), (0, 1, 0));

        // Слепим 100 и 104 вручную (так выглядят старые карты).
        assert_eq!(map.move_room(100, 0, 0, 0).unwrap(), vec![104]);
        // 104 связана со 103 шагом на юг — она остаётся, остальное сдвигается на юг.
        let moved = map.separate(104);
        assert_eq!(moved, vec![100]);
        assert_eq!(map.room(104).unwrap().coords(), (0, 0, 0));
        assert_eq!(map.room(100).unwrap().coords(), (0, -1, 0));
        assert_eq!(map.zone_rooms_at(1, 0, 0, 0), vec![104]);
        // Больше разводить нечего.
        assert!(map.separate(104).is_empty());
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
