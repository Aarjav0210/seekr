//! seekr.app: the finder in a window of its own, brought up from any app by a
//! global hotkey. Nothing else is involved. The hotkey is a Carbon hot key,
//! which needs no permission ([`hotkey`]). The window is a non-activating
//! panel on every desktop, full-screen ones included, so the app in front
//! stays in front (its menu bar too) and gets the keyboard back by itself
//! when the panel goes away: after ⏎ the app that was opened has it. The
//! finder's frames are drawn into a grid of cells ([`grid`]) that the panel
//! paints itself ([`paint`]), so it looks as it did in a terminal. While the
//! panel is hidden nothing runs: the frame timer stops, and the finder waits,
//! loaded, for the next press.
//!
//! The hotkey is `--hotkey`, else `defaults read io.github.aarjav0210.seekr
//! hotkey` (what install.sh --hotkey writes), else ⌘Space.
mod glyphs;
mod grid;
mod hotkey;
mod keys;
mod paint;
mod place;

use crate::tui::{App as Finder, Host};
use anyhow::{anyhow, Context, Result};
use grid::Grid;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSBitmapImageFileType, NSColor, NSEvent, NSEventModifierFlags, NSPanel, NSPasteboard,
    NSPasteboardTypeString, NSResponder, NSStatusWindowLevel, NSView, NSWindow,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_core_foundation::{CFPreferencesCopyAppValue, CFString};
use objc2_foundation::{
    NSDate, NSDictionary, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize, NSTimer,
};
use paint::Painter;
use ratatui::Terminal;
use seekr_core::Config;
use std::cell::{Cell, RefCell};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// seekr.app's bundle identifier, which is also its defaults domain.
pub const BUNDLE_ID: &str = "io.github.aarjav0210.seekr";
const DEFAULT_HOTKEY: &str = "cmd+space";
/// How often the finder ticks while the window is up (it does nothing while hidden).
const FRAME: f64 = 1.0 / 60.0;

pub struct Options {
    pub hotkey: Option<String>,
    /// Show the window right away instead of waiting for the hotkey.
    pub show: bool,
    /// Draw the window for `query` into this PNG and exit, showing nothing.
    pub snapshot: Option<PathBuf>,
    pub query: String,
    /// Only parse the hotkey: exit 0 if seekr understands it.
    pub check: bool,
}

pub fn run(opts: Options) -> Result<()> {
    let spec = opts.hotkey.clone().or_else(saved_hotkey).unwrap_or_else(|| DEFAULT_HOTKEY.into());
    let key = hotkey::parse(&spec).map_err(|e| anyhow!(e))?;
    if opts.check {
        return Ok(());
    }
    let mtm = MainThreadMarker::new().context("seekr app has to run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if let Some(png) = &opts.snapshot {
        return snapshot(mtm, png, &opts.query);
    }
    let Some(_lock) = single_instance()? else {
        eprintln!("seekr app: already running");
        return Ok(());
    };
    let launcher = Launcher::new(mtm)?;
    LAUNCHER.with(|l| *l.borrow_mut() = Some(launcher.clone()));
    app.setDelegate(Some(ProtocolObject::from_ref(&*launcher.delegate)));
    let version = env!("CARGO_PKG_VERSION");
    match hotkey::register(key, || with_launcher(Launcher::hotkey)) {
        Ok(()) => eprintln!("seekr app {version}: {spec} brings up the window"),
        // Still of use without it: opening seekr.app shows the window. Exiting
        // would only have launchd start it again every ten seconds.
        Err(status) => eprintln!(
            "seekr app {version}: could not register the hotkey {spec} (Carbon error {status}); opening seekr.app shows the window"
        ),
    }
    if opts.show {
        launcher.show();
    }
    app.run();
    Ok(())
}

