//! Перевод зон MudMapper в карту Клубка ([`klubok_map::Map`]).
//!
//! Что важно знать:
//! - Сетка у каждого файла `.mm2` своя, поэтому координаты разных зон друг с другом
//!   не согласованы. Клубок рисует карту по зонам (номер зоны у комнаты), так что это не мешает.
//! - У части комнат MudMapper не записал vnum (в названии нет `[12345]`). Такие комнаты
//!   получают выдуманный номер от [`SYNTHETIC_VNUM_BASE`]: они нужны, чтобы карта
//!   рисовалась и путь через них находился, но сервер о них ничего не скажет.
//! - Один vnum иногда есть в нескольких файлах — берётся первый.
//! - Выход, нарисованный на карте, но не видимый в строке выходов сервера, считается скрытым.

use std::collections::HashMap;

use klubok_map::{Direction, Exit, Map, Room};

use crate::{Mm2Color, Mm2Zone};

/// С этого номера начинаются выдуманные vnum комнат без номера.
pub const SYNTHETIC_VNUM_BASE: u32 = 3_000_000_000;
/// Сколько номеров выделено на одну зону-файл.
const SYNTHETIC_PER_ZONE: u32 = 100_000;

/// Выдуманный ли это номер комнаты (комната из MudMapper без vnum).
pub fn is_synthetic_vnum(vnum: u32) -> bool {
    vnum >= SYNTHETIC_VNUM_BASE
}

/// Итоги перевода.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub zones: usize,
    /// Сколько комнат попало на карту.
    pub rooms: usize,
    /// Из них без настоящего vnum.
    pub synthetic: usize,
    /// Комнаты, чей vnum уже встречался в другом файле (пропущены).
    pub duplicates: usize,
    pub exits: usize,
    /// Выходы, у которых цель так и не нашлась.
    pub unresolved_exits: usize,
}

/// Переводит зоны (имя файла без `.mm2`, содержимое) в карту.
pub fn to_map(zones: &[(String, Mm2Zone)]) -> (Map, ImportStats) {
    let mut stats = ImportStats {
        zones: zones.len(),
        ..ImportStats::default()
    };

    // 1. Номер для каждой комнаты: (номер файла, Tag) → vnum на карте.
    let zone_index: HashMap<&str, usize> = zones
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();
    let mut by_tag: HashMap<(usize, i32), u32> = HashMap::new();
    let mut taken: HashMap<u32, usize> = HashMap::new();
    for (zi, (_, zone)) in zones.iter().enumerate() {
        for room in &zone.rooms {
            let (Some(tag), vnum) = (room.tag, room.vnum) else {
                continue;
            };
            let vnum = vnum.unwrap_or_else(|| synthetic(zi, tag));
            if taken.contains_key(&vnum) {
                stats.duplicates += 1;
                continue;
            }
            taken.insert(vnum, zi);
            by_tag.insert((zi, tag), vnum);
        }
    }

    // 2. Комнаты с выходами.
    let mut map = Map::new();
    for (zi, (zone_name, zone)) in zones.iter().enumerate() {
        let zone_number = main_zone_number(zone);
        for room in &zone.rooms {
            let Some(tag) = room.tag else { continue };
            let Some(&vnum) = by_tag.get(&(zi, tag)) else {
                continue; // дубликат
            };
            let mut out = Room::new(vnum);
            out.name = room.name.clone();
            out.area = zone_name.clone();
            out.zone = if is_synthetic_vnum(vnum) {
                zone_number
            } else {
                vnum / 100
            };
            // В MudMapper y растёт на юг, у нас — на север.
            out.x = room.x;
            out.y = -room.y;
            out.z = room.level;
            out.note = room.note.clone();
            if let Some(Mm2Color::Rgb(r, g, b)) = room.color {
                out.color = format!("#{r:02x}{g:02x}{b:02x}");
            }
            for (flag, set) in [
                ("dt", room.death_trap),
                ("aggro", room.aggro),
                ("flydt", room.fly_death_trap),
                ("nostep", room.no_step),
            ] {
                if set {
                    out.flags.push(flag.to_string());
                }
            }

            let (visible, _) = crate::parse_game_exits(&room.game_exits);
            for exit in &room.exits {
                let Some(dir) = Direction::from_msdp_key(&exit.dir) else {
                    continue;
                };
                let to = if let Some(link) = &exit.to_zone {
                    zone_index
                        .get(link.zone.as_str())
                        .zip(link.to_tag)
                        .and_then(|(&z, t)| by_tag.get(&(z, t)).copied())
                } else {
                    exit.to_tag.and_then(|t| by_tag.get(&(zi, t)).copied())
                };
                if exit.linked && to.is_none() {
                    stats.unresolved_exits += 1;
                }
                stats.exits += 1;
                out.exits.insert(
                    dir,
                    Exit {
                        to,
                        door: exit.door.then(|| "дверь".to_string()),
                        hidden: exit.linked && !visible.iter().any(|d| d == &exit.dir),
                    },
                );
            }
            if is_synthetic_vnum(vnum) {
                stats.synthetic += 1;
            }
            stats.rooms += 1;
            map.insert_room(out);
        }
    }
    (map, stats)
}

