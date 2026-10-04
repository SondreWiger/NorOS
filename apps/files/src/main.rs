//! NorOS Files: browse folders, open files, and keep things tidy.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};

const APP_CSS: &str = r#"
.places { padding: 8px; }
.places row { padding: 6px 10px; border-radius: 8px; }
.places-title { font-size: 11px; font-weight: 700; opacity: 0.55; margin: 10px 10px 4px; }
.file-tile { padding: 10px 6px; border-radius: 10px; }
.file-name { font-size: 12px; }
.path-label { font-weight: 600; }
.status { font-size: 12px; opacity: 0.6; padding: 4px 12px; }
.empty { opacity: 0.5; font-size: 15px; }
"#;

/// Everything a window needs to navigate.
struct Browser {
    window: gtk::ApplicationWindow,
    list: gtk::DirectoryList,
    filter: gtk::CustomFilter,
    show_hidden: RefCell<bool>,
    back: RefCell<Vec<gio::File>>,
    forward: RefCell<Vec<gio::File>>,
    path_label: gtk::Label,
    status: gtk::Label,
    back_button: gtk::Button,
    forward_button: gtk::Button,
}

impl Browser {
    fn current(&self) -> Option<gio::File> {
        self.list.file()
    }

    fn go(&self, folder: gio::File) {
        if let Some(current) = self.current() {
            if current.equal(&folder) {
                return;
            }
            self.back.borrow_mut().push(current);
        }
        self.forward.borrow_mut().clear();
        self.show(folder);
    }

    fn show(&self, folder: gio::File) {
        let label = folder
            .path()
            .map(|p| pretty_path(&p))
            .unwrap_or_else(|| folder.uri().to_string());
        self.path_label.set_text(&label);
        self.window.set_title(Some(&folder.basename().map(|b| b.display().to_string()).unwrap_or(label)));
        self.list.set_file(Some(&folder));
        self.back_button.set_sensitive(!self.back.borrow().is_empty());
        self.forward_button.set_sensitive(!self.forward.borrow().is_empty());
    }

    fn go_back(&self) {
        let Some(previous) = self.back.borrow_mut().pop() else { return };
        if let Some(current) = self.current() {
            self.forward.borrow_mut().push(current);
        }
        self.show(previous);
    }

    fn go_forward(&self) {
        let Some(next) = self.forward.borrow_mut().pop() else { return };
        if let Some(current) = self.current() {
            self.back.borrow_mut().push(current);
        }
        self.show(next);
    }

    fn go_up(&self) {
        if let Some(parent) = self.current().and_then(|f| f.parent()) {
            self.go(parent);
        }
    }

    fn open(&self, info: &gio::FileInfo) {
        let Some(file) = file_of(info) else { return };
        if info.file_type() == gio::FileType::Directory {
            self.go(file);
        } else if let Err(err) = gio::AppInfo::launch_default_for_uri(&file.uri(), None::<&gio::AppLaunchContext>) {
            self.status.set_text(&format!("No app can open “{}”: {}", info.display_name(), err.message()));
        }
    }
}

fn pretty_path(path: &std::path::Path) -> String {
    let home = glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "Home".to_string(),
        Ok(rest) => format!("Home / {}", rest.display().to_string().replace('/', " / ")),
        Err(_) => path.display().to_string().replace('/', " / ").trim_start().to_string(),
    }
}

fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file")?.downcast().ok()
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Files")
        .flags(gio::ApplicationFlags::NON_UNIQUE | gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(|app| {
        new_window(app, gio::File::for_path(glib::home_dir()));
    });
    app.connect_open(|app, files, _| {
        for file in files {
            let folder = if file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) == gio::FileType::Directory {
                file.clone()
            } else {
                file.parent().unwrap_or_else(|| gio::File::for_path(glib::home_dir()))
            };
            new_window(app, folder);
        }
    });
    app.run()
}

fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(APP_CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn new_window(app: &gtk::Application, start: gio::File) {
    load_css();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .default_width(980)
        .default_height(620)
        .build();

    // Header: navigation, location, actions.
    let header = gtk::HeaderBar::new();
    let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
    back_button.set_tooltip_text(Some("Back  (Alt + ←)"));
    let forward_button = gtk::Button::from_icon_name("go-next-symbolic");
    forward_button.set_tooltip_text(Some("Forward  (Alt + →)"));
    let up_button = gtk::Button::from_icon_name("go-up-symbolic");
    up_button.set_tooltip_text(Some("Enclosing folder  (Alt + ↑)"));
    let nav = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav.add_css_class("linked");
    nav.append(&back_button);
    nav.append(&forward_button);
    nav.append(&up_button);
    header.pack_start(&nav);
    let path_label = gtk::Label::new(None);
    path_label.add_css_class("path-label");
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    header.set_title_widget(Some(&path_label));
    let new_folder = gtk::Button::from_icon_name("folder-new-symbolic");
    new_folder.set_tooltip_text(Some("New folder  (Ctrl + Shift + N)"));
    let hidden_button = gtk::ToggleButton::new();
    hidden_button.set_icon_name("view-reveal-symbolic");
    hidden_button.set_tooltip_text(Some("Show hidden files  (Ctrl + H)"));
    header.pack_end(&hidden_button);
    header.pack_end(&new_folder);
    window.set_titlebar(Some(&header));

    // The folder contents: folders first, then by name; hidden files filtered.
    let list = gtk::DirectoryList::new(Some("standard::*"), None::<&gio::File>);
    let show_hidden = Rc::new(RefCell::new(false));
    let filter = {
        let show_hidden = show_hidden.clone();
        gtk::CustomFilter::new(move |obj| {
            let info = obj.downcast_ref::<gio::FileInfo>().unwrap();
            *show_hidden.borrow() || !(info.is_hidden() || info.name().to_string_lossy().starts_with('.'))
        })
    };
    let filtered = gtk::FilterListModel::new(Some(list.clone()), Some(filter.clone()));
    let sorter = gtk::CustomSorter::new(|a, b| {
        let a = a.downcast_ref::<gio::FileInfo>().unwrap();
        let b = b.downcast_ref::<gio::FileInfo>().unwrap();
        let dir = |i: &gio::FileInfo| i.file_type() != gio::FileType::Directory;
        dir(a)
            .cmp(&dir(b))
            .then_with(|| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()))
            .into()
    });
    let sorted = gtk::SortListModel::new(Some(filtered), Some(sorter));
    let selection = gtk::SingleSelection::new(Some(sorted));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tile.add_css_class("file-tile");
        let image = gtk::Image::new();
        image.set_pixel_size(56);
        let label = gtk::Label::new(None);
        label.add_css_class("file-name");
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(2);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_justify(gtk::Justification::Center);
        label.set_max_width_chars(14);
        tile.append(&image);
        tile.append(&label);
        item.set_child(Some(&tile));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(info) = item.item().and_downcast::<gio::FileInfo>() else { return };
        let tile = item.child().and_downcast::<gtk::Box>().unwrap();
        let image = tile.first_child().and_downcast::<gtk::Image>().unwrap();
        let label = tile.last_child().and_downcast::<gtk::Label>().unwrap();
        match info.icon() {
            Some(icon) => image.set_from_gicon(&icon),
            None => image.set_icon_name(Some("text-x-generic")),
        }
        label.set_text(&info.display_name());
        tile.set_tooltip_text(Some(&info.display_name()));
    });

    let grid = gtk::GridView::new(Some(selection.clone()), Some(factory));
    grid.set_max_columns(12);
    grid.set_min_columns(2);
    grid.set_single_click_activate(false);
    let scroller = gtk::ScrolledWindow::builder().child(&grid).vexpand(true).hexpand(true).build();

    let empty = gtk::Label::new(Some("This folder is empty"));
    empty.add_css_class("empty");
    let content = gtk::Stack::new();
    content.add_named(&scroller, Some("files"));
    content.add_named(&empty, Some("empty"));

    let status = gtk::Label::new(None);
    status.add_css_class("status");
    status.set_xalign(0.0);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    {
        let status = status.clone();
        let content = content.clone();
        let list = list.clone();
        selection.connect_items_changed(move |model, _, _, _| {
            let n = model.n_items();
            if !list.is_loading() {
                content.set_visible_child_name(if n == 0 { "empty" } else { "files" });
            }
            status.set_text(&format!("{n} item{}", if n == 1 { "" } else { "s" }));
        });
    }
    {
        let content = content.clone();
        let selection = selection.clone();
        list.connect_loading_notify(move |list| {
            if !list.is_loading() {
                content.set_visible_child_name(if selection.n_items() == 0 { "empty" } else { "files" });
            }
        });
    }

    let main_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_area.append(&content);
    main_area.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    main_area.append(&status);

    let browser = Rc::new(Browser {
        window: window.clone(),
        list: list.clone(),
        filter,
        show_hidden: RefCell::new(false),
        back: RefCell::default(),
        forward: RefCell::default(),
        path_label,
        status: status.clone(),
        back_button: back_button.clone(),
        forward_button: forward_button.clone(),
    });
    // The filter reads the shared flag; keep both in step.
    {
        let flag = show_hidden.clone();
        let browser_weak = Rc::downgrade(&browser);
        hidden_button.connect_toggled(move |b| {
            *flag.borrow_mut() = b.is_active();
            if let Some(browser) = browser_weak.upgrade() {
                *browser.show_hidden.borrow_mut() = b.is_active();
                browser.filter.changed(gtk::FilterChange::Different);
            }
        });
    }

    // Sidebar with the usual places.
    let sidebar = places_sidebar(&browser);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&sidebar));
    paned.set_end_child(Some(&main_area));
    paned.set_position(190);
    paned.set_shrink_start_child(false);
    window.set_child(Some(&paned));

    // Wiring.
    {
        let browser = browser.clone();
        let selection = selection.clone();
        grid.connect_activate(move |_, position| {
            if let Some(info) = selection.item(position).and_downcast::<gio::FileInfo>() {
                browser.open(&info);
            }
        });
    }
    {
        let b = browser.clone();
        back_button.connect_clicked(move |_| b.go_back());
        let b = browser.clone();
        forward_button.connect_clicked(move |_| b.go_forward());
        let b = browser.clone();
        up_button.connect_clicked(move |_| b.go_up());
    }
    {
        let b = browser.clone();
        new_folder.connect_clicked(move |_| create_folder(&b));
    }
    add_context_menu(&grid, &selection, &browser);
    add_shortcuts(&window, &selection, &browser, &hidden_button);

    browser.show(start);
    window.present();
}

