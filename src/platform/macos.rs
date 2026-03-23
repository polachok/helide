use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{mpsc, LazyLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSDocumentController, NSEventModifierFlags, NSMenu, NSMenuItem,
    NSModalResponseOK, NSOpenPanel,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSDictionary, NSObject, NSProcessInfo, NSString,
    NSUserDefaults,
};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Channel for sending events from objc callbacks to the winit event loop.
/// The sender lives in static storage (used by objc handlers),
/// the receiver is drained by flush_pending_files().
static OPEN_FILE_TX: LazyLock<mpsc::Sender<PathBuf>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    // Store receiver in a separate static
    *OPEN_FILE_RX.lock().unwrap() = Some(rx);
    tx
});
static OPEN_FILE_RX: LazyLock<std::sync::Mutex<Option<mpsc::Receiver<PathBuf>>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

static EVENT_PROXY: LazyLock<std::sync::Mutex<Option<EventLoopProxy<UserEvent>>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

pub fn set_event_proxy(proxy: EventLoopProxy<UserEvent>) {
    *EVENT_PROXY.lock().unwrap() = Some(proxy);
}

/// Drain any file paths received from objc handlers and send them as UserEvents.
pub fn flush_pending_files() {
    let proxy = EVENT_PROXY.lock().unwrap();
    let Some(proxy) = proxy.as_ref() else { return };

    if let Some(rx) = OPEN_FILE_RX.lock().unwrap().as_ref() {
        while let Ok(path) = rx.try_recv() {
            let _ = proxy.send_event(UserEvent::OpenFile(path));
        }
    }
}

/// Note a file as recently opened (shows in File > Open Recent).
/// Note a file as recently opened and refresh the Open Recent menu.
pub fn note_recent_document(path: &std::path::Path) {
    use objc2_foundation::NSURL;
    let mtm = MainThreadMarker::new().unwrap();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSDocumentController::sharedDocumentController(mtm).noteNewRecentDocumentURL(&url);
    refresh_recent_menu(mtm);
}

fn refresh_recent_menu(mtm: MainThreadMarker) {
    RECENT_MENU.with(|cell| {
        let borrow = cell.borrow();
        let Some(menu) = borrow.as_ref() else { return };

        menu.removeAllItems();

        let doc_controller = NSDocumentController::sharedDocumentController(mtm);
        let urls = doc_controller.recentDocumentURLs();

        for url in urls.iter() {
            unsafe {
                let Some(path) = url.path() else { continue };
                let item = NSMenuItem::new(mtm);
                // Show just the filename
                if let Some(last) = url.lastPathComponent() {
                    item.setTitle(&last);
                } else {
                    item.setTitle(&path);
                }
                item.setRepresentedObject(Some(&url));
                item.setAction(Some(sel!(openRecentFile:)));
                // Target the menu handler
                MENU_HANDLER.with(|h| {
                    if let Some(handler) = h.borrow().as_ref() {
                        item.setTarget(Some(handler));
                    }
                });
                menu.addItem(&item);
            }
        }

        if !urls.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        let clear = NSMenuItem::new(mtm);
        clear.setTitle(ns_string!("Clear Menu"));
        unsafe {
            clear.setAction(Some(sel!(clearRecentDocuments:)));
            let doc_controller = NSDocumentController::sharedDocumentController(mtm);
            clear.setTarget(Some(&doc_controller));
        }
        menu.addItem(&clear);
    });
}

fn send_event(event: UserEvent) {
    let proxy = EVENT_PROXY.lock().unwrap();
    if let Some(proxy) = proxy.as_ref() {
        let _ = proxy.send_event(event);
    } else if let UserEvent::OpenFile(path) = event {
        // Proxy not ready yet — queue via channel
        let _ = OPEN_FILE_TX.send(path);
    }
}

