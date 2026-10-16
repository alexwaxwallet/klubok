//! Маппер в сессии: обновляет карту по MSDP `ROOM`, ведёт игрока по пути (`#go`),
//! выполняет команды `#map` и сохраняет изменения в хранилище.

use std::collections::VecDeque;

use klubok_map::{
    Direction, Exit, Map, MapStore, Room, RoomInfo, Tracker, find_path, find_rooms_by_name,
};
use klubok_proto::MsdpValue;

/// Комнаты без настоящего номера (из карт MudMapper) начинаются с этого номера.
/// Сервер о них ничего не скажет, поэтому при ходьбе их не сверяем.
const SYNTHETIC_VNUM_BASE: u32 = 3_000_000_000;

/// Сколько раз можно перестроить путь, если игрок попал не туда.
const MAX_REROUTES: u32 = 3;

/// Часть карты для рисования: комнаты текущей зоны на текущем уровне.
#[derive(Debug, Clone, PartialEq)]
pub struct MapView {
    pub current: u32,
    pub zone: u32,
    pub area: String,
    pub z: i32,
    pub rooms: Vec<Room>,
    /// Комнаты оставшегося пути `#go` — для подсветки.
    pub path: Vec<u32>,
}

/// Что маппер просит сделать сессию.
#[derive(Debug, Clone, PartialEq)]
pub enum MapperEvent {
    /// Отправить команду серверу (шаг пути).
    Send(String),
    Echo(String),
    Warning(String),
    View(MapView),
}

/// Идём по пути `#go`.
struct Walk {
    /// Оставшиеся шаги: направление и комната, куда он приведёт.
    steps: VecDeque<(Direction, u32)>,
    target: u32,
    reroutes: u32,
}

pub struct Mapper {
    map: Map,
    tracker: Tracker,
    store: Option<Box<dyn MapStore + Send>>,
    walk: Option<Walk>,
    /// Направления, которые игрок отправил серверу и которые ещё не привели в новую комнату.
    /// Нужны, чтобы записать скрытый выход: сервер о нём в MSDP не скажет.
    pending_moves: VecDeque<Direction>,
    /// Ошибку записи показываем один раз, чтобы не засыпать экран.
    store_failed: bool,
}

impl Default for Mapper {
    fn default() -> Self {
        Mapper::new(Map::new(), None)
    }
}

impl Mapper {
    /// `store` — куда сохранять изменения (`None` — карта только в памяти).
    pub fn new(map: Map, store: Option<Box<dyn MapStore + Send>>) -> Mapper {
        Mapper {
            map,
            tracker: Tracker::new(),
            store,
            walk: None,
            pending_moves: VecDeque::new(),
            store_failed: false,
        }
    }

    pub fn map(&self) -> &Map {
        &self.map
    }

    pub fn current(&self) -> Option<u32> {
        self.tracker.current()
    }

    /// Игрок (или триггер) отправил команду серверу. Если это шаг — запоминаем направление.
    pub fn note_sent(&mut self, command: &str) {
        if let Some(dir) = Direction::from_command(command.trim()) {
            self.pending_moves.push_back(dir);
            // Длинная очередь — значит, шаги не доходят (стена, бой); старые выбрасываем.
            while self.pending_moves.len() > 10 {
                self.pending_moves.pop_front();
            }
        }
    }

    /// Соединение закрылось.
    pub fn reset(&mut self) {
        self.walk = None;
        self.pending_moves.clear();
        self.tracker.reset();
    }

    /// Пришёл MSDP `ROOM`.
    pub fn on_room(&mut self, value: &MsdpValue) -> Vec<MapperEvent> {
        let mut out = Vec::new();
        let Some(info) = room_info(value) else {
            return out;
        };
        let vnum = info.vnum;
        let before = self.map.room(vnum).cloned();
        let moved = self.tracker.update(&info);
        let previous = self.tracker.previous();
        let result = self.map.update_from_msdp(info);

        if moved {
            let step = self.pending_moves.pop_front();
            if let (Some(dir), Some(prev)) = (step, previous) {
                self.learn_exit(prev, dir, vnum);
            }
        }

        if before.as_ref() != self.map.room(vnum) {
            self.save(vnum, &mut out);
        }
        // Наложение комнат (`result.conflicts`) видно на карте; игрок поправит сам.
        if moved {
            self.continue_walk(vnum, &mut out);
        }
        if moved || result.created || before.as_ref() != self.map.room(vnum) {
            out.extend(self.view().map(MapperEvent::View));
        }
        out
    }

