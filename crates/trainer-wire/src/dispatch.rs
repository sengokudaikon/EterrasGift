//! One command at a time, matching the current helper's dispatch model.
//!
//! The eventual Unity adapter calls execute_one after the original ExecuteTasks. This queue does
//! not establish main-thread identity or grant access to Unity; it only owns scheduling state.
//! A timeout before execution cancels the command. Once execution starts, timeout means uncertainty,
//! and the slot stays busy until the operation finishes. It must never trigger an automatic retry.

use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<R> {
    Completed(R),
    /// Execution unwound before publishing a result. Effects may already have happened.
    Abandoned,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Cancellation<R> {
    NotStarted,
    /// The operation owns the slot and may already have changed game state.
    MayHaveRun,
    Finished(Outcome<R>),
}

enum State<C, R> {
    Idle,
    Pending(C),
    Running { interested: bool },
    Finished(Outcome<R>),
}

type Shared<C, R> = Arc<Mutex<State<C, R>>>;

pub struct Dispatcher<C, R> {
    state: Shared<C, R>,
}

impl<C, R> Clone for Dispatcher<C, R> {
    fn clone(&self) -> Self {
        Self { state: Arc::clone(&self.state) }
    }
}

impl<C, R> Default for Dispatcher<C, R> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
        }
    }
}

impl<C, R> Dispatcher<C, R> {
    /// A busy dispatcher returns the unsubmitted command. No game operation has started for it.
    pub fn submit(&self, command: C) -> Result<Pending<C, R>, C> {
        let mut state = self.state.lock().expect("dispatch state poisoned");
        if !matches!(*state, State::Idle) {
            return Err(command);
        }
        *state = State::Pending(command);
        Ok(Pending {
            state: Some(Arc::clone(&self.state)),
        })
    }

    /// Execute at most one command without holding the queue lock during user/game code.
    /// Returns false if there was no pending command. Recursive calls cannot claim a running slot.
    pub fn execute_one(&self, execute: impl FnOnce(C) -> R) -> bool {
        let command = {
            let mut state = self.state.lock().expect("dispatch state poisoned");
            if !matches!(*state, State::Pending(_)) {
                return false;
            }
            let State::Pending(command) = std::mem::replace(&mut *state, State::Running { interested: true }) else {
                unreachable!()
            };
            command
        };
        let completion = Completion {
            state: Arc::clone(&self.state),
            outcome: None,
        };
        let result = execute(command);
        completion.finish(result);
        true
    }
}

/// The sole receiver for one command. Dropping it cancels pending work, never in-flight work.
pub struct Pending<C, R> {
    state: Option<Shared<C, R>>,
}

impl<C, R> Pending<C, R> {
    pub fn try_result(&mut self) -> Option<Outcome<R>> {
        let shared = self.state.as_ref()?;
        let mut state = shared.lock().expect("dispatch state poisoned");
        if !matches!(*state, State::Finished(_)) {
            return None;
        }
        let State::Finished(result) = std::mem::replace(&mut *state, State::Idle) else {
            unreachable!()
        };
        drop(state);
        self.state = None;
        Some(result)
    }

    /// Consume the receiver at a timeout/disconnect boundary. None means its result was already read.
    pub fn cancel(mut self) -> Option<Cancellation<R>> {
        self.cancel_inner()
    }

    fn cancel_inner(&mut self) -> Option<Cancellation<R>> {
        let shared = self.state.take()?;
        let previous = {
            let mut state = shared.lock().expect("dispatch state poisoned");
            if matches!(*state, State::Running { .. }) {
                *state = State::Running { interested: false };
                return Some(Cancellation::MayHaveRun);
            }
            std::mem::replace(&mut *state, State::Idle)
        };
        // Commands/results are dropped outside the lock; their destructors can run arbitrary code.
        match previous {
            State::Pending(_) => Some(Cancellation::NotStarted),
            State::Finished(result) => Some(Cancellation::Finished(result)),
            _ => unreachable!("a live receiver owns a pending, running or completed slot"),
        }
    }
}

impl<C, R> Drop for Pending<C, R> {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

struct Completion<C, R> {
    state: Shared<C, R>,
    outcome: Option<Outcome<R>>,
}

impl<C, R> Completion<C, R> {
    fn finish(mut self, result: R) {
        self.outcome = Some(Outcome::Completed(result));
    }
}

impl<C, R> Drop for Completion<C, R> {
    fn drop(&mut self) {
        let result = self.outcome.take().unwrap_or(Outcome::Abandoned);
        let mut state = self.state.lock().expect("dispatch state poisoned");
        if matches!(*state, State::Running { interested: true }) {
            *state = State::Finished(result);
        } else {
            *state = State::Idle;
            drop(state);
            drop(result);
        }
    }
}