fn places_sidebar(browser: &Rc<Browser>) -> gtk::ScrolledWindow {
    let home = glib::home_dir();
    let mut places: Vec<(String, &str, PathBuf)> = vec![("Home".into(), "user-home-symbolic", home.clone())];
    for (dir, icon) in [
        (glib::UserDirectory::Desktop, "user-desktop-symbolic"),
        (glib::UserDirectory::Documents, "folder-documents-symbolic"),
        (glib::UserDirectory::Downloads, "folder-download-symbolic"),
        (glib::UserDirectory::Pictures, "folder-pictures-symbolic"),
        (glib::UserDirectory::Music, "folder-music-symbolic"),
        (glib::UserDirectory::Videos, "folder-videos-symbolic"),
    ] {
        if let Some(path) = glib::user_special_dir(dir).filter(|p| p.exists() && *p != home) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            places.push((name, icon, path));
        }
    }
    places.push(("Computer".into(), "drive-harddisk-symbolic", PathBuf::from("/")));

    let list = gtk::ListBox::new();
    list.add_css_class("places");
    list.add_css_class("navigation-sidebar");
    let title = gtk::Label::new(Some("PLACES"));
    title.add_css_class("places-title");
    title.set_xalign(0.0);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&title);
    column.append(&list);

    for (name, icon, _) in &places {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&gtk::Label::new(Some(name)));
        list.append(&row);
    }
    let browser = browser.clone();
    list.connect_row_activated(move |_, row| {
        if let Some((_, _, path)) = places.get(row.index() as usize) {
            browser.go(gio::File::for_path(path));
        }
    });
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&column)
        .build()
}

fn selected_info(selection: &gtk::SingleSelection) -> Option<gio::FileInfo> {
    selection.selected_item().and_downcast::<gio::FileInfo>()
}

fn create_folder(browser: &Rc<Browser>) {
    let Some(folder) = browser.current() else { return };
    ask_name(&browser.window, "New Folder", "Create", "untitled folder", {
        let browser = browser.clone();
        move |name| {
            let target = folder.child(&name);
            if let Err(err) = target.make_directory(gio::Cancellable::NONE) {
                browser.status.set_text(&format!("Could not create “{name}”: {}", err.message()));
            }
        }
    });
}