    /// Игрок пришёл из `prev` в `to`, отправив `dir`. Если такого выхода не было — он скрытый.
    fn learn_exit(&mut self, prev: u32, dir: Direction, to: u32) {
        let Some(room) = self.map.room(prev) else {
            return;
        };
        match room.exits.get(&dir) {
            Some(exit) if exit.to == Some(to) => {}
            Some(exit) if exit.to.is_none() => {
                let exit = Exit {
                    to: Some(to),
                    ..exit.clone()
                };
                let _ = self.map.add_exit(prev, dir, exit);
            }
            // Выход был, но вёл в другое место: скорее всего, шаг не тот (бегство, телепорт).
            Some(_) => return,
            None => {
                let exit = Exit {
                    to: Some(to),
                    door: None,
                    hidden: true,
                };
                let _ = self.map.add_exit(prev, dir, exit);
            }
        }
        let mut sink = Vec::new();
        self.save(prev, &mut sink);
    }

    fn save(&mut self, vnum: u32, out: &mut Vec<MapperEvent>) {
        let (Some(store), Some(room)) = (self.store.as_mut(), self.map.room(vnum)) else {
            return;
        };
        if let Err(e) = store.save_room(room)
            && !self.store_failed
        {
            self.store_failed = true;
            out.push(MapperEvent::Warning(format!("карта не сохраняется: {e}")));
        }
    }

    /// Текущая зона для рисования.
    pub fn view(&self) -> Option<MapView> {
        let current = self.tracker.current()?;
        let here = self.map.room(current)?;
        let rooms = self
            .map
            .rooms()
            .filter(|r| r.zone == here.zone && r.z == here.z)
            .cloned()
            .collect();
        let path = self
            .walk
            .as_ref()
            .map(|w| w.steps.iter().map(|(_, to)| *to).collect())
            .unwrap_or_default();
        Some(MapView {
            current,
            zone: here.zone,
            area: here.area.clone(),
            z: here.z,
            rooms,
            path,
        })
    }

    // ───────────────────────────── путь ─────────────────────────────

    fn continue_walk(&mut self, now: u32, out: &mut Vec<MapperEvent>) {
        let Some(walk) = self.walk.as_ref() else {
            return;
        };
        let Some(&(_, expected)) = walk.steps.front() else {
            self.walk = None;
            return;
        };
        let (target, reroutes) = (walk.target, walk.reroutes);
        if now == expected || expected >= SYNTHETIC_VNUM_BASE {
            if let Some(walk) = self.walk.as_mut() {
                walk.steps.pop_front();
            }
        } else if reroutes >= MAX_REROUTES {
            self.walk = None;
            out.push(MapperEvent::Warning(
                "путь сбился несколько раз — остановился".to_string(),
            ));
            return;
        } else {
            // Попали не туда — ищем путь заново.
            let Some(steps) = self.route(now, target) else {
                self.walk = None;
                out.push(MapperEvent::Warning(
                    "сбились с пути, а другого нет — остановился".to_string(),
                ));
                return;
            };
            out.push(MapperEvent::Echo("сбились с пути, иду заново".to_string()));
            self.walk = Some(Walk {
                steps,
                target,
                reroutes: reroutes + 1,
            });
        }
        self.send_next_step(out);
    }

    fn send_next_step(&mut self, out: &mut Vec<MapperEvent>) {
        let Some(walk) = self.walk.as_ref() else {
            return;
        };
        match walk.steps.front() {
            Some((dir, _)) => out.push(MapperEvent::Send(dir.command().to_string())),
            None => {
                self.walk = None;
                out.push(MapperEvent::Echo("пришли".to_string()));
            }
        }
    }

    /// Путь как очередь шагов (направление, куда приведёт).
    fn route(&self, from: u32, to: u32) -> Option<VecDeque<(Direction, u32)>> {
        let dirs = find_path(&self.map, from, to)?;
        let mut room = from;
        let mut steps = VecDeque::new();
        for dir in dirs {
            room = self.map.room(room)?.exits.get(&dir)?.to?;
            steps.push_back((dir, room));
        }
        Some(steps)
    }

    // ───────────────────────────── команды ─────────────────────────────

