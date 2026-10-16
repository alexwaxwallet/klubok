//! Схлопывание пустых строк.
//!
//! Сервер Былин вставляет `\r\n` перед каждой порцией вывода, а в некомпактном
//! режиме — ещё и пустую строку перед промптом. Отсюда двойные пустые строки,
//! которые в JMC не убрать.

use klubok_proto::StyledLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlankLineSettings {
    /// Сколько пустых строк подряд оставлять (0 — убирать все).
    pub max_consecutive: usize,
    /// Убирать пустые строки сразу после промпта.
    pub trim_after_prompt: bool,
}

impl Default for BlankLineSettings {
    fn default() -> Self {
        BlankLineSettings {
            max_consecutive: 1,
            trim_after_prompt: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BlankLineFilter {
    pub settings: BlankLineSettings,
    /// Сколько пустых строк подряд уже пропущено на экран.
    blank_run: usize,
    /// Предыдущим был промпт, и с тех пор ещё не было непустой строки.
    after_prompt: bool,
}

impl BlankLineFilter {
    pub fn new(settings: BlankLineSettings) -> Self {
        BlankLineFilter {
            settings,
            ..Default::default()
        }
    }

    /// Показывать ли строку.
    pub fn keep(&mut self, line: &StyledLine) -> bool {
        if !line.is_blank() {
            self.blank_run = 0;
            self.after_prompt = false;
            return true;
        }
        if self.after_prompt && self.settings.trim_after_prompt {
            return false;
        }
        if self.blank_run >= self.settings.max_consecutive {
            return false;
        }
        self.blank_run += 1;
        true
    }

    /// Пришёл промпт.
    pub fn on_prompt(&mut self) {
        self.blank_run = 0;
        self.after_prompt = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прогоняет строки через фильтр; "> " в начале означает промпт.
    fn run(settings: BlankLineSettings, input: &[&str]) -> Vec<String> {
        let mut f = BlankLineFilter::new(settings);
        let mut out = Vec::new();
        for &s in input {
            if s.starts_with('>') {
                f.on_prompt();
                out.push(s.to_string());
            } else if f.keep(&StyledLine::plain(s)) {
                out.push(s.to_string());
            }
        }
        out
    }

    #[test]
    fn по_умолчанию() {
        let out = run(
            BlankLineSettings::default(),
            &[
                "> ",
                "",
                "",
                "Вы ударили.",
                "",
                "",
                "",
                "Он умер.",
                "",
                "> ",
            ],
        );
        assert_eq!(out, ["> ", "Вы ударили.", "", "Он умер.", "", "> "]);
    }

    #[test]
    fn убрать_все() {
        let settings = BlankLineSettings {
            max_consecutive: 0,
            trim_after_prompt: false,
        };
        let out = run(settings, &["a", "", " ", "b"]);
        assert_eq!(out, ["a", "b"]);
    }

    #[test]
    fn оставить_после_промпта() {
        let settings = BlankLineSettings {
            max_consecutive: 1,
            trim_after_prompt: false,
        };
        let out = run(settings, &["> ", "", "", "a"]);
        assert_eq!(out, ["> ", "", "a"]);
    }
}
