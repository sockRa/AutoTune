//! Concrete local console adapter. Never invoked by a main or by CPU tests.
use super::*;
use crate::monitor::windows_clock::WindowsClock;
use std::io::{IsTerminal, Write};
use windows_sys::Win32::System::Console::{
    GetNumberOfConsoleInputEvents, GetStdHandle, ReadConsoleInputW, INPUT_RECORD, KEY_EVENT,
    STD_INPUT_HANDLE,
};
fn capture_anchor(request: &Request, clock: &WindowsClock, facts: &mut Facts) -> Option<Anchor> {
    facts
        .observed_clock
        .get_or_insert((clock.qpc_frequency(), clock.clock_token()));
    if clock.clock_token() != request.time.epoch.clock_token
        || clock.qpc_frequency() != request.time.frequency
    {
        return None;
    }
    clock
        .capture()
        .map_err(|e| {
            facts.clock_error.get_or_insert(e.into());
        })
        .ok()
}
pub(super) fn capture(request: Request, clock: &WindowsClock) -> Record {
    let mut facts = Facts {
        stdin_terminal: std::io::stdin().is_terminal(),
        stdout_terminal: std::io::stdout().is_terminal(),
        ..Default::default()
    };
    facts.before = capture_anchor(&request, clock, &mut facts);
    if facts.stdin_terminal
        && facts.stdout_terminal
        && facts
            .before
            .is_some_and(|a| validate_now(&request, request.time.issued, a).is_ok())
    {
        let mut output = std::io::stdout().lock();
        let result = writeln!(
            output,
            "{}\nApprove only this request by typing: approve {}\nOtherwise type: decline",
            request.canonical,
            request.digest.to_hex()
        )
        .and_then(|()| output.flush());
        if let Err(e) = result {
            facts.write_error = Some(e.into());
        } else {
            read(&request, clock, &mut facts, &mut output);
        }
    }
    facts.receipt = capture_anchor(&request, clock, &mut facts);
    Record::finish(request, facts, Source::Console)
}
fn read(request: &Request, clock: &WindowsClock, facts: &mut Facts, output: &mut impl Write) {
    // Exclusive Rust stdin ownership plus the fixed controller's no-other-console-
    // reader contract prevents consumption between the count and single-event
    // read. Other same-account software is outside the containment threat model.
    let _input_owner = std::io::stdin().lock();
    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut timeline = match Clock::temporal(
        request.time.frequency,
        request.time.epoch.clock_token,
        request.time.epoch,
    ) {
        Ok(c) => c,
        Err(e) => {
            facts.policy_error = Some(e);
            return;
        }
    };
    if let Some(a) = facts.before {
        if let Err(e) = timeline.observe(a) {
            facts.policy_error = Some(e);
            return;
        }
    }
    loop {
        facts.polls += 1;
        if facts.polls > 2048 {
            facts.overflow = true;
            return;
        }
        let Some(a) = capture_anchor(request, clock, facts) else {
            return;
        };
        match timeline.observe(a) {
            Err(e) => {
                facts.policy_error = Some(e);
                facts.rejected_anchor = Some(a);
                return;
            }
            Ok(n) if n >= request.time.deadline => {
                facts.deadline_expired = true;
                facts.rejected_anchor = Some(a);
                return;
            }
            Ok(_) => {}
        }
        facts.last_pending = 0;
        if unsafe { GetNumberOfConsoleInputEvents(handle, &mut facts.last_pending) } == 0 {
            facts.read_error = Some(std::io::Error::last_os_error().into());
            return;
        }
        if facts.last_pending == 0 {
            std::thread::sleep(std::time::Duration::from_nanos(
                autotune_fma_reference::profile::Profile::approved_design()
                    .limits
                    .controller_tick_ns,
            ));
            continue;
        }
        let mut event = INPUT_RECORD::default();
        facts.last_returned = 0;
        if unsafe { ReadConsoleInputW(handle, &mut event, 1, &mut facts.last_returned) } == 0 {
            facts.read_error = Some(std::io::Error::last_os_error().into());
            return;
        }
        if facts.last_returned != 1 {
            facts.invalid_input = true;
            return;
        }
        let mut recorded = InputEvent {
            kind: event.EventType,
            ..Default::default()
        };
        if u32::from(event.EventType) == KEY_EVENT {
            // The discriminant selects KeyEvent; uChar is read only as UTF-16.
            let key = unsafe { event.Event.KeyEvent };
            recorded = InputEvent {
                kind: event.EventType,
                down: key.bKeyDown,
                repeats: key.wRepeatCount,
                key: key.wVirtualKeyCode,
                scan: key.wVirtualScanCode,
                unicode: unsafe { key.uChar.UnicodeChar },
                controls: key.dwControlKeyState,
            };
        }
        let previous = facts.length;
        let done = input_event(facts, recorded);
        // Echo only validated ASCII; raw event values remain retained separately.
        let echo = if facts.length > previous {
            output.write_all(&facts.input[previous..facts.length])
        } else if facts.length < previous {
            output.write_all(b"\x08 \x08")
        } else {
            Ok(())
        };
        if let Err(e) = echo.and_then(|()| output.flush()) {
            facts.write_error = Some(e.into());
            return;
        }
        if done {
            return;
        }
    }
}
