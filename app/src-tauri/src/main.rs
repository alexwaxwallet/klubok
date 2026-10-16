// В релизе на Windows не открывать рядом с окном чёрную консоль. НЕ УДАЛЯТЬ.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    klubok_lib::run()
}
