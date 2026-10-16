# Клубок

Мад-клиент для [Былин](https://bylins.su): триггеры и алиасы как в JMC, карта и спидволк как в MudMapper — в одном приложении для Windows, Linux и Android.

> Проект в самом начале разработки.

## Сборка

Нужен [Rust](https://rustup.rs).

```bash
cargo build --release
cargo run -p klubok-cli -- --host bylins.su --port 4000
```

Консольный клиент — отладочный: умеет telnet, сжатие MCCP v2, MSDP, кодировки
UTF-8 / KOI8-R / CP1251, ANSI-цвета и схлопывание лишних пустых строк.

```bash
cargo run -p klubok-cli -- --help
cargo run -p klubok-cli -- --debug                        # показывать MSDP и переговоры
cargo run -p klubok-cli -- --record recordings/x.local.bin # записать трафик
cargo run -p klubok-cli -- --replay recordings/x.local.bin # прогнать запись без сети
```

## Лицензия

[GPL-3.0-or-later](LICENSE)
