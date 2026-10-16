//! Прогон настоящего трафика локального сервера Былин через сессию.
//!
//! Запись `recordings/bylins-login-look.bin`: подключение, MCCP v2, MSDP,
//! выбор кодировки UTF-8, вход Тестером, «смотреть», «выходы».

use klubok_core::{Automation, SessionConfig, SessionOutput, replay};

const RECORDING: &[u8] = include_bytes!("../../../recordings/bylins-login-look.bin");

fn texts(outputs: &[SessionOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            SessionOutput::Line(l) => Some(l.text()),
            _ => None,
        })
        .collect()
}

#[test]
fn запись_начинается_с_mccp() {
    // Сразу после приветствия сервер включает сжатие — значит, тест действительно проверяет MCCP.
    assert!(RECORDING.windows(5).any(|w| w == b"\xff\xfaV\xff\xf0"));
}

#[test]
fn вход_и_осмотр_комнаты() {
    let out = replay(SessionConfig::default(), Automation::new(), RECORDING, 4096).unwrap();
    let lines = texts(&out);

    assert!(lines.iter().any(|l| l == "Тронный зал"));
    assert!(lines.iter().any(|l| l.starts_with("Север  - [185]")));
    // ESC-коды не просочились в текст.
    assert!(lines.iter().all(|l| !l.contains('\x1b')));
    // Пустых строк подряд не больше одной.
    assert!(
        lines
            .windows(2)
            .all(|w| !(w[0].is_empty() && w[1].is_empty()))
    );

    let room = out.iter().find_map(|o| match o {
        SessionOutput::Msdp(name, value) if name == "ROOM" => Some(value.clone()),
        _ => None,
    });
    let room = room.expect("сервер прислал MSDP ROOM");
    assert_eq!(room.get("VNUM").and_then(|v| v.as_str()), Some("100"));
    assert_eq!(
        room.get("NAME").and_then(|v| v.as_str()),
        Some("Тронный зал")
    );
}

#[test]
fn нарезка_пакетов_не_влияет_на_результат() {
    let whole = texts(
        &replay(
            SessionConfig::default(),
            Automation::new(),
            RECORDING,
            usize::MAX,
        )
        .unwrap(),
    );
    for chunk in [1, 3, 17, 512] {
        let pieces = texts(
            &replay(
                SessionConfig::default(),
                Automation::new(),
                RECORDING,
                chunk,
            )
            .unwrap(),
        );
        assert_eq!(pieces, whole, "размер куска {chunk}");
    }
}
