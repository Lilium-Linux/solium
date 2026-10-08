#![expect(
    unsafe_code,
    reason = "a hook armed with lua_sethook from a signal handler, and the signal sent to a thread"
)]

//! The clock of a call into an effect's Lua: the call runs with no hook, and
//! a hook is armed only once it has run past its budget (Ruling 4, mechanism
//! (b)).
//!
//! FX-S5 measured a count hook doubling the genie's `mesh` call (92 µs
//! against 45 µs at p99), whether it stands or is set around each call: Lua
//! 5.4 takes its slow path on every instruction while any count hook is set,
//! so the cost is the hook being there, not its firing. So a call runs bare,
//! and one watchdog thread waits for the deadline of every call in flight. A
//! call still running at its deadline is sent [`Clock`]'s signal, and the
//! handler, on the call's own thread, arms a hook that stops it at its next
//! instruction: `lua_sethook` is the one call Lua 5.4 lets a signal handler
//! make (`ldebug.c`), and how `lua.c` stops a script on Ctrl-C. The
//! watchdog never calls it itself: from another thread it would walk the
//! running call's frames while an error unwinding or a GC step frees them.
//! `tests::a_call_past_its_budget_is_stopped_and_leaves_no_hook_behind`.

use std::ffi::{CStr, c_int};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use mlua::{Lua, ffi};

/// What a stopped call's error says, after where it was stopped.
const STOPPED: &CStr = c"the effect ran past its budget and was stopped";

/// Which real-time signal the watchdog sends, above `SIGRTMIN`. When anything
/// else in the process handles it there is no watchdog, and every call is
/// hooked instead (`sandbox::tests::the_hook_a_call_falls_back_to_stops_it_and_goes_with_it`).
const SIGNAL_ABOVE_RTMIN: c_int = 5;

thread_local! {
    /// The state this thread's watched call runs on, null between calls:
    /// what the signal handler arms.
    static ARMED: AtomicPtr<ffi::lua_State> = const { AtomicPtr::new(ptr::null_mut()) };
    /// When this thread's watched call is due, in nanoseconds after [`base`].
    static DUE: AtomicU64 = const { AtomicU64::new(u64::MAX) };
    /// Whether the hook stopped this thread's call.
    static STOPPED_HERE: AtomicBool = const { AtomicBool::new(false) };
}

/// The instant every [`DUE`] counts from.
fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn nanos_after_base(at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(base()).as_nanos()).unwrap_or(u64::MAX)
}

/// The state a call from Rust runs `lua`'s Lua on: what the hook is armed on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct State(*mut ffi::lua_State);

/// `lua`'s state, as a call from Rust runs on it.
/// `tests::a_call_past_its_budget_is_stopped_and_leaves_no_hook_behind`.
pub(crate) fn state_of(lua: &Lua) -> Option<State> {
    let mut raw = ptr::null_mut();
    // SAFETY: the closure copies the pointer out and touches no stack.
    let read = unsafe { lua.exec_raw::<()>((), |state| raw = state) };
    (read.is_ok() && !raw.is_null()).then_some(State(raw))
}

/// A watched call: whose thread, and when it is due.
#[derive(Debug)]
struct Flight {
    id: u64,
    thread: libc::pthread_t,
    due: Instant,
    sent: bool,
}

#[derive(Debug, Default)]
struct Flights {
    next: u64,
    calls: Vec<Flight>,
    /// When the watchdog wakes next; `None` while it waits for a call.
    wakes: Option<Instant>,
}

/// The watchdog: every call in flight, and the signal it sends one that is
/// past its due.
#[derive(Debug)]
pub(crate) struct Clock {
    flights: Mutex<Flights>,
    wake: Condvar,
    signal: c_int,
}

