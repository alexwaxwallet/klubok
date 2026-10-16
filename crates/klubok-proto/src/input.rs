//! Входной поток: сырые байты из сокета → telnet-события.
//!
//! Склеивает telnet-парсер и распаковку MCCP v2. Сжатие может начаться
//! посреди пакета (сразу после `IAC SB COMPRESS2 IAC SE`) и закончиться
//! посреди пакета (конец zlib-потока) — оба случая обрабатываются здесь.

use flate2::{Decompress, FlushDecompress, Status};

use crate::telnet::{TelnetEvent, TelnetParser};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("ошибка распаковки MCCP: {0}")]
    Decompress(#[from] flate2::DecompressError),
}

pub struct InputDecoder {
    telnet: TelnetParser,
    /// `Some`, пока идёт сжатый поток.
    inflater: Option<Decompress>,
}

impl Default for InputDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl InputDecoder {
    pub fn new() -> Self {
        InputDecoder {
            telnet: TelnetParser::new(),
            inflater: None,
        }
    }

    pub fn is_compressed(&self) -> bool {
        self.inflater.is_some()
    }

    /// Разбирает пакет из сокета.
    pub fn feed(&mut self, mut input: &[u8]) -> Result<Vec<TelnetEvent>, InputError> {
        let mut events = Vec::new();

        while !input.is_empty() {
            match &mut self.inflater {
                Some(inflater) => {
                    let (plain, used, ended) = inflate(inflater, input)?;
                    input = &input[used..];
                    if ended {
                        // Сервер закончил сжатие; остаток пакета — обычные байты.
                        self.inflater = None;
                    }
                    // Внутри сжатого потока MCCP заново не начинается, поэтому съедается всё.
                    self.telnet.feed(&plain, &mut events);
                    if !ended && used == 0 {
                        break;
                    }
                }
                None => {
                    let before = events.len();
                    let used = self.telnet.feed(input, &mut events);
                    input = &input[used..];
                    // Смотрим только на события этого вызова: старый CompressionStarted не считается.
                    if events.len() > before
                        && events.last() == Some(&TelnetEvent::CompressionStarted)
                    {
                        self.inflater = Some(Decompress::new(true));
                    }
                }
            }
        }

        Ok(events)
    }
}

/// Распаковывает сколько получится. Возвращает (распакованное, сколько входа съедено, закончился ли поток).
fn inflate(inflater: &mut Decompress, input: &[u8]) -> Result<(Vec<u8>, usize, bool), InputError> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 4 + 1024);
    let mut pos = 0;

    loop {
        if out.capacity() - out.len() < 1024 {
            out.reserve(16 * 1024);
        }
        let before_in = inflater.total_in();
        let status = inflater.decompress_vec(&input[pos..], &mut out, FlushDecompress::None)?;
        let used = (inflater.total_in() - before_in) as usize;
        pos += used;

        if status == Status::StreamEnd {
            return Ok((out, pos, true));
        }
        let output_full = out.len() == out.capacity();
        // Выходим, когда вход кончился (или zlib больше ничего не берёт), а выходной буфер не забит.
        if !output_full && (pos >= input.len() || used == 0) {
            return Ok((out, pos, false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut c = Compress::new(Compression::default(), true);
        let mut out = Vec::with_capacity(data.len() + 64);
        c.compress_vec(data, &mut out, FlushCompress::Finish)
            .unwrap();
        out
    }

    /// Собирает весь текст из событий.
    fn text(events: &[TelnetEvent]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in events {
            if let TelnetEvent::Data(d) = e {
                out.extend(d);
            }
        }
        out
    }

    fn stream() -> (Vec<u8>, Vec<u8>) {
        let inner = "Базарная площадь\r\n".repeat(200).into_bytes();
        let mut inner_with_ga = inner.clone();
        inner_with_ga.extend(b"> \xff\xf9");

        let mut wire = b"plain\xff\xfaV\xff\xf0".to_vec();
        wire.extend(zlib(&inner_with_ga));
        wire.extend(b"after");

        let mut expected = b"plain".to_vec();
        expected.extend(&inner);
        expected.extend(b"> after");
        (wire, expected)
    }

    #[test]
    fn весь_поток_одним_пакетом() {
        let (wire, expected) = stream();
        let mut d = InputDecoder::new();
        let events = d.feed(&wire).unwrap();
        assert_eq!(text(&events), expected);
        assert!(events.contains(&TelnetEvent::GoAhead));
        assert!(!d.is_compressed());
    }

    #[test]
    fn поток_любыми_кусками() {
        let (wire, expected) = stream();
        for chunk in [1, 2, 7, 64, 333] {
            let mut d = InputDecoder::new();
            let mut events = Vec::new();
            for piece in wire.chunks(chunk) {
                events.extend(d.feed(piece).unwrap());
            }
            assert_eq!(text(&events), expected, "размер куска {chunk}");
        }
    }
}
