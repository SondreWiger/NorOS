//! NorOS System Settings.
//!
//! Every control edits ~/.config/noros/config.toml directly. The desktop watches
//! that file, so changes show up immediately; apps pick them up when reopened.

use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};
use lys::{BarPosition, ButtonStyle, Config, DockPosition, Mode, Side};

const APP_CSS: &str = r#"
.settings-page { padding: 24px 32px 32px; }
.page-title { font-size: 26px; font-weight: 800; margin-bottom: 4px; }
.page-subtitle { opacity: 0.65; margin-bottom: 18px; }
.group-title { font-weight: 700; font-size: 13px; opacity: 0.7; margin: 18px 4px 6px; }
.group { border-radius: 12px; background: alpha(currentColor, 0.05); border: 1px solid alpha(currentColor, 0.08); }
.setting-row { padding: 10px 14px; min-height: 36px; }
.setting-row + .setting-row { border-top: 1px solid alpha(currentColor, 0.07); }
.row-title { font-weight: 600; }
.row-subtitle { font-size: 12px; opacity: 0.6; }
.swatch { min-width: 28px; min-height: 28px; padding: 0; border-radius: 50%; border: 2px solid transparent; box-shadow: inset 0 0 0 1px alpha(black, 0.15); }
.swatch.selected { border-color: currentColor; }
.wallpaper-choice { padding: 6px 12px; border-radius: 8px; }
.wallpaper-choice.selected { background: @accent_bg_color; color: @accent_fg_color; }
.css-editor { font-family: monospace; font-size: 12px; padding: 10px; }
.shortcut-keys { font-family: monospace; font-weight: 700; padding: 2px 8px; border-radius: 6px; background: alpha(currentColor, 0.08); }
"#;

type State = Rc<RefCell<Config>>;

/// Change the config, save it and update the files GTK apps read.
fn update(state: &State, change: impl FnOnce(&mut Config)) {
    let mut config = state.borrow_mut();
    change(&mut config);
    if let Err(err) = config.save() {
        eprintln!("noros-settings: could not save settings: {err}");
    }
    if let Err(err) = lys::apply_to_apps(&config) {
        eprintln!("noros-settings: could not update app theme: {err}");
    }
    lys::apply_gsettings(&config);
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Settings")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build);
    app.run()
}

fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let mut css = APP_CSS.to_string();
    for (i, (_, hex)) in lys::ACCENTS.iter().enumerate() {
        css.push_str(&format!(".swatch-{i} {{ background: {hex}; }}\n"));
    }
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn build(app: &gtk::Application) {
    load_css();
    let state: State = Rc::new(RefCell::new(Config::load()));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Settings")
        .default_width(900)
        .default_height(640)
        .build();

    let header = gtk::HeaderBar::new();
    let reset = gtk::Button::with_label("Reset All");
    reset.set_tooltip_text(Some("Restore every setting to the NorOS defaults"));
    header.pack_end(&reset);
    window.set_titlebar(Some(&header));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    populate(&stack, &state, &window);

    {
        let stack = stack.clone();
        let state = state.clone();
        let window = window.clone();
        reset.connect_clicked(move |_| {
            update(&state, |c| *c = Config::default());
            // Rebuild the pages so every control shows the restored values.
            let visible = stack.visible_child_name();
            while let Some(child) = stack.first_child() {
                stack.remove(&child);
            }
            populate(&stack, &state, &window);
            if let Some(name) = visible {
                stack.set_visible_child_name(&name);
            }
        });
    }

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_width_request(200);

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.append(&sidebar);
    layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    stack.set_hexpand(true);
    layout.append(&stack);
    window.set_child(Some(&layout));
    window.present();
}

fn populate(stack: &gtk::Stack, state: &State, window: &gtk::ApplicationWindow) {
    stack.add_titled(&appearance_page(state), Some("appearance"), "Appearance");
    stack.add_titled(&desktop_page(state, window), Some("desktop"), "Desktop & Dock");
    stack.add_titled(&windows_page(state), Some("windows"), "Windows");
    stack.add_titled(&updates_page(window), Some("updates"), "Updates & Recovery");
    stack.add_titled(&css_page(), Some("css"), "Custom CSS");
    stack.add_titled(&shortcuts_page(), Some("shortcuts"), "Shortcuts");
    stack.add_titled(&about_page(), Some("about"), "About");
}

