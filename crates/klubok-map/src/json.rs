//! Экспорт и импорт карты в JSON, чтобы игроки могли делиться картами.
//!
//! Формат файла:
//! ```json
//! { "format": "klubok-map", "version": 1,
//!   "rooms": [ { "vnum": 4014, "name": "…", "exits": { "n": { "to": 4011 } }, … } ] }
//! ```

use serde::{Deserialize, Serialize};

use crate::map::Map;
use crate::model::Room;

/// Метка формата в файле.
pub const JSON_FORMAT: &str = "klubok-map";
/// Версия формата JSON.
pub const JSON_VERSION: u32 = 1;

/// Ошибки импорта/экспорта JSON.
#[derive(Debug, thiserror::Error)]
pub enum JsonError {
    #[error("ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("это не карта Клубка (format = {0:?})")]
    WrongFormat(String),
    #[error("карта в формате версии {0}, программа понимает до {JSON_VERSION}")]
    UnsupportedVersion(u32),
}

/// Содержимое файла карты.
#[derive(Debug, Serialize, Deserialize)]
struct MapFile {
    format: String,
    version: u32,
    rooms: Vec<Room>,
}

/// Карта → JSON (с отступами, удобно читать и сравнивать).
pub fn export_json(map: &Map) -> Result<String, JsonError> {
    let file = MapFile {
        format: JSON_FORMAT.to_string(),
        version: JSON_VERSION,
        rooms: map.rooms().cloned().collect(),
    };
    Ok(serde_json::to_string_pretty(&file)?)
}

/// JSON → карта. Чтобы влить чужую карту в свою, переложите комнаты
/// через [`Map::insert_room`] (комнаты с тем же номером заменятся).
pub fn import_json(text: &str) -> Result<Map, JsonError> {
    let file: MapFile = serde_json::from_str(text)?;
    if file.format != JSON_FORMAT {
        return Err(JsonError::WrongFormat(file.format));
    }
    if file.version > JSON_VERSION {
        return Err(JsonError::UnsupportedVersion(file.version));
    }
    let mut map = Map::new();
    for room in file.rooms {
        map.insert_room(room);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Direction, Exit};

    #[test]
    fn json_roundtrip() {
        let mut map = Map::new();
        let mut room = Room::new(4014);
        room.name = "Базарная площадь".to_string();
        room.area = "Лесная деревня".to_string();
        room.zone = 40;
        room.terrain = "city".to_string();
        room.x = -3;
        room.z = 2;
        room.note = "банк".to_string();
        room.exits.insert(Direction::North, Exit::to(Some(4011)));
        room.exits.insert(Direction::West, Exit::to(None));
        room.exits.insert(
            Direction::Down,
            Exit {
                to: Some(4099),
                door: Some("люк".to_string()),
                hidden: true,
            },
        );
        map.insert_room(room);
        map.insert_room(Room::new(4011));

        let text = export_json(&map).unwrap();
        assert!(text.contains("\"n\""));
        assert_eq!(import_json(&text).unwrap(), map);
    }

    #[test]
    fn rejects_foreign_files() {
        let wrong = r#"{"format":"mudlet","version":1,"rooms":[]}"#;
        assert!(matches!(import_json(wrong), Err(JsonError::WrongFormat(_))));
        let newer = r#"{"format":"klubok-map","version":9,"rooms":[]}"#;
        assert!(matches!(
            import_json(newer),
            Err(JsonError::UnsupportedVersion(9))
        ));
        assert!(matches!(import_json("не json"), Err(JsonError::Json(_))));
    }
}
