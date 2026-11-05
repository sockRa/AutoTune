//! Successful reported starting timeline, including nested calls and the original
//! cleanup window. Failed reports remain readable; this never issues admission.
use super::*;
use crate::monitor::clock::Clock;

pub(super) fn assess(
    document: &Object,
    scope: &Scope,
    snapshot: Option<&super::super::starting::ReportedSnapshot>,
    runtime: Option<&super::super::runtime_evidence::Reported>,
    deadline: u64,
) -> Result<Anchor, TimingFault> {
    let d = scope.draft();
    let p = autotune_fma_reference::profile::Profile::approved_design().limits;
    let missing = |_| TimingFault::Missing;
    let read_anchor = |v: &Value| {
        anchor(v)
            .map(|mut a| {
                a.clock_token = d.clock_token;
                a
            })
            .map_err(missing)
    };
    let get_anchor = |o: &Object, name: &str| read_anchor(field(o, name).map_err(missing)?);
    let trace = object(field(document, "trace").map_err(missing)?).map_err(missing)?;
    let control = object(field(document, "control").map_err(missing)?).map_err(missing)?;
    let calls = array(field(trace, "calls").map_err(missing)?).map_err(missing)?;
    if calls.len() != 4 {
        return Err(TimingFault::Missing);
    }
    let mut pairs = [(d.epoch, d.epoch); 4];
    for (pair, call) in pairs.iter_mut().zip(calls) {
        let o = object(call).map_err(missing)?;
        *pair = (get_anchor(o, "before")?, get_anchor(o, "after")?);
    }
    let trigger = get_anchor(trace, "stop_trigger")?;
    let drain = get_anchor(trace, "drain_receipt")?;
    let completion = object(field(control, "completion").map_err(missing)?).map_err(missing)?;
    let complete = get_anchor(completion, "anchor")?;
    let receipt = get_anchor(control, "completion_receipt")?;
    let mut clock =
        Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| TimingFault::Clock)?;
    let mut previous = get_anchor(document, "receipt")?;
    clock.observe(previous).map_err(|_| TimingFault::Clock)?;
    for a in pairs[..3]
        .iter()
        .flat_map(|(a, b)| [*a, *b])
        .chain([trigger])
    {
        if a.qpc_before < previous.qpc_after {
            return Err(TimingFault::Order);
        }
        let n = clock.observe(a).map_err(|_| TimingFault::Clock)?;
        if n >= deadline || n >= p.preflight_ns {
            return Err(TimingFault::Deadline);
        }
        previous = a;
    }
    let trigger_ns = Clock::temporal(d.frequency, d.clock_token, d.epoch)
        .and_then(|mut c| c.observe(trigger))
        .map_err(|_| TimingFault::Clock)?;
    let window = object(field(control, "window").map_err(missing)?).map_err(missing)?;
    let cooperative = trigger_ns
        .saturating_add(p.cooperative_stop_ns)
        .min(p.pilot_session_ns);
    let exit = trigger_ns
        .saturating_add(p.exit_observation_ns)
        .min(p.pilot_session_ns);
    if get_anchor(control, "trigger")? != trigger
        || uint(window, "trigger_ns").map_err(missing)? != trigger_ns
        || uint(window, "cooperative_ns").map_err(missing)? != cooperative
        || uint(window, "exit_ns").map_err(missing)? != exit
    {
        return Err(TimingFault::Window);
    }
    for a in [pairs[3].0, pairs[3].1, complete, receipt, drain] {
        if a.qpc_before < previous.qpc_after {
            return Err(TimingFault::Order);
        }
        if clock.observe(a).map_err(|_| TimingFault::Clock)? >= cooperative {
            return Err(TimingFault::Deadline);
        }
        previous = a;
    }
    // Native discovery occurs inside Open; the final three version queries
    // occur inside Describe. Each branch retains its own serial clock history.
    let native = array(field(document, "native_calls").map_err(missing)?).map_err(missing)?;
    if native.len() < 3 {
        return Err(TimingFault::Native);
    }
    let mut native_clock =
        Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| TimingFault::Clock)?;
    let mut last = d.epoch;
    for (i, call) in native.iter().enumerate() {
        let o = object(call).map_err(missing)?;
        let (begin, end) = (get_anchor(o, "before")?, get_anchor(o, "after")?);
        let enclosing = pairs[usize::from(i >= native.len() - 3)];
        if begin.qpc_before < enclosing.0.qpc_after || end.qpc_after > enclosing.1.qpc_before {
            return Err(TimingFault::Native);
        }
        for a in [begin, end] {
            if a.qpc_before < last.qpc_after {
                return Err(TimingFault::Order);
            }
            native_clock.observe(a).map_err(|_| TimingFault::Clock)?;
            last = a;
        }
    }
    for check in runtime.ok_or(TimingFault::Missing)?.checks() {
        let a = check.anchor.ok_or(TimingFault::Missing)?;
        if a.qpc_before < pairs[0].0.qpc_after || a.qpc_after > pairs[0].1.qpc_before {
            return Err(TimingFault::Native);
        }
    }
    let snapshot = snapshot.ok_or(TimingFault::Missing)?.snapshot();
    if snapshot.entry() != pairs[2].0 {
        return Err(TimingFault::Settings);
    }
    let mut settings_clock =
        Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| TimingFault::Clock)?;
    let mut last = snapshot.entry();
    settings_clock
        .observe(last)
        .map_err(|_| TimingFault::Clock)?;
    for o in &snapshot.draft().observations {
        for a in [o.before, o.after] {
            let a = a.ok_or(TimingFault::Missing)?;
            if a.qpc_before < last.qpc_after || a.qpc_after > pairs[2].1.qpc_before {
                return Err(TimingFault::Settings);
            }
            settings_clock.observe(a).map_err(|_| TimingFault::Clock)?;
            last = a;
        }
    }
    Ok(drain)
}