// ── Layout helpers ───────────────────────────────────────────────────

fn page(title: &str, subtitle: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("settings-page");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("page-title");
    t.set_xalign(0.0);
    content.append(&t);
    let s = gtk::Label::new(Some(subtitle));
    s.add_css_class("page-subtitle");
    s.set_xalign(0.0);
    s.set_wrap(true);
    content.append(&s);
    let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
    clamp.set_halign(gtk::Align::Fill);
    clamp.append(&content);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    (scroller, content)
}

fn group(parent: &gtk::Box, title: &str) -> gtk::Box {
    let label = gtk::Label::new(Some(title));
    label.add_css_class("group-title");
    label.set_xalign(0.0);
    parent.append(&label);
    let group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add_css_class("group");
    parent.append(&group);
    group
}

fn row(group: &gtk::Box, title: &str, subtitle: Option<&str>, control: &impl IsA<gtk::Widget>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("setting-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("row-title");
    t.set_xalign(0.0);
    text.append(&t);
    if let Some(subtitle) = subtitle {
        let s = gtk::Label::new(Some(subtitle));
        s.add_css_class("row-subtitle");
        s.set_xalign(0.0);
        s.set_wrap(true);
        text.append(&s);
    }
    row.append(&text);
    control.set_valign(gtk::Align::Center);
    row.append(control);
    group.append(&row);
}

fn switch(state: &State, value: bool, apply: impl Fn(&mut Config, bool) + 'static) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(value);
    let state = state.clone();
    switch.connect_active_notify(move |s| update(&state, |c| apply(c, s.is_active())));
    switch
}

fn dropdown(state: &State, options: &[&str], selected: u32, apply: impl Fn(&mut Config, u32) + 'static) -> gtk::DropDown {
    let dropdown = gtk::DropDown::from_strings(options);
    dropdown.set_selected(selected);
    let state = state.clone();
    dropdown.connect_selected_notify(move |d| update(&state, |c| apply(c, d.selected())));
    dropdown
}

fn slider(state: &State, min: f64, max: f64, value: f64, apply: impl Fn(&mut Config, f64) + 'static) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
    scale.set_value(value);
    scale.set_width_request(220);
    scale.set_draw_value(true);
    scale.set_digits(0);
    let state = state.clone();
    scale.connect_value_changed(move |s| update(&state, |c| apply(c, s.value())));
    scale
}

// ── Pages ────────────────────────────────────────────────────────────

fn appearance_page(state: &State) -> gtk::ScrolledWindow {
    let (scroller, content) = page("Appearance", "Colors and shapes for the whole desktop.");
    let config = state.borrow().clone();

    let g = group(&content, "Style");
    let mode = dropdown(state, &["Dark", "Light"], (config.appearance.mode == Mode::Light) as u32, |c, i| {
        c.appearance.mode = if i == 1 { Mode::Light } else { Mode::Dark };
    });
    row(&g, "Appearance", Some("Apps follow when they are next opened."), &mode);

    // Accent swatches plus a free color picker.
    let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let buttons: Rc<RefCell<Vec<gtk::Button>>> = Rc::default();
    for (i, (name, hex)) in lys::ACCENTS.iter().enumerate() {
        let b = gtk::Button::new();
        b.add_css_class("swatch");
        b.add_css_class(&format!("swatch-{i}"));
        b.set_tooltip_text(Some(name));
        if config.appearance.accent.eq_ignore_ascii_case(hex) {
            b.add_css_class("selected");
        }
        let state = state.clone();
        let all = buttons.clone();
        b.connect_clicked(move |me| {
            update(&state, |c| c.appearance.accent = hex.to_string());
            for other in all.borrow().iter() {
                other.remove_css_class("selected");
            }
            me.add_css_class("selected");
        });
        buttons.borrow_mut().push(b.clone());
        swatches.append(&b);
    }
    let picker = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    if let Some((r, g, b)) = lys::parse_hex(&config.appearance.accent) {
        picker.set_rgba(&gdk::RGBA::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0));
    }
    picker.set_tooltip_text(Some("Any color"));
    {
        let state = state.clone();
        let all = buttons.clone();
        picker.connect_rgba_notify(move |p| {
            let c = p.rgba();
            let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let hex = lys::to_hex((to_byte(c.red()), to_byte(c.green()), to_byte(c.blue())));
            if state.borrow().appearance.accent.eq_ignore_ascii_case(&hex) {
                return;
            }
            update(&state, |cfg| cfg.appearance.accent = hex);
            for other in all.borrow().iter() {
                other.remove_css_class("selected");
            }
        });
    }
    swatches.append(&picker);
    row(&g, "Accent color", None, &swatches);

    let g = group(&content, "Shape");
    let radius = slider(state, 0.0, 24.0, config.appearance.corner_radius as f64, |c, v| {
        c.appearance.corner_radius = v.round() as u32;
    });
    row(&g, "Corner radius", Some("0 for sharp, brutalist edges."), &radius);
    let opacity = slider(state, 30.0, 100.0, (config.appearance.panel_opacity * 100.0).round(), |c, v| {
        c.appearance.panel_opacity = v / 100.0;
    });
    row(&g, "Panel opacity", Some("How see-through the menu bar and dock are (%)."), &opacity);

    scroller
}

