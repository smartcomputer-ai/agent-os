pub(crate) mod app;
pub(crate) mod app_event;
pub(crate) mod app_event_sender;
pub(crate) mod bottom_pane;
pub(crate) mod cell;
pub(crate) mod frame;
pub(crate) mod terminal;
pub(crate) mod transcript;

pub(crate) use app::{ChatTuiShellOptions, run_shell};
