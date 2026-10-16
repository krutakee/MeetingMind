//! Recording: everything that is the same on Windows and macOS. Devices and OS calls come in through the traits in
//! `capture`, `audio` and `system`; nothing here touches the OS directly.

pub mod audio_timeline;
pub mod clock;
pub mod disk;
pub mod ffmpeg;
pub mod layout;
pub mod metadata;
pub mod pacer;
pub mod recovery;
pub mod session;
pub mod state;
pub mod timefmt;

#[cfg(any(test, feature = "test-doubles"))]
pub mod testing;
