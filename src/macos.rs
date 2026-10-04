//! AppKit window chrome and Finder document opening.
use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::runtime::{AnyObject, ClassBuilder, ProtocolObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::AppEvent;

static PROXY: OnceLock<EventLoopProxy<AppEvent>> = OnceLock::new();

/// Clip the content layer after wgpu installs it, including after surface recovery.
pub fn round_window(window: &Window, maximized: bool) {
    let RawWindowHandle::AppKit(handle) = window
        .window_handle()
        .expect("the live window has a native handle")
        .as_raw()
    else {
        unreachable!("a macOS window has an AppKit handle");
    };
    // SAFETY: winit lends its live NSView for the duration of this call. Renderer
    // calls this only on the event-loop/main thread, after creating the surface.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.setWantsLayer(true);
    let layer = view.layer().expect("the layer-backed window has a layer");
    let radius = if maximized || window.fullscreen().is_some() {
        0.0
    } else {
        12.0
    };
    // Avoid restarting Core Animation updates on every repaint.
    if layer.cornerRadius() != radius {
        layer.setCornerRadius(radius);
    }
    if !layer.masksToBounds() {
        layer.setMasksToBounds(true);
    }
}

extern "C-unwind" fn open_urls(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    let Some(proxy) = PROXY.get() else {
        return;
    };
    // Review displays one selected PDF at a time, so a multi-selection leaves
    // the last document open.
    for url in urls {
        if let Some(path) = url.path() {
            let _ = proxy.send_event(AppEvent::OpenFile(PathBuf::from(path.to_string())));
        }
    }
}

/// Add Finder document opening to winit's application delegate without replacing it.
pub fn install(proxy: EventLoopProxy<AppEvent>) {
    PROXY
        .set(proxy)
        .expect("the Finder handler is installed once");
    let mtm = MainThreadMarker::new().expect("the event loop runs on the main thread");
    let application = NSApplication::sharedApplication(mtm);
    let delegate = application
        .delegate()
        .expect("winit installed an application delegate");
    let delegate =
        <ProtocolObject<dyn NSApplicationDelegate> as AsRef<AnyObject>>::as_ref(&delegate);
    let mut builder = ClassBuilder::new(c"ReviewApplicationDelegate", delegate.class())
        .expect("the Finder handler class is registered once");
    // SAFETY: The method has NSApplicationDelegate's application:openURLs: signature.
    unsafe {
        builder.add_method(
            sel!(application:openURLs:),
            open_urls as extern "C-unwind" fn(_, _, _, _),
        );
    }
    let class = builder.register();
    // SAFETY: The new class adds no ivars and directly subclasses the object's current class.
    unsafe { AnyObject::set_class(delegate, class) };
}
