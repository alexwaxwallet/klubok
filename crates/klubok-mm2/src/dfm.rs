//! Универсальный разбор двоичного потока компонентов Delphi («TPF0», он же двоичный DFM).
//!
//! Так Delphi (`TWriter`/`TReader`, `WriteComponent`) сохраняет объекты в поток:
//!
//! ```text
//! "TPF0"                         — сигнатура (4 байта)
//! [флаги]                        — необязательный байт 0xF0 | флаги (наследование/позиция/inline)
//! класс: ShortString             — длина 1 байт + байты, например "TMudRoom2"
//! имя:   ShortString             — имя компонента (часто пустое)
//! свойства: { имя: ShortString, значение }…  0x00
//! дети:     { компонент без "TPF0" }…        0x00
//! ```
//!
//! Значение — это байт типа (`vaInt8`, `vaString`, …) и данные. Все числа — little-endian.
//! Строки `vaString`/`vaLString`/`vaIdent` — в кодировке ANSI (для русской Windows это cp1251),
//! `vaWString` — UTF-16LE, `vaUTF8String` — UTF-8.

use encoding_rs::WINDOWS_1251;
use thiserror::Error;

/// Сигнатура двоичного потока компонентов.
pub const SIGNATURE: &[u8; 4] = b"TPF0";

/// Ошибка разбора потока.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum DfmError {
    /// Данные кончились раньше, чем ожидалось.
    #[error("неожиданный конец данных на смещении {offset}")]
    UnexpectedEof { offset: usize },
    /// В начале нет «TPF0».
    #[error("нет сигнатуры TPF0 на смещении {offset}")]
    BadSignature { offset: usize },
    /// Неизвестный код типа значения.
    #[error("неизвестный тип значения {code} на смещении {offset}")]
    UnknownValueType { code: u8, offset: usize },
    /// Длина строки/блока отрицательная или больше файла.
    #[error("неверная длина {len} на смещении {offset}")]
    BadLength { len: i64, offset: usize },
}

/// Значение свойства. Имена вариантов повторяют коды Delphi (`TValueType`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `vaNull` (0) — пустое значение (на практике — только конец списка).
    Null,
    /// `vaList` (1) — список значений (например, `TStrings.Strings`).
    List(Vec<Value>),
    /// `vaInt8`/`vaInt16`/`vaInt32` (2/3/4) — целое; размер в файле нам не важен.
    Int(i32),
    /// `vaExtended` (5) — 80-битное вещественное, переведённое в `f64`.
    Extended(f64),
    /// `vaString`/`vaLString`/`vaWString`/`vaUTF8String` (6/12/18/20) — строка, уже в UTF-8.
    String(String),
    /// `vaIdent` (7) — идентификатор (имя константы, например `clRed`).
    Ident(String),
    /// `vaFalse`/`vaTrue` (8/9).
    Bool(bool),
    /// `vaBinary` (10) — сырые байты (`DefineBinaryProperty`).
    Binary(Vec<u8>),
    /// `vaSet` (11) — множество: список имён элементов.
    Set(Vec<String>),
    /// `vaNil` (13).
    Nil,
    /// `vaCollection` (14) — коллекция: элементы со своими свойствами.
    Collection(Vec<CollectionItem>),
    /// `vaSingle` (15) — 4-байтное вещественное.
    Single(f32),
    /// `vaCurrency` (16) — деньги: целое, делённое на 10000.
    Currency(f64),
    /// `vaDate` (17) — `TDateTime`: дни с 30.12.1899 (дробная часть — время суток).
    Date(f64),
    /// `vaInt64` (19).
    Int64(i64),
}

impl Value {
    /// Целое значение, если это `Int`/`Int64`, влезающее в `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(i64::from(*v)),
            Value::Int64(v) => Some(*v),
            _ => None,
        }
    }

    /// Строка (для `String` и `Ident`).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::Ident(s) => Some(s),
            _ => None,
        }
    }

    /// Логическое значение.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Элемент коллекции (`vaCollection`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionItem {
    /// Порядковый номер, если записан (обычно его нет).
    pub order: Option<i32>,
    /// Свойства элемента.
    pub props: Vec<(String, Value)>,
}

