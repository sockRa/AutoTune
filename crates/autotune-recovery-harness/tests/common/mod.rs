use std::process::Command;

/// Test wrappers must not allocate a console, even when run by a GUI host.
pub fn command() -> Command {
    let command = Command::new(env!("CARGO_BIN_EXE_autotune-recovery-harness"));
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
    };
    command
}
