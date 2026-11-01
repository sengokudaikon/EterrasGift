//! A single admission queue shared by Mods and game preparation. Enqueue on the UI thread; wait only on workers.
//! A Play requested after an edit cannot overtake that edit, including when requested from Home.

use super::library::ModLibrary;
use eg_core::operations::Cancellation;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use eg_i18n::phrases;

phrases! {
    Closing: "The mod library is closing."
    Faulted: "The mod library stopped after a fault."
    Stopped: "The mod library stopped."
}

type Job = Box<dyn FnOnce(&ModLibrary) + Send>;
type FaultHook = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;
struct Owner {
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    cancel: Cancellation,
    fault: Arc<AtomicBool>,
    on_fault: FaultHook,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.sender.get_mut().unwrap().take();
        if let Some(worker) = self.worker.get_mut().unwrap().take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub struct ModWorker(Arc<Owner>);
impl ModWorker {
    pub fn new(library: ModLibrary) -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let worker = thread::spawn(move || {
            while let Ok(job) = receiver.recv() {
                job(&library);
            }
        });
        Self(Arc::new(Owner {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
            cancel: Cancellation::default(),
            fault: Arc::new(AtomicBool::new(false)),
            on_fault: Arc::default(),
        }))
    }
    /// Called on the worker's thread when a task panics, whoever queued it (Mods, or a start's preparation from Home), so
    /// the app can end at once instead of looking for the fault.
    pub fn on_fault(&self, hook: impl Fn() + Send + 'static) {
        *self.0.on_fault.lock().unwrap() = Some(Box::new(hook));
    }
    pub fn cancellation(&self) -> Cancellation {
        self.0.cancel.clone()
    }
    pub fn faulted(&self) -> bool {
        self.0.fault.load(Ordering::Acquire)
    }
    pub fn enqueue<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<mpsc::Receiver<io::Result<T>>> {
        let (reply, receive) = mpsc::channel();
        let fault = self.0.fault.clone();
        let on_fault = self.0.on_fault.clone();
        let sender = self.0.sender.lock().unwrap();
        let sender = sender.as_ref().ok_or_else(|| io::Error::other(Closing.get()))?;
        sender
            .send(Box::new(move |library| {
                let outcome = if fault.load(Ordering::Acquire) {
                    Err(io::Error::other(Faulted.get()))
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(library))).unwrap_or_else(|_| {
                        fault.store(true, Ordering::Release);
                        if let Some(hook) = on_fault.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
                            hook();
                        }
                        Err(io::Error::other(Faulted.get()))
                    })
                };
                let _ = reply.send(outcome);
            }))
            .map_err(|_| io::Error::other(Stopped.get()))?;
        Ok(receive)
    }
    /// For the game host and fixtures. Never wait in an interactive UI callback.
    pub fn call<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        self.enqueue(task)?.recv().map_err(|_| io::Error::other(Stopped.get()))?
    }
    /// Cancels source builds, rejects new tasks, drains accepted mutations and joins the worker.
    pub fn close(&self) {
        self.0.cancel.cancel();
        self.0.sender.lock().unwrap().take();
        if let Some(worker) = self.0.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
