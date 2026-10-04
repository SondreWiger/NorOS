//! Spotlight-style launcher: type to find an app or a system action, Enter to run it.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::spawn;

enum Target {
    App(gio::AppInfo),
    Command(&'static str),
}

struct Entry {
    title: String,
    subtitle: String,
    icon: Option<gio::Icon>,
    icon_name: &'static str,
    keywords: String,
    target: Target,
}

const ACTIONS: &[(&str, &str, &str, &str)] = &[
    ("Terminal", "Open a new terminal", "utilities-terminal-symbolic", "foot"),
    ("About NorOS", "Version and system information", "computer-symbolic", "noros-shell --about"),
    ("Restart", "Restart this computer", "system-reboot-symbolic", "systemctl reboot"),
    ("Shut Down", "Turn off this computer", "system-shutdown-symbolic", "systemctl poweroff"),
];

fn pid_file() -> PathBuf {
    glib::user_runtime_dir().join("noros-launcher.pid")
}

/// If a launcher is already open, close it and report true.
pub fn close_running() -> bool {
    let Ok(pid) = std::fs::read_to_string(pid_file()) else { return false };
    let Ok(pid) = pid.trim().parse::<u32>() else { return false };
    let alive = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .map(|cmd| cmd.contains("--launcher"))
        .unwrap_or(false);
    if alive {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        let _ = std::fs::remove_file(pid_file());
    }
    alive
}

fn collect_entries() -> Vec<Entry> {
    let mut entries: Vec<Entry> = ACTIONS
        .iter()
        .map(|(title, subtitle, icon, command)| Entry {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            icon: None,
            icon_name: icon,
            keywords: format!("{title} {subtitle}").to_lowercase(),
            target: Target::Command(command),
        })
        .collect();

    let mut apps: Vec<_> = gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect();
    apps.sort_by_key(|a| a.display_name().to_lowercase());
    for app in apps {
        let title = app.display_name().to_string();
        // Skip apps that duplicate a built-in action (e.g. the terminal).
        if entries.iter().any(|e| e.title.eq_ignore_ascii_case(&title)) {
            continue;
        }
        let subtitle = app.description().map(|d| d.to_string()).unwrap_or_default();
        let keywords = format!("{title} {subtitle} {}", app.executable().display()).to_lowercase();
        entries.push(Entry {
            title,
            subtitle,
            icon: app.icon(),
            icon_name: "application-x-executable-symbolic",
            keywords,
            target: Target::App(app),
        });
    }
    entries
}

fn run_entry(entry: &Entry) {
    match &entry.target {
        Target::App(app) => {
            if let Err(err) = app.launch(&[], None::<&gio::AppLaunchContext>) {
                eprintln!("noros-shell: failed to launch {}: {err}", entry.title);
            }
        }
        Target::Command(command) => spawn(command),
    }
}

fn make_row(entry: &Entry) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("launcher-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let image = match &entry.icon {
        Some(icon) => gtk::Image::from_gicon(icon),
        None => gtk::Image::from_icon_name(entry.icon_name),
    };
    image.set_pixel_size(28);
    line.append(&image);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title = gtk::Label::new(Some(&entry.title));
    title.set_xalign(0.0);
    title.add_css_class("launcher-title");
    text.append(&title);
    if !entry.subtitle.is_empty() {
        let subtitle = gtk::Label::new(Some(&entry.subtitle));
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.add_css_class("launcher-subtitle");
        text.append(&subtitle);
    }
    line.append(&text);
    row.set_child(Some(&line));
    row
}

pub fn build(app: &gtk::Application) {
    let _ = std::fs::write(pid_file(), std::process::id().to_string());

    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-launcher");
    window.init_layer_shell();
    window.set_namespace(Some("noros-launcher"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, 160);
    window.set_default_size(640, -1);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    panel.add_css_class("launcher");

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search apps and actions"));
    search.add_css_class("launcher-search");
    panel.append(&search);

    let list = gtk::ListBox::new();
    list.add_css_class("launcher-list");
    list.set_selection_mode(gtk::SelectionMode::Browse);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .max_content_height(380)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    panel.append(&scroller);

    let entries = Rc::new(collect_entries());
    // Indices into `entries` for the rows currently shown.
    let shown: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));

    let refill = {
        let entries = entries.clone();
        let shown = shown.clone();
        let list = list.clone();
        move |query: &str| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let query = query.trim().to_lowercase();
            let mut indices: Vec<usize> = (0..entries.len())
                .filter(|&i| query.is_empty() || entries[i].keywords.contains(&query))
                .collect();
            // Titles that start with the query come first.
            indices.sort_by_key(|&i| !entries[i].title.to_lowercase().starts_with(&query));
            indices.truncate(40);
            for &i in &indices {
                list.append(&make_row(&entries[i]));
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
            *shown.borrow_mut() = indices;
        }
    };
    refill("");

    let launch_selected = {
        let entries = entries.clone();
        let shown = shown.clone();
        let list = list.clone();
        let app = app.clone();
        move || {
            let row = list.selected_row().or_else(|| list.row_at_index(0));
            if let Some(row) = row {
                if let Some(&i) = shown.borrow().get(row.index() as usize) {
                    run_entry(&entries[i]);
                }
            }
            app.quit();
        }
    };

    search.connect_search_changed(move |entry| refill(&entry.text()));
    {
        let launch = launch_selected.clone();
        search.connect_activate(move |_| launch());
    }
    {
        let launch = launch_selected.clone();
        list.connect_row_activated(move |_, _| launch());
    }

    // Arrow keys move the selection while typing; Escape closes.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let list = list.clone();
        let app = app.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let step = match key {
                gdk::Key::Escape => {
                    app.quit();
                    return glib::Propagation::Stop;
                }
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let current = list.selected_row().map(|r| r.index()).unwrap_or(-1);
            if let Some(row) = list.row_at_index((current + step).max(0)) {
                list.select_row(Some(&row));
                row.grab_focus();
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    window.connect_close_request(|_| {
        let _ = std::fs::remove_file(pid_file());
        glib::Propagation::Proceed
    });
    app.connect_shutdown(|_| {
        let _ = std::fs::remove_file(pid_file());
    });

    window.set_child(Some(&panel));
    window.present();
    search.grab_focus();
}