/// The watchdog, started on first use; `None` where it cannot run (its
/// signal is handled by something else, or its thread does not start), and
/// a call is hooked instead
/// (`sandbox::tests::the_hook_a_call_falls_back_to_stops_it_and_goes_with_it`).
pub(crate) fn clock() -> Option<&'static Clock> {
    static CLOCK: OnceLock<Option<&'static Clock>> = OnceLock::new();
    *CLOCK.get_or_init(start)
}

fn start() -> Option<&'static Clock> {
    let signal = libc::SIGRTMIN() + SIGNAL_ABOVE_RTMIN;
    if signal > libc::SIGRTMAX() {
        return None;
    }
    // SAFETY: a zeroed `sigaction` is one to be written into, and a null new
    // action only reads the one installed.
    let installed = unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        (libc::sigaction(signal, ptr::null(), &raw mut old) == 0).then_some(old.sa_sigaction)
    };
    if installed != Some(libc::SIG_DFL) {
        tracing::warn!(
            signal,
            "effects: the watchdog's signal is taken; calls into effects are hooked instead"
        );
        return None;
    }
    // SAFETY: the action loads atomics and calls `lua_sethook`, which Lua
    // lets a signal handler call (`on_signal`).
    if let Err(err) = unsafe { signal_hook_registry::register(signal, on_signal) } {
        tracing::warn!(%err, "effects: no watchdog; calls into effects are hooked instead");
        return None;
    }
    let clock: &'static Clock = Box::leak(Box::new(Clock {
        flights: Mutex::new(Flights::default()),
        wake: Condvar::new(),
        signal,
    }));
    match std::thread::Builder::new()
        .name("effect-clock".to_owned())
        .spawn(move || clock.watch())
    {
        Ok(_) => Some(clock),
        Err(err) => {
            tracing::warn!(%err, "effects: no watchdog; calls into effects are hooked instead");
            None
        }
    }
}

