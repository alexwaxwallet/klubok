//! Хранилище карты: порт [`MapStore`] и две реализации —
//! [`MemoryStore`] (для тестов) и [`SqliteStore`] (на диске).

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::map::Map;
use crate::model::{Direction, Exit, Room};

/// Текущая версия схемы базы (хранится в `PRAGMA user_version`).
pub const SCHEMA_VERSION: i32 = 2;

/// Ошибки хранилища.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("ошибка SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("база карты новее программы: версия схемы {found}, поддерживается до {SCHEMA_VERSION}")]
    UnsupportedSchema { found: i32 },
    #[error("в базе неизвестное направление выхода {dir:?} у комнаты {vnum}")]
    BadDirection { vnum: u32, dir: String },
}

/// Порт хранилища карты. Ядро сохраняет комнату после каждого изменения.
pub trait MapStore {
    /// Загрузить всю карту.
    fn load(&mut self) -> Result<Map, StoreError>;
    /// Сохранить (создать или заменить) одну комнату вместе с её выходами.
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError>;
    /// Удалить комнату и её выходы. Выходы других комнат в неё не трогаются.
    fn delete_room(&mut self, vnum: u32) -> Result<(), StoreError>;

    /// Сохранить все комнаты карты (например, после импорта из JSON).
    fn save_map(&mut self, map: &Map) -> Result<(), StoreError> {
        for room in map.rooms() {
            self.save_room(room)?;
        }
        Ok(())
    }
}

/// Хранилище в памяти — для тестов.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    rooms: BTreeMap<u32, Room>,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl MapStore for MemoryStore {
    fn load(&mut self) -> Result<Map, StoreError> {
        let mut map = Map::new();
        for room in self.rooms.values() {
            map.insert_room(room.clone());
        }
        Ok(map)
    }

    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        self.rooms.insert(room.vnum, room.clone());
        Ok(())
    }

    fn delete_room(&mut self, vnum: u32) -> Result<(), StoreError> {
        self.rooms.remove(&vnum);
        Ok(())
    }
}

