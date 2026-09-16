pub mod clipboard;
#[cfg(feature = "history")]
pub mod history;
pub mod local;

#[cfg(feature = "history")]
pub use history::HistoryRepo;