use std::collections::VecDeque;

use crate::event::ReducerEvent;

pub enum Task {
    Reducer(ReducerEvent),
}

#[derive(Default)]
pub struct Scheduler {
    reducer_queue: VecDeque<ReducerEvent>,
    next_plan_id: u64,
}

impl Scheduler {
    pub fn push_reducer(&mut self, event: ReducerEvent) {
        self.reducer_queue.push_back(event);
    }

    /// Retained as a transitional no-op while plan runtime is being removed.
    pub fn push_plan(&mut self, _instance_id: u64) {}

    pub fn pop(&mut self) -> Option<Task> {
        self.reducer_queue.pop_front().map(Task::Reducer)
    }

    pub fn alloc_plan_id(&mut self) -> u64 {
        let id = self.next_plan_id;
        self.next_plan_id += 1;
        id
    }

    pub fn is_empty(&self) -> bool {
        self.reducer_queue.is_empty()
    }

    pub fn clear(&mut self) {
        self.reducer_queue.clear();
    }

    pub fn set_next_plan_id(&mut self, next: u64) {
        self.next_plan_id = next;
    }

    pub fn next_plan_id(&self) -> u64 {
        self.next_plan_id
    }
}
