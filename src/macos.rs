//! Finder sends documents through the application delegate, not command-line arguments.
use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::runtime::{AnyObject, ClassBuilder, ProtocolObject, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};
use winit::event_loop::EventLoopProxy;

use crate::AppEvent;

static PROXY: OnceLock<EventLoopProxy<AppEvent>> = OnceLock::new();

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
