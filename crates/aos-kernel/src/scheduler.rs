use std::collections::VecDeque;

use crate::event::KernelEvent;

#[derive(Default)]
pub struct Scheduler {
    queue: VecDeque<KernelEvent>,
}

impl Scheduler {
    pub fn push(&mut self, event: KernelEvent) {
        self.queue.push_back(event);
    }

    pub fn pop(&mut self) -> Option<KernelEvent> {
        self.queue.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
