//! Карта: комнаты, выходы, зоны, отслеживание текущей комнаты, поиск пути.
//!
//! Важный инвариант домена: выходы **однонаправленные**. Если из комнаты A
//! на север можно попасть в B, это ничего не говорит о пути из B обратно в A
//! (он может вести в другую комнату или отсутствовать вовсе). Обратный выход
//! записывается только тогда, когда его реально прошли.
//!
//! Модули:
//! - [`model`] — `Direction`, `Exit`, `Room`;
//! - [`map`] — `Map`: комнаты, обновление из MSDP `ROOM`, раскладка координат, ручные правки;
//! - [`tracker`] — `Tracker`: текущая комната игрока;
//! - [`path`] — поиск пути (BFS), спидволк, поиск комнат по названию;
//! - [`store`] — хранилище: порт `MapStore`, `MemoryStore`, `SqliteStore`;
//! - [`json`] — экспорт/импорт карты в JSON.

pub mod json;
pub mod map;
pub mod model;
pub mod path;
pub mod store;
pub mod tracker;

pub use json::{JsonError, export_json, import_json};
pub use map::{Map, MapError, RoomInfo, UpdateResult};
pub use model::{Direction, Exit, Room};
pub use path::{find_path, find_rooms_by_name, speedwalk_commands};
pub use store::{MapStore, MemoryStore, SqliteStore, StoreError};
pub use tracker::Tracker;
