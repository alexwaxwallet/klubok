//! ANSI-цвета: строка с ESC-кодами → `StyledLine` (куски текста со стилем).
//!
//! Стиль переживает конец строки: если сервер включил красный и не сбросил,
//! следующая строка тоже красная. Поэтому `AnsiParser` хранит текущий стиль.

/// Цвет текста или фона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Цвет по умолчанию (задаёт тема интерфейса).
    #[default]
    Default,
    /// Одна из 16 стандартных: 0–7 обычные, 8–15 яркие.
    Named(u8),
    /// Палитра из 256 цветов (`38;5;n`).
    Indexed(u8),
    /// Полноцветный (`38;2;r;g;b`).
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Кусок текста одного стиля.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// Строка, готовая к показу: текст без ESC-кодов, разбитый на куски по стилям.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledLine {
    pub spans: Vec<Span>,
}

impl StyledLine {
    /// Строка из простого текста без цветов.
    pub fn plain(text: &str) -> StyledLine {
        let mut line = StyledLine::default();
        line.push(text, Style::default());
        line
    }

    /// Текст без стилей — по нему работают триггеры.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Пустая строка: нет ничего, кроме пробелов.
    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }

    /// Добавляет текст; если стиль тот же, что у последнего куска, — дописывает к нему.
    pub fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_string(),
                style,
            }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnsiParser {
    style: Style,
}

impl AnsiParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Разбирает одну строку (уже без перевода строки).
    pub fn parse(&mut self, input: &str) -> StyledLine {
        let mut line = StyledLine::default();
        let mut text = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if chars.peek() != Some(&'[') {
                    continue; // одиночный ESC — выбрасываем
                }
                chars.next();
                // CSI: параметры (цифры и `;`) и финальная буква.
                let mut params = String::new();
                let mut final_char = None;
                for p in chars.by_ref() {
                    if p.is_ascii_digit() || p == ';' {
                        params.push(p);
                    } else {
                        final_char = Some(p);
                        break;
                    }
                }
                // Цвет меняет только `m`; остальные последовательности (курсор и т.п.) игнорируем.
                if final_char == Some('m') {
                    line.push(&text, self.style);
                    text.clear();
                    self.apply_sgr(&params);
                }
            } else if c == '\t' || !c.is_control() {
                text.push(c);
            }
            // Прочие управляющие символы (звонок \x07 и т.п.) не показываем.
        }

        line.push(&text, self.style);
        line
    }

    /// Применяет `ESC[...m` (SGR) к текущему стилю.
    fn apply_sgr(&mut self, params: &str) {
        // Пустой список (`ESC[m`) означает сброс.
        let codes: Vec<u32> = if params.is_empty() {
            vec![0]
        } else {
            params.split(';').map(|p| p.parse().unwrap_or(0)).collect()
        };

        let mut i = 0;
        while i < codes.len() {
            let s = &mut self.style;
            match codes[i] {
                0 => *s = Style::default(),
                1 => s.bold = true,
                3 => s.italic = true,
                4 => s.underline = true,
                22 => s.bold = false,
                23 => s.italic = false,
                24 => s.underline = false,
                n @ 30..=37 => s.fg = Color::Named((n - 30) as u8),
                39 => s.fg = Color::Default,
                n @ 40..=47 => s.bg = Color::Named((n - 40) as u8),
                49 => s.bg = Color::Default,
                n @ 90..=97 => s.fg = Color::Named((n - 90 + 8) as u8),
                n @ 100..=107 => s.bg = Color::Named((n - 100 + 8) as u8),
                n @ (38 | 48) => {
                    // Расширенные цвета: `38;5;n` или `38;2;r;g;b`.
                    let (color, used) = extended_color(&codes[i + 1..]);
                    if let Some(color) = color {
                        if n == 38 {
                            s.fg = color;
                        } else {
                            s.bg = color;
                        }
                    }
                    i += used;
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn extended_color(rest: &[u32]) -> (Option<Color>, usize) {
    let byte = |i: usize| rest.get(i).map(|&v| v.min(255) as u8);
    match rest.first() {
        Some(5) => (byte(1).map(Color::Indexed), 2),
        Some(2) => match (byte(1), byte(2), byte(3)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(r, g, b)), 4),
            _ => (None, rest.len()),
        },
        _ => (None, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fg(n: u8) -> Style {
        Style {
            fg: Color::Named(n),
            ..Style::default()
        }
    }

    #[test]
    fn цвета_былин() {
        let mut p = AnsiParser::new();
        let line = p.parse("\x1b[0;36mБазарная площадь\x1b[0;0m (город)");
        assert_eq!(line.text(), "Базарная площадь (город)");
        assert_eq!(line.spans[0].style, fg(6));
        assert_eq!(line.spans[1].style, Style::default());
    }

    #[test]
    fn стиль_переживает_конец_строки() {
        let mut p = AnsiParser::new();
        p.parse("\x1b[1;31mначало");
        let next = p.parse("продолжение");
        assert_eq!(
            next.spans[0].style,
            Style {
                bold: true,
                ..fg(1)
            }
        );
    }

    #[test]
    fn пустая_строка_с_кодами() {
        let mut p = AnsiParser::new();
        assert!(p.parse("\x1b[0m  \x1b[33m").is_blank());
        assert!(!p.parse("\x1b[0m .").is_blank());
    }

    #[test]
    fn расширенные_и_неизвестные_коды() {
        let mut p = AnsiParser::new();
        let line = p.parse("\x1b[38;5;208mа\x1b[48;2;1;2;3mб\x1b[2Jв\x07");
        assert_eq!(line.text(), "абв");
        assert_eq!(line.spans[0].style.fg, Color::Indexed(208));
        assert_eq!(line.spans[1].style.bg, Color::Rgb(1, 2, 3));
    }
}