/// One copy at a time: a lock in the data dir, held while the process lives.
fn single_instance() -> Result<Option<File>> {
    let dir = Config::data_dir();
    std::fs::create_dir_all(&dir)?;
    let file = File::options().create(true).truncate(false).write(true).open(dir.join("app.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The hotkey install.sh saved: `defaults write io.github.aarjav0210.seekr hotkey ctrl+space`.
fn saved_hotkey() -> Option<String> {
    let value = CFPreferencesCopyAppValue(&CFString::from_str("hotkey"), &CFString::from_str(BUNDLE_ID))?;
    Some(value.downcast_ref::<CFString>()?.to_string())
}

thread_local! {
    static LAUNCHER: RefCell<Option<Rc<Launcher>>> = const { RefCell::new(None) };
}

/// Run `f` on the launcher. AppKit and Carbon call in from the main thread
/// only, sometimes from inside each other, so it is handed out as a fresh
/// `Rc` each time rather than kept borrowed.
fn with_launcher(f: impl FnOnce(&Launcher)) {
    if let Some(l) = LAUNCHER.with(|l| l.borrow().clone()) {
        f(&l);
    }
}

struct Launcher {
    mtm: MainThreadMarker,
    finder: RefCell<Finder>,
    term: RefCell<Terminal<Grid>>,
    painter: Painter,
    panel: Retained<Panel>,
    view: Retained<GridView>,
    delegate: Retained<Delegate>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    visible: Cell<bool>,
    /// Trackpad scrolling not yet worth a whole row.
    scrolled: Cell<f64>,
}

impl Launcher {
    fn new(mtm: MainThreadMarker) -> Result<Rc<Launcher>> {
        let finder = Finder::new(Host::Window)?;
        let Ok(term) = Terminal::new(Grid::new(80, 24));
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(800.0, 480.0));
        let panel = Panel::new(mtm, frame);
        let view = GridView::new(mtm, frame);
        panel.setContentView(Some(&view));
        panel.makeFirstResponder(Some(&view));
        let delegate = Delegate::new(mtm);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Ok(Rc::new(Launcher {
            mtm,
            finder: RefCell::new(finder),
            term: RefCell::new(term),
            painter: Painter::new(),
            panel,
            view,
            delegate,
            timer: RefCell::new(None),
            visible: Cell::new(false),
            scrolled: Cell::new(0.0),
        }))
    }

    /// The hotkey: bring the window up, or put it away when it is up.
    fn hotkey(&self) {
        if self.visible.get() {
            self.park();
        } else {
            self.show();
        }
    }

    fn show(&self) {
        if self.visible.get() {
            self.panel.makeKeyAndOrderFront(None);
            return;
        }
        let Some(screen) = place::target_screen(self.mtm) else {
            return;
        };
        let at = place::place(screen.visibleFrame(), self.painter.cell, paint::PADDING);
        self.term.borrow_mut().backend_mut().resize(at.cols, at.rows);
        self.finder.borrow_mut().wake_up();
        self.render();
        self.panel.setFrame_display(at.frame, false);
        self.visible.set(true);
        self.panel.makeKeyAndOrderFront(None);
        self.panel.invalidateShadow();
        self.start_timer();
    }

    /// Away without a dismissal (the hotkey again, a click elsewhere): the
    /// query stays, selected, for the next press.
    fn park(&self) {
        self.finder.borrow_mut().parked();
        self.render();
        self.hide();
    }

    fn hide(&self) {
        if !self.visible.replace(false) {
            return;
        }
        self.stop_timer();
        self.panel.orderOut(None);
        // Opening seekr.app itself (Finder, `open`) activates it; hand the
        // focus back to whatever was in front before.
        let app = NSApplication::sharedApplication(self.mtm);
        if app.isActive() {
            app.hide(None);
        }
    }

    /// Draw the finder's frame into the grid; repaint the window if it changed.
    fn render(&self) {
        let mut term = self.term.borrow_mut();
        let mut finder = self.finder.borrow_mut();
        let Ok(_) = term.draw(|f| finder.draw(f));
        if term.backend_mut().take_changed() {
            self.view.setNeedsDisplay(true);
        }
    }

    fn tick(&self) {
        self.finder.borrow_mut().tick();
        self.render();
    }

    /// After input: draw it, and go away if the finder was dismissed.
    fn after_input(&self) {
        self.render();
        if self.finder.borrow().is_hidden() {
            self.hide();
        }
    }

    fn key(&self, event: &NSEvent) {
        let flags = event.modifierFlags();
        let held = keys::Held {
            shift: flags.contains(NSEventModifierFlags::Shift),
            control: flags.contains(NSEventModifierFlags::Control),
            option: flags.contains(NSEventModifierFlags::Option),
            command: flags.contains(NSEventModifierFlags::Command),
        };
        let chars = event.characters().map(|s| s.to_string()).unwrap_or_default();
        let plain = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
        // The few ⌘ shortcuts a text field has; the finder's own come below.
        if held.command && !held.control && !held.option {
            match plain.to_ascii_lowercase().as_str() {
                "v" => {
                    let text = NSPasteboard::generalPasteboard()
                        .stringForType(unsafe { NSPasteboardTypeString })
                        .map(|s| s.to_string());
                    if let Some(text) = text {
                        self.finder.borrow_mut().paste(&text);
                    }
                    return self.after_input();
                }
                "a" => {
                    self.finder.borrow_mut().select_query();
                    return self.after_input();
                }
                "w" | "q" | "h" => return self.park(),
                _ => {}
            }
        }
        let Some((code, mods)) = keys::translate(event.keyCode(), &chars, &plain, held) else {
            return;
        };
        self.finder.borrow_mut().handle_key(code, mods);
        self.after_input();
    }

    fn scroll(&self, event: &NSEvent) {
        let dy = event.scrollingDeltaY();
        let rows = if event.hasPreciseScrollingDeltas() {
            // Trackpads report points: a row per cell height scrolled.
            let total = self.scrolled.get() + dy;
            let rows = (total / self.painter.cell.height).trunc();
            self.scrolled.set(total - rows * self.painter.cell.height);
            rows
        } else {
            dy.signum()
        };
        if rows == 0.0 {
            return;
        }
        // Content moving up (dy < 0) moves the selection down the list.
        let step = if rows < 0.0 { 1 } else { -1 };
        for _ in 0..rows.abs() as usize {
            self.finder.borrow_mut().step(step);
        }
        self.after_input();
    }

    fn focus_lost(&self) {
        if self.visible.get() {
            self.park();
        }
    }

    fn paint(&self) {
        let term = self.term.borrow();
        self.painter.paint(term.backend(), self.view.bounds(), self.panel.backingScaleFactor());
    }

    fn start_timer(&self) {
        if self.timer.borrow().is_some() {
            return;
        }
        let target: &AnyObject = &self.delegate;
        // SAFETY: the delegate answers tick: (see Delegate), and the timer keeps it alive.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(FRAME, target, sel!(tick:), None, true)
        };
        *self.timer.borrow_mut() = Some(timer);
    }

    fn stop_timer(&self) {
        if let Some(t) = self.timer.borrow_mut().take() {
            t.invalidate();
        }
    }
}

/// Draw the window for `query` into a PNG without showing it: what the next
/// hotkey press would show, for checking the drawing.
fn snapshot(mtm: MainThreadMarker, png: &Path, query: &str) -> Result<()> {
    let launcher = Launcher::new(mtm)?;
    LAUNCHER.with(|l| *l.borrow_mut() = Some(launcher.clone()));
    {
        let mut finder = launcher.finder.borrow_mut();
        finder.settle(Duration::from_secs(20));
        finder.paste(query);
        finder.settle(Duration::from_secs(5));
    }
    // The same screen choice and frame timer as a hotkey press, minus the showing.
    let screen = place::target_screen(mtm).context("no screen to size the window for")?;
    let at = place::place(screen.visibleFrame(), launcher.painter.cell, paint::PADDING);
    launcher.term.borrow_mut().backend_mut().resize(at.cols, at.rows);
    launcher.start_timer();
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.25));
    launcher.stop_timer();
    launcher.render();
    launcher.panel.setFrame_display(at.frame, false);
    let bounds = launcher.view.bounds();
    let rep = launcher.view.bitmapImageRepForCachingDisplayInRect(bounds).context("no bitmap for the window")?;
    launcher.view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    // SAFETY: PNG with no properties.
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        .context("could not encode the PNG")?;
    std::fs::write(png, data.to_vec()).with_context(|| format!("writing {}", png.display()))?;
    println!("{} ({}×{} cells)", png.display(), at.cols, at.rows);
    Ok(())
}