// Menu action handler
define_class!(
    #[derive(Debug)]
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct MenuHandler;

    impl MenuHandler {
        #[unsafe(method(newFile:))]
        fn new_file(&self, _sender: *mut NSObject) {
            send_event(UserEvent::NewFile);
        }

        #[unsafe(method(openFile:))]
        fn open_file(&self, _sender: *mut NSObject) {
            let mtm = MainThreadMarker::new().unwrap();
            unsafe {
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(true);
                panel.setCanChooseDirectories(false);
                panel.setAllowsMultipleSelection(false);

                let response = panel.runModal();
                if response == NSModalResponseOK {
                    if let Some(url) = panel.URL() {
                        if let Some(path_str) = url.path() {
                            send_event(UserEvent::OpenFile(PathBuf::from(path_str.to_string())));
                        }
                    }
                }
            }
        }

        #[unsafe(method(openDirectory:))]
        fn open_directory(&self, _sender: *mut NSObject) {
            let mtm = MainThreadMarker::new().unwrap();
            unsafe {
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(false);
                panel.setCanChooseDirectories(true);
                panel.setAllowsMultipleSelection(false);

                let response = panel.runModal();
                if response == NSModalResponseOK {
                    if let Some(url) = panel.URL() {
                        if let Some(path_str) = url.path() {
                            send_event(UserEvent::OpenDirectory(PathBuf::from(
                                path_str.to_string(),
                            )));
                        }
                    }
                }
            }
        }

        #[unsafe(method(saveFile:))]
        fn save_file(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Save);
        }

        #[unsafe(method(closeBuffer:))]
        fn close_buffer(&self, _sender: *mut NSObject) {
            send_event(UserEvent::CloseBuffer);
        }

        #[unsafe(method(helideUndo:))]
        fn undo(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Undo);
        }

        #[unsafe(method(helideRedo:))]
        fn redo(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Redo);
        }

        #[unsafe(method(helidePaste:))]
        fn paste(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Paste);
        }

        #[unsafe(method(helideTutor:))]
        fn tutor(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Tutor);
        }

        #[unsafe(method(openRecentFile:))]
        fn open_recent_file(&self, sender: &NSMenuItem) {
            unsafe {
                use objc2_foundation::NSURL;
                if let Some(obj) = sender.representedObject() {
                    // representedObject is the NSURL
                    let url: &NSURL = &*(Retained::as_ptr(&obj) as *const NSURL);
                    if let Some(path) = url.path() {
                        send_event(UserEvent::OpenFile(PathBuf::from(path.to_string())));
                    }
                }
            }
        }
    }
);

impl MenuHandler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

thread_local! {
    static MENU_HANDLER: RefCell<Option<Retained<MenuHandler>>> = const { RefCell::new(None) };
    static RECENT_MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

/// Set up the native macOS menu bar.
pub fn setup_menu_bar() {
    let mtm = MainThreadMarker::new().expect("must be called on main thread");
    let app = NSApplication::sharedApplication(mtm);

    let handler = MenuHandler::new(mtm);

    unsafe {
        let main_menu = NSMenu::new(mtm);

        let app_menu = create_app_menu(mtm);
        let app_menu_item = NSMenuItem::new(mtm);
        app_menu_item.setSubmenu(Some(&app_menu));
        if let Some(services_menu) = app_menu.itemWithTitle(ns_string!("Services")) {
            app.setServicesMenu(services_menu.submenu().as_deref());
        }
        main_menu.addItem(&app_menu_item);

        let file_menu = create_file_menu(mtm, &handler);
        let file_menu_item = NSMenuItem::new(mtm);
        file_menu_item.setSubmenu(Some(&file_menu));
        main_menu.addItem(&file_menu_item);

        let edit_menu = create_edit_menu(mtm, &handler);
        let edit_menu_item = NSMenuItem::new(mtm);
        edit_menu_item.setSubmenu(Some(&edit_menu));
        main_menu.addItem(&edit_menu_item);

        let win_menu = create_window_menu(mtm);
        let win_menu_item = NSMenuItem::new(mtm);
        win_menu_item.setSubmenu(Some(&win_menu));
        main_menu.addItem(&win_menu_item);
        app.setWindowsMenu(Some(&win_menu));

        let help_menu = create_help_menu(mtm, &handler);
        let help_menu_item = NSMenuItem::new(mtm);
        help_menu_item.setSubmenu(Some(&help_menu));
        main_menu.addItem(&help_menu_item);
        app.setHelpMenu(Some(&help_menu));

        app.setMainMenu(Some(&main_menu));
    }

    MENU_HANDLER.with(|cell| {
        *cell.borrow_mut() = Some(handler);
    });
}

unsafe fn create_app_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    let process_name = NSProcessInfo::processInfo().processName();

    let about = NSMenuItem::new(mtm);
    about.setTitle(&ns_string!("About ").stringByAppendingString(&process_name));
    about.setAction(Some(sel!(orderFrontStandardAboutPanel:)));
    menu.addItem(&about);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let services = NSMenuItem::new(mtm);
    let services_menu = NSMenu::new(mtm);
    services.setTitle(ns_string!("Services"));
    services.setSubmenu(Some(&services_menu));
    menu.addItem(&services);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let hide = NSMenuItem::new(mtm);
    hide.setTitle(&ns_string!("Hide ").stringByAppendingString(&process_name));
    hide.setKeyEquivalent(ns_string!("h"));
    hide.setAction(Some(sel!(hide:)));
    menu.addItem(&hide);

    let hide_others = NSMenuItem::new(mtm);
    hide_others.setTitle(ns_string!("Hide Others"));
    hide_others.setKeyEquivalent(ns_string!("h"));
    hide_others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Option | NSEventModifierFlags::Command);
    hide_others.setAction(Some(sel!(hideOtherApplications:)));
    menu.addItem(&hide_others);

    let show_all = NSMenuItem::new(mtm);
    show_all.setTitle(ns_string!("Show All"));
    show_all.setAction(Some(sel!(unhideAllApplications:)));
    menu.addItem(&show_all);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let quit = NSMenuItem::new(mtm);
    quit.setTitle(&ns_string!("Quit ").stringByAppendingString(&process_name));
    quit.setKeyEquivalent(ns_string!("q"));
    quit.setAction(Some(sel!(terminate:)));
    menu.addItem(&quit);

    menu
}

