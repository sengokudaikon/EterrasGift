use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;
use task_runtime::{Refusal, Runtime, Serial};

const PATIENCE: Duration = Duration::from_secs(10);

#[test]
fn jobs_run_one_at_a_time_in_the_order_accepted() {
    let runtime = Runtime::start().unwrap();
    let serial = Serial::start(Vec::new(), &runtime.tasks());
    let clone = serial.clone();
    for n in 0..50 {
        let queue = if n % 2 == 0 { &serial } else { &clone };
        queue.enqueue(move |seen: &mut Vec<u32>| seen.push(n)).unwrap();
    }
    let seen = serial.enqueue(|seen| seen.clone()).unwrap().blocking_recv().unwrap();
    assert_eq!(seen, (0..50).collect::<Vec<_>>());
    assert!(!runtime.shutdown());
}

#[test]
fn a_close_runs_what_it_accepted_and_refuses_the_rest() {
    let runtime = Runtime::start().unwrap();
    let serial = Serial::start(0u32, &runtime.tasks());
    let (open, gate) = mpsc::channel::<()>();
    let first = serial
        .enqueue(move |count| {
            gate.recv_timeout(PATIENCE).unwrap();
            *count += 1;
        })
        .unwrap();
    let second = serial.enqueue(|count| *count + 1).unwrap();
    let closing = serial.clone();
    let close = thread::spawn(move || closing.close());
    open.send(()).unwrap();
    close.join().unwrap();
    first.blocking_recv().unwrap();
    assert_eq!(second.blocking_recv().unwrap(), 2);
    assert_eq!(serial.enqueue(|count| *count).unwrap_err(), Refusal::Closed);
    // Again, and from another clone: it waits the same way, which by now is not at all.
    serial.clone().close();
    assert!(!runtime.shutdown());
}

#[test]
fn the_runtime_closing_around_it_still_runs_what_was_queued() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let serial = Serial::start(Vec::new(), &tasks);
    let closing = tasks.token();
    let first = serial
        .enqueue(move |seen: &mut Vec<&str>| {
            // Holds the worker until the runtime has begun to close, so the next job is queued across it.
            while !closing.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            seen.push("first");
        })
        .unwrap();
    let second = serial
        .enqueue(|seen| {
            seen.push("second");
            seen.clone()
        })
        .unwrap();
    assert!(!runtime.shutdown());
    first.blocking_recv().unwrap();
    assert_eq!(second.blocking_recv().unwrap(), ["first", "second"]);
    assert_eq!(serial.enqueue(|_| ()).unwrap_err(), Refusal::Closed);
}

#[test]
fn a_panic_stops_it_and_drops_the_queued_jobs_and_only_it_says_so() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let serial = Serial::start(0u32, &tasks);
    let (open, gate) = mpsc::channel::<()>();
    let panics = serial
        .enqueue(move |_| -> u32 {
            gate.recv_timeout(PATIENCE).unwrap();
            panic!("a job that panics");
        })
        .unwrap();
    let queued = serial.enqueue(|count| *count).unwrap();
    open.send(()).unwrap();
    assert!(panics.blocking_recv().is_err());
    assert!(serial.stopped(), "by the time a reply ends, it says why");
    assert!(queued.blocking_recv().is_err());
    assert_eq!(serial.enqueue(|count| *count).unwrap_err(), Refusal::Stopped);
    assert!(!tasks.faulted(), "the panic is the owner's to report");
    serial.close();
    assert!(!runtime.shutdown());
}

#[test]
fn on_a_runtime_already_closing_nothing_is_accepted() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    assert!(!runtime.shutdown());
    let serial = Serial::start((), &tasks);
    assert_eq!(serial.enqueue(|_| ()).unwrap_err(), Refusal::Closed);
    serial.close();
}

#[test]
fn closers_at_once_each_wait_for_what_was_accepted() {
    let runtime = Runtime::start().unwrap();
    let serial = Serial::start((), &runtime.tasks());
    let (open, gate) = mpsc::channel::<()>();
    let ran = Arc::new(AtomicBool::new(false));
    let done = ran.clone();
    let job = serial
        .enqueue(move |_| {
            gate.recv_timeout(PATIENCE).unwrap();
            done.store(true, Ordering::SeqCst);
        })
        .unwrap();
    let closers: Vec<_> = (0..2)
        .map(|_| {
            let (serial, ran) = (serial.clone(), ran.clone());
            thread::spawn(move || {
                serial.close();
                ran.load(Ordering::SeqCst)
            })
        })
        .collect();
    // Both closers are inside close() by now: neither may come back before the job it accepted has run.
    thread::sleep(Duration::from_millis(100));
    open.send(()).unwrap();
    for closer in closers {
        assert!(closer.join().unwrap(), "a close came back while a job it accepted was still to run");
    }
    job.blocking_recv().unwrap();
    assert!(!runtime.shutdown());
}
