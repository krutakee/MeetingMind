//! Child-process helpers that differ by operating system.

use std::process::Command;

/// Stops a child console program (FFmpeg) from flashing a console window on Windows. No effect elsewhere.
pub fn no_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
