use std::cell::RefCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSModalResponseOK, NSOpenPanel,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSObject, NSProcessInfo};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

thread_local! {
    static EVENT_PROXY: RefCell<Option<EventLoopProxy<UserEvent>>> = const { RefCell::new(None) };
}

/// Store the event loop proxy so menu handlers can send events.
pub fn set_event_proxy(proxy: EventLoopProxy<UserEvent>) {
    EVENT_PROXY.with(|cell| {
        *cell.borrow_mut() = Some(proxy);
    });
}

fn send_event(event: UserEvent) {
    EVENT_PROXY.with(|cell| {
        if let Some(proxy) = cell.borrow().as_ref() {
            let _ = proxy.send_event(event);
        }
    });
}

// Menu action handler
declare_class!(
    struct MenuHandler;

    unsafe impl ClassType for MenuHandler {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "HelideMenuHandler";
    }

    impl DeclaredClass for MenuHandler {
        type Ivars = ();
    }

    unsafe impl MenuHandler {
        #[method(newFile:)]
        fn new_file(&self, _sender: *mut NSObject) {
            send_event(UserEvent::NewFile);
        }

        #[method(openFile:)]
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

        #[method(openDirectory:)]
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

        #[method(saveFile:)]
        fn save_file(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Save);
        }

        #[method(closeBuffer:)]
        fn close_buffer(&self, _sender: *mut NSObject) {
            send_event(UserEvent::CloseBuffer);
        }

        #[method(helideUndo:)]
        fn undo(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Undo);
        }

        #[method(helideRedo:)]
        fn redo(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Redo);
        }

        #[method(helidePaste:)]
        fn paste(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Paste);
        }

        #[method(helideTutor:)]
        fn tutor(&self, _sender: *mut NSObject) {
            send_event(UserEvent::Tutor);
        }
    }
);

impl MenuHandler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let alloc = mtm.alloc::<Self>().set_ivars(());
        unsafe { msg_send_id![super(alloc), init] }
    }
}

thread_local! {
    static MENU_HANDLER: RefCell<Option<Retained<MenuHandler>>> = const { RefCell::new(None) };
}

/// Set up the native macOS menu bar.
pub fn setup_menu_bar() {
    let mtm = MainThreadMarker::new().expect("must be called on main thread");
    let app = NSApplication::sharedApplication(mtm);

    let handler = MenuHandler::new(mtm);

    unsafe {
        let main_menu = NSMenu::new(mtm);

        // App menu
        let app_menu = create_app_menu(mtm);
        let app_menu_item = NSMenuItem::new(mtm);
        app_menu_item.setSubmenu(Some(&app_menu));
        if let Some(services_menu) = app_menu.itemWithTitle(ns_string!("Services")) {
            app.setServicesMenu(services_menu.submenu().as_deref());
        }
        main_menu.addItem(&app_menu_item);

        // File menu
        let file_menu = create_file_menu(mtm, &handler);
        let file_menu_item = NSMenuItem::new(mtm);
        file_menu_item.setSubmenu(Some(&file_menu));
        main_menu.addItem(&file_menu_item);

        // Edit menu
        let edit_menu = create_edit_menu(mtm, &handler);
        let edit_menu_item = NSMenuItem::new(mtm);
        edit_menu_item.setSubmenu(Some(&edit_menu));
        main_menu.addItem(&edit_menu_item);

        // Window menu
        let win_menu = create_window_menu(mtm);
        let win_menu_item = NSMenuItem::new(mtm);
        win_menu_item.setSubmenu(Some(&win_menu));
        main_menu.addItem(&win_menu_item);
        app.setWindowsMenu(Some(&win_menu));

        // Help menu
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
    hide_others.setKeyEquivalentModifierMask(
        NSEventModifierFlags::NSEventModifierFlagOption
            | NSEventModifierFlags::NSEventModifierFlagCommand,
    );
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
        NSEventModifierFlags::NSEventModifierFlagControl
            | NSEventModifierFlags::NSEventModifierFlagCommand,
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
