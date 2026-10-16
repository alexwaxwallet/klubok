//! Сессия: прикладной слой.
//!
//! `Session` — единственный владелец состояния. Сеть, UI и таймеры присылают ей
//! сообщения `SessionInput`, она отвечает сообщениями `SessionOutput`.

pub mod net;
pub mod session;

pub use net::{replay, run_session};
pub use session::{Session, SessionConfig, SessionInput, SessionOutput};
