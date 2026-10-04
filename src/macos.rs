//! AppKit window chrome and Finder document opening.
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ClassBuilder, ProtocolObject, Sel};
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSControlStateValueOn, NSEventModifierFlags,
    NSF1FunctionKey, NSF11FunctionKey, NSMenu, NSMenuItem, NSView,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::{
    AppEvent,
    desktop::Action,
    native_ui::Appearance,
    persistence::{Bookmark, MAX_BOOKMARKS, State},
    viewer::Viewer,
};

static PROXY: OnceLock<EventLoopProxy<AppEvent>> = OnceLock::new();
static ACTIONS: Mutex<Vec<Action>> = Mutex::new(Vec::new());
static MENU_STATE: Mutex<Option<MenuState>> = Mutex::new(None);

#[derive(Clone, PartialEq)]
struct MenuState {
    recent: Vec<(PathBuf, usize)>,
    bookmarks: Vec<Bookmark>,
    has_history: bool,
    restore_session: bool,
    current_bookmarked: bool,
    document: bool,
    printable: bool,
    appearance: Appearance,
    blocked: bool,
}

struct Shortcut {
    key: String,
    modifiers: NSEventModifierFlags,
}

impl Shortcut {
    fn command(key: &str) -> Self {
        Self {
            key: key.into(),
            modifiers: NSEventModifierFlags::Command,
        }
    }

    fn command_shift(key: &str) -> Self {
        Self {
            key: key.into(),
            modifiers: NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        }
    }

    fn function(key: u32) -> Self {
        Self {
            key: char::from_u32(key).unwrap().into(),
            modifiers: NSEventModifierFlags::empty(),
        }
    }
}

fn menu(title: &str, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    menu.setAutoenablesItems(false);
    menu
}

