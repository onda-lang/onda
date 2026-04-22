use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
static RUN_TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn run_termination_signal_handler(_sig: libc::c_int) {
    RUN_TERMINATION_REQUESTED.store(true, Ordering::Release);
}

#[cfg(unix)]
pub(crate) struct RunSignalGuard {
    previous_sigint: libc::sighandler_t,
    previous_sigterm: libc::sighandler_t,
}

#[cfg(unix)]
impl Drop for RunSignalGuard {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.previous_sigint);
            libc::signal(libc::SIGTERM, self.previous_sigterm);
        }
        RUN_TERMINATION_REQUESTED.store(false, Ordering::Release);
    }
}

#[cfg(unix)]
pub(crate) fn install_run_signal_handlers() -> RunSignalGuard {
    RUN_TERMINATION_REQUESTED.store(false, Ordering::Release);
    let handler = run_termination_signal_handler as *const () as libc::sighandler_t;
    let previous_sigint = unsafe { libc::signal(libc::SIGINT, handler) };
    let previous_sigterm = unsafe { libc::signal(libc::SIGTERM, handler) };
    RunSignalGuard {
        previous_sigint,
        previous_sigterm,
    }
}

#[cfg(not(unix))]
pub(crate) struct RunSignalGuard;

#[cfg(not(unix))]
pub(crate) fn install_run_signal_handlers() -> RunSignalGuard {
    RunSignalGuard
}

#[cfg(unix)]
pub(crate) fn run_termination_requested() -> bool {
    RUN_TERMINATION_REQUESTED.load(Ordering::Acquire)
}

#[cfg(not(unix))]
pub(crate) fn run_termination_requested() -> bool {
    false
}