unsafe fn create_file_menu(mtm: MainThreadMarker, handler: &MenuHandler) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("File"));

    let new = NSMenuItem::new(mtm);
    new.setTitle(ns_string!("New"));
    new.setKeyEquivalent(ns_string!("n"));
    new.setAction(Some(sel!(newFile:)));
    new.setTarget(Some(handler));
    menu.addItem(&new);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let open = NSMenuItem::new(mtm);
    open.setTitle(ns_string!("Open..."));
    open.setKeyEquivalent(ns_string!("o"));
    open.setAction(Some(sel!(openFile:)));
    open.setTarget(Some(handler));
    menu.addItem(&open);

    // Open Recent submenu
    let recent_menu = NSMenu::new(mtm);
    recent_menu.setTitle(ns_string!("Open Recent"));
    let recent_item = NSMenuItem::new(mtm);
    recent_item.setTitle(ns_string!("Open Recent"));
    recent_item.setSubmenu(Some(&recent_menu));
    menu.addItem(&recent_item);

    RECENT_MENU.with(|cell| {
        *cell.borrow_mut() = Some(recent_menu);
    });

    // Populate from existing recent documents
    refresh_recent_menu(mtm);

    let open_dir = NSMenuItem::new(mtm);
    open_dir.setTitle(ns_string!("Open Directory..."));
    open_dir.setKeyEquivalent(ns_string!("O"));
    open_dir.setAction(Some(sel!(openDirectory:)));
    open_dir.setTarget(Some(handler));
    menu.addItem(&open_dir);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let save = NSMenuItem::new(mtm);
    save.setTitle(ns_string!("Save"));
    save.setKeyEquivalent(ns_string!("s"));
    save.setAction(Some(sel!(saveFile:)));
    save.setTarget(Some(handler));
    menu.addItem(&save);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let close_buf = NSMenuItem::new(mtm);
    close_buf.setTitle(ns_string!("Close"));
    close_buf.setKeyEquivalent(ns_string!("w"));
    close_buf.setAction(Some(sel!(closeBuffer:)));
    close_buf.setTarget(Some(handler));
    menu.addItem(&close_buf);

    menu
}