/// Компонент: класс, имя, свойства по порядку и вложенные компоненты.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    /// Имя класса, например `TMudRoom2`.
    pub class: String,
    /// Имя компонента (может быть пустым).
    pub name: String,
    /// Флаги из префиксного байта (1 — наследован, 2 — есть позиция, 4 — inline).
    pub flags: u8,
    /// Позиция среди детей (если флаг 2).
    pub child_pos: Option<i32>,
    /// Свойства в том порядке, как они записаны.
    pub props: Vec<(String, Value)>,
    /// Вложенные компоненты.
    pub children: Vec<Component>,
}

impl Component {
    /// Первое свойство с таким именем (без учёта регистра — Delphi тоже его не учитывает).
    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.props
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}

/// Разбирает один компонент, начиная с «TPF0». Возвращает компонент и число прочитанных байт.
pub fn parse_component(data: &[u8]) -> Result<(Component, usize), DfmError> {
    let mut r = Reader { data, pos: 0 };
    let component = r.read_root()?;
    Ok((component, r.pos))
}

/// Разбирает несколько компонентов подряд (каждый со своим «TPF0») до конца данных.
pub fn parse_all(data: &[u8]) -> Result<Vec<Component>, DfmError> {
    let mut r = Reader { data, pos: 0 };
    let mut out = Vec::new();
    while r.pos < data.len() {
        out.push(r.read_root()?);
    }
    Ok(out)
}

