//! Where macOS drew the window's traffic lights, so the integrated title bar (the tab strip)
//! centres its content on them and starts after them. Their size and place change between macOS
//! versions; asking AppKit beats a table of versions.

use crate::chrome::MacLights;

/// The lights of the app's main window in egui points (`zoom` is egui's zoom factor). `None` off
/// the main thread, without a window, while the buttons are hidden (full screen) or when the
/// answer makes no sense.
#[cfg(target_os = "macos")]
pub(crate) fn measure(zoom: f32) -> Option<MacLights> {
    use objc2_app_kit::{NSApplication, NSWindowButton};
    let mtm = objc2::MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let window = app.mainWindow().or_else(|| app.keyWindow())?;
    let close = window.standardWindowButton(NSWindowButton::CloseButton)?;
    let zoom_button = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    if zoom_button.isHiddenOrHasHiddenAncestor() {
        return None;
    }
    // Window coordinates start at the window's bottom-left; egui's at its top-left.
    let height = window.frame().size.height;
    let last = zoom_button.convertRect_toView(zoom_button.bounds(), None);
    let first = close.convertRect_toView(close.bounds(), None);
    let center_y = height - (first.origin.y + first.size.height / 2.0);
    let z = f64::from(zoom.max(0.01));
    MacLights { right: ((last.origin.x + last.size.width) / z) as f32, center_y: (center_y / z) as f32, diameter: (first.size.width / z) as f32 }
        .sane()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn measure(_zoom: f32) -> Option<MacLights> {
    None
}
