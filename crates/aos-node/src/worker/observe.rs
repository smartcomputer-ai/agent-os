use aos_node::WorldJournalAdvance;
use tokio::sync::broadcast;

#[derive(Clone)]
pub(super) struct WorldObserverHub {
    tx: broadcast::Sender<WorldJournalAdvance>,
}

impl Default for WorldObserverHub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }
}

impl WorldObserverHub {
    pub(super) fn subscribe(&self) -> broadcast::Receiver<WorldJournalAdvance> {
        self.tx.subscribe()
    }

    pub(super) fn publish(&self, advance: WorldJournalAdvance) {
        let _ = self.tx.send(advance);
    }
}
