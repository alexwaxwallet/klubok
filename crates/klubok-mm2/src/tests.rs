//! Тесты загрузки `.mm2` на сгенерированных байтах (реальные карты владельца в репозиторий
//! не кладём).

use super::*;

/// Маленький «писатель» TPF0 — только то, что нужно для тестов.
struct Writer(Vec<u8>);

impl Writer {
    fn room() -> Self {
        let mut w = Writer(b"TPF0".to_vec());
        w.short("TMudRoom2");
        w.short("");
        w
    }
    fn short(&mut self, s: &str) {
        self.0.push(s.len() as u8);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn int(mut self, name: &str, v: i32) -> Self {
        self.short(name);
        if let Ok(b) = i8::try_from(v) {
            self.0.push(2);
            self.0.extend(b.to_le_bytes());
        } else {
            self.0.push(4);
            self.0.extend(v.to_le_bytes());
        }
        self
    }
    fn bool(mut self, name: &str, v: bool) -> Self {
        self.short(name);
        self.0.push(if v { 9 } else { 8 });
        self
    }
    /// Строка как в реальных картах: `vaUTF8String`.
    fn utf8(mut self, name: &str, v: &str) -> Self {
        self.short(name);
        self.0.push(20);
        self.0.extend((v.len() as i32).to_le_bytes());
        self.0.extend_from_slice(v.as_bytes());
        self
    }
    /// Строка выходов как в реальных картах: `vaWString`.
    fn wide(mut self, name: &str, v: &str) -> Self {
        let units: Vec<u16> = v.encode_utf16().collect();
        self.short(name);
        self.0.push(18);
        self.0.extend((units.len() as i32).to_le_bytes());
        for u in units {
            self.0.extend(u.to_le_bytes());
        }
        self
    }
    fn ident(mut self, name: &str, v: &str) -> Self {
        self.short(name);
        self.0.push(7);
        self.short(v);
        self
    }
    fn strings(mut self, name: &str, items: &[&str]) -> Self {
        self.short(name);
        self.0.push(1);
        for s in items {
            self.0.push(20);
            self.0.extend((s.len() as i32).to_le_bytes());
            self.0.extend_from_slice(s.as_bytes());
        }
        self.0.push(0);
        self
    }
    /// Флаги ChN…ChD по строке направлений, например "ne".
    fn ch(self, dirs: &str) -> Self {
        let mut w = self;
        for (code, prop) in [
            ('n', "ChN"),
            ('s', "ChS"),
            ('w', "ChW"),
            ('e', "ChE"),
            ('u', "ChU"),
            ('d', "ChD"),
        ] {
            w = w.bool(prop, dirs.contains(code));
        }
        w
    }
    /// Координаты как в файле: X — строка, Y — столбец.
    fn at(self, row: i32, col: i32, level: i32) -> Self {
        self.int("X", row)
            .int("Y", col)
            .int("L", level)
            .int("DX", 8)
            .int("DY", 9)
            .int("DL", 0)
    }
    fn end(mut self) -> Vec<u8> {
        self.0.extend([0, 0]);
        self.0
    }
}

fn exit<'a>(room: &'a Mm2Room, dir: &str) -> &'a Mm2Exit {
    room.exits.iter().find(|e| e.dir == dir).unwrap()
}

#[test]
fn empty_file_is_empty_zone() {
    let zone = load_mm2(&[]).unwrap();
    assert!(zone.rooms.is_empty());
    assert_eq!(zone.view_offset, None);
}

#[test]
fn loads_rooms_and_resolves_neighbours() {
    let mut data = Vec::new();
    // Комната 1 в (строка 0, столбец 0), выход на восток.
    data.extend(
        Writer::room()
            .int("Tag", 1)
            .wide("Exits", "В")
            .utf8("Hint", "Заброшенная тропинка [90001]")
            .ch("e")
            .utf8("Description", "Лужа.\r\nДеревья.")
            .at(0, 0, 0)
            .end(),
    );
    // Комната 2 восточнее, выходы на запад и вверх (дверь закрыта).
    data.extend(
        Writer::room()
            .int("Tag", 2)
            .wide("Exits", "З(^)")
            .utf8("Hint", "Поляна [90002]")
            .ch("wu")
            .ident("BColor", "clYellow")
            .at(0, 1, 0)
            .utf8("Note", "г помогу")
            .end(),
    );
    // Комната 3 над комнатой 2, без vnum, цвет числом, выход вниз и на юг (несвязанный).
    data.extend(
        Writer::room()
            .int("Tag", 3)
            .wide("Exits", "Юv")
            .utf8("Hint", "Чердак")
            .ch("d")
            .int("BColor", 0x0040_80FF)
            .at(0, 1, 1)
            .bool("DT", true)
            .int("ImageIndex", 1)
            .end(),
    );

    let zone = load_mm2(&data).unwrap();
    assert_eq!(zone.rooms.len(), 3);
    assert_eq!(zone.view_offset, Some((8, 9, 0)));

    let r1 = &zone.rooms[0];
    assert_eq!(r1.tag, Some(1));
    assert_eq!(r1.vnum, Some(90001));
    assert_eq!(r1.name, "Заброшенная тропинка");
    assert_eq!(r1.description, "Лужа.\r\nДеревья.");
    assert_eq!((r1.x, r1.y, r1.level), (0, 0, 0));
    assert_eq!(r1.exits.len(), 1);
    assert_eq!(exit(r1, "e").to_vnum, Some(90002));

    let r2 = &zone.rooms[1];
    assert_eq!((r2.x, r2.y), (1, 0), "столбец Y файла — это x");
    assert_eq!(exit(r2, "w").to_tag, Some(1));
    assert_eq!(exit(r2, "u").to_vnum, None, "над комнатой 2 ничего нет");
    assert!(exit(r2, "u").door);
    assert_eq!(r2.color, Some(Mm2Color::Rgb(255, 255, 0)));
    assert_eq!(r2.note, "г помогу");

    let r3 = &zone.rooms[2];
    assert_eq!(r3.vnum, None);
    assert_eq!(r3.name, "Чердак");
    assert_eq!(exit(r3, "d").to_tag, Some(2));
    let south = exit(r3, "s");
    assert!(!south.linked);
    assert_eq!(south.to_tag, None);
    assert_eq!(r3.color, Some(Mm2Color::Rgb(0xFF, 0x80, 0x40)));
    assert!(r3.death_trap);
    assert_eq!(r3.icon, Some(1));
}