    /// `#go`, `#stop`, `#map …`.
    pub fn command(&mut self, name: &str, args: &[String]) -> Vec<MapperEvent> {
        let mut out = Vec::new();
        match name {
            "go" => self.go(&args.join(" "), &mut out),
            "stop" => {
                if self.walk.take().is_some() {
                    out.push(MapperEvent::Echo("остановился".to_string()));
                }
            }
            "map" => self.map_command(args, &mut out),
            _ => {}
        }
        out
    }

    fn go(&mut self, query: &str, out: &mut Vec<MapperEvent>) {
        let query = query.trim();
        let Some(from) = self.tracker.current() else {
            out.push(MapperEvent::Warning(
                "не знаю, где вы: сделайте шаг или «смотреть»".to_string(),
            ));
            return;
        };
        if query.is_empty() {
            out.push(MapperEvent::Warning(
                "куда? #go <номер комнаты или часть названия>".to_string(),
            ));
            return;
        }
        let candidates: Vec<u32> = match query.parse::<u32>() {
            Ok(vnum) => vec![vnum],
            Err(_) => find_rooms_by_name(&self.map, query),
        };
        // Из нескольких подходящих комнат — ближайшая (первые 200, чтобы не думать долго).
        let best = candidates
            .iter()
            .take(200)
            .filter_map(|&to| self.route(from, to).map(|steps| (to, steps)))
            .min_by_key(|(_, steps)| steps.len());
        let Some((target, steps)) = best else {
            out.push(MapperEvent::Warning(match candidates.len() {
                0 => format!("комната «{query}» на карте не найдена"),
                _ => format!("пути до «{query}» по карте нет"),
            }));
            return;
        };
        let name = self
            .map
            .room(target)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        out.push(MapperEvent::Echo(format!(
            "иду в «{name}» [{target}], шагов: {}",
            steps.len()
        )));
        self.walk = Some(Walk {
            steps,
            target,
            reroutes: 0,
        });
        self.send_next_step(out);
        out.extend(self.view().map(MapperEvent::View));
    }

    fn map_command(&mut self, args: &[String], out: &mut Vec<MapperEvent>) {
        let sub = args.first().map(|s| s.to_lowercase()).unwrap_or_default();
        let rest = args.get(1..).unwrap_or_default().join(" ");
        let current = self.tracker.current();
        match sub.as_str() {
            "" => {
                let here = current
                    .and_then(|v| self.map.room(v))
                    .map(|r| format!("{} [{}], зона {} «{}»", r.name, r.vnum, r.zone, r.area))
                    .unwrap_or_else(|| "неизвестно".to_string());
                out.push(MapperEvent::Echo(format!(
                    "Карта: комнат {}. Вы здесь: {here}.\n\
                     #go <номер|название> — идти, #stop — остановиться,\n\
                     #map find <название>, #map note <текст>, #map exit <направление> <номер>, \
                     #map unexit <направление>",
                    self.map.len()
                )));
            }
            "find" => {
                let found = find_rooms_by_name(&self.map, &rest);
                let mut text = format!("найдено комнат: {}", found.len());
                for vnum in found.iter().take(30) {
                    if let Some(r) = self.map.room(*vnum) {
                        text.push_str(&format!("\n  [{}] {} — {}", r.vnum, r.name, r.area));
                    }
                }
                out.push(MapperEvent::Echo(text));
            }
            "note" => {
                let Some(vnum) = current else {
                    out.push(MapperEvent::Warning("не знаю, где вы".to_string()));
                    return;
                };
                let _ = self.map.set_note(vnum, &rest);
                self.save(vnum, out);
                out.push(MapperEvent::Echo("заметка записана".to_string()));
                out.extend(self.view().map(MapperEvent::View));
            }
            "exit" | "unexit" => {
                let Some(vnum) = current else {
                    out.push(MapperEvent::Warning("не знаю, где вы".to_string()));
                    return;
                };
                let Some(dir) = args.get(1).and_then(|d| Direction::from_command(d)) else {
                    out.push(MapperEvent::Warning(
                        "направление: север, восток, юг, запад, вверх, вниз".to_string(),
                    ));
                    return;
                };
                if sub == "exit" {
                    let to = args.get(2).and_then(|v| v.parse::<u32>().ok());
                    if to.is_none() {
                        out.push(MapperEvent::Warning(
                            "#map exit <направление> <номер комнаты>".to_string(),
                        ));
                        return;
                    }
                    let exit = Exit {
                        to,
                        door: None,
                        hidden: true,
                    };
                    let _ = self.map.add_exit(vnum, dir, exit);
                } else {
                    let _ = self.map.remove_exit(vnum, dir);
                }
                self.save(vnum, out);
                out.push(MapperEvent::Echo("выход изменён".to_string()));
                out.extend(self.view().map(MapperEvent::View));
            }
            other => out.push(MapperEvent::Warning(format!(
                "#map {other}: неизвестная подкоманда, см. #map"
            ))),
        }
    }
}

