//! Поиск пути и комнат.

use std::collections::{BTreeMap, VecDeque};

use crate::map::Map;
use crate::model::Direction;

/// Кратчайший путь (по числу шагов) из комнаты `from` в комнату `to`.
///
/// Идём только по выходам, про которые известно, куда они ведут
/// (`Exit.to == Some`), и только в их направлении — выходы однонаправленные.
/// `None` — пути нет (или `from` нет на карте); `Some(vec![])` — уже на месте.
pub fn find_path(map: &Map, from: u32, to: u32) -> Option<Vec<Direction>> {
    map.room(from)?;
    if from == to {
        return Some(Vec::new());
    }

    // Поиск в ширину. Для каждой посещённой комнаты помним, откуда и каким
    // шагом в неё пришли, чтобы потом восстановить путь с конца.
    let mut came_from: BTreeMap<u32, (u32, Direction)> = BTreeMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(from);

    while let Some(vnum) = queue.pop_front() {
        let Some(room) = map.room(vnum) else {
            continue; // выход ведёт в комнату, которой ещё нет на карте
        };
        for (dir, exit) in &room.exits {
            let Some(next) = exit.to else { continue };
            if next == from || came_from.contains_key(&next) {
                continue;
            }
            came_from.insert(next, (vnum, *dir));
            if next == to {
                return Some(rebuild_path(&came_from, from, to));
            }
            queue.push_back(next);
        }
    }
    None
}

/// Восстановить путь по записям «откуда пришли».
fn rebuild_path(came_from: &BTreeMap<u32, (u32, Direction)>, from: u32, to: u32) -> Vec<Direction> {
    let mut path = Vec::new();
    let mut vnum = to;
    while vnum != from {
        let (prev, dir) = came_from[&vnum];
        path.push(dir);
        vnum = prev;
    }
    path.reverse();
    path
}

/// Русские команды для прохождения пути: `[North, East]` → `["север", "восток"]`.
pub fn speedwalk_commands(path: &[Direction]) -> Vec<String> {
    path.iter().map(|dir| dir.command().to_string()).collect()
}

/// Номера комнат, в названии которых есть `query` (без учёта регистра),
/// по возрастанию номера. Пустой запрос ничего не находит.
pub fn find_rooms_by_name(map: &Map, query: &str) -> Vec<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    map.rooms()
        .filter(|room| room.name.to_lowercase().contains(&query))
        .map(|room| room.vnum)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Exit, Room};
    use Direction::*;

    fn room(vnum: u32, name: &str, exits: &[(Direction, Option<u32>)]) -> Room {
        let mut room = Room::new(vnum);
        room.name = name.to_string();
        for (dir, to) in exits {
            room.exits.insert(*dir, Exit::to(*to));
        }
        room
    }

    /// 1 →в 2 →с 3 →вв 4; 1 →ю 5 (тупик); 2 →з 1; выход 3 →з неизвестно куда.
    fn sample_map() -> Map {
        let mut map = Map::new();
        map.insert_room(room(
            1,
            "Базарная площадь",
            &[(East, Some(2)), (South, Some(5))],
        ));
        map.insert_room(room(2, "Улица", &[(North, Some(3)), (West, Some(1))]));
        map.insert_room(room(3, "Перекрёсток", &[(Up, Some(4)), (West, None)]));
        map.insert_room(room(4, "Чердак", &[]));
        map.insert_room(room(5, "Тупиковая улица", &[]));
        map
    }

    #[test]
    fn finds_path_through_several_rooms() {
        let map = sample_map();
        let path = find_path(&map, 1, 4).unwrap();
        assert_eq!(path, vec![East, North, Up]);
        assert_eq!(speedwalk_commands(&path), vec!["восток", "север", "вверх"]);
        assert_eq!(find_path(&map, 2, 2), Some(vec![]));
    }

    #[test]
    fn no_path_against_one_way_exits() {
        let map = sample_map();
        // Из 4 и 5 выходов нет, обратные не придумываются.
        assert_eq!(find_path(&map, 4, 1), None);
        assert_eq!(find_path(&map, 5, 1), None);
        // Неизвестной комнаты нет на карте.
        assert_eq!(find_path(&map, 99, 1), None);
        assert_eq!(find_path(&map, 1, 99), None);
    }

    #[test]
    fn finds_rooms_by_name_ignoring_case() {
        let map = sample_map();
        assert_eq!(find_rooms_by_name(&map, "УЛИЦ"), vec![2, 5]);
        assert_eq!(find_rooms_by_name(&map, "площадь"), vec![1]);
        assert!(find_rooms_by_name(&map, "  ").is_empty());
    }
}
