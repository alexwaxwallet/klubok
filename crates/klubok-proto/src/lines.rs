//! Разбивка текста на строки.
//!
//! Былины обычно шлют `\r\n`, но в отдельных сообщениях встречается `\n\r`.
//! Любая пара из `\r` и `\n` (в любом порядке) — это один перевод строки,
//! иначе появляются фантомные пустые строки.

/// Что получилось из текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawLine {
    /// Строка, закончившаяся переводом строки (без самого перевода).
    Line(Vec<u8>),
    /// Незаконченная строка, после которой пришёл `IAC GA`, — промпт.
    Prompt(Vec<u8>),
}

#[derive(Debug, Default, Clone)]
pub struct LineAssembler {
    buf: Vec<u8>,
    /// Каким символом закончилась предыдущая строка: парный ему следующий символ проглатывается.
    last_break: Option<u8>,
}

impl LineAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, data: &[u8], out: &mut Vec<RawLine>) {
        for &b in data {
            match b {
                b'\r' | b'\n' => {
                    let other = if b == b'\r' { b'\n' } else { b'\r' };
                    if self.last_break == Some(other) {
                        // Вторая половина пары `\r\n` / `\n\r`.
                        self.last_break = None;
                    } else {
                        out.push(RawLine::Line(std::mem::take(&mut self.buf)));
                        self.last_break = Some(b);
                    }
                }
                // NUL сервер иногда шлёт как заполнитель — он не нужен.
                0 => {}
                _ => {
                    self.buf.push(b);
                    self.last_break = None;
                }
            }
        }
    }

    /// Сервер прислал `IAC GA`: незаконченная строка — это промпт.
    pub fn go_ahead(&mut self, out: &mut Vec<RawLine>) {
        if !self.buf.is_empty() {
            out.push(RawLine::Prompt(std::mem::take(&mut self.buf)));
        }
    }

    /// Есть ли незаконченная строка (например, «Введите имя: » без GA).
    pub fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }

    /// Забирает незаконченную строку как промпт. Вызывается по таймауту, когда
    /// сервер замолчал, не закончив строку и не прислав GA.
    pub fn take_partial(&mut self) -> Option<RawLine> {
        if self.buf.is_empty() {
            None
        } else {
            Some(RawLine::Prompt(std::mem::take(&mut self.buf)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(chunks: &[&[u8]]) -> Vec<RawLine> {
        let mut a = LineAssembler::new();
        let mut out = Vec::new();
        for c in chunks {
            a.feed(c, &mut out);
        }
        out
    }

    fn line(s: &str) -> RawLine {
        RawLine::Line(s.as_bytes().to_vec())
    }

    #[test]
    fn crlf_и_lfcr() {
        assert_eq!(
            lines(&[b"a\r\nb\n\rc\nd\r\n"]),
            vec![line("a"), line("b"), line("c"), line("d")]
        );
    }

    #[test]
    fn двойной_перевод_это_одна_пустая_строка() {
        assert_eq!(
            lines(&[b"a\r\n\r\nb\r\n"]),
            vec![line("a"), line(""), line("b")]
        );
        assert_eq!(
            lines(&[b"a\n\r\n\rb\n\r"]),
            vec![line("a"), line(""), line("b")]
        );
    }

    #[test]
    fn пара_разрезана_между_пакетами() {
        assert_eq!(
            lines(&[b"a\r", b"\nb\r", b"\n"]),
            vec![line("a"), line("b")]
        );
    }

    #[test]
    fn промпт_по_ga_и_по_таймауту() {
        let mut a = LineAssembler::new();
        let mut out = Vec::new();
        a.feed(b"x\r\n20H> ", &mut out);
        a.go_ahead(&mut out);
        assert_eq!(out, vec![line("x"), RawLine::Prompt(b"20H> ".to_vec())]);

        a.feed(b"Select one : ", &mut out);
        assert!(a.has_partial());
        assert_eq!(
            a.take_partial(),
            Some(RawLine::Prompt(b"Select one : ".to_vec()))
        );
        assert!(!a.has_partial());
    }
}