fn desktop_page(state: &State, window: &gtk::ApplicationWindow) -> gtk::ScrolledWindow {
    let (scroller, content) = page("Desktop & Dock", "Wallpaper, dock and menu bar. Changes apply instantly.");
    let config = state.borrow().clone();

    // Wallpaper
    let g = group(&content, "Wallpaper");
    let choices = gtk::FlowBox::new();
    choices.set_selection_mode(gtk::SelectionMode::None);
    choices.set_max_children_per_line(5);
    let buttons: Rc<RefCell<Vec<(String, gtk::Button)>>> = Rc::default();
    let mark = {
        let buttons = buttons.clone();
        move |selected: &str| {
            for (id, b) in buttons.borrow().iter() {
                if id == selected {
                    b.add_css_class("selected");
                } else {
                    b.remove_css_class("selected");
                }
            }
        }
    };
    for (id, name) in lys::WALLPAPERS {
        let b = gtk::Button::with_label(name);
        b.add_css_class("wallpaper-choice");
        let state = state.clone();
        let mark = mark.clone();
        b.connect_clicked(move |_| {
            update(&state, |c| c.desktop.wallpaper = id.to_string());
            mark(id);
        });
        buttons.borrow_mut().push((id.to_string(), b.clone()));
        choices.insert(&b, -1);
    }
    let custom = gtk::Button::with_label("Choose Image…");
    custom.add_css_class("wallpaper-choice");
    {
        let state = state.clone();
        let window = window.clone();
        let mark = mark.clone();
        custom.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder().title("Choose a Wallpaper").filters(&filters).build();
            let state = state.clone();
            let mark = mark.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(source) = file.path() else { return };
                // Keep a private copy so the wallpaper survives the original moving.
                let dir = lys::wallpaper_dir();
                let _ = std::fs::create_dir_all(&dir);
                let name = source.file_name().map(|n| n.to_owned()).unwrap_or_else(|| "wallpaper".into());
                let target = dir.join(name);
                if std::fs::copy(&source, &target).is_ok() {
                    let path = target.to_string_lossy().into_owned();
                    update(&state, |c| c.desktop.wallpaper = path);
                    mark("");
                }
            });
        });
    }
    choices.insert(&custom, -1);
    mark(&config.desktop.wallpaper);
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrap.add_css_class("setting-row");
    wrap.append(&choices);
    g.append(&wrap);

    // Dock
    let g = group(&content, "Dock");
    row(&g, "Show the dock", None, &switch(state, config.dock.visible, |c, v| c.dock.visible = v));
    let position = match config.dock.position {
        DockPosition::Bottom => 0,
        DockPosition::Left => 1,
        DockPosition::Right => 2,
    };
    let dock_pos = dropdown(state, &["Bottom", "Left", "Right"], position, |c, i| {
        c.dock.position = match i {
            1 => DockPosition::Left,
            2 => DockPosition::Right,
            _ => DockPosition::Bottom,
        };
    });
    row(&g, "Position on screen", None, &dock_pos);
    let size = slider(state, 32.0, 80.0, config.dock.icon_size as f64, |c, v| c.dock.icon_size = v.round() as u32);
    row(&g, "Icon size", None, &size);

    // Menu bar
    let g = group(&content, "Menu Bar");
    row(&g, "Show the menu bar", None, &switch(state, config.menu_bar.visible, |c, v| c.menu_bar.visible = v));
    let bar_pos = dropdown(state, &["Top", "Bottom"], (config.menu_bar.position == BarPosition::Bottom) as u32, |c, i| {
        c.menu_bar.position = if i == 1 { BarPosition::Bottom } else { BarPosition::Top };
    });
    row(&g, "Position on screen", None, &bar_pos);
    row(&g, "Show the date", None, &switch(state, config.menu_bar.show_date, |c, v| c.menu_bar.show_date = v));
    row(&g, "24-hour clock", None, &switch(state, config.menu_bar.clock_24h, |c, v| c.menu_bar.clock_24h = v));
    row(&g, "Show seconds", None, &switch(state, config.menu_bar.show_seconds, |c, v| c.menu_bar.show_seconds = v));

    scroller
}