/// Курсор по байтам. Все методы возвращают ошибку вместо паники при нехватке данных.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn eof(&self) -> DfmError {
        DfmError::UnexpectedEof { offset: self.pos }
    }

    fn take(&mut self, n: usize) -> Result<&[u8], DfmError> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.eof())?;
        if end > self.data.len() {
            return Err(self.eof());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Читает ровно N байт в массив.
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DfmError> {
        let mut buf = [0u8; N];
        buf.copy_from_slice(self.take(N)?);
        Ok(buf)
    }

    fn u8(&mut self) -> Result<u8, DfmError> {
        Ok(self.take(1)?[0])
    }

    fn peek(&self) -> Result<u8, DfmError> {
        self.data.get(self.pos).copied().ok_or_else(|| self.eof())
    }

    fn i32(&mut self) -> Result<i32, DfmError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// Длина блока (4 байта со знаком), проверенная на разумность.
    fn len32(&mut self, unit: usize) -> Result<usize, DfmError> {
        let offset = self.pos;
        let len = self.i32()?;
        let bad = DfmError::BadLength {
            len: i64::from(len),
            offset,
        };
        let len = usize::try_from(len).map_err(|_| bad.clone())?;
        // Не даём «длине» из испорченного файла превысить остаток данных.
        if len.saturating_mul(unit) > self.data.len() - self.pos {
            return Err(bad);
        }
        Ok(len)
    }

    /// ShortString: байт длины + байты в cp1251.
    fn short_string(&mut self) -> Result<String, DfmError> {
        let len = usize::from(self.u8()?);
        Ok(decode_ansi(self.take(len)?))
    }

    fn read_root(&mut self) -> Result<Component, DfmError> {
        let offset = self.pos;
        if self
            .take(4)
            .map_err(|_| DfmError::BadSignature { offset })?
            != SIGNATURE
        {
            return Err(DfmError::BadSignature { offset });
        }
        self.read_component()
    }

    fn read_component(&mut self) -> Result<Component, DfmError> {
        let mut c = Component::default();
        // Префикс флагов: старшая тетрада 0xF.
        if self.peek()? & 0xF0 == 0xF0 {
            c.flags = self.u8()? & 0x0F;
            if c.flags & 0x02 != 0 {
                c.child_pos = match self.read_value()? {
                    Value::Int(v) => Some(v),
                    _ => None,
                };
            }
        }
        c.class = self.short_string()?;
        c.name = self.short_string()?;
        c.props = self.read_props()?;
        // Дети — до нулевого байта.
        while self.peek()? != 0 {
            c.children.push(self.read_component()?);
        }
        self.u8()?; // конец списка детей
        Ok(c)
    }

    /// Свойства до пустого имени (байт 0).
    fn read_props(&mut self) -> Result<Vec<(String, Value)>, DfmError> {
        let mut props = Vec::new();
        loop {
            let name = self.short_string()?;
            if name.is_empty() {
                return Ok(props);
            }
            let value = self.read_value()?;
            props.push((name, value));
        }
    }

    fn read_value(&mut self) -> Result<Value, DfmError> {
        let offset = self.pos;
        let code = self.u8()?;
        let value = match code {
            0 => Value::Null,
            1 => {
                let mut items = Vec::new();
                while self.peek()? != 0 {
                    items.push(self.read_value()?);
                }
                self.u8()?;
                Value::List(items)
            }
            2 => Value::Int(i32::from(i8::from_le_bytes(self.array()?))),
            3 => Value::Int(i32::from(i16::from_le_bytes(self.array()?))),
            4 => Value::Int(self.i32()?),
            5 => Value::Extended(extended_to_f64(self.array()?)),
            6 => Value::String(self.short_string()?),
            7 => Value::Ident(self.short_string()?),
            8 => Value::Bool(false),
            9 => Value::Bool(true),
            10 => {
                let len = self.len32(1)?;
                Value::Binary(self.take(len)?.to_vec())
            }
            11 => {
                let mut names = Vec::new();
                loop {
                    let name = self.short_string()?;
                    if name.is_empty() {
                        break;
                    }
                    names.push(name);
                }
                Value::Set(names)
            }
            12 => {
                let len = self.len32(1)?;
                Value::String(decode_ansi(self.take(len)?))
            }
            13 => Value::Nil,
            14 => {
                let mut items = Vec::new();
                while self.peek()? != 0 {
                    // Перед элементом может стоять его номер (целое).
                    let order = if matches!(self.peek()?, 2..=4) {
                        self.read_value()?.as_i64().map(|v| v as i32)
                    } else {
                        None
                    };
                    let list_offset = self.pos;
                    if self.u8()? != 1 {
                        return Err(DfmError::UnknownValueType {
                            code: self.data[list_offset],
                            offset: list_offset,
                        });
                    }
                    let props = self.read_props()?;
                    items.push(CollectionItem { order, props });
                }
                self.u8()?;
                Value::Collection(items)
            }
            15 => Value::Single(f32::from_le_bytes(self.array()?)),
            16 => Value::Currency(i64::from_le_bytes(self.array()?) as f64 / 10000.0),
            17 => Value::Date(f64::from_le_bytes(self.array()?)),
            18 => {
                let len = self.len32(2)?;
                let bytes = self.take(len * 2)?;
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect();
                Value::String(String::from_utf16_lossy(&units))
            }
            19 => Value::Int64(i64::from_le_bytes(self.array()?)),
            20 => {
                let len = self.len32(1)?;
                Value::String(String::from_utf8_lossy(self.take(len)?).into_owned())
            }
            _ => return Err(DfmError::UnknownValueType { code, offset }),
        };
        Ok(value)
    }
}

