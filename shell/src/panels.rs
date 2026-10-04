//! The menu bar and the dock, laid out according to the user's config.

use gtk::{gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use lys::{BarPosition, Config, DockPosition};

use crate::spawn;

const TERMINAL: &str = "foot";

pub fn build_menu_bar(app: &gtk::Application, config: &Config) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-panel");
    window.init_layer_shell();
    window.set_namespace(Some("noros-menubar"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    let edge = match config.menu_bar.position {
        BarPosition::Top => Edge::Top,
        BarPosition::Bottom => Edge::Bottom,
    };
    for e in [edge, Edge::Left, Edge::Right] {
        window.set_anchor(e, true);
    }
    window.auto_exclusive_zone_enable();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("menubar");
    if config.menu_bar.position == BarPosition::Bottom {
        bar.add_css_class("at-bottom");
    }

    // Left: the NorOS menu, then the name of what you're looking at.
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    left.append(&system_menu(config.menu_bar.position));
    let title = gtk::Label::new(Some("Desktop"));
    title.add_css_class("menubar-title");
    left.append(&title);
    bar.set_start_widget(Some(&left));

    // Right: search and the clock.
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let search = gtk::Button::from_icon_name("system-search-symbolic");
    search.add_css_class("menubar-item");
    search.set_tooltip_text(Some("Search  (Super + Space)"));
    search.connect_clicked(|_| spawn("noros-shell --launcher"));
    right.append(&search);

    let clock = gtk::Label::new(None);
    clock.add_css_class("menubar-clock");
    let format = clock_format(&config.menu_bar);
    update_clock(&clock, &format);
    let weak = clock.downgrade();
    glib::timeout_add_seconds_local(1, move || {
        // Stop ticking once the bar is rebuilt and this label is gone.
        let Some(clock) = weak.upgrade() else { return glib::ControlFlow::Break };
        update_clock(&clock, &format);
        glib::ControlFlow::Continue
    });
    right.append(&clock);
    bar.set_end_widget(Some(&right));

    window.set_child(Some(&bar));
    window.present();
    window
}

/// (date format, time format) for the clock.
fn clock_format(bar: &lys::MenuBar) -> (Option<&'static str>, &'static str) {
    let time = match (bar.clock_24h, bar.show_seconds) {
        (true, false) => "%H:%M",
        (true, true) => "%H:%M:%S",
        (false, false) => "%l:%M %p",
        (false, true) => "%l:%M:%S %p",
    };
    (bar.show_date.then_some("%a %e %b"), time)
}

fn update_clock(label: &gtk::Label, (date, time): &(Option<&'static str>, &'static str)) {
    let Ok(now) = glib::DateTime::now_local() else { return };
    // %e and %l pad single digits with a space; squeeze those out.
    let tidy = |fmt: &str| {
        now.format(fmt)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    let text = match date {
        Some(date) => format!("{}     {}", tidy(date), tidy(time)),
        None => tidy(time),
    };
    label.set_text(&text);
}

/// The ◆ NorOS menu: settings, about, and power controls.
fn system_menu(position: BarPosition) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.add_css_class("menubar-logo");
    button.set_child(Some(&gtk::Label::new(Some("◆ NorOS"))));
    if position == BarPosition::Bottom {
        button.set_direction(gtk::ArrowType::Up);
    }

    let popover = gtk::Popover::new();
    popover.add_css_class("noros-menu");
    popover.set_has_arrow(false);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let entries: [(&str, &'static str); 8] = [
        ("About NorOS", "noros-shell --about"),
        ("System Settings…", "noros-settings"),
        ("—", ""),
        ("Files", "noros-files"),
        ("Terminal", TERMINAL),
        ("—", ""),
        ("Restart…", "systemctl reboot"),
        ("Shut Down…", "systemctl poweroff"),
    ];
    for (label, command) in entries {
        if label == "—" {
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            continue;
        }
        let item = gtk::Button::with_label(label);
        item.add_css_class("menu-item");
        if let Some(child) = item.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
        }
        let popover = popover.clone();
        item.connect_clicked(move |_| {
            popover.popdown();
            spawn(command);
        });
        list.append(&item);
    }
    popover.set_child(Some(&list));
    button.set_popover(Some(&popover));
    button
}

/// What a dock entry looks like and does.
struct DockEntry {
    name: String,
    tile: &'static str,
    icon: Option<&'static str>,
    gicon: Option<gio::Icon>,
    launch: Launch,
}

enum Launch {
    Command(&'static str),
    App(gio::AppInfo),
}

/// NorOS's own apps get colored tiles; anything else shows its normal icon.
fn dock_entry(id: &str) -> Option<DockEntry> {
    let known: Option<(&str, &'static str, &'static str)> = match id {
        "noros-search" => Some(("Search", "tile-search", "system-search-symbolic")),
        "no.noros.Files.desktop" => Some(("Files", "tile-files", "folder-symbolic")),
        "foot.desktop" => Some(("Terminal", "tile-terminal", "utilities-terminal-symbolic")),
        "no.noros.Text.desktop" => Some(("Text Editor", "tile-text", "accessories-text-editor-symbolic")),
        "no.noros.Settings.desktop" => Some(("Settings", "tile-settings", "emblem-system-symbolic")),
        "noros-about" => Some(("About NorOS", "tile-about", "computer-symbolic")),
        _ => None,
    };
    let launch = match id {
        "noros-search" => Launch::Command("noros-shell --launcher"),
        "noros-about" => Launch::Command("noros-shell --about"),
        _ => Launch::App(gio::AppInfo::all().into_iter().find(|a| a.id().as_deref() == Some(id))?),
    };
    Some(match known {
        Some((name, tile, icon)) => DockEntry {
            name: name.to_string(),
            tile,
            icon: Some(icon),
            gicon: None,
            launch,
        },
        None => {
            let Launch::App(info) = &launch else { return None };
            DockEntry {
                name: info.display_name().to_string(),
                tile: "tile-app",
                icon: None,
                gicon: info.icon(),
                launch,
            }
        }
    })
}

pub fn build_dock(app: &gtk::Application, config: &Config) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-panel");
    window.init_layer_shell();
    window.set_namespace(Some("noros-dock"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);

    let (edge, orientation, side_class) = match config.dock.position {
        DockPosition::Bottom => (Edge::Bottom, gtk::Orientation::Horizontal, "horizontal"),
        DockPosition::Left => (Edge::Left, gtk::Orientation::Vertical, "left"),
        DockPosition::Right => (Edge::Right, gtk::Orientation::Vertical, "right"),
    };
    window.set_anchor(edge, true);
    window.set_margin(edge, 10);
    window.auto_exclusive_zone_enable();

    let dock = gtk::Box::new(orientation, 10);
    dock.add_css_class("dock");
    dock.add_css_class(side_class);
    if orientation == gtk::Orientation::Vertical {
        dock.add_css_class("vertical");
    }

    let icon_px = (config.dock.icon_size as f64 * 0.5).round() as i32;
    for id in &config.dock.apps {
        let Some(entry) = dock_entry(id) else { continue };
        let button = gtk::Button::new();
        button.add_css_class("dock-item");
        button.add_css_class(entry.tile);
        button.set_tooltip_text(Some(&entry.name));
        let image = match (&entry.icon, &entry.gicon) {
            (Some(name), _) => gtk::Image::from_icon_name(name),
            (None, Some(icon)) => gtk::Image::from_gicon(icon),
            (None, None) => gtk::Image::from_icon_name("application-x-executable-symbolic"),
        };
        image.set_pixel_size(if entry.icon.is_some() { icon_px } else { (icon_px as f64 * 1.5) as i32 });
        button.set_child(Some(&image));
        let launch = entry.launch;
        button.connect_clicked(move |_| match &launch {
            Launch::Command(command) => spawn(command),
            Launch::App(info) => {
                if let Err(err) = info.launch(&[], None::<&gio::AppLaunchContext>) {
                    eprintln!("noros-shell: failed to launch: {err}");
                }
            }
        });
        dock.append(&button);
    }

    window.set_child(Some(&dock));
    window.present();
    window
}