fn windows_page(state: &State) -> gtk::ScrolledWindow {
    let (scroller, content) = page("Windows", "How window title bars look. Open apps update right away.");
    let config = state.borrow().clone();

    let g = group(&content, "Window Buttons");
    let side = dropdown(state, &["Left (like a Mac)", "Right (like Windows)"], (config.windows.buttons_side == Side::Right) as u32, |c, i| {
        c.windows.buttons_side = if i == 1 { Side::Right } else { Side::Left };
    });
    row(&g, "Close, minimize, maximize", None, &side);
    let style = dropdown(state, &["Traffic lights", "Plain icons"], (config.windows.button_style == ButtonStyle::Plain) as u32, |c, i| {
        c.windows.button_style = if i == 1 { ButtonStyle::Plain } else { ButtonStyle::Traffic };
    });
    row(&g, "Button style", None, &style);

    let g = group(&content, "Moving Windows");
    let hint = gtk::Label::new(Some("Super + drag"));
    hint.add_css_class("shortcut-keys");
    row(&g, "Move a window from anywhere inside it", Some("Hold the Super (⌘) key and drag."), &hint);

    scroller
}

fn css_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page(
        "Custom CSS",
        "Restyle anything in the desktop. This CSS loads last and overrides every other setting. \
         Saved to ~/.config/noros/theme.css and applied the moment you press Apply.",
    );

    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(&std::fs::read_to_string(lys::user_css_path()).unwrap_or_default());
    let view = gtk::TextView::with_buffer(&buffer);
    view.add_css_class("css-editor");
    view.set_monospace(true);
    view.set_vexpand(true);
    let frame = gtk::ScrolledWindow::builder().min_content_height(320).child(&view).build();
    frame.add_css_class("group");
    content.append(&frame);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_margin_top(12);
    let example = gtk::Button::with_label("Insert Example");
    let revert = gtk::Button::with_label("Revert");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    let status = gtk::Label::new(None);
    status.set_hexpand(true);
    status.set_xalign(0.0);
    status.add_css_class("row-subtitle");
    actions.append(&status);
    actions.append(&example);
    actions.append(&revert);
    actions.append(&apply);
    content.append(&actions);

    {
        let buffer = buffer.clone();
        example.connect_clicked(move |_| {
            buffer.insert_at_cursor(
                "\n/* A sharper, flatter dock */\n.dock { border-radius: 0; box-shadow: none; }\n.dock-item { border-radius: 4px; }\n\n/* A different menu bar font */\n.menubar { font-family: \"DejaVu Sans Mono\"; }\n",
            );
        });
    }
    {
        let buffer = buffer.clone();
        let status = status.clone();
        revert.connect_clicked(move |_| {
            buffer.set_text(&std::fs::read_to_string(lys::user_css_path()).unwrap_or_default());
            status.set_text("Reverted to the saved file.");
        });
    }
    apply.connect_clicked(move |_| {
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let path = lys::user_css_path();
        let _ = std::fs::create_dir_all(lys::noros_dir());
        match std::fs::write(&path, text.as_str()) {
            Ok(()) => status.set_text("Applied."),
            Err(err) => status.set_text(&format!("Could not save: {err}")),
        }
    });

    scroller
}