/// Строка ANSI (cp1251) → UTF-8.
fn decode_ansi(bytes: &[u8]) -> String {
    WINDOWS_1251
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

/// 80-битное вещественное Intel (`Extended`) → `f64`.
///
/// Раскладка: 64 бита мантиссы (с явной единицей), 15 бит порядка, 1 бит знака.
fn extended_to_f64(b: [u8; 10]) -> f64 {
    let mut m = [0u8; 8];
    m.copy_from_slice(&b[..8]);
    let mantissa = u64::from_le_bytes(m);
    let se = u16::from_le_bytes([b[8], b[9]]);
    let sign = if se & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from(se & 0x7FFF);
    if exp == 0 && mantissa == 0 {
        return 0.0 * sign;
    }
    if exp == 0x7FFF {
        return if mantissa << 1 == 0 {
            sign * f64::INFINITY
        } else {
            f64::NAN
        };
    }
    // значение = мантисса / 2^63 * 2^(порядок - 16383)
    sign * (mantissa as f64) * 2f64.powi(exp - 16383 - 63)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Помощник: ShortString в байтах.
    fn ss(s: &str) -> Vec<u8> {
        let mut v = vec![s.len() as u8];
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn parses_simple_component() {
        let mut d = b"TPF0".to_vec();
        d.extend(ss("TFoo"));
        d.extend(ss("Foo1"));
        d.extend(ss("A"));
        d.extend([2, 0xFF]); // Int8 -1
        d.extend(ss("B"));
        d.extend([3, 0x34, 0x12]); // Int16
        d.extend(ss("C"));
        d.extend([4, 1, 0, 0, 0]); // Int32
        d.extend(ss("D"));
        d.push(9); // True
        d.extend(ss("E"));
        d.push(6);
        d.extend(ss("abc"));
        d.extend([0, 0]); // конец свойств, конец детей

        let (c, used) = parse_component(&d).unwrap();
        assert_eq!(used, d.len());
        assert_eq!(c.class, "TFoo");
        assert_eq!(c.name, "Foo1");
        assert_eq!(c.prop("a"), Some(&Value::Int(-1)));
        assert_eq!(c.prop("B"), Some(&Value::Int(0x1234)));
        assert_eq!(c.prop("C"), Some(&Value::Int(1)));
        assert_eq!(c.prop("D"), Some(&Value::Bool(true)));
        assert_eq!(c.prop("E"), Some(&Value::String("abc".into())));
    }

    #[test]
    fn parses_strings_in_all_encodings() {
        let mut d = b"TPF0".to_vec();
        d.extend(ss("T"));
        d.push(0);
        // vaString cp1251: «Да»
        d.extend(ss("S"));
        d.extend([6, 2, 0xC4, 0xE0]);
        // vaLString cp1251
        d.extend(ss("L"));
        d.extend([12, 2, 0, 0, 0, 0xC4, 0xE0]);
        // vaWString: «ВЗ» = U+0412 U+0417
        d.extend(ss("W"));
        d.extend([18, 2, 0, 0, 0, 0x12, 0x04, 0x17, 0x04]);
        // vaUTF8String
        d.extend(ss("U"));
        d.push(20);
        d.extend(4i32.to_le_bytes());
        d.extend("Да".as_bytes());
        d.extend([0, 0]);

        let (c, _) = parse_component(&d).unwrap();
        assert_eq!(c.prop("S").unwrap().as_str(), Some("Да"));
        assert_eq!(c.prop("L").unwrap().as_str(), Some("Да"));
        assert_eq!(c.prop("W").unwrap().as_str(), Some("ВЗ"));
        assert_eq!(c.prop("U").unwrap().as_str(), Some("Да"));
    }

    #[test]
    fn parses_lists_sets_binary_collection_and_children() {
        let mut d = b"TPF0".to_vec();
        d.extend(ss("TForm"));
        d.extend(ss("F"));
        d.extend(ss("Lines.Strings"));
        d.push(1);
        d.push(6);
        d.extend(ss("x"));
        d.push(6);
        d.extend(ss("y"));
        d.push(0);
        d.extend(ss("Anchors"));
        d.push(11);
        d.extend(ss("akLeft"));
        d.extend(ss("akTop"));
        d.push(0);
        d.extend(ss("Data"));
        d.extend([10, 3, 0, 0, 0, 1, 2, 3]);
        d.extend(ss("Cols"));
        d.push(14);
        d.push(1); // элемент без номера
        d.extend(ss("Width"));
        d.extend([2, 10]);
        d.push(0);
        d.extend([2, 5, 1]); // элемент с номером 5
        d.push(0);
        d.push(0); // конец коллекции
        d.extend(ss("Color"));
        d.push(7);
        d.extend(ss("clRed"));
        d.push(0); // конец свойств формы
        // ребёнок с флагом позиции
        d.extend([0xF2, 2, 7]);
        d.extend(ss("TButton"));
        d.extend(ss("B1"));
        d.push(0);
        d.push(0);
        d.push(0); // конец детей формы

        let (c, used) = parse_component(&d).unwrap();
        assert_eq!(used, d.len());
        assert_eq!(
            c.prop("Lines.Strings"),
            Some(&Value::List(vec![
                Value::String("x".into()),
                Value::String("y".into())
            ]))
        );
        assert_eq!(
            c.prop("Anchors"),
            Some(&Value::Set(vec!["akLeft".into(), "akTop".into()]))
        );
        assert_eq!(c.prop("Data"), Some(&Value::Binary(vec![1, 2, 3])));
        assert_eq!(
            c.prop("Cols"),
            Some(&Value::Collection(vec![
                CollectionItem {
                    order: None,
                    props: vec![("Width".into(), Value::Int(10))]
                },
                CollectionItem {
                    order: Some(5),
                    props: vec![]
                },
            ]))
        );
        assert_eq!(c.prop("Color"), Some(&Value::Ident("clRed".into())));
        assert_eq!(c.children.len(), 1);
        assert_eq!(c.children[0].class, "TButton");
        assert_eq!(c.children[0].flags, 2);
        assert_eq!(c.children[0].child_pos, Some(7));
    }

    #[test]
    fn parses_floats() {
        let mut d = b"TPF0".to_vec();
        d.extend(ss("T"));
        d.push(0);
        // Extended 1.5: мантисса 0xC000…, порядок 16383
        d.extend(ss("X"));
        d.push(5);
        d.extend(0xC000_0000_0000_0000u64.to_le_bytes());
        d.extend(16383u16.to_le_bytes());
        d.extend(ss("S"));
        d.push(15);
        d.extend(2.5f32.to_le_bytes());
        d.extend(ss("I"));
        d.push(19);
        d.extend((-7i64).to_le_bytes());
        d.extend([0, 0]);
        let (c, _) = parse_component(&d).unwrap();
        assert_eq!(c.prop("X"), Some(&Value::Extended(1.5)));
        assert_eq!(c.prop("S"), Some(&Value::Single(2.5)));
        assert_eq!(c.prop("I"), Some(&Value::Int64(-7)));
    }

    #[test]
    fn parse_all_reads_concatenated_streams() {
        let mut one = b"TPF0".to_vec();
        one.extend(ss("TA"));
        one.extend([0, 0, 0]);
        let mut d = one.clone();
        d.extend(&one);
        assert_eq!(parse_all(&d).unwrap().len(), 2);
        assert_eq!(parse_all(&[]).unwrap().len(), 0);
    }

    #[test]
    fn errors_instead_of_panics() {
        assert_eq!(
            parse_component(b"XXXX"),
            Err(DfmError::BadSignature { offset: 0 })
        );
        // Обрезанный файл.
        let mut d = b"TPF0".to_vec();
        d.extend(ss("T"));
        d.push(0);
        d.extend(ss("A"));
        d.push(4);
        d.push(1);
        assert!(matches!(
            parse_component(&d),
            Err(DfmError::UnexpectedEof { .. })
        ));
        // Неизвестный тип.
        let mut d = b"TPF0".to_vec();
        d.extend(ss("T"));
        d.push(0);
        d.extend(ss("A"));
        d.push(99);
        assert!(matches!(
            parse_component(&d),
            Err(DfmError::UnknownValueType { code: 99, .. })
        ));
        // Огромная длина.
        let mut d = b"TPF0".to_vec();
        d.extend(ss("T"));
        d.push(0);
        d.extend(ss("A"));
        d.push(20);
        d.extend(i32::MAX.to_le_bytes());
        assert!(matches!(
            parse_component(&d),
            Err(DfmError::BadLength { .. })
        ));
    }
}
