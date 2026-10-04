//! NorOS Text Editor: plain text, fast, no surprises.

use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};

const APP_CSS: &str = r#"
.editor { font-size: 13px; padding: 16px 20px; }
.editor text { background: transparent; }
.status { font-size: 12px; opacity: 0.6; padding: 4px 12px; }
"#;

struct Editor {
    window: gtk::ApplicationWindow,
    buffer: gtk::TextBuffer,
    file: RefCell<Option<gio::File>>,
    status: gtk::Label,
}

impl Editor {
    fn name(&self) -> String {
        self.file
            .borrow()
            .as_ref()
            .and_then(|f| f.basename())
            .map(|b| b.display().to_string())
            .unwrap_or_else(|| "Untitled".into())
    }

    fn refresh_title(&self) {
        let marker = if self.buffer.is_modified() { "• " } else { "" };
        self.window.set_title(Some(&format!("{marker}{}", self.name())));
    }

    fn load(self: &Rc<Self>, file: gio::File) {
        let this = self.clone();
        let source = file.clone();
        source.load_contents_async(gio::Cancellable::NONE, move |result| match result {
            Ok((bytes, _)) => {
                let text = String::from_utf8_lossy(&bytes);
                this.buffer.set_text(&text);
                this.buffer.place_cursor(&this.buffer.start_iter());
                this.buffer.set_modified(false);
                *this.file.borrow_mut() = Some(file);
                this.refresh_title();
                this.status.set_text("Opened.");
            }
            Err(err) => this.status.set_text(&format!("Could not open: {}", err.message())),
        });
    }

    /// Save to the current file, or ask where to save. `then` runs after a successful save.
    fn save(self: &Rc<Self>, then: Option<Box<dyn Fn()>>) {
        let current = self.file.borrow().clone();
        match current {
            Some(file) => self.write(file, then),
            None => self.save_as(then),
        }
    }