fn rename(browser: &Rc<Browser>, info: &gio::FileInfo) {
    let Some(file) = file_of(info) else { return };
    let current = info.display_name().to_string();
    ask_name(&browser.window, "Rename", "Rename", &current, {
        let browser = browser.clone();
        move |name| {
            if let Err(err) = file.set_display_name(&name, gio::Cancellable::NONE) {
                browser.status.set_text(&format!("Could not rename: {}", err.message()));
            }
        }
    });
}

fn trash(browser: &Rc<Browser>, info: &gio::FileInfo) {
    let Some(file) = file_of(info) else { return };
    match file.trash(gio::Cancellable::NONE) {
        Ok(()) => browser.status.set_text(&format!("Moved “{}” to the Trash", info.display_name())),
        Err(err) => browser.status.set_text(&format!("Could not move to Trash: {}", err.message())),
    }
}

/// A small dialog with one text field.
fn ask_name(parent: &gtk::ApplicationWindow, title: &str, action: &str, initial: &str, done: impl Fn(String) + 'static) {
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(360)
        .resizable(false)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    content.append(&entry);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label(action);
    ok.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&ok);
    content.append(&buttons);
    dialog.set_child(Some(&content));

    let done = Rc::new(done);
    let submit = {
        let dialog = dialog.clone();
        let entry = entry.clone();
        move || {
            let name = entry.text().trim().to_string();
            if !name.is_empty() && !name.contains('/') {
                done(name);
            }
            dialog.close();
        }
    };
    {
        let submit = submit.clone();
        ok.connect_clicked(move |_| submit());
    }
    entry.connect_activate(move |_| submit());
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

fn add_context_menu(grid: &gtk::GridView, selection: &gtk::SingleSelection, browser: &Rc<Browser>) {
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let selection = selection.clone();
    let browser = browser.clone();
    let grid_widget = grid.clone();
    click.connect_pressed(move |_, _, x, y| {
        let menu = gtk::Popover::new();
        menu.set_parent(&grid_widget);
        menu.set_has_arrow(false);
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let add = |label: &str, action: Box<dyn Fn()>| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            if let Some(child) = button.child().and_downcast::<gtk::Label>() {
                child.set_xalign(0.0);
            }
            let menu = menu.clone();
            button.connect_clicked(move |_| {
                menu.popdown();
                action();
            });
            column.append(&button);
        };

        if let Some(info) = selected_info(&selection) {
            let (b, i) = (browser.clone(), info.clone());
            add("Open", Box::new(move || b.open(&i)));
            let (b, i) = (browser.clone(), info.clone());
            add("Rename…", Box::new(move || rename(&b, &i)));
            let (b, i) = (browser.clone(), info.clone());
            add("Move to Trash", Box::new(move || trash(&b, &i)));
            let path = file_of(&info).and_then(|f| f.path());
            add(
                "Copy Path",
                Box::new(move || {
                    if let (Some(path), Some(display)) = (&path, gdk::Display::default()) {
                        display.clipboard().set_text(&path.to_string_lossy());
                    }
                }),
            );
            column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let b = browser.clone();
        add("New Folder…", Box::new(move || create_folder(&b)));
        let b = browser.clone();
        add(
            "Open Terminal Here",
            Box::new(move || {
                if let Some(path) = b.current().and_then(|f| f.path()) {
                    let _ = std::process::Command::new("foot").arg("--working-directory").arg(path).spawn();
                }
            }),
        );

        menu.set_child(Some(&column));
        menu.connect_closed(|menu| menu.unparent());
        menu.popup();
    });
    grid.add_controller(click);
}

fn add_shortcuts(window: &gtk::ApplicationWindow, selection: &gtk::SingleSelection, browser: &Rc<Browser>, hidden: &gtk::ToggleButton) {
    let keys = gtk::EventControllerKey::new();
    let selection = selection.clone();
    let browser = browser.clone();
    let hidden = hidden.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::BackSpace | gdk::Key::Left if alt || key == gdk::Key::BackSpace => browser.go_back(),
            gdk::Key::Right if alt => browser.go_forward(),
            gdk::Key::Up if alt => browser.go_up(),
            gdk::Key::h if ctrl => hidden.set_active(!hidden.is_active()),
            gdk::Key::N if ctrl && shift => create_folder(&browser),
            gdk::Key::Delete => {
                if let Some(info) = selected_info(&selection) {
                    trash(&browser, &info);
                }
            }
            gdk::Key::F2 => {
                if let Some(info) = selected_info(&selection) {
                    rename(&browser, &info);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
}
