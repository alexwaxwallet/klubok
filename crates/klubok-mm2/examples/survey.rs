//! Сводка по всем `.mm2` в папке: файлы, комнаты, выходы, ошибки.
//!
//! `cargo run -p klubok-mm2 --example survey -- [папка]`
//! По умолчанию — папка карт владельца; если её нет, пример просто сообщает об этом.

use std::collections::HashSet;

const DEFAULT_DIR: &str = "/mnt/c/Users/Alex/Desktop/mud/mudmapper/Maps/Былины";

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_DIR.to_string());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("папки {dir} нет — нечего смотреть");
        return;
    };

    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("mm2"))
        .collect();
    paths.sort();

    let zone_names: HashSet<String> = paths
        .iter()
        .filter_map(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let (mut files, mut empty, mut errors) = (0, 0, 0);
    let (mut rooms, mut with_vnum, mut pipes) = (0, 0, 0);
    let (mut exits, mut linked, mut resolved, mut to_zone, mut dangling, mut doors) =
        (0, 0, 0, 0, 0, 0);
    let (mut zone_links, mut zone_links_bad) = (0, 0);
    let (mut notes, mut colored, mut with_desc) = (0, 0, 0);
    let mut vnums = HashSet::new();
    let mut extra_props = HashSet::new();

    for path in &paths {
        files += 1;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("{}: не читается: {e}", path.display());
                errors += 1;
                continue;
            }
        };
        if bytes.is_empty() {
            empty += 1;
            continue;
        }
        let zone = match klubok_mm2::load_mm2(&bytes) {
            Ok(z) => z,
            Err(e) => {
                println!("{}: {e}", path.display());
                errors += 1;
                continue;
            }
        };
        pipes += zone.pipes.len();
        for room in &zone.rooms {
            rooms += 1;
            if let Some(v) = room.vnum {
                with_vnum += 1;
                vnums.insert(v);
            }
            notes += usize::from(!room.note.is_empty());
            colored += usize::from(room.color.is_some());
            with_desc += usize::from(!room.description.is_empty());
            for (name, _) in &room.extra {
                extra_props.insert(name.clone());
            }
            for link in &room.zone_links {
                zone_links += 1;
                if !zone_names.contains(&link.zone) {
                    zone_links_bad += 1;
                }
            }
            for exit in &room.exits {
                exits += 1;
                doors += usize::from(exit.door);
                if exit.linked {
                    linked += 1;
                    if exit.to_tag.is_some() {
                        resolved += 1;
                    } else if exit.to_zone.is_some() {
                        to_zone += 1;
                    } else {
                        dangling += 1;
                    }
                }
            }
        }
    }

    println!("Папка: {dir}");
    println!("Файлов .mm2: {files} (пустых: {empty}, с ошибками: {errors})");
    println!(
        "Комнат: {rooms}, с vnum: {with_vnum} (разных vnum: {}), труб: {pipes}",
        vnums.len()
    );
    println!("С описанием: {with_desc}, с заметкой: {notes}, с цветом: {colored}");
    println!("Выходов всего: {exits} (с закрытой дверью: {doors})");
    println!(
        "  связанных на карте: {linked}: в комнату зоны {resolved}, в другую зону {to_zone}, \
         в никуда {dangling}"
    );
    println!(
        "  только из строки сервера (не связаны): {}",
        exits - linked
    );
    println!("Переходов в другие зоны: {zone_links} (зона-файл не найден: {zone_links_bad})");
    if !extra_props.is_empty() {
        println!("Неизвестные свойства: {extra_props:?}");
    }
}