/// MSDP `ROOM` → данные для карты. `None` — нет номера комнаты.
fn room_info(value: &MsdpValue) -> Option<RoomInfo> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(MsdpValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str| value.get(key)?.as_str()?.trim().parse::<u32>().ok();
    let vnum = number("VNUM")?;
    let exits = match value.get("EXITS") {
        Some(MsdpValue::Table(items)) => items
            .iter()
            .filter_map(|(dir, to)| {
                let dir = Direction::from_msdp_key(dir)?;
                Some((dir, to.as_str().and_then(|v| v.trim().parse().ok())))
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(RoomInfo {
        vnum,
        name: text("NAME"),
        area: text("AREA"),
        zone: number("ZONE").unwrap_or(vnum / 100),
        terrain: text("TERRAIN"),
        exits,
        from_room: number("FROM_ROOM"),
        from_direction: value
            .get("FROM_DIRECTION")
            .and_then(MsdpValue::as_str)
            .and_then(Direction::from_msdp_key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_msdp(vnum: u32, exits: &[(&str, u32)]) -> MsdpValue {
        let s = |t: &str| MsdpValue::String(t.to_string());
        MsdpValue::Table(vec![
            ("VNUM".into(), s(&vnum.to_string())),
            ("NAME".into(), s(&format!("комната {vnum}"))),
            ("ZONE".into(), s(&(vnum / 100).to_string())),
            (
                "EXITS".into(),
                MsdpValue::Table(
                    exits
                        .iter()
                        .map(|(d, to)| (d.to_string(), s(&to.to_string())))
                        .collect(),
                ),
            ),
        ])
    }

    fn sent(events: &[MapperEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                MapperEvent::Send(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    /// 100 → (восток) 101 → (север) 102; обратно из 102 только на юг, из 101 на запад.
    fn walked(m: &mut Mapper) {
        m.on_room(&room_msdp(100, &[("e", 101)]));
        m.on_room(&room_msdp(101, &[("w", 100), ("n", 102)]));
        m.on_room(&room_msdp(102, &[("s", 101)]));
    }

    #[test]
    fn идёт_по_пути_шаг_за_шагом() {
        let mut m = Mapper::default();
        walked(&mut m);
        let out = m.command("go", &["100".into()]);
        assert_eq!(sent(&out), ["юг"]);
        let out = m.on_room(&room_msdp(101, &[("w", 100), ("n", 102)]));
        assert_eq!(sent(&out), ["запад"]);
        let out = m.on_room(&room_msdp(100, &[("e", 101)]));
        assert!(sent(&out).is_empty());
        assert!(out.contains(&MapperEvent::Echo("пришли".into())));
    }

    #[test]
    fn скрытый_выход_запоминается_по_шагу() {
        let mut m = Mapper::default();
        m.on_room(&room_msdp(100, &[]));
        m.note_sent("вверх");
        m.on_room(&room_msdp(200, &[]));
        let exit = &m.map().room(100).unwrap().exits[&Direction::Up];
        assert_eq!(exit.to, Some(200));
        assert!(exit.hidden);
        // Обратный выход сам не появляется.
        assert!(m.map().room(200).unwrap().exits.is_empty());
    }

    #[test]
    fn поиск_по_названию_и_остановка() {
        let mut m = Mapper::default();
        walked(&mut m);
        let out = m.command("go", &["комната".into(), "100".into()]);
        assert_eq!(sent(&out), ["юг"]);
        let out = m.command("stop", &[]);
        assert_eq!(out, [MapperEvent::Echo("остановился".into())]);
        let out = m.on_room(&room_msdp(101, &[("w", 100), ("n", 102)]));
        assert!(sent(&out).is_empty());
    }
}
