//! Only one Sentinel at a time per signed-in user.
//!
//! With a background mode, Sentinel can already be running (hidden in the notification area)
//! when it is started again, from a shortcut or at sign-in. A second copy would fight the first
//! over the database and folder watchers, so the second one asks the first to show its window and
//! exits. Built on a named mutex (who is first) and a named event (the request to show), both
//! in the per-session `Local\` namespace, so it neither crosses users nor needs any permission.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows::core::PCWSTR;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The first running copy. Dropping it (at exit) releases the claim.
pub(crate) struct Guard {
    mutex: isize,
    event: isize,
}

pub(crate) enum Instance {
    First(Guard),
    /// Another copy is running; it has been asked to show its window.
    AlreadyRunning,
}

/// Claim `name`, or ask the copy that holds it to show itself. If Windows will not let the claim
/// be made at all, Sentinel runs unprotected rather than not at all.
pub(crate) fn acquire(name: &str) -> Instance {
    let mutex_name = wide(&format!(r"Local\{name}"));
    let event_name = wide(&format!(r"Local\{name}.show"));
    // SAFETY: the name is a NUL-terminated UTF-16 string that outlives the call.
    let mutex = match unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) } {
        Ok(h) => h,
        Err(err) => {
            tracing::warn!(error = %err, "single-instance claim failed; continuing without it");
            return Instance::First(Guard { mutex: 0, event: 0 });
        }
    };
    // SAFETY: reads the calling thread's last error immediately after the call above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: valid NUL-terminated name; the handles are closed below.
        unsafe {
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr())) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
            let _ = CloseHandle(mutex);
        }
        return Instance::AlreadyRunning;
    }
    // SAFETY: valid NUL-terminated name. Auto-reset, so each request wakes the waiter once.
    let event = unsafe { CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())) };
    let event = event.map_or_else(
        |err| {
            tracing::warn!(error = %err, "could not create the show request; a second start cannot raise this window");
            0
        },
        |h| h.0 as isize,
    );
    Instance::First(Guard {
        mutex: mutex.0 as isize,
        event,
    })
}

impl Guard {
    #[cfg(test)]
    /// Block until another copy asks this one to show itself, or `timeout_ms` passes (use
    /// `None` to wait for ever). Returns whether a request arrived.
    pub(crate) fn wait_for_request(&self, timeout_ms: Option<u32>) -> bool {
        if self.event == 0 {
            std::thread::sleep(std::time::Duration::from_millis(u64::from(
                timeout_ms.unwrap_or(1000),
            )));
            return false;
        }
        // SAFETY: the event handle stays open for as long as this guard exists.
        let r = unsafe {
            WaitForSingleObject(
                HANDLE(self.event as *mut c_void),
                timeout_ms.unwrap_or(INFINITE),
            )
        };
        r.0 == 0 // WAIT_OBJECT_0
    }

    /// A copy of the event, for a thread that outlives a borrow of the guard.
    pub(crate) fn request_waiter(&self) -> RequestWaiter {
        RequestWaiter(self.event)
    }
}

/// Waits for show requests from another thread. The event stays open until the process exits.
pub(crate) struct RequestWaiter(isize);

impl RequestWaiter {
    pub(crate) fn wait(&self) -> bool {
        if self.0 == 0 {
            return false;
        }
        // SAFETY: the event is owned by the guard in `run`, which lives until the process exits.
        unsafe { WaitForSingleObject(HANDLE(self.0 as *mut c_void), INFINITE).0 == 0 }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: closes handles this guard created and nothing else uses after it is dropped.
        unsafe {
            if self.event != 0 {
                let _ = CloseHandle(HANDLE(self.event as *mut c_void));
            }
            if self.mutex != 0 {
                let _ = CloseHandle(HANDLE(self.mutex as *mut c_void));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn unique() -> String {
        format!("sentinel-test-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn the_first_copy_wins_and_a_second_asks_it_to_show_itself() {
        let name = unique();
        let Instance::First(first) = acquire(&name) else {
            panic!("the first claim must succeed");
        };
        assert!(!first.wait_for_request(Some(50)), "no request yet");
        assert!(matches!(acquire(&name), Instance::AlreadyRunning));
        assert!(
            first.wait_for_request(Some(2000)),
            "the second start was relayed"
        );
        assert!(
            !first.wait_for_request(Some(50)),
            "one request wakes the waiter once"
        );
        // Several second starts are each relayed.
        assert!(matches!(acquire(&name), Instance::AlreadyRunning));
        assert!(first.wait_for_request(Some(2000)));
    }

    #[test]
    fn different_names_do_not_interfere_and_the_claim_ends_with_the_first_copy() {
        let (a, b) = (unique(), unique());
        let first_a = acquire(&a);
        assert!(matches!(first_a, Instance::First(_)));
        assert!(matches!(acquire(&b), Instance::First(_)));
        drop(first_a);
        assert!(
            matches!(acquire(&a), Instance::First(_)),
            "a released claim can be taken again"
        );
    }

    #[test]
    fn a_waiter_on_another_thread_is_woken() {
        let name = unique();
        let Instance::First(first) = acquire(&name) else {
            panic!("first");
        };
        let waiter = first.request_waiter();
        let handle = std::thread::spawn(move || waiter.wait());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(matches!(acquire(&name), Instance::AlreadyRunning));
        assert!(handle.join().unwrap());
    }
}
