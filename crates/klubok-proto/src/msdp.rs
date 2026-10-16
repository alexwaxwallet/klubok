//! MSDP (Mud Server Data Protocol): структурированные данные от сервера.
//!
//! Былины присылают через него комнату (`ROOM`), опыт, золото и т.п.
//! Формат: `VAR имя VAL значение`, где значение — строка, массив или таблица.

use crate::charset::Charset;
use crate::telnet::{opt, subnegotiation};

const VAR: u8 = 1;
const VAL: u8 = 2;
const TABLE_OPEN: u8 = 3;
const TABLE_CLOSE: u8 = 4;
const ARRAY_OPEN: u8 = 5;
const ARRAY_CLOSE: u8 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsdpValue {
    String(String),
    Array(Vec<MsdpValue>),
    /// Таблица; порядок ключей сохраняется как у сервера.
    Table(Vec<(String, MsdpValue)>),
}

impl MsdpValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            MsdpValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Значение по ключу, если это таблица.
    pub fn get(&self, key: &str) -> Option<&MsdpValue> {
        match self {
            MsdpValue::Table(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

impl std::fmt::Display for MsdpValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsdpValue::String(s) => write!(f, "{s:?}"),
            MsdpValue::Array(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            MsdpValue::Table(items) => {
                write!(f, "{{")?;
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k} {v}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// Разбирает данные подкоманды MSDP в список пар (переменная, значение).
pub fn decode(data: &[u8], charset: Charset) -> Vec<(String, MsdpValue)> {
    let mut reader = Reader {
        data,
        pos: 0,
        charset,
    };
    reader.pairs(None)
}

/// Готовая подкоманда `REPORT <переменная>` — просим сервер присылать её при изменении.
pub fn report(variable: &str) -> Vec<u8> {
    command("REPORT", variable)
}

/// Готовая подкоманда `SEND <переменная>` — просим прислать значение один раз.
pub fn send(variable: &str) -> Vec<u8> {
    command("SEND", variable)
}

fn command(name: &str, value: &str) -> Vec<u8> {
    let mut data = vec![VAR];
    data.extend(name.as_bytes());
    data.push(VAL);
    data.extend(value.as_bytes());
    subnegotiation(opt::MSDP, &data)
}

/// Читает байты по порядку. Разбор рекурсивный: таблица содержит пары, значения могут быть таблицами.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    charset: Charset,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Читает пары `VAR имя VAL значение` до `close` (или до конца данных).
    fn pairs(&mut self, close: Option<u8>) -> Vec<(String, MsdpValue)> {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if Some(b) == close {
                self.pos += 1;
                break;
            }
            if b != VAR {
                self.pos += 1; // мусор — пропускаем
                continue;
            }
            self.pos += 1;
            let name = self.string();
            if self.peek() == Some(VAL) {
                self.pos += 1;
                out.push((name, self.value()));
            } else {
                out.push((name, MsdpValue::String(String::new())));
            }
        }
        out
    }

    fn value(&mut self) -> MsdpValue {
        match self.peek() {
            Some(TABLE_OPEN) => {
                self.pos += 1;
                MsdpValue::Table(self.pairs(Some(TABLE_CLOSE)))
            }
            Some(ARRAY_OPEN) => {
                self.pos += 1;
                let mut items = Vec::new();
                while let Some(b) = self.peek() {
                    if b == ARRAY_CLOSE {
                        self.pos += 1;
                        break;
                    }
                    if b == VAL {
                        self.pos += 1;
                        items.push(self.value());
                    } else {
                        self.pos += 1;
                    }
                }
                MsdpValue::Array(items)
            }
            _ => MsdpValue::String(self.string()),
        }
    }

    /// Строка до ближайшего служебного байта (1–6).
    fn string(&mut self) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if (VAR..=ARRAY_CLOSE).contains(&b) {
                break;
            }
            self.pos += 1;
        }
        self.charset.decode(&self.data[start..self.pos])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> MsdpValue {
        MsdpValue::String(v.to_string())
    }

    #[test]
    fn комната_былин() {
        // VAR ROOM VAL { VAR VNUM VAL 4014 VAR NAME VAL Базарная площадь VAR EXITS VAL { VAR n VAL 4011 } }
        let mut data = vec![VAR];
        data.extend(b"ROOM");
        data.extend([VAL, TABLE_OPEN, VAR]);
        data.extend(b"VNUM");
        data.push(VAL);
        data.extend(b"4014");
        data.push(VAR);
        data.extend(b"NAME");
        data.push(VAL);
        data.extend("Базарная площадь".as_bytes());
        data.push(VAR);
        data.extend(b"EXITS");
        data.extend([VAL, TABLE_OPEN, VAR, b'n', VAL]);
        data.extend(b"4011");
        data.extend([TABLE_CLOSE, TABLE_CLOSE]);

        let vars = decode(&data, Charset::Utf8);
        assert_eq!(vars.len(), 1);
        let (name, room) = &vars[0];
        assert_eq!(name, "ROOM");
        assert_eq!(room.get("VNUM"), Some(&s("4014")));
        assert_eq!(room.get("NAME"), Some(&s("Базарная площадь")));
        assert_eq!(room.get("EXITS").and_then(|e| e.get("n")), Some(&s("4011")));
    }

    #[test]
    fn массив() {
        let data = [
            VAR,
            b'L',
            VAL,
            ARRAY_OPEN,
            VAL,
            b'a',
            VAL,
            b'b',
            ARRAY_CLOSE,
            VAR,
            b'X',
            VAL,
            b'1',
        ];
        assert_eq!(
            decode(&data, Charset::Utf8),
            vec![
                ("L".to_string(), MsdpValue::Array(vec![s("a"), s("b")])),
                ("X".to_string(), s("1")),
            ]
        );
    }

    #[test]
    fn команда_report() {
        assert_eq!(report("ROOM"), b"\xff\xfaE\x01REPORT\x02ROOM\xff\xf0");
    }
}