fn shortcuts_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page("Shortcuts", "Super is the ⌘ key on Mac keyboards and the Windows key on PCs.");
    let g = group(&content, "Everywhere");
    for (keys, what) in [
        ("Super + Space", "Search apps and actions"),
        ("Super + Enter", "Open a terminal"),
        ("Super + E", "Open Files"),
        ("Super + Q", "Close the window"),
        ("Super + M", "Maximize or restore the window"),
        ("Alt + Tab", "Switch windows"),
        ("Super + drag", "Move a window"),
        ("Ctrl + Alt + F1…F12", "Switch to a text console"),
    ] {
        let k = gtk::Label::new(Some(keys));
        k.add_css_class("shortcut-keys");
        row(&g, what, None, &k);
    }
    scroller
}

fn about_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page("About", "");
    let g = group(&content, "This System");
    let version = gtk::Label::new(Some(&format!("NorOS {}", env!("CARGO_PKG_VERSION"))));
    row(&g, "Version", None, &version);
    let open = gtk::Button::with_label("About NorOS");
    open.connect_clicked(|_| {
        let _ = std::process::Command::new("noros-shell").arg("--about").spawn();
    });
    row(&g, "System details", None, &open);
    let folder = gtk::Button::with_label("Open Settings Folder");
    folder.connect_clicked(|_| {
        let _ = std::fs::create_dir_all(lys::noros_dir());
        let _ = std::process::Command::new("noros-files").arg(lys::noros_dir()).spawn();
    });
    row(&g, "Settings are plain files", Some("~/.config/noros — edit them by hand if you like."), &folder);
    scroller
}

// ── Updates & Recovery ───────────────────────────────────────────────

