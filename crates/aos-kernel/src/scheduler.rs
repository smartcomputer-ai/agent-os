use std::collections::VecDeque;

use crate::event::ReducerEvent;

pub enum Task {
    Reducer(ReducerEvent),
    Plan(u64),
}

#[derive(Default)]
pub struct Scheduler {
    reducer_queue: VecDeque<ReducerEvent>,
    plan_queue: VecDeque<u64>,
    last_was_plan: bool,
    next_plan_id: u64,
}

impl Scheduler {
    pub fn push_reducer(&mut self, event: ReducerEvent) {
        self.reducer_queue.push_back(event);
    }

    pub fn push_plan(&mut self, instance_id: u64) {
        self.plan_queue.push_back(instance_id);
    }

    pub fn pop(&mut self) -> Option<Task> {
        match (self.reducer_queue.is_empty(), self.plan_queue.is_empty()) {
            (true, true) => None,
            (false, true) => self.reducer_queue.pop_front().map(Task::Reducer),
            (true, false) => self.plan_queue.pop_front().map(Task::Plan),
            (false, false) => {
                // Round-robin between plan and reducer to keep fairness.
                if self.last_was_plan {
                    self.last_was_plan = false;
                    self.reducer_queue.pop_front().map(Task::Reducer)
                } else {
                    self.last_was_plan = true;
                    self.plan_queue.pop_front().map(Task::Plan)
                }
            }
        }
    }

    pub fn alloc_plan_id(&mut self) -> u64 {
        let id = self.next_plan_id;
        self.next_plan_id += 1;
        id
    }

    pub fn is_empty(&self) -> bool {
        self.reducer_queue.is_empty() && self.plan_queue.is_empty()
    }

    pub fn clear(&mut self) {
        self.reducer_queue.clear();
        self.plan_queue.clear();
    }

    pub fn set_next_plan_id(&mut self, next: u64) {
        self.next_plan_id = next;
    }

    pub fn next_plan_id(&self) -> u64 {
        self.next_plan_id
    }
}