#[allow(clippy::too_many_arguments)]
fn menu_item(
    title: &str,
    action: Option<Action>,
    shortcut: Option<Shortcut>,
    enabled: bool,
    checked: bool,
    target: &AnyObject,
    actions: &mut Vec<Action>,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let key = shortcut.as_ref().map_or("", |shortcut| &shortcut.key);
    // SAFETY: menu_action: is installed below with NSMenuItem's action signature.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action.as_ref().map(|_| sel!(menuAction:)),
            &NSString::from_str(key),
        )
    };
    if let Some(shortcut) = shortcut {
        item.setKeyEquivalentModifierMask(shortcut.modifiers);
    }
    item.setEnabled(enabled);
    if checked {
        item.setState(NSControlStateValueOn);
    }
    if let Some(action) = action {
        item.setTag(actions.len() as isize);
        actions.push(action);
        // SAFETY: The application delegate outlives every item in its main menu.
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

fn add_submenu(main: &NSMenu, title: &str, submenu: &NSMenu, mtm: MainThreadMarker) {
    let item = menu_item(title, None, None, true, false, main, &mut Vec::new(), mtm);
    item.setSubmenu(Some(submenu));
    main.addItem(&item);
}

fn add_separator(menu: &NSMenu, mtm: MainThreadMarker) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

fn build_menu(
    state: &MenuState,
    target: &AnyObject,
    mtm: MainThreadMarker,
) -> (Retained<NSMenu>, Vec<Action>) {
    let main = menu("Review", mtm);
    let mut actions = Vec::new();
    let enabled = |available| available && !state.blocked;

    let application = menu("Review", mtm);
    application.addItem(&menu_item(
        "Quit Review",
        Some(Action::Quit),
        Some(Shortcut::command("q")),
        enabled(true),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_submenu(&main, "Review", &application, mtm);

    let file = menu("File", mtm);
    file.addItem(&menu_item(
        "Open…",
        Some(Action::Open),
        Some(Shortcut::command("o")),
        enabled(true),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_separator(&file, mtm);
    file.addItem(&menu_item(
        "Save",
        Some(Action::Save(false)),
        Some(Shortcut::command("s")),
        enabled(state.document),
        false,
        target,
        &mut actions,
        mtm,
    ));
    file.addItem(&menu_item(
        "Save As…",
        Some(Action::Save(true)),
        Some(Shortcut::command_shift("s")),
        enabled(state.document),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_separator(&file, mtm);
    file.addItem(&menu_item(
        "Print…",
        Some(Action::Print),
        Some(Shortcut::command("p")),
        enabled(state.printable),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_separator(&file, mtm);
    file.addItem(&menu_item(
        "Close Tab",
        Some(Action::CloseTab),
        Some(Shortcut::command("w")),
        enabled(state.document),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_submenu(&main, "File", &file, mtm);

    let recent = menu("Recent", mtm);
    if state.recent.is_empty() {
        recent.addItem(&menu_item(
            "No recent files",
            None,
            None,
            false,
            false,
            target,
            &mut actions,
            mtm,
        ));
    } else {
        for (path, page) in &state.recent {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            recent.addItem(&menu_item(
                &format!("{name} · Page {}", page.saturating_add(1)),
                Some(Action::OpenRecent(path.clone())),
                None,
                enabled(true),
                false,
                target,
                &mut actions,
                mtm,
            ));
        }
    }
    add_separator(&recent, mtm);
    recent.addItem(&menu_item(
        "Clear History…",
        Some(Action::ClearHistory),
        None,
        enabled(state.has_history),
        false,
        target,
        &mut actions,
        mtm,
    ));
    recent.addItem(&menu_item(
        "Restore Tabs on Startup",
        Some(Action::RestoreSession(!state.restore_session)),
        None,
        enabled(true),
        state.restore_session,
        target,
        &mut actions,
        mtm,
    ));
    add_submenu(&main, "Recent", &recent, mtm);

    let bookmarks = menu("Bookmarks", mtm);
    bookmarks.addItem(&menu_item(
        if state.current_bookmarked {
            "Remove This Page Bookmark"
        } else {
            "Bookmark This Page"
        },
        Some(Action::ToggleBookmark),
        Some(Shortcut::command("b")),
        enabled(
            state.document && (state.current_bookmarked || state.bookmarks.len() < MAX_BOOKMARKS),
        ),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_separator(&bookmarks, mtm);
    if state.bookmarks.is_empty() {
        bookmarks.addItem(&menu_item(
            "No personal bookmarks",
            None,
            None,
            false,
            false,
            target,
            &mut actions,
            mtm,
        ));
    } else {
        let remove = menu("Remove Bookmark", mtm);
        for (index, bookmark) in state.bookmarks.iter().enumerate() {
            let name = bookmark
                .path
                .file_name()
                .unwrap_or(bookmark.path.as_os_str())
                .to_string_lossy();
            let label = format!("{name} · Page {}", bookmark.reading.page.saturating_add(1));
            bookmarks.addItem(&menu_item(
                &label,
                Some(Action::OpenBookmark(bookmark.clone())),
                None,
                enabled(true),
                false,
                target,
                &mut actions,
                mtm,
            ));
            remove.addItem(&menu_item(
                &label,
                Some(Action::RemoveBookmark(index)),
                None,
                enabled(true),
                false,
                target,
                &mut actions,
                mtm,
            ));
        }
        add_submenu(&bookmarks, "Remove Bookmark", &remove, mtm);
    }
    if state.bookmarks.len() >= MAX_BOOKMARKS && !state.current_bookmarked {
        bookmarks.addItem(&menu_item(
            "Bookmark limit reached",
            None,
            None,
            false,
            false,
            target,
            &mut actions,
            mtm,
        ));
    }
    add_submenu(&main, "Bookmarks", &bookmarks, mtm);

    let view = menu("View", mtm);
    let appearance = menu("Appearance", mtm);
    for choice in [
        Appearance::System,
        Appearance::Light,
        Appearance::Dark,
        Appearance::HighContrast,
    ] {
        appearance.addItem(&menu_item(
            choice.label(),
            Some(Action::Appearance(choice)),
            None,
            enabled(true),
            state.appearance == choice,
            target,
            &mut actions,
            mtm,
        ));
    }
    add_submenu(&view, "Appearance", &appearance, mtm);
    view.addItem(&menu_item(
        "Fullscreen",
        Some(Action::Fullscreen),
        Some(Shortcut::function(NSF11FunctionKey)),
        enabled(true),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_submenu(&main, "View", &view, mtm);

    let help = menu("Help", mtm);
    help.addItem(&menu_item(
        "Keyboard Shortcuts",
        Some(Action::Help),
        Some(Shortcut::function(NSF1FunctionKey)),
        enabled(true),
        false,
        target,
        &mut actions,
        mtm,
    ));
    add_submenu(&main, "Help", &help, mtm);

    (main, actions)
}

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

extern "C-unwind" fn menu_action(_delegate: &AnyObject, _selector: Sel, item: &NSMenuItem) {
    let action = ACTIONS
        .lock()
        .expect("menu actions lock is not poisoned")
        .get(item.tag() as usize)
        .cloned();
    if let (Some(proxy), Some(action)) = (PROXY.get(), action) {
        let _ = proxy.send_event(AppEvent::Menu(action));
    }
}

/// Keep the native application menu in sync with document and library state.
pub fn sync_menu(
    state: &State,
    viewer: Option<&Viewer>,
    appearance: Appearance,
    printable: bool,
    blocked: bool,
) {
    let current_bookmarked = viewer.is_some_and(|viewer| {
        state.bookmarks.iter().any(|bookmark| {
            bookmark.path == viewer.state_key()
                && bookmark.reading.page == viewer.reading_state().page
        })
    });
    let next = MenuState {
        recent: state
            .recent
            .iter()
            .map(|entry| (entry.path.clone(), entry.reading.page))
            .collect(),
        bookmarks: state.bookmarks.clone(),
        has_history: !state.recent.is_empty() || !state.session.files.is_empty(),
        restore_session: state.restore_session,
        current_bookmarked,
        document: viewer.is_some(),
        printable,
        appearance,
        blocked,
    };
    let mut menu_state = MENU_STATE.lock().expect("menu state lock is not poisoned");
    if menu_state.as_ref() == Some(&next) {
        return;
    }

    let mtm = MainThreadMarker::new().expect("the event loop runs on the main thread");
    let application = NSApplication::sharedApplication(mtm);
    let delegate = application
        .delegate()
        .expect("winit installed an application delegate");
    let target = <ProtocolObject<dyn NSApplicationDelegate> as AsRef<AnyObject>>::as_ref(&delegate);
    let (menu, actions) = build_menu(&next, target, mtm);
    *ACTIONS.lock().expect("menu actions lock is not poisoned") = actions;
    application.setMainMenu(Some(&menu));
    *menu_state = Some(next);
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
    // SAFETY: Both methods have the signatures required by their selectors.
    unsafe {
        builder.add_method(
            sel!(application:openURLs:),
            open_urls as extern "C-unwind" fn(_, _, _, _),
        );
        builder.add_method(
            sel!(menuAction:),
            menu_action as extern "C-unwind" fn(_, _, _),
        );
    }
    let class = builder.register();
    // SAFETY: The new class adds no ivars and directly subclasses the object's current class.
    unsafe { AnyObject::set_class(delegate, class) };
}