/// Run `noros-update` with administrator rights, asking for the user's password first.
/// The password goes to sudo over a pipe and is never stored.
fn run_privileged(window: &gtk::ApplicationWindow, why: &str, args: Vec<String>, done: impl Fn(bool, String) + 'static) {
    let dialog = gtk::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Administrator Password")
        .default_width(380)
        .resizable(false)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    let text = gtk::Label::new(Some(&format!("{why}\nEnter your password to allow this.")));
    text.set_wrap(true);
    text.set_xalign(0.0);
    content.append(&text);
    let entry = gtk::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    content.append(&entry);
    let status = gtk::Label::new(None);
    status.add_css_class("row-subtitle");
    status.set_xalign(0.0);
    content.append(&status);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("Allow");
    ok.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&ok);
    content.append(&buttons);
    dialog.set_child(Some(&content));

    let done = Rc::new(done);
    let submit = {
        let dialog = dialog.clone();
        let entry = entry.clone();
        let status = status.clone();
        let ok = ok.clone();
        move || {
            let mut argv: Vec<String> = vec!["sudo".into(), "-S".into(), "-k".into(), "-p".into(), "".into(), "noros-update".into()];
            argv.extend(args.iter().cloned());
            let argv: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
            let process = match gio::Subprocess::newv(
                &argv,
                gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
            ) {
                Ok(p) => p,
                Err(err) => {
                    status.set_text(&err.to_string());
                    return;
                }
            };
            ok.set_sensitive(false);
            status.set_text("Working…");
            let input = format!("{}\n", entry.text());
            let dialog = dialog.clone();
            let status = status.clone();
            let ok = ok.clone();
            let done = done.clone();
            process.clone().communicate_utf8_async(Some(input), gio::Cancellable::NONE, move |result| {
                let output = result.ok().and_then(|(out, _)| out).map(|s| s.to_string()).unwrap_or_default();
                let success = process.is_successful();
                if !success && (output.contains("incorrect password") || output.contains("Sorry, try again")) {
                    ok.set_sensitive(true);
                    status.set_text("Wrong password. Try again.");
                    return;
                }
                dialog.close();
                done(success, output.trim().to_string());
            });
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
}

fn show_message(window: &gtk::ApplicationWindow, title: &str, detail: &str) {
    let alert = gtk::AlertDialog::builder().message(title).detail(detail).modal(true).build();
    alert.show(Some(window));
}

fn format_date(secs: u64) -> String {
    glib::DateTime::from_unix_local(secs as i64)
        .and_then(|d| d.format("%e %b %Y, %H:%M"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn updates_page(window: &gtk::ApplicationWindow) -> gtk::ScrolledWindow {
    let (scroller, content) = page(
        "Updates & Recovery",
        "NorOS takes a snapshot of the system before every change. If something breaks, roll back to how it was — or pick a snapshot from the boot menu to try it first.",
    );
    let status: serde_json::Value = std::process::Command::new("noros-update")
        .args(["status", "--json"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default();
    let live = status["live"].as_bool().unwrap_or(true);

    let g = group(&content, "This System");
    let version = gtk::Label::new(Some(&format!("NorOS {}", status["version"].as_str().unwrap_or(env!("CARGO_PKG_VERSION")))));
    row(&g, "Version", None, &version);
    if live {
        let install = gtk::Button::with_label("Install NorOS…");
        install.add_css_class("suggested-action");
        install.connect_clicked(|_| {
            let _ = std::process::Command::new("noros-installer").spawn();
        });
        row(&g, "Running from the live disk", Some("Nothing is saved here. Install NorOS to keep your work and get snapshots."), &install);
        return scroller;
    }
    if status["reboot_required"].as_bool().unwrap_or(false) {
        let restart = gtk::Button::with_label("Restart Now");
        restart.add_css_class("suggested-action");
        restart.connect_clicked(|_| {
            let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
        });
        row(&g, "A rollback is ready", Some("Restart to start the snapshot you chose."), &restart);
    }

    let g = group(&content, "Updates");
    let upgrade = gtk::Button::with_label("Update Now…");
    {
        let window = window.clone();
        upgrade.connect_clicked(move |_| {
            // Shown in a terminal so you can see exactly what is downloaded.
            let _ = std::process::Command::new("foot")
                .args(["--title", "NorOS Update", "sh", "-c", "sudo noros-update upgrade; echo; echo 'Press Enter to close.'; read _"])
                .spawn();
            let _ = &window;
        });
    }
    row(
        &g,
        "Install system updates",
        Some("Downloads updates from the Debian servers NorOS is built on. This only happens when you press the button; a snapshot is taken first."),
        &upgrade,
    );

    let g = group(&content, "Snapshots");
    let take = gtk::Button::with_label("Take Snapshot…");
    {
        let window = window.clone();
        take.connect_clicked(move |_| {
            let window2 = window.clone();
            run_privileged(&window, "Take a snapshot of the system.", vec!["snapshot".into(), "Manual snapshot".into()], move |ok, output| {
                show_message(&window2, if ok { "Snapshot taken" } else { "Couldn't take a snapshot" }, &output);
            });
        });
    }
    row(&g, "Take a snapshot now", Some("Before trying something risky."), &take);

    let snapshots = status["snapshots"].as_array().cloned().unwrap_or_default();
    if snapshots.is_empty() {
        let none = gtk::Label::new(Some("None yet"));
        row(&g, "No snapshots", None, &none);
    }
    for snapshot in snapshots {
        let id = snapshot["id"].as_u64().unwrap_or(0);
        let description = snapshot["description"].as_str().unwrap_or("").to_string();
        let when = format_date(snapshot["created"].as_u64().unwrap_or(0));
        let auto = if snapshot["automatic"].as_bool().unwrap_or(false) { " · automatic" } else { "" };
        let rollback = gtk::Button::with_label("Roll Back…");
        let window = window.clone();
        let title = format!("#{id}  {description}");
        rollback.connect_clicked(move |_| {
            let window2 = window.clone();
            run_privileged(
                &window,
                &format!("Roll the system back to snapshot #{id}. Your current system is kept as a snapshot, and your files in Home are not touched."),
                vec!["rollback".into(), id.to_string()],
                move |ok, output| {
                    show_message(&window2, if ok { "Restart to finish" } else { "Rollback failed" }, &output);
                },
            );
        });
        row(&g, &title, Some(&format!("{when}{auto}")), &rollback);
    }
    scroller
}