#[test]
fn exits_go_through_pipes() {
    let mut data = Vec::new();
    data.extend(
        Writer::room()
            .int("Tag", 1)
            .wide("Exits", "Ю")
            .utf8("Hint", "Север [1]")
            .ch("s")
            .at(0, 0, 0)
            .end(),
    );
    for row in 1..=3 {
        data.extend(
            Writer::room()
                .wide("Exits", "СЮ")
                .ch("ns")
                .at(row, 0, 0)
                .bool("Pipe", true)
                .end(),
        );
    }
    data.extend(
        Writer::room()
            .int("Tag", 2)
            .wide("Exits", "С")
            .utf8("Hint", "Юг [2]")
            .ch("n")
            .at(4, 0, 0)
            .end(),
    );
    let zone = load_mm2(&data).unwrap();
    assert_eq!(zone.rooms.len(), 2);
    assert_eq!(zone.pipes.len(), 3);
    assert!(zone.pipes.iter().all(|p| p.vertical));
    assert_eq!(exit(&zone.rooms[0], "s").to_vnum, Some(2));
    assert_eq!(exit(&zone.rooms[1], "n").to_vnum, Some(1));
}

#[test]
fn exit_to_other_zone() {
    let data = Writer::room()
        .int("Tag", 7)
        .wide("Exits", "СЮ")
        .utf8("Hint", "Опушка [500]")
        .ch("n")
        .at(0, 0, 0)
        .strings(
            "AutoMaps.Strings",
            &["Черное Болото|на север|36|", "Ад|вниз|2|отд;г куда"],
        )
        .end();
    let zone = load_mm2(&data).unwrap();
    let room = &zone.rooms[0];
    assert_eq!(room.zone_links.len(), 2);
    assert_eq!(room.zone_links[1].dir, "d");
    assert_eq!(room.zone_links[1].commands, "отд;г куда");
    let north = exit(room, "n");
    let link = north.to_zone.as_ref().unwrap();
    assert_eq!(link.zone, "Черное Болото");
    assert_eq!(link.to_tag, Some(36));
    // На юг — выход из строки сервера, без связи и без перехода.
    assert_eq!(exit(room, "s").to_zone, None);
}

#[test]
fn unknown_properties_and_classes_are_kept() {
    let mut data = Writer::room()
        .int("Tag", 1)
        .utf8("Hint", "X")
        .int("Mystery", 5)
        .end();
    let mut other = b"TPF0".to_vec();
    other.extend([6]);
    other.extend(b"TOther");
    other.extend([0, 0, 0]);
    data.extend(other);
    let zone = load_mm2(&data).unwrap();
    assert_eq!(
        zone.rooms[0].extra,
        vec![("Mystery".to_string(), Value::Int(5))]
    );
    assert_eq!(zone.skipped_classes, vec!["TOther".to_string()]);
}

#[test]
fn broken_file_is_error() {
    let mut data = Writer::room().int("Tag", 1).end();
    data.truncate(data.len() - 3);
    assert!(load_mm2(&data).is_err());
}

#[test]
fn helpers() {
    assert_eq!(split_vnum("Тропа [ 12 ] "), ("Тропа".to_string(), Some(12)));
    assert_eq!(split_vnum("Тропа [abc]"), ("Тропа [abc]".to_string(), None));
    let (dirs, doors) = parse_game_exits("(С)ВЮ(З)^v");
    assert_eq!(dirs, ["n", "e", "s", "w", "u", "d"]);
    assert_eq!(doors, ["n", "w"]);
}

/// Сводка по картам владельца, если папка есть на этой машине (иначе тест ничего не делает).
/// Подробная печать — `cargo run -p klubok-mm2 --example survey`.
#[test]
fn survey_owner_maps_if_present() {
    let dir = std::path::Path::new("/mnt/c/Users/Alex/Desktop/mud/mudmapper/Maps/Былины");
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("папки с картами нет — пропуск");
        return;
    };
    let mut errors = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mm2") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Err(e) = load_mm2(&bytes) {
            eprintln!("{}: {e}", path.display());
            errors += 1;
        }
    }
    assert_eq!(errors, 0, "ошибки разбора карт владельца");
}
