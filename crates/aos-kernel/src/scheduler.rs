use std::collections::VecDeque;

use crate::event::ReducerEvent;

pub enum Task {
    Reducer(ReducerEvent),
    Plan(u64),
}

#[derive(Default)]
pub struct Scheduler {
    queue: VecDeque<Task>,
    next_plan_id: u64,
}

impl Scheduler {
    pub fn push_reducer(&mut self, event: ReducerEvent) {
        self.queue.push_back(Task::Reducer(event));
    }

    pub fn push_plan(&mut self, instance_id: u64) {
        self.queue.push_back(Task::Plan(instance_id));
    }

    pub fn pop(&mut self) -> Option<Task> {
        self.queue.pop_front()
    }

    pub fn alloc_plan_id(&mut self) -> u64 {
        let id = self.next_plan_id;
        self.next_plan_id += 1;
        id
    }
}
