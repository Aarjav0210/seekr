//! Where the window goes: on the screen the user is working on (the one with
//! the front app's front window, else the mouse's), centred, a little above
//! the middle, and sized for that screen.
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen, NSWorkspace};
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_graphics::{
    kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString};

/// The grid's largest size in points; smaller screens get less.
const MAX_TEXT: NSSize = NSSize { width: 1100.0, height: 600.0 };

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The window's frame, in Cocoa's screen coordinates (origin bottom left).
    pub frame: NSRect,
    pub cols: u16,
    pub rows: u16,
}

/// The window for a screen whose usable area (no menu bar, no Dock) is
/// `visible`: a grid of `cell`s at most 72% of its width and 55% of its height,
/// plus `pad` on every side, centred, its top 16% of the way down.
pub fn place(visible: NSRect, cell: NSSize, pad: NSSize) -> Placement {
    let text_w = MAX_TEXT.width.min((visible.size.width * 0.72).floor());
    let text_h = MAX_TEXT.height.min((visible.size.height * 0.55).floor());
    let cols = ((text_w / cell.width).floor() as u16).max(20);
    let rows = ((text_h / cell.height).floor() as u16).max(8);
    let w = cols as f64 * cell.width + 2.0 * pad.width;
    let h = rows as f64 * cell.height + 2.0 * pad.height;
    let x = visible.origin.x + ((visible.size.width - w) / 2.0).floor();
    let top = visible.origin.y + visible.size.height - (visible.size.height * 0.16).floor();
    Placement { frame: NSRect::new(NSPoint::new(x, (top - h).floor()), NSSize::new(w, h)), cols, rows }
}

/// The screen to show the window on.
pub fn target_screen(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    let on = |p: NSPoint| screens.iter().find(|s| contains(s.frame(), p));
    front_window_center(&screens)
        .and_then(on)
        .or_else(|| on(NSEvent::mouseLocation()))
        .or_else(|| NSScreen::mainScreen(mtm))
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x && p.x < r.origin.x + r.size.width && p.y >= r.origin.y && p.y < r.origin.y + r.size.height
}

/// The centre of the front app's frontmost window, in Cocoa coordinates.
/// The window list gives bounds and owners without any permission (titles
/// would need Screen Recording; they are not asked for).
fn front_window_center(screens: &NSArray<NSScreen>) -> Option<NSPoint> {
    let pid = NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier();
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    // SAFETY: CFArray/CFDictionary/CFString/CFNumber are toll-free bridged with
    // their NS counterparts; the window list holds dictionaries.
    let windows: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*list as *const CFArray as *const NSArray<NSDictionary<NSString, AnyObject>>) };
    let key = |k: &CFString| -> &NSString { unsafe { &*(k as *const CFString as *const NSString) } };
    let (owner_key, layer_key, bounds_key) = unsafe { (key(kCGWindowOwnerPID), key(kCGWindowLayer), key(kCGWindowBounds)) };
    let number = |d: &NSDictionary<NSString, AnyObject>, k: &NSString| -> Option<f64> {
        d.objectForKey(k)?.downcast::<NSNumber>().ok().map(|n| n.doubleValue())
    };
    // Front to back: the first ordinary window (layer 0) of the front app.
    let w = windows
        .iter()
        .find(|w| number(w, owner_key) == Some(pid as f64) && number(w, layer_key) == Some(0.0))?;
    let bounds = w.objectForKey(bounds_key)?.downcast::<NSDictionary>().ok()?;
    // SAFETY: the bounds dictionary maps NSStrings to NSNumbers.
    let bounds: &NSDictionary<NSString, AnyObject> =
        unsafe { &*(&*bounds as *const NSDictionary as *const NSDictionary<NSString, AnyObject>) };
    let get = |k: &str| number(bounds, &NSString::from_str(k));
    let (x, y, width, height) = (get("X")?, get("Y")?, get("Width")?, get("Height")?);
    // The window list counts y down from the top of the main screen; Cocoa up from its bottom.
    let main_height = screens.firstObject()?.frame().size.height;
    Some(NSPoint::new(x + width / 2.0, main_height - (y + height / 2.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: NSSize = NSSize { width: 8.0, height: 16.0 };
    const PAD: NSSize = NSSize { width: 12.0, height: 10.0 };

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    #[test]
    fn a_big_screen_gets_the_largest_grid() {
        // A 1728×1117 Retina laptop screen minus a 33-point menu bar.
        let p = place(rect(0.0, 0.0, 1728.0, 1084.0), CELL, PAD);
        assert_eq!((p.cols, p.rows), (137, 37));
        assert_eq!(p.frame.size, NSSize::new(137.0 * 8.0 + 24.0, 37.0 * 16.0 + 20.0));
        // Centred, its top 16% of the way down.
        assert_eq!(p.frame.origin.x, ((1728.0 - p.frame.size.width) / 2.0_f64).floor());
        assert_eq!(p.frame.origin.y + p.frame.size.height, 1084.0 - (1084.0_f64 * 0.16).floor());
    }

    #[test]
    fn small_and_offset_screens() {
        let p = place(rect(1728.0, -200.0, 1024.0, 640.0), CELL, PAD);
        assert_eq!(p.cols, (1024.0_f64 * 0.72 / 8.0).floor() as u16);
        assert_eq!(p.rows, (640.0_f64 * 0.55 / 16.0).floor() as u16);
        assert!(p.frame.origin.x >= 1728.0 && p.frame.origin.x + p.frame.size.width <= 1728.0 + 1024.0);
        assert!(p.frame.origin.y >= -200.0 && p.frame.origin.y + p.frame.size.height <= 440.0);
        assert!(contains(rect(0.0, 0.0, 10.0, 10.0), NSPoint::new(0.0, 9.9)));
        assert!(!contains(rect(0.0, 0.0, 10.0, 10.0), NSPoint::new(10.0, 5.0)));
    }
}
