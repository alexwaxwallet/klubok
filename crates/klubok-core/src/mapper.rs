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
    /// Комната в центре панели: игрок или выбранная в поиске (`MapEdit::Focus`).
    pub center: u32,
    pub zone: u32,
    pub area: String,
    /// Показанный уровень (игрок может смотреть выше/ниже себя).
    pub z: i32,
    /// Уровень, на котором стоит игрок.
    pub own_z: i32,
    pub rooms: Vec<Room>,
    /// Комнаты той же зоны уровнем выше и ниже — рисуются полупрозрачно, как в MudMapper.
    pub ghosts: Vec<Room>,
    /// Комнаты оставшегося пути `#go` — для подсветки.
    pub path: Vec<u32>,
}

/// Правка карты из интерфейса (режим правки на панели карты).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MapEdit {
    /// Сдвинуть комнаты на `dx`, `dy` клеток (y растёт на север) и `dz` уровней.
    Move {
        rooms: Vec<u32>,
        dx: i32,
        dy: i32,
        dz: i32,
    },
    /// Удалить комнаты с карты. Выходы других комнат, ведущие в них, остаются:
    /// если комнату снова посетят, она появится и связи восстановятся.
    Delete { rooms: Vec<u32> },
    /// Покрасить клетки (`#rrggbb`; пусто — снять цвет).
    Color { rooms: Vec<u32>, color: String },
    /// Поставить (`on`) или снять пометку: `dt`, `aggro`, `flydt`, `nostep`.
    Flag {
        rooms: Vec<u32>,
        flag: String,
        on: bool,
    },
    /// Найти комнаты по части названия, номеру или заметке — ответ `MapperEvent::Found`.
    Find { query: String },
    /// Показать на панели карту вокруг комнаты `vnum` (любой зоны); `None` — вернуться к игроку.
    Focus { vnum: Option<u32> },
    /// Записать заметку к комнате (пусто — стереть).
    Note { vnum: u32, note: String },
    /// Посмотреть на уровень выше/ниже (`dz` = ±1); `dz` = 0 — вернуться на свой уровень.
    /// Карта не меняется; при переходе в другую комнату вид возвращается к игроку.
    Level { dz: i32 },
}

/// Что маппер просит сделать сессию.
#[derive(Debug, Clone, PartialEq)]
pub enum MapperEvent {
    /// Отправить команду серверу (шаг пути).
    Send(String),
    Echo(String),
    Warning(String),
    View(MapView),
    /// Результат поиска из панели карты.
    Found(Vec<FoundRoom>),
}

/// Найденная комната (для списка в панели карты).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FoundRoom {
    pub vnum: u32,
    pub name: String,
    pub area: String,
    pub note: String,
}

