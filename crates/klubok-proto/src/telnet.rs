//! Telnet: отделяет служебные команды (IAC …) от текста.
//!
//! Поток от сервера — это текст, в который вкраплены команды, начинающиеся с байта
//! `IAC` (255). Парсер — конечный автомат: он помнит, на каком месте команды
//! остановился, поэтому команда может быть разрезана между двумя пакетами из сети.

/// Байты-команды telnet (RFC 854).
pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const GA: u8 = 249;
pub const SE: u8 = 240;

/// Номера опций, с которыми работает клиент.
pub mod opt {
    pub const NAWS: u8 = 31;
    pub const MSDP: u8 = 69;
    /// MCCP v1. Былины шлют для него нестандартный маркер, поэтому мы от него отказываемся.
    pub const COMPRESS: u8 = 85;
    /// MCCP v2 — сжатие потока от сервера (zlib).
    pub const COMPRESS2: u8 = 86;
}

/// Глагол переговоров: «я буду», «я не буду», «сделай», «не делай».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Will,
    Wont,
    Do,
    Dont,
}

impl Verb {
    fn from_byte(b: u8) -> Option<Verb> {
        match b {
            WILL => Some(Verb::Will),
            WONT => Some(Verb::Wont),
            DO => Some(Verb::Do),
            DONT => Some(Verb::Dont),
            _ => None,
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Verb::Will => WILL,
            Verb::Wont => WONT,
            Verb::Do => DO,
            Verb::Dont => DONT,
        }
    }
}

/// Что парсер нашёл в потоке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelnetEvent {
    /// Обычные байты текста (с уже «разудвоенными» `IAC IAC`).
    Data(Vec<u8>),
    /// `IAC GA` — сервер закончил вывод и ждёт ввода (обычно сразу после промпта).
    GoAhead,
    /// `IAC WILL/WONT/DO/DONT <опция>`.
    Negotiate(Verb, u8),
    /// `IAC SB <опция> <данные> IAC SE`.
    Subnegotiation(u8, Vec<u8>),
    /// Сервер начал MCCP v2: всё, что идёт дальше, сжато zlib.
    CompressionStarted,
}

/// Где внутри команды мы сейчас находимся.
#[derive(Debug, Clone)]
enum State {
    Text,
    Iac,
    Verb(Verb),
    SbOption,
    SbData { option: u8, data: Vec<u8> },
    SbIac { option: u8, data: Vec<u8> },
}

#[derive(Debug, Clone)]
pub struct TelnetParser {
    state: State,
}

impl Default for TelnetParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TelnetParser {
    pub fn new() -> Self {
        TelnetParser { state: State::Text }
    }

    /// Разбирает очередной кусок данных и дописывает найденное в `events`.
    ///
    /// Возвращает, сколько байт съедено. Обычно — все. Исключение: начало MCCP.
    /// После `IAC SB COMPRESS2 IAC SE` разбор останавливается, потому что
    /// остаток куска уже сжат и его сначала нужно распаковать.
    pub fn feed(&mut self, input: &[u8], events: &mut Vec<TelnetEvent>) -> usize {
        let mut text: Vec<u8> = Vec::new();

        for (i, &byte) in input.iter().enumerate() {
            // `std::mem::replace` забирает текущее состояние, оставляя на его месте Text.
            // Так можно перемещать накопленные данные (Vec) без клонирования.
            let state = std::mem::replace(&mut self.state, State::Text);
            self.state = match state {
                State::Text if byte == IAC => State::Iac,
                State::Text => {
                    text.push(byte);
                    State::Text
                }
                State::Iac => match byte {
                    // Удвоенный IAC — это просто байт 255 в тексте.
                    IAC => {
                        text.push(IAC);
                        State::Text
                    }
                    GA => {
                        flush_text(&mut text, events);
                        events.push(TelnetEvent::GoAhead);
                        State::Text
                    }
                    SB => State::SbOption,
                    other => match Verb::from_byte(other) {
                        Some(verb) => State::Verb(verb),
                        // Прочие команды (NOP, AYT…) клиенту не нужны — пропускаем.
                        None => State::Text,
                    },
                },
                State::Verb(verb) => {
                    flush_text(&mut text, events);
                    events.push(TelnetEvent::Negotiate(verb, byte));
                    State::Text
                }
                State::SbOption => State::SbData {
                    option: byte,
                    data: Vec::new(),
                },
                State::SbData { option, mut data } => {
                    if byte == IAC {
                        State::SbIac { option, data }
                    } else {
                        data.push(byte);
                        State::SbData { option, data }
                    }
                }
                State::SbIac { option, mut data } => match byte {
                    SE => {
                        flush_text(&mut text, events);
                        if option == opt::COMPRESS2 {
                            events.push(TelnetEvent::CompressionStarted);
                            return i + 1;
                        }
                        events.push(TelnetEvent::Subnegotiation(option, data));
                        State::Text
                    }
                    IAC => {
                        data.push(IAC);
                        State::SbData { option, data }
                    }
                    // Сломанная подкоманда: выбрасываем её и возвращаемся к тексту.
                    _ => State::Text,
                },
            };
        }

        flush_text(&mut text, events);
        input.len()
    }
}

