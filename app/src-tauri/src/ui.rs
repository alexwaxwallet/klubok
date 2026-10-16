//! Перевод событий ядра в простой JSON для интерфейса.
//!
//! Цвета превращаются в строки, понятные CSS: `"c0"`…`"c15"` — 16 стандартных цветов
//! (их оттенки задаёт тема интерфейса), `"#rrggbb"` — всё остальное.

use klubok_core::SessionOutput;
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
    Error {
        text: String,
    },
    Disconnected,
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
            SessionOutput::Disconnected => Some(UiEvent::Disconnected),
            SessionOutput::Send(_) => None,
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
