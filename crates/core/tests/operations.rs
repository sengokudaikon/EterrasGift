use eg_core::operations::{OperationError, Operations};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn cancellation_does_not_misreport_a_started_operation_as_undone() {
    let operations = Arc::new(Operations::default());
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (complete, result) = mpsc::channel();
    operations
        .spawn(
            move |cancel| {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(cancel.is_cancelled());
                42
            },
            move |outcome| complete.send(outcome).unwrap(),
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    operations.cancel();
    assert_eq!(operations.spawn(|_| (), |_| ()), Err(OperationError::Closing));
    release.send(()).unwrap();
    operations.close();
    assert_eq!(result.recv().unwrap(), Ok(42));
    assert!(!operations.faulted());
}

#[test]
fn panics_are_reported_and_shutdown_joins_the_worker() {
    let operations = Operations::default();
    let (send, receive) = mpsc::channel();
    operations
        .spawn(|_| panic!("fixture panic"), move |result: Result<(), _>| send.send(result).unwrap())
        .unwrap();
    assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), Err(OperationError::Panicked));
    operations.close();
    assert!(operations.faulted());
}

#[test]
fn simultaneous_closes_are_both_barriers() {
    let operations = Arc::new(Operations::default());
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    operations
        .spawn(
            move |_| {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            },
            |_| (),
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done, receive) = mpsc::channel();
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let operations = operations.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                operations.close();
                done.send(()).unwrap();
            })
        })
        .collect();
    // Both callers must still be waiting while the accepted worker is blocked.
    assert!(receive.recv_timeout(Duration::from_millis(100)).is_err());
    release.send(()).unwrap();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(receive.try_iter().count(), 2);
}