    fn save_as(self: &Rc<Self>, then: Option<Box<dyn Fn()>>) {
        let dialog = gtk::FileDialog::builder().title("Save As").initial_name(self.name()).build();
        let this = self.clone();
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                this.write(file, then);
            }
        });
    }

    fn write(self: &Rc<Self>, file: gio::File, then: Option<Box<dyn Fn()>>) {
        let text = self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), false);
        match file.replace_contents(text.as_bytes(), None, false, gio::FileCreateFlags::NONE, gio::Cancellable::NONE) {
            Ok(_) => {
                *self.file.borrow_mut() = Some(file);
                self.buffer.set_modified(false);
                self.refresh_title();
                self.status.set_text("Saved.");
                if let Some(then) = then {
                    then();
                }
            }
            Err(err) => self.status.set_text(&format!("Could not save: {}", err.message())),
        }
    }

    fn open_dialog(self: &Rc<Self>, app: &gtk::Application) {
        let dialog = gtk::FileDialog::builder().title("Open").build();
        let this = self.clone();
        let app = app.clone();
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            // Reuse this window if it's an untouched empty document.
            if this.file.borrow().is_none() && !this.buffer.is_modified() && this.buffer.char_count() == 0 {
                this.load(file);
            } else {
                new_window(&app, Some(file));
            }
        });
    }

    /// Ask before throwing away unsaved work. `proceed` runs once it's safe.
    fn confirm_discard(self: &Rc<Self>, proceed: impl Fn() + 'static) {
        if !self.buffer.is_modified() {
            proceed();
            return;
        }
        let alert = gtk::AlertDialog::builder()
            .message(format!("Save changes to “{}”?", self.name()))
            .detail("Your changes will be lost if you don't save them.")
            .buttons(["Cancel", "Don't Save", "Save"])
            .cancel_button(0)
            .default_button(2)
            .modal(true)
            .build();
        let this = self.clone();
        let proceed = Rc::new(proceed);
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| match choice {
            Ok(1) => proceed(),
            Ok(2) => {
                let proceed = proceed.clone();
                this.save(Some(Box::new(move || proceed())));
            }
            _ => {}
        });
    }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Text")
        .flags(gio::ApplicationFlags::NON_UNIQUE | gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(|app| new_window(app, None));
    app.connect_open(|app, files, _| {
        for file in files {
            new_window(app, Some(file.clone()));
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

fn new_window(app: &gtk::Application, file: Option<gio::File>) {
    load_css();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .default_width(820)
        .default_height(620)
        .build();

    let header = gtk::HeaderBar::new();
    let open = gtk::Button::with_label("Open");
    open.set_tooltip_text(Some("Open a file  (Ctrl + O)"));
    let new = gtk::Button::from_icon_name("document-new-symbolic");
    new.set_tooltip_text(Some("New document  (Ctrl + N)"));
    header.pack_start(&open);
    header.pack_start(&new);
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    save.set_tooltip_text(Some("Save  (Ctrl + S)"));
    let options = gtk::MenuButton::new();
    options.set_icon_name("open-menu-symbolic");
    header.pack_end(&options);
    header.pack_end(&save);
    window.set_titlebar(Some(&header));

    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::with_buffer(&buffer);
    view.add_css_class("editor");
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_vexpand(true);
    let scroller = gtk::ScrolledWindow::builder().child(&view).build();

    let status = gtk::Label::new(None);
    status.add_css_class("status");
    status.set_xalign(0.0);
    let position = gtk::Label::new(Some("Line 1, Column 1"));
    position.add_css_class("status");
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status.set_hexpand(true);
    bottom.append(&status);
    bottom.append(&position);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&scroller);
    layout.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    layout.append(&bottom);
    window.set_child(Some(&layout));

    let editor = Rc::new(Editor {
        window: window.clone(),
        buffer: buffer.clone(),
        file: RefCell::new(None),
        status,
    });
    editor.refresh_title();

    // Options menu: wrapping and font.
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);
    let wrap = gtk::CheckButton::with_label("Wrap long lines");
    wrap.set_active(true);
    let mono = gtk::CheckButton::with_label("Monospace font");
    mono.set_active(true);
    let save_as = gtk::Button::with_label("Save As…");
    save_as.add_css_class("flat");
    menu_box.append(&wrap);
    menu_box.append(&mono);
    menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    menu_box.append(&save_as);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&menu_box));
    options.set_popover(Some(&popover));
    {
        let wrap_view = view.clone();
        wrap.connect_toggled(move |c| {
            wrap_view.set_wrap_mode(if c.is_active() { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });
        });
        let mono_view = view.clone();
        mono.connect_toggled(move |c| mono_view.set_monospace(c.is_active()));
    }

    // Wiring.
    {
        let editor = editor.clone();
        buffer.connect_modified_changed(move |_| editor.refresh_title());
    }
    {
        let position = position.clone();
        buffer.connect_mark_set(move |buffer, iter, mark| {
            if mark.name().as_deref() == Some("insert") {
                let _ = buffer;
                position.set_text(&format!("Line {}, Column {}", iter.line() + 1, iter.line_offset() + 1));
            }
        });
    }
    {
        let editor = editor.clone();
        save.connect_clicked(move |_| editor.save(None));
    }
    {
        let editor = editor.clone();
        let popover = popover.clone();
        save_as.connect_clicked(move |_| {
            popover.popdown();
            editor.save_as(None);
        });
    }
    {
        let editor = editor.clone();
        let app = app.clone();
        open.connect_clicked(move |_| editor.open_dialog(&app));
    }
    {
        let app = app.clone();
        new.connect_clicked(move |_| new_window(&app, None));
    }
    {
        // Closing with unsaved changes asks first.
        let editor = editor.clone();
        window.connect_close_request(move |window| {
            if !editor.buffer.is_modified() {
                return glib::Propagation::Proceed;
            }
            let window = window.clone();
            let editor2 = editor.clone();
            editor.confirm_discard(move || {
                editor2.buffer.set_modified(false);
                window.close();
            });
            glib::Propagation::Stop
        });
    }

    let keys = gtk::EventControllerKey::new();
    {
        let editor = editor.clone();
        let app = app.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if !mods.contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::s if !shift => editor.save(None),
                gdk::Key::S if shift => editor.save_as(None),
                gdk::Key::o => editor.open_dialog(&app),
                gdk::Key::n => new_window(&app, None),
                gdk::Key::w => editor.window.close(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    window.add_controller(keys);

    if let Some(file) = file {
        editor.load(file);
    }
    window.present();
    view.grab_focus();
}
