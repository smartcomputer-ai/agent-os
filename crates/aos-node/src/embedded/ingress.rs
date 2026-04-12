use std::collections::VecDeque;
use std::sync::Mutex;

use crate::{InboxSeq, SubmissionEnvelope};

#[derive(Default)]
pub struct LocalIngressQueue {
    inner: Mutex<LocalIngressState>,
}

#[derive(Default)]
struct LocalIngressState {
    next_seq: u64,
    queued: VecDeque<(InboxSeq, SubmissionEnvelope)>,
}

impl LocalIngressQueue {
    pub fn enqueue(&self, submission: SubmissionEnvelope) -> InboxSeq {
        let mut inner = self
            .inner
            .lock()
            .expect("local ingress queue mutex poisoned");
        let seq = InboxSeq::from_u64(inner.next_seq);
        inner.next_seq = inner.next_seq.saturating_add(1);
        inner.queued.push_back((seq.clone(), submission));
        seq
    }

    pub fn drain_all(&self) -> Vec<(InboxSeq, SubmissionEnvelope)> {
        let mut inner = self
            .inner
            .lock()
            .expect("local ingress queue mutex poisoned");
        inner.queued.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        let inner = self
            .inner
            .lock()
            .expect("local ingress queue mutex poisoned");
        inner.queued.len()
    }
}
