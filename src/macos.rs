//! Finder sends documents as Apple events, not command-line arguments.
//! Register a handler without replacing winit's NSApplication delegate.
use std::path::PathBuf;

use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel};
use objc2_foundation::{
    MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSObject, NSObjectProtocol,
};
use winit::event_loop::EventLoopProxy;

const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class is main-thread-only.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = EventLoopProxy<PathBuf>]
    struct OpenDocumentsHandler;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for OpenDocumentsHandler {}

    impl OpenDocumentsHandler {
        // SAFETY: This is the two-descriptor signature required by NSAppleEventManager.
        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let Some(items) = event.paramDescriptorForKeyword(DIRECT_OBJECT) else {
                return;
            };
            // Apple event list indices start at one. Review displays one PDF at
            // a time, so a multi-selection leaves the last document open.
            for index in 1..=items.numberOfItems() {
                let Some(url) = items.descriptorAtIndex(index).and_then(|item| item.fileURLValue()) else {
                    continue;
                };
                if let Some(path) = url.path() {
                    let _ = self.ivars().send_event(PathBuf::from(path.to_string()));
                }
            }
        }
    }
);

pub struct OpenDocuments {
    _handler: Retained<OpenDocumentsHandler>,
}

impl OpenDocuments {
    /// Call after building the event loop and retain until it finishes.
    pub fn install(proxy: EventLoopProxy<PathBuf>) -> Self {
        let mtm = MainThreadMarker::new().expect("the event loop runs on the main thread");
        // SAFETY: Initialize the allocated NSObject subclass after setting its ivars.
        let handler: Retained<OpenDocumentsHandler> = unsafe {
            msg_send![
                super(OpenDocumentsHandler::alloc(mtm).set_ivars(proxy)),
                init
            ]
        };
        // SAFETY: The retained handler implements the selector with the required signature.
        unsafe {
            NSAppleEventManager::sharedAppleEventManager()
                .setEventHandler_andSelector_forEventClass_andEventID(
                    &handler,
                    sel!(handleOpenDocuments:withReplyEvent:),
                    CORE_EVENT_CLASS,
                    OPEN_DOCUMENTS,
                );
        }
        Self { _handler: handler }
    }
}

impl Drop for OpenDocuments {
    fn drop(&mut self) {
        NSAppleEventManager::sharedAppleEventManager()
            .removeEventHandlerForEventClass_andEventID(CORE_EVENT_CLASS, OPEN_DOCUMENTS);
    }
}
