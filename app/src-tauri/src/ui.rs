//! Перевод событий ядра в простой JSON для интерфейса.
//!
//! Цвета превращаются в строки, понятные CSS: `"c0"`…`"c15"` — 16 стандартных цветов
//! (их оттенки задаёт тема интерфейса), `"#rrggbb"` — всё остальное.

use klubok_automation::{WindowOp, WindowShow};
use klubok_core::{MapView, SessionOutput};
use klubok_proto::{Color, MsdpValue, StyledLine};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct UiSpan {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
}

/// Выход комнаты из MSDP: направление (`n`, `e`, …) и номер комнаты, куда он ведёт.
#[derive(Debug, Clone, Serialize)]
pub struct UiExit {
    pub dir: String,
    pub vnum: String,
}

/// Событие для интерфейса. В JSON — объект с полем `type`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum UiEvent {
    Line {
        spans: Vec<UiSpan>,
    },
    Prompt {
        spans: Vec<UiSpan>,
    },
    Room {
        vnum: String,
        name: String,
        area: String,
        exits: Vec<UiExit>,
    },
    Info {
        text: String,
    },
    /// Сообщение клиента для игрока (`#output`, списки, автовход).
    Echo {
        text: String,
    },
    /// Предупреждение (ошибка в команде, неподдерживаемая команда).
    Warning {
        text: String,
    },
    /// Связь оборвалась с ошибкой (или не удалось подключиться) — можно переподключаться.
    ConnectionLost {
        text: String,
    },
    Bell,
    Hotkeys {
        hotkeys: Vec<UiHotkey>,
    },
    /// Строка в окно вывода (`#woutput`).
    Woutput {
        window: usize,
        spans: Vec<UiSpan>,
    },
    /// Заголовок окна (`#wname`).
    Wname {
        window: usize,
        name: String,
    },
    /// Показать/скрыть окно: `mode` — `toggle`, `show`, `hide`.
    Wshow {
        window: usize,
        mode: &'static str,
    },
    Wclear {
        window: usize,
    },
    /// Найденные комнаты (поиск в панели карты).
    MapFound {
        rooms: Vec<klubok_core::FoundRoom>,
    },
    /// Состояние персонажа (жизнь, бодрость, группа…).
    Status(klubok_core::CharStatus),
    /// Карта текущей зоны и уровня.
    Map {
        current: u32,
        center: u32,
        zone: u32,
        area: String,
        z: i32,
        own_z: i32,
        rooms: Vec<UiMapRoom>,
        /// Комнаты уровнем выше/ниже — только клетки, полупрозрачно.
        ghosts: Vec<UiMapGhost>,
        path: Vec<u32>,
    },
    Disconnected,
}