/// Хранилище в файле SQLite. SQLite встроен в программу (feature `bundled`),
/// поэтому системная библиотека не нужна ни на Windows, ни на Android.
#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Открыть (или создать) базу в файле и привести схему к актуальной.
    pub fn open(path: impl AsRef<Path>) -> Result<SqliteStore, StoreError> {
        let conn = Connection::open(path)?;
        // Журнал WAL и «обычная» синхронизация: запись комнаты на каждом шаге
        // не ждёт полного сброса на диск, а база при сбое всё равно остаётся целой.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
        SqliteStore::from_connection(conn)
    }

    /// База в памяти (пропадает при закрытии).
    pub fn open_in_memory() -> Result<SqliteStore, StoreError> {
        SqliteStore::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<SqliteStore, StoreError> {
        let mut store = SqliteStore { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Создать или обновить схему по `PRAGMA user_version`.
    fn migrate(&mut self) -> Result<(), StoreError> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema { found: version });
        }
        if version < 1 {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE rooms (
                     vnum    INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL,
                     area    TEXT NOT NULL,
                     zone    INTEGER NOT NULL,
                     terrain TEXT NOT NULL,
                     x       INTEGER NOT NULL,
                     y       INTEGER NOT NULL,
                     z       INTEGER NOT NULL,
                     note    TEXT NOT NULL DEFAULT ''
                 );
                 CREATE TABLE exits (
                     from_vnum INTEGER NOT NULL,
                     dir       TEXT NOT NULL,
                     to_vnum   INTEGER NULL,
                     door      TEXT NULL,
                     hidden    INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (from_vnum, dir)
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            // Цвет клетки и пометки (`dt`, `aggro`…) через запятую.
            self.conn.execute_batch(
                "BEGIN;
                 ALTER TABLE rooms ADD COLUMN color TEXT NOT NULL DEFAULT '';
                 ALTER TABLE rooms ADD COLUMN flags TEXT NOT NULL DEFAULT '';
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    /// Записать комнату внутри уже открытой транзакции.
    fn write_room(tx: &rusqlite::Transaction, room: &Room) -> Result<(), StoreError> {
        tx.execute(
            "INSERT OR REPLACE INTO rooms
                 (vnum, name, area, zone, terrain, x, y, z, note, color, flags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                room.vnum,
                room.name,
                room.area,
                room.zone,
                room.terrain,
                room.x,
                room.y,
                room.z,
                room.note,
                room.color,
                room.flags.join(",")
            ],
        )?;
        tx.execute("DELETE FROM exits WHERE from_vnum = ?1", params![room.vnum])?;
        for (dir, exit) in &room.exits {
            tx.execute(
                "INSERT INTO exits (from_vnum, dir, to_vnum, door, hidden)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![room.vnum, dir.msdp_key(), exit.to, exit.door, exit.hidden],
            )?;
        }
        Ok(())
    }
}

impl MapStore for SqliteStore {
    fn load(&mut self) -> Result<Map, StoreError> {
        let mut map = Map::new();

        let mut rooms = self.conn.prepare(
            "SELECT vnum, name, area, zone, terrain, x, y, z, note, color, flags FROM rooms",
        )?;
        let rows = rooms.query_map([], |row| {
            Ok(Room {
                vnum: row.get(0)?,
                name: row.get(1)?,
                area: row.get(2)?,
                zone: row.get(3)?,
                terrain: row.get(4)?,
                exits: BTreeMap::new(),
                x: row.get(5)?,
                y: row.get(6)?,
                z: row.get(7)?,
                note: row.get(8)?,
                color: row.get(9)?,
                flags: split_flags(&row.get::<_, String>(10)?),
            })
        })?;
        for room in rows {
            map.insert_room(room?);
        }

        let mut exits = self
            .conn
            .prepare("SELECT from_vnum, dir, to_vnum, door, hidden FROM exits")?;
        let rows = exits.query_map([], |row| {
            let from: u32 = row.get(0)?;
            let dir: String = row.get(1)?;
            let exit = Exit {
                to: row.get(2)?,
                door: row.get(3)?,
                hidden: row.get(4)?,
            };
            Ok((from, dir, exit))
        })?;
        // Собираем выходы отдельно, чтобы не держать заимствование карты.
        let mut loaded = Vec::new();
        for row in rows {
            loaded.push(row?);
        }
        for (from, dir, exit) in loaded {
            let Some(direction) = Direction::from_msdp_key(&dir) else {
                return Err(StoreError::BadDirection { vnum: from, dir });
            };
            // Выход комнаты, которой нет, — мусор; пропускаем его.
            if map.room(from).is_some() {
                // Ошибки тут быть не может: комнату только что проверили.
                let _ = map.add_exit(from, direction, exit);
            }
        }
        Ok(map)
    }

    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        SqliteStore::write_room(&tx, room)?;
        tx.commit()?;
        Ok(())
    }

    fn delete_room(&mut self, vnum: u32) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM exits WHERE from_vnum = ?1", params![vnum])?;
        tx.execute("DELETE FROM rooms WHERE vnum = ?1", params![vnum])?;
        tx.commit()?;
        Ok(())
    }

    /// Вся карта — одной транзакцией (намного быстрее, чем по комнате).
    fn save_map(&mut self, map: &Map) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        for room in map.rooms() {
            SqliteStore::write_room(&tx, room)?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// `"dt,aggro"` → `["dt", "aggro"]`.
fn split_flags(text: &str) -> Vec<String> {
    text.split(',')
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
}

/// Версия схемы в открытой базе (для диагностики).
pub fn schema_version(store: &SqliteStore) -> Result<Option<i32>, StoreError> {
    Ok(store
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::RoomInfo;

    fn sample_map() -> Map {
        let mut map = Map::new();
        map.update_from_msdp(RoomInfo {
            vnum: 4014,
            name: "Базарная площадь".to_string(),
            area: "Лесная деревня".to_string(),
            zone: 40,
            terrain: "city".to_string(),
            exits: vec![(Direction::North, Some(4011)), (Direction::East, None)],
            from_room: None,
            from_direction: None,
        });
        map.update_from_msdp(RoomInfo {
            vnum: 4011,
            name: "Улица".to_string(),
            zone: 40,
            from_room: Some(4014),
            ..RoomInfo::default()
        });
        map.add_exit(
            4011,
            Direction::Down,
            Exit {
                to: Some(4099),
                door: Some("люк".to_string()),
                hidden: true,
            },
        )
        .unwrap();
        map.set_note(4014, "здесь банк").unwrap();
        map
    }

    fn check_store(store: &mut dyn MapStore) {
        let map = sample_map();
        store.save_map(&map).unwrap();
        assert_eq!(store.load().unwrap(), map);

        // Перезапись комнаты заменяет и её выходы.
        let mut room = map.room(4014).unwrap().clone();
        room.exits.remove(&Direction::East);
        room.x = 7;
        store.save_room(&room).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.room(4014), Some(&room));

        store.delete_room(4011).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        // Однонаправленный выход в удалённую комнату остаётся.
        assert_eq!(
            loaded.room(4014).unwrap().exits[&Direction::North].to,
            Some(4011)
        );
    }

    #[test]
    fn memory_store_roundtrip() {
        check_store(&mut MemoryStore::new());
    }

    #[test]
    fn sqlite_store_roundtrip_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("map.sqlite");
        {
            let mut store = SqliteStore::open(&path).unwrap();
            assert_eq!(schema_version(&store).unwrap(), Some(SCHEMA_VERSION));
            check_store(&mut store);
        }
        // Повторное открытие того же файла: схема не пересоздаётся, данные на месте.
        let mut store = SqliteStore::open(&path).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.room(4014).unwrap().note, "здесь банк");
        assert_eq!(loaded.room(4014).unwrap().x, 7);
    }

    #[test]
    fn sqlite_rejects_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("map.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }
        let err = SqliteStore::open(&path).unwrap_err();
        assert!(matches!(err, StoreError::UnsupportedSchema { found: 99 }));
    }
}
