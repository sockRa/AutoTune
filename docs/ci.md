# Rust workspace verification — E1.1

The [workflow](../.github/workflows/ci.yml) checks the package-declared Rust
1.99.1 baseline on Ubuntu 24.04 and Windows Server 2025 (default MSVC target).
It uses the committed lockfile, read-only repository permissions and no stored
checkout credentials. It does not publish, upload artifacts or run executables
against a GPU. Toolchain/dependency provisioning occurs only on hosted runners;
the verification commands then use offline Cargo resolution.

Both jobs check formatting and Clippy for **every workspace test target** with
warnings denied. Linux runs the entire workspace's portable paths. Windows runs
tests in `autotune-probe`, `autotune-power-trial`, `autotune-recovery-contract` and
`autotune-recovery-core`, plus the independent synthetic-only
`autotune-measurement` library. Probe tests explicitly use synthetic adapters.

Windows recovery/storage/harness tests are **compiled, linked and linted, not executed**
by this CI definition. Their approved scratch-root, NTFS/ACL and enclosing-job
requirements need a separately qualified runner. In particular, the assertion
that Cargo's enclosing job rejects guardian launch is not portable to arbitrary
hosted runners. Existing assertions and ignored acceptance cases are unchanged.
This scope is not Windows recovery acceptance, hardware qualification, or a
passing product release gate. No `--ignored`, live probe, benchmark or elevation
command belongs in this workflow.

## Local equivalent (installed toolchain)

Use the installed toolchain matching 1.99.1 and the host's existing linker setup:

```text
cargo +1.99.1 fmt --all -- --check
cargo +1.99.1 clippy --workspace --all-targets --locked --offline -- -D warnings
cargo +1.99.1 test --workspace --no-run --locked --offline
cargo +1.99.1 test -p autotune-probe -p autotune-power-trial -p autotune-recovery-contract -p autotune-recovery-core -p autotune-measurement --locked --offline
```

On Linux, replace the test commands with `cargo +1.99.1 test --workspace --locked --offline`.
If the existing toolchain is named `stable` rather than `1.99.1`, first verify
`rustc +stable --version` reports 1.99.1 and substitute `+stable`; do not install
a named toolchain just to run these commands.
These commands never authorize installing missing tools or fetching dependencies
locally. Missing prerequisites are failures/blockers, not permission to bypass a
check. Local Windows GNU results do not verify either hosted Linux or Windows
MSVC execution. See [ROADMAP.md](../ROADMAP.md) for current evidence and handoff;
configured CI is not an observed remote pass.
