use eg_trainer_wire::dispatch::{Cancellation, Dispatcher, Outcome};
use std::sync::{Arc, Barrier};

#[test]
fn pending_timeout_proves_no_execution_and_frees_the_slot() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let request = dispatcher.submit(7).unwrap();
    assert_eq!(request.cancel(), Some(Cancellation::NotStarted));
    assert!(!dispatcher.execute_one(|_| panic!("canceled command executed")));
    assert!(dispatcher.submit(8).is_ok());
}

#[test]
fn busy_rejection_returns_the_unsent_command_and_completion_is_read_once() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let mut request = dispatcher.submit(7).unwrap();
    assert_eq!(dispatcher.submit(8).err(), Some(8));
    assert!(dispatcher.execute_one(|value| value * 2));
    assert_eq!(dispatcher.submit(9).err(), Some(9));
    assert_eq!(request.try_result(), Some(Outcome::Completed(14)));
    assert_eq!(request.try_result(), None);
    assert!(dispatcher.submit(10).is_ok());
}

#[test]
fn recursive_dispatch_is_busy_without_deadlocking() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let mut request = dispatcher.submit(7).unwrap();
    dispatcher.execute_one(|value| {
        assert_eq!(dispatcher.submit(8).err(), Some(8));
        assert!(!dispatcher.execute_one(|_| panic!("recursive execution")));
        value
    });
    assert_eq!(request.try_result(), Some(Outcome::Completed(7)));
}

#[test]
fn in_flight_timeout_is_uncertain_and_cannot_allow_a_second_operation() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let request = dispatcher.submit(7).unwrap();
    let started = Arc::new(Barrier::new(2));
    let finish = Arc::new(Barrier::new(2));
    let worker = {
        let (dispatcher, started, finish) = (dispatcher.clone(), started.clone(), finish.clone());
        std::thread::spawn(move || {
            dispatcher.execute_one(|value| {
                started.wait();
                finish.wait();
                value * 2
            })
        })
    };
    started.wait();
    assert_eq!(request.cancel(), Some(Cancellation::MayHaveRun));
    assert_eq!(dispatcher.submit(8).err(), Some(8));
    finish.wait();
    assert!(worker.join().unwrap());
    assert!(dispatcher.submit(9).is_ok());
}

#[test]
fn dropping_a_receiver_cancels_pending_work() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    drop(dispatcher.submit(7).unwrap());
    assert!(!dispatcher.execute_one(|_| panic!("orphaned command executed")));
}

#[test]
fn timeout_after_completion_preserves_the_known_result() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let request = dispatcher.submit(7).unwrap();
    dispatcher.execute_one(|value| value * 2);
    assert_eq!(request.cancel(), Some(Cancellation::Finished(Outcome::Completed(14))));
}

#[test]
fn rust_panic_reports_abandoned_instead_of_replaying_the_command() {
    let dispatcher = Dispatcher::<i32, i32>::default();
    let mut request = dispatcher.submit(7).unwrap();
    let result = std::panic::catch_unwind(|| dispatcher.execute_one(|_| panic!("operation failed after possible effects")));
    assert!(result.is_err());
    assert_eq!(request.try_result(), Some(Outcome::Abandoned));
    assert!(!dispatcher.execute_one(|_| panic!("command replayed")));
    assert!(dispatcher.submit(8).is_ok());
}