/// Комната для рисования карты. `y` растёт на север.
/// Комната соседнего уровня: только где рисовать.
#[derive(Debug, Clone, Serialize)]
pub struct UiMapGhost {
    pub vnum: u32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiMapRoom {
    pub vnum: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    pub exits: Vec<UiMapExit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiMapExit {
    pub dir: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub door: bool,
}

fn map_event(view: MapView) -> UiEvent {
    UiEvent::Map {
        current: view.current,
        center: view.center,
        zone: view.zone,
        area: view.area,
        z: view.z,
        own_z: view.own_z,
        rooms: view.rooms.into_iter().map(ui_room).collect(),
        ghosts: view
            .ghosts
            .into_iter()
            .map(|r| UiMapGhost {
                vnum: r.vnum,
                x: r.x,
                y: r.y,
                z: r.z,
            })
            .collect(),
        path: view.path,
    }
}

fn ui_room(r: klubok_map::Room) -> UiMapRoom {
    UiMapRoom {
        vnum: r.vnum,
        exits: r
            .exits
            .iter()
            .map(|(dir, exit)| UiMapExit {
                dir: dir.msdp_key(),
                to: exit.to,
                hidden: exit.hidden,
                door: exit.door.is_some(),
            })
            .collect(),
        name: r.name,
        x: r.x,
        y: r.y,
        note: r.note,
        color: r.color,
        flags: r.flags,
    }
}

/// Горячая клавиша: `key` в записи JMC (`Ctrl+NUM7`), `body` — что выполнить.
#[derive(Debug, Clone, Serialize)]
pub struct UiHotkey {
    pub key: String,
    pub body: String,
}

impl UiEvent {
    /// `None` — событие интерфейсу не нужно.
    pub fn from_output(output: SessionOutput) -> Option<UiEvent> {
        match output {
            SessionOutput::Line(line) => Some(UiEvent::Line {
                spans: spans(&line),
            }),
            SessionOutput::Prompt(line) => Some(UiEvent::Prompt {
                spans: spans(&line),
            }),
            SessionOutput::Msdp(name, value) if name == "ROOM" => Some(room(&value)),
            SessionOutput::Msdp(..) => None,
            SessionOutput::Info(text) => Some(UiEvent::Info { text }),
            SessionOutput::Echo(text) => Some(UiEvent::Echo { text }),
            SessionOutput::Warning(text) => Some(UiEvent::Warning { text }),
            SessionOutput::Bell => Some(UiEvent::Bell),
            SessionOutput::HotkeysChanged(list) => Some(UiEvent::Hotkeys {
                hotkeys: list
                    .into_iter()
                    .map(|h| UiHotkey {
                        key: h.key,
                        body: h.body,
                    })
                    .collect(),
            }),
            SessionOutput::Map(view) => Some(map_event(view)),
            SessionOutput::Status(status) => Some(UiEvent::Status(status)),
            SessionOutput::MapFound(rooms) => Some(UiEvent::MapFound { rooms }),
            SessionOutput::Window(op) => Some(match op {
                WindowOp::Output { window, line } => UiEvent::Woutput {
                    window,
                    spans: spans(&line),
                },
                WindowOp::Name { window, name } => UiEvent::Wname { window, name },
                WindowOp::Show { window, mode } => UiEvent::Wshow {
                    window,
                    mode: match mode {
                        WindowShow::Toggle => "toggle",
                        WindowShow::Show => "show",
                        WindowShow::Hide => "hide",
                    },
                },
                WindowOp::Clear { window } => UiEvent::Wclear { window },
            }),
            SessionOutput::Disconnected => Some(UiEvent::Disconnected),
            // Сохранение настроек и отправка серверу обрабатываются до интерфейса.
            SessionOutput::AutomationChanged(_)
            | SessionOutput::Send(_)
            | SessionOutput::Log(_)

            | SessionOutput::CloseRequested => None,
        }
    }
}

fn room(value: &MsdpValue) -> UiEvent {
    let field = |key: &str| {
        value
            .get(key)
            .and_then(MsdpValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let exits = match value.get("EXITS") {
        Some(MsdpValue::Table(items)) => items
            .iter()
            .map(|(dir, vnum)| UiExit {
                dir: dir.clone(),
                vnum: vnum.as_str().unwrap_or_default().to_string(),
            })
            .collect(),
        _ => Vec::new(),
    };
    UiEvent::Room {
        vnum: field("VNUM"),
        name: field("NAME"),
        area: field("AREA"),
        exits,
    }
}

fn spans(line: &StyledLine) -> Vec<UiSpan> {
    line.spans
        .iter()
        .map(|span| {
            let st = span.style;
            // Как в классических мад-клиентах: жирный + обычный цвет = яркий цвет.
            let fg = match st.fg {
                Color::Named(n) if st.bold && n < 8 => Color::Named(n + 8),
                other => other,
            };
            UiSpan {
                text: span.text.clone(),
                fg: css_color(fg),
                bg: css_color(st.bg),
                bold: st.bold,
                italic: st.italic,
                underline: st.underline,
            }
        })
        .collect()
}

fn css_color(color: Color) -> Option<String> {
    match color {
        Color::Default => None,
        Color::Named(n) => Some(format!("c{n}")),
        Color::Indexed(n) if n < 16 => Some(format!("c{n}")),
        Color::Indexed(n) => {
            let (r, g, b) = xterm_256(n);
            Some(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

/// Цвета 16–255 стандартной палитры xterm: куб 6×6×6 и 24 оттенка серого.
fn xterm_256(n: u8) -> (u8, u8, u8) {
    if n >= 232 {
        let v = 8 + (n - 232) * 10;
        return (v, v, v);
    }
    let i = n - 16;
    let level = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
    (level(i / 36), level((i / 6) % 6), level(i % 6))
}

#[cfg(test)]
mod tests {
    use super::*;
    use klubok_proto::AnsiParser;

    #[test]
    fn жирный_красный_становится_ярким() {
        let line = AnsiParser::new().parse("\x1b[0;1;31mкрасный");
        let spans = spans(&line);
        assert_eq!(spans[0].fg.as_deref(), Some("c9"));
    }

    #[test]
    fn палитра_256() {
        assert_eq!(xterm_256(16), (0, 0, 0));
        assert_eq!(xterm_256(196), (255, 0, 0));
        assert_eq!(xterm_256(255), (238, 238, 238));
    }
}
