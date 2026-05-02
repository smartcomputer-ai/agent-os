use tokio::sync::mpsc;

use crate::chat::protocol::ChatEvent;
use crate::chat::tui::app_event::UiEvent;

#[derive(Clone, Debug)]
pub(crate) struct AppEventSender {
    tx: mpsc::UnboundedSender<UiEvent>,
}

impl AppEventSender {
    pub(crate) fn new(tx: mpsc::UnboundedSender<UiEvent>) -> Self {
        Self { tx }
    }

    pub(crate) fn send(&self, event: UiEvent) {
        let _ = self.tx.send(event);
    }

    pub(crate) fn chat(&self, event: ChatEvent) {
        self.send(UiEvent::Chat(event));
    }

    pub(crate) fn exit(&self) {
        self.send(UiEvent::ExitRequested);
    }
}
