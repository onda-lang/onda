//! Coalesce worker notifications into one host wake until the next poll.
use super::ControllerEvent;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SendError, Sender},
    Arc, Mutex,
};

type Callback = Arc<dyn Fn() + Send + Sync>;
#[derive(Default)]
struct Wake {
    pending: AtomicBool,
    callback: Mutex<Option<Callback>>,
}
#[derive(Clone)]
pub(super) struct EventSender {
    sender: Sender<ControllerEvent>,
    wake: Arc<Wake>,
}
impl EventSender {
    pub(super) fn channel() -> (Self, Receiver<ControllerEvent>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sender,
                wake: Arc::default(),
            },
            receiver,
        )
    }
    pub(super) fn send(&self, event: ControllerEvent) -> Result<(), SendError<ControllerEvent>> {
        self.sender.send(event)?;
        if !self.wake.pending.swap(true, Ordering::AcqRel) {
            self.notify();
        }
        Ok(())
    }
    pub(super) fn set_wakeup(&self, callback: impl Fn() + Send + Sync + 'static) {
        *self.wake.callback.lock().unwrap() = Some(Arc::new(callback));
        if self.wake.pending.load(Ordering::Acquire) {
            self.notify();
        }
    }
    pub(super) fn begin_poll(&self) {
        // Reset before draining: an event arriving after the drain must wake us.
        self.wake.pending.store(false, Ordering::Release);
    }
    fn notify(&self) {
        let callback = self.wake.callback.lock().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    fn event() -> ControllerEvent {
        ControllerEvent::TcpResponse {
            generation: 1,
            line: String::new(),
        }
    }
    #[test]
    fn coalesces_and_wakes_for_events_queued_before_registration_and_during_poll() {
        let (sender, receiver) = EventSender::channel();
        sender.send(event()).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        sender.set_wakeup(move || {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
        sender.send(event()).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        sender.begin_poll();
        while receiver.try_recv().is_ok() {}
        sender.send(event()).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
