//! Сессия: прикладной слой.
//!
//! `Session` — единственный владелец состояния. Сеть, UI и таймеры присылают ей
//! сообщения `SessionInput`, она отвечает сообщениями `SessionOutput`.

pub mod login;
pub mod net;
pub mod profile;
pub mod session;

pub use klubok_automation::{Automation, Hotkey, ImportReport};
pub use login::Credentials;
pub use net::{replay, run_session};
pub use profile::{Profile, ProfileStore, decode_settings_file};
pub use session::{Session, SessionConfig, SessionInput, SessionOutput};
