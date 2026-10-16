//! Кодировки, которые понимает сервер Былин.

use encoding_rs::{Encoding, KOI8_R, UTF_8, WINDOWS_1251};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Charset {
    #[default]
    Utf8,
    Koi8R,
    Cp1251,
}

impl Charset {
    fn encoding(self) -> &'static Encoding {
        match self {
            Charset::Utf8 => UTF_8,
            Charset::Koi8R => KOI8_R,
            Charset::Cp1251 => WINDOWS_1251,
        }
    }

    /// Какую цифру выбрать в меню «Using keytable» при подключении.
    pub fn server_menu_choice(self) -> &'static str {
        match self {
            Charset::Koi8R => "0",
            Charset::Cp1251 => "2",
            Charset::Utf8 => "5",
        }
    }

    /// Байты от сервера → строка. Битые байты заменяются на «�», а не роняют клиент.
    pub fn decode(self, bytes: &[u8]) -> String {
        let (text, _, _) = self.encoding().decode(bytes);
        text.into_owned()
    }

    /// Строка → байты для сервера.
    pub fn encode(self, text: &str) -> Vec<u8> {
        let (bytes, _, _) = self.encoding().encode(text);
        bytes.into_owned()
    }
}

impl std::str::FromStr for Charset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace(['-', '_'], "").as_str() {
            "utf8" => Ok(Charset::Utf8),
            "koi8r" | "koi8" => Ok(Charset::Koi8R),
            "cp1251" | "windows1251" | "win1251" => Ok(Charset::Cp1251),
            _ => Err(format!(
                "неизвестная кодировка «{s}» (есть: utf8, koi8r, cp1251)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn туда_и_обратно() {
        for cs in [Charset::Utf8, Charset::Koi8R, Charset::Cp1251] {
            let text = "Вы стоите посреди лужи. Я!";
            assert_eq!(cs.decode(&cs.encode(text)), text, "{cs:?}");
        }
    }

    #[test]
    fn я_в_cp1251_это_байт_255() {
        assert_eq!(Charset::Cp1251.encode("я"), vec![0xff]);
    }

    #[test]
    fn разбор_имени() {
        assert_eq!("UTF-8".parse(), Ok(Charset::Utf8));
        assert_eq!("koi8-r".parse(), Ok(Charset::Koi8R));
        assert_eq!("windows-1251".parse(), Ok(Charset::Cp1251));
        assert!("latin1".parse::<Charset>().is_err());
    }
}