impl Clock {
    fn lock(&self) -> MutexGuard<'_, Flights> {
        self.flights.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The watchdog's loop: send each call past its due the signal, once,
    /// and sleep until the next due or the next call.
    fn watch(&self) {
        let mut flights = self.lock();
        loop {
            let now = Instant::now();
            for flight in flights
                .calls
                .iter_mut()
                .filter(|flight| !flight.sent && flight.due <= now)
            {
                // SAFETY: a flight's thread is inside its call, and leaves
                // this list under this lock before the call returns, so it
                // is alive.
                let _ = unsafe { libc::pthread_kill(flight.thread, self.signal) };
                flight.sent = true;
            }
            flights.wakes = flights
                .calls
                .iter()
                .filter(|flight| !flight.sent)
                .map(|flight| flight.due)
                .min();
            flights = match flights.wakes {
                None => self
                    .wake
                    .wait(flights)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    self.wake
                        .wait_timeout(flights, due.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    /// Put a call due at `due`, on this thread, in the watchdog's list, and
    /// wake the watchdog only if it would sleep past it
    /// (`tests::a_call_is_stopped_at_its_own_deadline_whatever_else_is_in_flight`).
    fn add(&self, due: Instant) -> u64 {
        let mut flights = self.lock();
        flights.next += 1;
        let id = flights.next;
        // SAFETY: `pthread_self` has no preconditions.
        let thread = unsafe { libc::pthread_self() };
        flights.calls.push(Flight {
            id,
            thread,
            due,
            sent: false,
        });
        let sooner = flights.wakes.is_none_or(|wakes| due < wakes);
        if sooner {
            flights.wakes = Some(due);
        }
        drop(flights);
        if sooner {
            self.wake.notify_one();
        }
        id
    }

    fn forget(&self, id: u64) {
        self.lock().calls.retain(|flight| flight.id != id);
    }

    /// Run `call`, whose Lua runs on `state`, stopped at its next
    /// instruction once it has run `budget`: what it returned, and whether
    /// it was stopped.
    /// `tests::a_call_past_its_budget_is_stopped_and_leaves_no_hook_behind`,
    /// `tests::a_call_is_stopped_at_its_own_deadline_whatever_else_is_in_flight`.
    pub(crate) fn run<R>(
        &self,
        state: State,
        budget: Duration,
        call: impl FnOnce() -> R,
    ) -> (R, bool) {
        let due = Instant::now() + budget;
        let _ = STOPPED_HERE.try_with(|stopped| stopped.store(false, Ordering::SeqCst));
        let _ = DUE.try_with(|at| at.store(nanos_after_base(due), Ordering::SeqCst));
        let _ = ARMED.try_with(|armed| armed.store(state.0, Ordering::SeqCst));
        let watching = Watching {
            clock: self,
            id: self.add(due),
            state: state.0,
        };
        let result = call();
        drop(watching);
        let stopped = STOPPED_HERE
            .try_with(|stopped| stopped.swap(false, Ordering::SeqCst))
            .unwrap_or(false);
        (result, stopped)
    }
}

/// A call in flight; dropping it ends the watch.
#[derive(Debug)]
struct Watching<'a> {
    clock: &'a Clock,
    id: u64,
    state: *mut ffi::lua_State,
}

impl Drop for Watching<'_> {
    fn drop(&mut self) {
        // Out of the watchdog's list first, so no signal is sent for this
        // call from here on; then nothing for a late one to arm; then no
        // hook, which the stop leaves armed
        // (`tests::a_call_past_its_budget_is_stopped_and_leaves_no_hook_behind`).
        self.clock.forget(self.id);
        let _ = ARMED.try_with(|armed| armed.store(ptr::null_mut(), Ordering::SeqCst));
        let _ = DUE.try_with(|due| due.store(u64::MAX, Ordering::SeqCst));
        // SAFETY: this thread's own state, its call over.
        unsafe { ffi::lua_sethook(self.state, None, 0, 0) };
    }
}

/// The signal's handler, on the thread it was sent to: arm the stop on the
/// call that thread runs, if it still runs one. Atomics and `lua_sethook`
/// only, which is what a signal handler may do here.
/// `tests::a_signal_for_a_call_already_returned_stops_nothing`.
fn on_signal() {
    let _ = ARMED.try_with(|armed| {
        let state = armed.load(Ordering::SeqCst);
        if !state.is_null() {
            // SAFETY: `state` is the one this thread runs a call on (cleared
            // before the call's watch takes the hook off), and `lua_sethook`
            // may be called from a handler that interrupts it (`ldebug.c`).
            unsafe { ffi::lua_sethook(state, Some(stop), ffi::LUA_MASKCOUNT, 1) };
        }
    });
}

/// The armed hook, at the call's next instruction: stop the call if it is
/// due, and otherwise take itself off, since the signal was for a call that
/// has returned. `tests::a_signal_for_a_call_already_returned_stops_nothing`.
unsafe extern "C-unwind" fn stop(state: *mut ffi::lua_State, _debug: *mut ffi::lua_Debug) {
    let due = DUE
        .try_with(|due| due.load(Ordering::SeqCst))
        .unwrap_or(u64::MAX);
    if nanos_after_base(Instant::now()) < due {
        // SAFETY: inside `state`'s hook, on its own thread.
        unsafe { ffi::lua_sethook(state, None, 0, 0) };
        return;
    }
    let _ = STOPPED_HERE.try_with(|stopped| stopped.store(true, Ordering::SeqCst));
    // SAFETY: a hook may raise an error, as `lua.c`'s `lstop` does; it
    // unwinds to the call's protected call, past this frame, which holds
    // nothing to drop. The hook stays armed, so a Rust callback that
    // swallows the error meets another at the next instruction.
    unsafe {
        ffi::luaL_where(state, 0);
        ffi::lua_pushstring(state, STOPPED.as_ptr());
        ffi::lua_concat(state, 2);
        ffi::lua_error(state)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use mlua::{Lua, ffi};

    use super::{clock, on_signal, state_of};

    fn hooked(lua: &Lua) -> bool {
        let mut hooked = true;
        // SAFETY: the closure reads the state's hook and touches no stack.
        let read =
            unsafe { lua.exec_raw::<()>((), |state| hooked = ffi::lua_gethook(state).is_some()) };
        read.expect("the hook read");
        hooked
    }

    /// **A call past its budget is stopped**, from the thread it runs on,
    /// within the budget and a margin, and the stop leaves no hook behind:
    /// the next call on the state runs bare.
    #[test]
    fn a_call_past_its_budget_is_stopped_and_leaves_no_hook_behind() {
        let lua = Lua::new();
        let clock = clock().expect("the watchdog starts");
        let state = state_of(&lua).expect("the state");
        let started = Instant::now();
        let (result, stopped) = clock.run(state, Duration::from_millis(5), || {
            lua.load("while true do end").exec()
        });
        assert!(
            started.elapsed() < Duration::from_millis(55),
            "{:?}",
            started.elapsed()
        );
        assert!(stopped && result.is_err(), "{result:?}");
        assert!(!hooked(&lua), "the stop left its hook armed");
    }

    /// **A signal sent for a call that has returned stops nothing**: one
    /// that arrives as the next call runs arms the hook, which sees that
    /// call is not due and takes itself off.
    #[test]
    fn a_signal_for_a_call_already_returned_stops_nothing() {
        let lua = Lua::new();
        let poke = lua
            .create_function(|_, ()| {
                on_signal();
                Ok(())
            })
            .expect("a function");
        lua.globals().set("poke", poke).expect("set");
        let clock = clock().expect("the watchdog starts");
        let state = state_of(&lua).expect("the state");
        let (result, stopped) = clock.run(state, Duration::from_secs(10), || {
            lua.load("poke() local n = 0 for i = 1, 100000 do n = n + i end return n")
                .eval::<i64>()
        });
        assert!(!stopped, "{result:?}");
        assert_eq!(result.expect("it ran"), 5_000_050_000);
        assert!(!hooked(&lua));
    }

    /// **A call is stopped at its own deadline, whatever else is in
    /// flight**: a 5 ms call that starts while a 1 s one runs on another
    /// thread is stopped at 5 ms, not when the long one is due (the watchdog
    /// is woken when it would sleep past a new call's deadline); a quick one
    /// beside it is not stopped at all; and the long one is stopped on its
    /// own thread. Run alone to see it fail: other tests' calls wake the one
    /// watchdog too.
    #[test]
    fn a_call_is_stopped_at_its_own_deadline_whatever_else_is_in_flight() {
        let (started, waiting) = std::sync::mpsc::channel();
        let long = std::thread::spawn(move || {
            let lua = Lua::new();
            let ready = lua
                .create_function(move |_, ()| {
                    let _ = started.send(());
                    Ok(())
                })
                .expect("a function");
            lua.globals().set("ready", ready).expect("set");
            let clock = clock().expect("the watchdog starts");
            let state = state_of(&lua).expect("the state");
            clock
                .run(state, Duration::from_secs(1), || {
                    lua.load("ready() while true do end").exec()
                })
                .1
        });
        waiting.recv().expect("the long call started");
        let lua = Lua::new();
        let clock = clock().expect("the watchdog starts");
        let state = state_of(&lua).expect("the state");
        let (quick, stopped) = clock.run(state, Duration::from_millis(5), || {
            lua.load("return 1").eval::<i64>()
        });
        assert_eq!((quick.ok(), stopped), (Some(1), false));
        let began = Instant::now();
        let (result, stopped) = clock.run(state, Duration::from_millis(5), || {
            lua.load("while true do end").exec()
        });
        assert!(stopped && result.is_err(), "{result:?}");
        assert!(
            began.elapsed() < Duration::from_millis(100),
            "stopped at {:?}, when the long call was due",
            began.elapsed()
        );
        assert!(
            long.join().expect("the long call"),
            "the long call was not stopped"
        );
    }
}