define_class!(
    // SAFETY: NSPanel has no subclassing requirements, and Panel adds no state.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SeekrPanel"]
    struct Panel;

    impl Panel {
        // A borderless panel only takes the keyboard when it says it can.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

impl Panel {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Panel> {
        let this = Self::alloc(mtm).set_ivars(());
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: NSPanel's designated initializer.
        let panel: Retained<Panel> = unsafe {
            msg_send![super(this), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        // Above ordinary windows, on whichever desktop is showing, over
        // full-screen apps too, and out of ⌘` and Mission Control.
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::Stationary,
        );
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        // The rounded panel is drawn by the view; the window around it is clear.
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setMovable(false);
        panel
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements, and GridView adds no state.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SeekrGridView"]
    struct GridView;

    impl GridView {
        // Rows count down from the top, like the grid's.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            with_launcher(Launcher::paint);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            with_launcher(|l| l.key(event));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            with_launcher(|l| l.scroll(event));
        }
    }
);

impl GridView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<GridView> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Delegate adds no state.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SeekrDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        // Opening seekr.app while it runs (Finder, Spotlight, `open`) shows the window.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            with_launcher(Launcher::show);
            true
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        // A click on another window: put the panel away, as Spotlight does.
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            with_launcher(Launcher::focus_lost);
        }
    }

    impl Delegate {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            with_launcher(Launcher::tick);
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Delegate> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's initializer.
        unsafe { msg_send![super(this), init] }
    }
}