fn synthetic(zone: usize, tag: i32) -> u32 {
    SYNTHETIC_VNUM_BASE
        + zone as u32 * SYNTHETIC_PER_ZONE
        + tag.rem_euclid(SYNTHETIC_PER_ZONE as i32) as u32
}

/// Самый частый номер зоны (`vnum / 100`) среди комнат файла — для комнат без vnum.
fn main_zone_number(zone: &Mm2Zone) -> u32 {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for vnum in zone.rooms.iter().filter_map(|r| r.vnum) {
        *counts.entry(vnum / 100).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(zone, n)| (n, std::cmp::Reverse(zone)))
        .map(|(zone, _)| zone)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mm2Exit, Mm2Room, Mm2ZoneLink};

    fn room(tag: i32, vnum: Option<u32>, x: i32, y: i32, exits: Vec<Mm2Exit>) -> Mm2Room {
        Mm2Room {
            tag: Some(tag),
            vnum,
            name: format!("комната {tag}"),
            x,
            y,
            game_exits: "СВЮЗ".into(),
            exits,
            ..Mm2Room::default()
        }
    }

    fn exit(dir: &str, to_tag: i32) -> Mm2Exit {
        Mm2Exit {
            dir: dir.into(),
            linked: true,
            to_tag: Some(to_tag),
            ..Mm2Exit::default()
        }
    }

    #[test]
    fn зоны_выходы_и_выдуманные_номера() {
        let a = Mm2Zone {
            rooms: vec![
                room(1, Some(4001), 0, 0, vec![exit("e", 2)]),
                room(
                    2,
                    None,
                    1,
                    0,
                    vec![exit("w", 1), {
                        let mut e = exit("u", 0);
                        e.to_tag = None;
                        e.to_zone = Some(Mm2ZoneLink {
                            zone: "Б".into(),
                            dir: "u".into(),
                            to_tag: Some(7),
                            commands: String::new(),
                        });
                        e
                    }],
                ),
            ],
            ..Mm2Zone::default()
        };
        let b = Mm2Zone {
            rooms: vec![
                room(7, Some(5000), 3, 3, vec![]),
                room(8, Some(4001), 0, 0, vec![]), // дубликат vnum
            ],
            ..Mm2Zone::default()
        };
        let (map, stats) = to_map(&[("А".into(), a), ("Б".into(), b)]);
        assert_eq!(stats.rooms, 3);
        assert_eq!(stats.duplicates, 1);
        assert_eq!(stats.synthetic, 1);

        let first = map.room(4001).unwrap();
        let second_vnum = first.exits[&Direction::East].to.unwrap();
        assert!(is_synthetic_vnum(second_vnum));
        let second = map.room(second_vnum).unwrap();
        assert_eq!(second.zone, 40); // по большинству комнат файла
        assert_eq!(second.exits[&Direction::Up].to, Some(5000));
        // Вверх нет в строке выходов сервера — выход скрытый.
        assert!(second.exits[&Direction::Up].hidden);
        assert!(!second.exits[&Direction::West].hidden);
        // y переворачивается: в MudMapper он растёт на юг.
        assert_eq!(map.room(5000).unwrap().y, -3);
    }
}