/// Сколько найденных комнат показывать.
const FOUND_LIMIT: usize = 50;

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
    /// С прошлого промпта игрок сменил комнату (иначе шаг не удался — см. `on_prompt`).
    moved_since_prompt: bool,
    /// Ошибку записи показываем один раз, чтобы не засыпать экран.
    store_failed: bool,
    /// На сколько уровней выше (+) или ниже (−) игрока смотрит панель карты.
    look_dz: i32,
    /// Комната, вокруг которой смотрим карту (вместо игрока).
    focus: Option<u32>,
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
            moved_since_prompt: false,
            store_failed: false,
            look_dz: 0,
            focus: None,
        }
    }

    pub fn map(&self) -> &Map {
        &self.map
    }

    pub fn current(&self) -> Option<u32> {
        self.tracker.current()
    }

    /// Строка от сервера. Сервер не сообщает в MSDP, что комната мирная, поэтому узнаём по
    /// тексту: «счет» («Тут вы чувствуете себя в безопасности.») или отказ начать драку.
    pub fn on_line(&mut self, text: &str) -> Vec<MapperEvent> {
        let mut out = Vec::new();
        if !PEACE_MARKERS.iter().any(|m| text.contains(m)) {
            return out;
        }
        let Some(vnum) = self.tracker.current() else {
            return out;
        };
        let already = self
            .map
            .room(vnum)
            .is_none_or(|r| r.flags.iter().any(|f| f == "peace"));
        if already {
            return out;
        }
        if self.map.set_flag(vnum, "peace", true).is_ok() {
            self.save(vnum, &mut out);
            out.push(MapperEvent::Echo("комната отмечена как мирная".to_string()));
            out.extend(self.view().map(MapperEvent::View));
        }
        out
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

    /// Промпт: ответ сервера на очередную команду. Если с прошлого промпта комната
    /// не сменилась, самый старый отправленный шаг не удался (стена, закрытая дверь) —
    /// выбрасываем его, чтобы следующий удачный шаг не записался под чужим направлением.
    pub fn on_prompt(&mut self) {
        if !self.moved_since_prompt {
            self.pending_moves.pop_front();
        }
        self.moved_since_prompt = false;
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
        let mut changed = result.moved.clone();

        if moved {
            self.moved_since_prompt = true;
            self.look_dz = 0;
            self.focus = None;
            let step = self.pending_moves.pop_front();
            // Скрытый выход записываем, только если шаг однозначен: в очереди был один.
            // Иначе непонятно, какая команда привела сюда, — лучше не записать, чем записать мусор.
            if let (Some(dir), Some(prev), true) = (step, previous, self.pending_moves.is_empty()) {
                self.learn_exit(prev, dir, vnum);
            }
            // Старые карты могли слепить комнаты в одну клетку — разводим ту, где стоим.
            changed.extend(self.map.separate(vnum));
        }

        if before.as_ref() != self.map.room(vnum) {
            self.save(vnum, &mut out);
        }
        for other in changed.iter().filter(|v| **v != vnum) {
            self.save(*other, &mut out);
        }
        if moved {
            self.continue_walk(vnum, &mut out);
        }
        if moved || result.created || !changed.is_empty() || before.as_ref() != self.map.room(vnum)
        {
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

    /// Поиск для панели карты: номер комнаты, часть названия или заметки.
    fn find(&self, query: &str) -> Vec<FoundRoom> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let by_vnum = query.parse::<u32>().ok();
        let mut rooms: Vec<&klubok_map::Room> = self
            .map
            .rooms()
            .filter(|r| {
                Some(r.vnum) == by_vnum
                    || r.name.to_lowercase().contains(&query)
                    || r.note.to_lowercase().contains(&query)
            })
            .collect();
        rooms.sort_by_key(|r| r.vnum);
        rooms
            .into_iter()
            .take(FOUND_LIMIT)
            .map(|r| FoundRoom {
                vnum: r.vnum,
                name: r.name.clone(),
                area: r.area.clone(),
                note: r.note.clone(),
            })
            .collect()
    }

    /// Текущая зона для рисования.
    pub fn view(&self) -> Option<MapView> {
        let current = self.tracker.current()?;
        let center = self.focus.unwrap_or(current);
        let here = self.map.room(center)?;
        let z = here.z + self.look_dz;
        let rooms = self
            .map
            .rooms()
            .filter(|r| r.zone == here.zone && r.z == z)
            .cloned()
            .collect();
        let ghosts = self
            .map
            .rooms()
            .filter(|r| r.zone == here.zone && (r.z - z).abs() == 1)
            .cloned()
            .collect();
        let path = self
            .walk
            .as_ref()
            .map(|w| w.steps.iter().map(|(_, to)| *to).collect())
            .unwrap_or_default();
        Some(MapView {
            current,
            center,
            zone: here.zone,
            area: here.area.clone(),
            z,
            own_z: here.z,
            rooms,
            ghosts,
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

    // ───────────────────────────── правка мышью ─────────────────────────────

    pub fn edit(&mut self, edit: MapEdit) -> Vec<MapperEvent> {
        let mut out = Vec::new();
        match edit {
            MapEdit::Move { rooms, dx, dy, dz } => {
                for vnum in rooms {
                    let Some(room) = self.map.room(vnum) else {
                        continue;
                    };
                    let (x, y, z) = (room.x + dx, room.y + dy, room.z + dz);
                    let _ = self.map.move_room(vnum, x, y, z);
                    self.save(vnum, &mut out);
                }
            }
            MapEdit::Delete { rooms } => {
                for vnum in rooms {
                    if self.map.remove_room(vnum).is_none() {
                        continue;
                    }
                    if let Some(store) = self.store.as_mut()
                        && let Err(e) = store.delete_room(vnum)
                    {
                        out.push(MapperEvent::Warning(format!(
                            "комната {vnum} не удалена: {e}"
                        )));
                    }
                }
                // Стоим в удалённой комнате — попросим сервер о ней заново при следующем шаге.
                if self
                    .tracker
                    .current()
                    .is_some_and(|v| self.map.room(v).is_none())
                {
                    self.tracker.reset();
                }
            }
            MapEdit::Color { rooms, color } => {
                for vnum in rooms {
                    if self.map.set_color(vnum, &color).is_ok() {
                        self.save(vnum, &mut out);
                    }
                }
            }
            MapEdit::Flag { rooms, flag, on } => {
                for vnum in rooms {
                    if self.map.set_flag(vnum, &flag, on).is_ok() {
                        self.save(vnum, &mut out);
                    }
                }
            }
            MapEdit::Find { query } => {
                out.push(MapperEvent::Found(self.find(&query)));
                return out; // карта не изменилась
            }
            MapEdit::Focus { vnum } => {
                self.focus = vnum.filter(|v| self.map.room(*v).is_some());
                self.look_dz = 0;
            }
            MapEdit::Note { vnum, note } => {
                if self.map.set_note(vnum, note.trim()).is_ok() {
                    self.save(vnum, &mut out);
                }
            }
            MapEdit::Level { dz } => {
                self.look_dz = if dz == 0 { 0 } else { self.look_dz + dz };
            }
        }
        out.extend(self.view().map(MapperEvent::View));
        out
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

    /// `#map fix`: в текущей зоне развести слипшиеся комнаты и убрать сомнительные
    /// скрытые выходы — те, что ведут туда же, куда видимый выход этой комнаты
    /// (их записывала старая версия, путая шаги в стену с удачными).
    fn fix_zone(&mut self, out: &mut Vec<MapperEvent>) {
        let Some(zone) = self
            .tracker
            .current()
            .and_then(|v| self.map.room(v))
            .map(|r| r.zone)
        else {
            out.push(MapperEvent::Warning("не знаю, где вы".to_string()));
            return;
        };
        let vnums: Vec<u32> = self
            .map
            .rooms()
            .filter(|r| r.zone == zone)
            .map(|r| r.vnum)
            .collect();

        let mut changed = std::collections::BTreeSet::new();
        let mut removed_exits = 0;
        for &vnum in &vnums {
            let Some(room) = self.map.room(vnum) else {
                continue;
            };
            let doubtful: Vec<Direction> = room
                .exits
                .iter()
                .filter(|(_, e)| {
                    e.hidden
                        && e.to.is_some()
                        && room.exits.values().any(|o| !o.hidden && o.to == e.to)
                })
                .map(|(d, _)| *d)
                .collect();
            for dir in doubtful {
                let _ = self.map.remove_exit(vnum, dir);
                removed_exits += 1;
                changed.insert(vnum);
            }
        }
        let mut separated = 0;
        // Каждое разведение может создать новое наложение дальше — несколько проходов.
        for _ in 0..5 {
            let mut any = false;
            for &vnum in &vnums {
                let moved = self.map.separate(vnum);
                if !moved.is_empty() {
                    any = true;
                    separated += 1;
                    changed.extend(moved);
                }
            }
            if !any {
                break;
            }
        }
        for vnum in changed {
            self.save(vnum, out);
        }
        out.push(MapperEvent::Echo(format!(
            "зона {zone}: разведено наложений {separated}, убрано сомнительных скрытых выходов {removed_exits}"
        )));
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
                     #map unexit <направление>,\n\
                     #map move <направление> [клеток], #map fix — развести слипшиеся комнаты зоны",
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
            "fix" => self.fix_zone(out),
            "move" => {
                let Some(vnum) = current else {
                    out.push(MapperEvent::Warning("не знаю, где вы".to_string()));
                    return;
                };
                let Some(dir) = args.get(1).and_then(|d| Direction::from_command(d)) else {
                    out.push(MapperEvent::Warning(
                        "#map move <направление> [сколько клеток]".to_string(),
                    ));
                    return;
                };
                let steps: i32 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(1);
                let Some(room) = self.map.room(vnum) else {
                    return;
                };
                let (dx, dy, dz) = dir.offset();
                let (x, y, z) = (
                    room.x + dx * steps,
                    room.y + dy * steps,
                    room.z + dz * steps,
                );
                let _ = self.map.move_room(vnum, x, y, z);
                self.save(vnum, out);
                out.push(MapperEvent::Echo("комната передвинута".to_string()));
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

/// Строки сервера, по которым видно, что комната мирная.
const PEACE_MARKERS: &[&str] = &[
    "Тут вы чувствуете себя в безопасности",
    "Здесь слишком мирно",
    "находясь в тишине и покое",
];

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

    #[test]
    fn шаг_в_стену_не_портит_выходы() {
        let mut m = Mapper::default();
        m.on_room(&room_msdp(100, &[("w", 101)]));
        m.on_prompt();
        m.note_sent("север"); // стена
        m.on_prompt(); // «Вы не можете туда пройти» и промпт — комната не сменилась
        m.note_sent("запад");
        m.on_room(&room_msdp(101, &[]));
        m.on_prompt();
        let exits = &m.map().room(100).unwrap().exits;
        assert!(!exits.contains_key(&Direction::North));
        assert_eq!(exits[&Direction::West].to, Some(101));
        assert!(!exits[&Direction::West].hidden);
    }

    #[test]
    fn правка_мышью_сдвиг_и_удаление() {
        let mut m = Mapper::default();
        walked(&mut m);
        m.edit(MapEdit::Move {
            rooms: vec![101, 102],
            dx: 2,
            dy: 0,
            dz: 0,
        });
        assert_eq!(m.map().room(101).unwrap().x, 3);
        assert_eq!(m.map().room(102).unwrap().x, 3);
        m.edit(MapEdit::Delete { rooms: vec![100] });
        assert!(m.map().room(100).is_none());
        // Выход в удалённую комнату остаётся — он однонаправленный и верный.
        assert_eq!(
            m.map().room(101).unwrap().exits[&Direction::West].to,
            Some(100)
        );
        m.edit(MapEdit::Color {
            rooms: vec![101],
            color: "#ffff00".into(),
        });
        m.edit(MapEdit::Flag {
            rooms: vec![101],
            flag: "dt".into(),
            on: true,
        });
        let room = m.map().room(101).unwrap();
        assert_eq!(
            (room.color.as_str(), room.flags.as_slice()),
            ("#ffff00", &["dt".to_string()][..])
        );
    }

    #[test]
    fn мирная_комната_по_тексту() {
        let mut m = Mapper::new(Map::new(), None);
        assert!(m.on_line("Тут вы чувствуете себя в безопасности.").is_empty()); // ещё нигде
        m.on_room(&room_msdp(100, &[]));
        assert!(m.on_line("Вы голодны.").is_empty());
        let events = m.on_line("Тут вы чувствуете себя в безопасности.");
        assert!(events.iter().any(|e| matches!(e, MapperEvent::Echo(_))));
        assert!(m.map.room(100).unwrap().flags.contains(&"peace".to_string()));
        // Второй раз молчим.
        assert!(m.on_line("Здесь слишком мирно, чтобы начинать драку.").is_empty());
    }

    #[test]
    fn просмотр_другого_уровня() {
        let mut m = Mapper::new(Map::new(), None);
        walked(&mut m);
        let view = |events: Vec<MapperEvent>| {
            events
                .into_iter()
                .find_map(|e| match e {
                    MapperEvent::View(v) => Some(v),
                    _ => None,
                })
                .unwrap()
        };
        let v = view(m.edit(MapEdit::Level { dz: 1 }));
        assert_eq!((v.z, v.own_z), (1, 0));
        assert!(v.rooms.is_empty());
        assert_eq!(v.ghosts.len(), 3); // свой уровень виден снизу
        let v = view(m.edit(MapEdit::Level { dz: 0 }));
        assert_eq!(v.z, 0);
        assert_eq!(v.rooms.len(), 3);
        // Шаг в другую комнату возвращает вид к игроку.
        m.edit(MapEdit::Level { dz: -1 });
        let v = view(m.on_room(&room_msdp(101, &[("w", 100), ("n", 102)])));
        assert_eq!(v.z, 0);
    }

    #[test]
    fn поиск_просмотр_и_заметка_из_панели() {
        let mut m = Mapper::new(Map::new(), None);
        walked(&mut m);
        let found = |events: Vec<MapperEvent>| -> Vec<u32> {
            events
                .into_iter()
                .find_map(|e| match e {
                    MapperEvent::Found(rooms) => Some(rooms.iter().map(|r| r.vnum).collect()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(found(m.edit(MapEdit::Find { query: "101".into() })), vec![101]);
        assert_eq!(found(m.edit(MapEdit::Find { query: "КОМНАТА".into() })).len(), 3);

        m.edit(MapEdit::Note { vnum: 100, note: " тут лавка ".into() });
        assert_eq!(found(m.edit(MapEdit::Find { query: "лавк".into() })), vec![100]);

        let center = |events: Vec<MapperEvent>| {
            events
                .into_iter()
                .find_map(|e| match e {
                    MapperEvent::View(v) => Some((v.current, v.center)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(center(m.edit(MapEdit::Focus { vnum: Some(100) })), (102, 100));
        // Шаг игрока — вид снова на нём.
        assert_eq!(center(m.on_room(&room_msdp(101, &[("w", 100), ("n", 102)]))), (101, 101));
    }
}

