use crate::chat::protocol::ChatEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiEvent {
    ComposerChanged,
    Chat(ChatEvent),
    Resize { cols: u16, rows: u16 },
    ExitRequested,
}
