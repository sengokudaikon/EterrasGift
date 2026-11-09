use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;
use task_runtime::{Closed, Runtime};

const PATIENCE: Duration = Duration::from_secs(10);

/// Polls a future with no Tokio context on this thread, as `slint::spawn_local` does on the UI thread.
fn await_without_tokio<T>(future: impl Future<Output = T>) -> T {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park_timeout(PATIENCE),
        }
    }
}

#[test]
fn a_result_arrives_on_a_thread_without_tokio() {
    let runtime = Runtime::start().unwrap();
    let task = runtime.tasks().spawn_blocking(|| 42).unwrap();
    assert_eq!(await_without_tokio(task).unwrap(), 42);
    assert!(!runtime.shutdown());
}

#[test]
fn the_reactor_drives_timers_and_channels() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let task = tasks
        .spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            7
        })
        .unwrap();
    assert_eq!(await_without_tokio(task).unwrap(), 7);
    assert!(!runtime.shutdown());
}

#[test]
fn cancellation_does_not_misreport_started_work_as_undone() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let token = tasks.token();
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let (finished, result) = mpsc::channel();
    tasks
        .spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv_timeout(PATIENCE).unwrap();
            finished.send(token.is_cancelled()).unwrap();
        })
        .unwrap();
    started.recv_timeout(PATIENCE).unwrap();
    // The barrier holds until the started work ends, and that work sees the cancellation and runs to its end.
    let closing = thread::spawn(move || runtime.shutdown());
    let observer = tasks.token();
    await_without_tokio(observer.cancelled());
    assert!(!closing.is_finished());
    assert_eq!(tasks.spawn_blocking(|| ()).err(), Some(Closed));
    assert_eq!(tasks.spawn(async {}).err(), Some(Closed));
    release.send(()).unwrap();
    assert!(!closing.join().unwrap());
    assert!(result.recv_timeout(PATIENCE).unwrap());
}

#[test]
fn a_panic_is_contained_reaches_the_awaiter_and_is_recorded() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let blocking = tasks.spawn_blocking(|| -> () { panic!("fixture panic") }).unwrap();
    assert!(await_without_tokio(blocking).unwrap_err().is_panic());
    assert!(tasks.faulted());
    // The runtime carries on, for blocking work and for futures.
    assert_eq!(await_without_tokio(tasks.spawn_blocking(|| 1).unwrap()).unwrap(), 1);
    assert_eq!(await_without_tokio(tasks.spawn(async { 2 }).unwrap()).unwrap(), 2);
    assert!(runtime.shutdown());
}

#[test]
fn a_panic_in_a_future_is_recorded_even_when_nobody_awaits_it() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let (entered, started) = mpsc::channel();
    drop(
        tasks
            .spawn(async move {
                entered.send(()).unwrap();
                panic!("fixture panic");
            })
            .unwrap(),
    );
    started.recv_timeout(PATIENCE).unwrap();
    assert!(runtime.shutdown());
}

#[test]
fn a_panic_in_blocking_work_is_recorded_even_when_nobody_awaits_it() {
    let runtime = Runtime::start().unwrap();
    drop(runtime.tasks().spawn_blocking(|| panic!("fixture panic")).unwrap());
    assert!(runtime.shutdown());
}

#[test]
fn work_ended_by_the_close_is_not_a_fault() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let token = tasks.token();
    let task = tasks.spawn(async move { token.cancelled().await }).unwrap();
    assert!(!runtime.shutdown());
    assert!(task.is_finished());
}

#[test]
fn a_child_token_stops_only_its_own_work() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let (one, other) = (tasks.token(), tasks.token());
    one.cancel();
    assert!(!other.is_cancelled());
    let task = tasks.spawn(async move { one.cancelled().await }).unwrap();
    await_without_tokio(task).unwrap();
    assert!(tasks.spawn(async {}).is_ok());
    assert!(!runtime.shutdown());
    assert!(other.is_cancelled());
}

#[test]
fn a_scope_closes_on_its_own_and_the_rest_carries_on() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let scope = tasks.scope();
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let token = scope.token();
    let (seen, result) = mpsc::channel();
    scope
        .spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv_timeout(PATIENCE).unwrap();
            seen.send(token.is_cancelled()).unwrap();
        })
        .unwrap();
    started.recv_timeout(PATIENCE).unwrap();
    let outer = tasks.token();
    let closing = {
        let scope = scope.clone();
        let tasks = tasks.clone();
        thread::spawn(move || tasks.block_on(scope.close()))
    };
    await_without_tokio(scope.token().cancelled());
    assert!(!closing.is_finished(), "the close waits for the scope's started work");
    assert_eq!(scope.spawn_blocking(|| ()).err(), Some(Closed));
    assert!(!outer.is_cancelled());
    assert_eq!(await_without_tokio(tasks.spawn_blocking(|| 1).unwrap()).unwrap(), 1);
    release.send(()).unwrap();
    closing.join().unwrap();
    assert!(result.recv_timeout(PATIENCE).unwrap());
    tasks.block_on(scope.close());
    assert!(!runtime.shutdown());
}

#[test]
fn the_runtime_waits_for_a_scope_nobody_closed_and_hears_its_panics() {
    let runtime = Runtime::start().unwrap();
    let outer = runtime.tasks().scope();
    let scope = outer.scope();
    let (entered, started) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let token = scope.token();
    scope
        .spawn_blocking(move || {
            entered.send(()).unwrap();
            while !token.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            flag.store(true, Ordering::Release);
        })
        .unwrap();
    started.recv_timeout(PATIENCE).unwrap();
    let task = scope.spawn_blocking(|| -> () { panic!("fixture panic") }).unwrap();
    assert!(await_without_tokio(task).unwrap_err().is_panic());
    assert!(outer.faulted());
    assert!(runtime.shutdown());
    assert!(finished.load(Ordering::Acquire));
}

#[test]
fn dropping_the_runtime_is_the_same_barrier() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let (entered, started) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let token = tasks.token();
    tasks
        .spawn_blocking(move || {
            entered.send(()).unwrap();
            while !token.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            flag.store(true, Ordering::Release);
        })
        .unwrap();
    started.recv_timeout(PATIENCE).unwrap();
    drop(runtime);
    assert!(finished.load(Ordering::Acquire));
}

#[test]
fn work_admitted_as_a_scope_closes_is_waited_for_or_refused() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let late = Arc::new(AtomicBool::new(false));
    for _ in 0..20_000 {
        let scope = tasks.scope();
        let closed = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(2));
        let spawner = {
            let (scope, closed, late, start) = (scope.clone(), closed.clone(), late.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                // Work the close accepted runs before the close comes back, so it never sees the close done.
                let _ = scope.spawn_blocking(move || {
                    if closed.load(Ordering::SeqCst) {
                        late.store(true, Ordering::SeqCst);
                    }
                });
            })
        };
        start.wait();
        tasks.block_on(scope.close());
        closed.store(true, Ordering::SeqCst);
        spawner.join().unwrap();
        assert!(!late.load(Ordering::SeqCst), "work was admitted after its scope's close had come back");
    }
    assert!(!runtime.shutdown());
    assert!(!late.load(Ordering::SeqCst), "work was admitted after its scope's close had come back");
}
