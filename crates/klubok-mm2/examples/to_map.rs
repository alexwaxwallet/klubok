//! Переводит папку `.mm2` в карту Клубка и печатает итоги:
//! `cargo run -p klubok-mm2 --example to_map -- <папка> [--out=карта.sqlite] [vnum_откуда vnum_куда]`.

use klubok_mm2::{convert, load_mm2};

fn main() {
    let (out, rest): (Vec<String>, Vec<String>) = std::env::args()
        .skip(1)
        .partition(|a| a.starts_with("--out="));
    let mut args = rest.into_iter();
    let dir = args.next().expect("укажите папку с .mm2");
    let mut zones = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("папка") {
        let path = entry.expect("файл").path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mm2"))
        {
            let bytes = std::fs::read(&path).expect("чтение");
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            zones.push((name, load_mm2(&bytes).expect("разбор")));
        }
    }
    let started = std::time::Instant::now();
    let (map, stats) = convert::to_map(&zones);
    println!("{stats:?} за {:?}", started.elapsed());
    if let Some(path) = out.first().map(|a| &a["--out=".len()..]) {
        use klubok_map::MapStore;
        let mut store = klubok_map::SqliteStore::open(path).expect("база");
        store.save_map(&map).expect("запись");
        println!("записано в {path}");
    }
    if let (Some(from), Some(to)) = (args.next(), args.next()) {
        let path = klubok_map::find_path(&map, from.parse().unwrap(), to.parse().unwrap());
        println!(
            "{:?}",
            path.map(|p| klubok_map::speedwalk_commands(&p).join(";"))
        );
    }
}