fn flush_text(text: &mut Vec<u8>, events: &mut Vec<TelnetEvent>) {
    if !text.is_empty() {
        events.push(TelnetEvent::Data(std::mem::take(text)));
    }
}

/// `IAC <глагол> <опция>` — ответ в переговорах.
pub fn negotiate(verb: Verb, option: u8) -> Vec<u8> {
    vec![IAC, verb.to_byte(), option]
}

/// `IAC SB <опция> <данные> IAC SE`, байт 255 в данных удваивается.
pub fn subnegotiation(option: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![IAC, SB, option];
    out.extend(escape_iac(data));
    out.extend([IAC, SE]);
    out
}

/// Удваивает байт 255 в тексте, чтобы сервер не принял его за команду.
pub fn escape_iac(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(chunks: &[&[u8]]) -> Vec<TelnetEvent> {
        let mut parser = TelnetParser::new();
        let mut events = Vec::new();
        for chunk in chunks {
            assert_eq!(parser.feed(chunk, &mut events), chunk.len());
        }
        events
    }

    #[test]
    fn приветствие_былин() {
        let events = parse(&[b"\xff\xfbE\xff\xfbV\xff\xfbU\r\nOnline: 1"]);
        assert_eq!(
            events,
            vec![
                TelnetEvent::Negotiate(Verb::Will, opt::MSDP),
                TelnetEvent::Negotiate(Verb::Will, opt::COMPRESS2),
                TelnetEvent::Negotiate(Verb::Will, opt::COMPRESS),
                TelnetEvent::Data(b"\r\nOnline: 1".to_vec()),
            ]
        );
    }

    #[test]
    fn команда_разрезана_между_пакетами() {
        let events = parse(&[b"abc\xff", b"\xfb", b"\x45def"]);
        assert_eq!(
            events,
            vec![
                TelnetEvent::Data(b"abc".to_vec()),
                TelnetEvent::Negotiate(Verb::Will, opt::MSDP),
                TelnetEvent::Data(b"def".to_vec()),
            ]
        );
    }

    #[test]
    fn удвоенный_iac_это_текст() {
        // Пара IAC IAC разрезана между пакетами — всё равно это один байт 255.
        let events = parse(&[b"\xef\xff", b"\xffz"]);
        assert_eq!(
            events,
            vec![
                TelnetEvent::Data(vec![0xef]),
                TelnetEvent::Data(vec![0xff, b'z']),
            ]
        );
    }

    #[test]
    fn go_ahead_после_промпта() {
        let events = parse(&[b"20H 85M> \xff\xf9"]);
        assert_eq!(
            events,
            vec![
                TelnetEvent::Data(b"20H 85M> ".to_vec()),
                TelnetEvent::GoAhead
            ]
        );
    }

    #[test]
    fn подкоманда_с_экранированием() {
        let events = parse(&[b"\xff\xfaE\x01RO", b"OM\x02\xff\xffx\xff\xf0"]);
        assert_eq!(
            events,
            vec![TelnetEvent::Subnegotiation(
                opt::MSDP,
                b"\x01ROOM\x02\xffx".to_vec()
            )]
        );
    }

    #[test]
    fn mccp_останавливает_разбор() {
        let mut parser = TelnetParser::new();
        let mut events = Vec::new();
        let input = b"hi\xff\xfaV\xff\xf0COMPRESSED";
        let used = parser.feed(input, &mut events);
        assert_eq!(&input[used..], b"COMPRESSED");
        assert_eq!(
            events,
            vec![
                TelnetEvent::Data(b"hi".to_vec()),
                TelnetEvent::CompressionStarted
            ]
        );
    }

    #[test]
    fn экранирование_исходящих() {
        assert_eq!(escape_iac(b"a\xffb"), b"a\xff\xffb");
        assert_eq!(
            subnegotiation(opt::MSDP, b"\x01X\x02\xff"),
            b"\xff\xfaE\x01X\x02\xff\xff\xff\xf0"
        );
    }
}
