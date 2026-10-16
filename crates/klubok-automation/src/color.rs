//! Цвета подсветки в стиле JMC: `light red`, `white,b black` (после `b` — фон).

use klubok_proto::{Color, Style};

/// Номер цвета (0–15) по имени JMC.
fn color_index(name: &str) -> Option<u8> {
    let name = name.trim().to_lowercase();
    let n = match name.as_str() {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "brown" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "grey" | "gray" | "light grey" | "light gray" => 7,
        "charcoal" | "dark grey" | "dark gray" => 8,
        "light red" => 9,
        "light green" => 10,
        "yellow" | "light yellow" | "light brown" => 11,
        "light blue" => 12,
        "light magenta" | "pink" => 13,
        "light cyan" => 14,
        "white" => 15,
        other => return other.parse::<u8>().ok().filter(|n| *n < 16),
    };
    Some(n)
}

/// Разбирает описание цвета. `None` — непонятное описание.
pub fn parse_highlight(spec: &str) -> Option<HighlightColor> {
    let mut color = HighlightColor::default();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some(bg) = part.strip_prefix("b ") {
            color.bg = Some(color_index(bg)?);
        } else if !part.is_empty() {
            color.fg = Some(color_index(part)?);
        }
    }
    (color.fg.is_some() || color.bg.is_some()).then_some(color)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HighlightColor {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
}

impl HighlightColor {
    pub fn apply(self, style: &mut Style) {
        if let Some(fg) = self.fg {
            style.fg = Color::Named(fg);
            // Цвет задан точно — жирность не должна делать его «ярче».
            style.bold = false;
        }
        if let Some(bg) = self.bg {
            style.bg = Color::Named(bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn цвета_jmc() {
        assert_eq!(
            parse_highlight("white,b black"),
            Some(HighlightColor {
                fg: Some(15),
                bg: Some(0)
            })
        );
        assert_eq!(parse_highlight("light red").unwrap().fg, Some(9));
        assert_eq!(parse_highlight("b blue").unwrap().bg, Some(4));
        assert_eq!(parse_highlight("фиолетовый"), None);
    }
}
