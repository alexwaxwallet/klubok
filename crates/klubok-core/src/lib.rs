//! Сессия: прикладной слой.
//!
//! `Session` — единственный владелец состояния. Сеть, UI и таймеры присылают ей
//! сообщения `SessionInput`, она отвечает сообщениями `SessionOutput`.

pub mod login;
pub mod mapper;
pub mod net;
pub mod profile;
pub mod session;

pub use klubok_automation::{Automation, Hotkey, ImportReport};
pub use login::Credentials;
pub use mapper::{MapEdit, MapView, Mapper};
pub use net::{replay, run_session};
pub use profile::{Profile, ProfileStore, decode_settings_file};
pub use session::{LogOp, Session, SessionConfig, SessionInput, SessionOutput};
