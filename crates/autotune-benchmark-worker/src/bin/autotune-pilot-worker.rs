#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    use autotune_benchmark_worker::{
        evidence::Reason,
        workload::{
            launch::{Arguments, Role},
            worker_entry,
        },
    };

    let raw = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>();
    let Ok(raw) = raw else {
        std::process::exit(2);
    };
    let Ok(args) = Arguments::parse(&raw) else {
        std::process::exit(2);
    };
    if args.role() != Role::Worker {
        std::process::exit(2);
    }

    // NativeAdmission intentionally has no issuer yet. The separate worker
    // executable now reaches the real entry/setup path but fails closed after
    // setup until the parent-to-worker grant protocol is implemented.
    let no_grant = Reason::new("parent_grant_unavailable")
        .expect("fixed native-grant rejection reason is valid");
    // SAFETY: this is the fixed worker executable's single call, before any
    // inherited endpoint has been adopted elsewhere in this process.
    let result = unsafe { worker_entry::entry(&args, |_session, _setup| Err(no_grant)) };
    let code = result.map_or(2, |outcome| outcome.exit_code());
    std::process::exit(code);
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    std::process::exit(2);
}
