//! The live preview with a synthetic screen (no FFmpeg needed).

use std::time::{Duration, Instant};

use meetingmind_lib::preview::{MAX_WIDTH, Preview};
use meetingmind_lib::recording::testing::SyntheticDevices;

fn wait_for_frame(preview: &Preview, since: u64) -> meetingmind_lib::preview::PreviewFrame {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(frame) = preview.latest_after(since) {
            return frame;
        }
        assert!(Instant::now() < deadline, "no preview picture arrived");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_started_preview_delivers_scaled_pictures_and_stops_cleanly() {
    let devices = SyntheticDevices::new(1920, 1080);
    let preview = Preview::default();
    assert!(!preview.is_running());

    preview.start(&devices, None).unwrap();
    let first = wait_for_frame(&preview, 0);

    assert!(preview.is_running());
    assert_eq!((first.width, first.height), (MAX_WIDTH, 360));
    assert_eq!(first.rgba.len(), (first.width * first.height * 4) as usize);
    assert!(
        preview.latest_after(first.sequence).is_none(),
        "asking for pictures newer than the newest gives nothing"
    );

    preview.stop();
    assert!(!preview.is_running());
    assert!(
        preview.latest_after(0).is_none(),
        "a stopped preview shows nothing"
    );
}

#[test]
fn starting_the_same_monitor_again_keeps_the_running_preview() {
    let devices = SyntheticDevices::new(640, 360);
    let preview = Preview::default();
    preview.start(&devices, Some("synthetic-monitor")).unwrap();
    let first = wait_for_frame(&preview, 0);

    preview.start(&devices, Some("synthetic-monitor")).unwrap();

    assert!(
        preview
            .latest_after(0)
            .is_some_and(|f| f.sequence >= first.sequence),
        "the picture is not thrown away by a repeated start"
    );
    preview.stop();
}

#[test]
fn a_monitor_that_is_gone_is_an_error_and_leaves_no_preview_running() {
    let devices = SyntheticDevices::new(640, 360);
    let preview = Preview::default();

    let error = preview.start(&devices, Some("DISPLAY9")).unwrap_err();

    assert!(error.contains("not found"), "{error}");
    assert!(!preview.is_running());
}
