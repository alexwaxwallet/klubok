//! Протокол: превращает сырые байты от сервера в строки с цветами и обратно.
//!
//! Здесь только чистые функции без сети и async: на вход байты, на выход результат.
//! Порядок обработки входящих данных:
//!
//! ```text
//! сокет → input (MCCP + telnet) → lines (переводы строк) → charset → ansi → StyledLine
//! ```

pub mod ansi;
pub mod charset;
pub mod input;
pub mod lines;
pub mod msdp;
pub mod telnet;

pub use ansi::{AnsiParser, Color, Span, Style, StyledLine};
pub use charset::Charset;
pub use input::{InputDecoder, InputError};
pub use lines::{LineAssembler, RawLine};
pub use msdp::MsdpValue;
pub use telnet::{TelnetEvent, Verb};
