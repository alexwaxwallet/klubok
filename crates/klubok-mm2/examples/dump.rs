//! Печатает все компоненты .mm2-файла в виде дерева свойств (для исследования формата).
//!
//! `cargo run -p klubok-mm2 --example dump -- путь/к/файлу.mm2`
fn main() {
    let path = std::env::args().nth(1).expect("укажите путь к .mm2");
    let data = std::fs::read(&path).expect("не удалось прочитать файл");
    match klubok_mm2::dfm::parse_all(&data) {
        Ok(list) => {
            for c in list {
                println!("{} '{}'", c.class, c.name);
                for (name, value) in &c.props {
                    println!("  {name} = {value:?}");
                }
                for child in &c.children {
                    println!("  child {} '{}' {:?}", child.class, child.name, child.props);
                }
            }
        }
        Err(e) => eprintln!("ошибка: {e}"),
    }
}