unsafe fn create_edit_menu(mtm: MainThreadMarker, handler: &MenuHandler) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("Edit"));

    let undo = NSMenuItem::new(mtm);
    undo.setTitle(ns_string!("Undo"));
    undo.setKeyEquivalent(ns_string!("z"));
    undo.setAction(Some(sel!(helideUndo:)));
    undo.setTarget(Some(handler));
    menu.addItem(&undo);

    let redo = NSMenuItem::new(mtm);
    redo.setTitle(ns_string!("Redo"));
    redo.setKeyEquivalent(ns_string!("Z"));
    redo.setAction(Some(sel!(helideRedo:)));
    redo.setTarget(Some(handler));
    menu.addItem(&redo);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let paste = NSMenuItem::new(mtm);
    paste.setTitle(ns_string!("Paste"));
    paste.setKeyEquivalent(ns_string!("v"));
    paste.setAction(Some(sel!(helidePaste:)));
    paste.setTarget(Some(handler));
    menu.addItem(&paste);

    menu
}

unsafe fn create_window_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("Window"));

    let minimize = NSMenuItem::new(mtm);
    minimize.setTitle(ns_string!("Minimize"));
    minimize.setKeyEquivalent(ns_string!("m"));
    minimize.setAction(Some(sel!(performMiniaturize:)));
    menu.addItem(&minimize);

    let zoom = NSMenuItem::new(mtm);
    zoom.setTitle(ns_string!("Zoom"));
    zoom.setAction(Some(sel!(performZoom:)));
    menu.addItem(&zoom);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let fullscreen = NSMenuItem::new(mtm);
    fullscreen.setTitle(ns_string!("Enter Full Screen"));
    fullscreen.setKeyEquivalent(ns_string!("f"));
    fullscreen.setKeyEquivalentModifierMask(
        NSEventModifierFlags::Control | NSEventModifierFlags::Command,
    );
    fullscreen.setAction(Some(sel!(toggleFullScreen:)));
    menu.addItem(&fullscreen);

    menu
}

unsafe fn create_help_menu(mtm: MainThreadMarker, handler: &MenuHandler) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(ns_string!("Help"));

    let tutor = NSMenuItem::new(mtm);
    tutor.setTitle(ns_string!("Helix Tutor"));
    tutor.setAction(Some(sel!(helideTutor:)));
    tutor.setTarget(Some(handler));
    menu.addItem(&tutor);

    menu
}

/// Register `application:openFiles:` on winit's NSApplicationDelegate.
/// Matches neovide's approach: subclass the delegate and swap the isa.
/// Call AFTER EventLoop::build() but BEFORE run_app().
pub fn register_open_file_handler() {
    unsafe extern "C-unwind" fn handle_open_files(
        _this: &mut AnyObject,
        _sel: objc2::runtime::Sel,
        _sender: &AnyObject,
        filenames: &NSArray<NSString>,
    ) {
        for filename in filenames.iter() {
            send_event(UserEvent::OpenFile(PathBuf::from(filename.to_string())));
        }
    }

    unsafe extern "C-unwind" fn handle_reopen(
        _this: &mut AnyObject,
        _sel: objc2::runtime::Sel,
        _sender: &AnyObject,
        has_visible: objc2::runtime::Bool,
    ) -> objc2::runtime::Bool {
        if !has_visible.as_bool() {
            // Show all windows when dock icon is clicked with no visible windows
            let mtm = MainThreadMarker::new().unwrap();
            let app = NSApplication::sharedApplication(mtm);
            for window in app.windows().iter() {
                window.makeKeyAndOrderFront(None);
            }
        }
        objc2::runtime::Bool::YES
    }

    let mtm = MainThreadMarker::new().expect("must be called on main thread");

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let delegate = app.delegate().unwrap();

        let class: &AnyClass = AnyObject::class(delegate.as_ref());

        let mut my_class = ClassBuilder::new(c"HelideApplicationDelegate", class).unwrap();
        my_class.add_method(
            sel!(application:openFiles:),
            handle_open_files as unsafe extern "C-unwind" fn(_, _, _, _) -> _,
        );
        my_class.add_method(
            sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            handle_reopen as unsafe extern "C-unwind" fn(_, _, _, _) -> _,
        );
        let class = my_class.register();

        AnyObject::set_class(delegate.as_ref(), class);
    }

    // Prevent AppKit from interpreting our command line args as files to open
    let keys = &[ns_string!("NSTreatUnknownArgumentsAsOpen")];
    let objects = &[ns_string!("NO") as &AnyObject];
    let dict = NSDictionary::from_slices(keys, objects);
    unsafe {
        NSUserDefaults::standardUserDefaults().registerDefaults(&dict);
    }
}
