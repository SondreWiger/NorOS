//! The menu bar along the top and the dock along the bottom.

use gtk::{glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::spawn;

const TERMINAL: &str = "foot";

pub fn build_menu_bar(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-panel");
    window.init_layer_shell();
    window.set_namespace(Some("noros-menubar"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.auto_exclusive_zone_enable();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("menubar");

    // Left: the NorOS menu, then the name of what you're looking at.
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    left.append(&system_menu());
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
    update_clock(&clock);
    let clock_ref = clock.clone();
    glib::timeout_add_seconds_local(1, move || {
        update_clock(&clock_ref);
        glib::ControlFlow::Continue
    });
    right.append(&clock);
    bar.set_end_widget(Some(&right));

    window.set_child(Some(&bar));
    window.present();
}

fn update_clock(label: &gtk::Label) {
    if let Ok(now) = glib::DateTime::now_local() {
        if let Ok(text) = now.format("%a %e %b   %H:%M") {
            let text = text.split_whitespace().collect::<Vec<_>>();
            // "Sat 4 Oct" + wide gap + "18:42"
            let (date, time) = text.split_at(text.len().saturating_sub(1));
            label.set_text(&format!("{}     {}", date.join(" "), time.join("")));
        }
    }
}

/// The ◆ NorOS menu: about, and power controls.
fn system_menu() -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.add_css_class("menubar-logo");
    let logo = gtk::Label::new(Some("◆ NorOS"));
    button.set_child(Some(&logo));

    let popover = gtk::Popover::new();
    popover.add_css_class("noros-menu");
    popover.set_has_arrow(false);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let entries: [(&str, Option<&'static str>); 6] = [
        ("About NorOS", Some("noros-shell --about")),
        ("Open Terminal", Some(TERMINAL)),
        ("Settings…  (coming in 0.2)", None),
        ("—", None),
        ("Restart…", Some("systemctl reboot")),
        ("Shut Down…", Some("systemctl poweroff")),
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
        match command {
            Some(command) => {
                let popover = popover.clone();
                item.connect_clicked(move |_| {
                    popover.popdown();
                    spawn(command);
                });
            }
            None => item.set_sensitive(false),
        }
        list.append(&item);
    }
    popover.set_child(Some(&list));
    button.set_popover(Some(&popover));
    button
}

struct DockItem {
    name: &'static str,
    icon: &'static str,
    tile: &'static str,
    command: &'static str,
}

const DOCK_ITEMS: &[DockItem] = &[
    DockItem { name: "Search", icon: "system-search-symbolic", tile: "tile-search", command: "noros-shell --launcher" },
    DockItem { name: "Terminal", icon: "utilities-terminal-symbolic", tile: "tile-terminal", command: TERMINAL },
    DockItem { name: "About NorOS", icon: "computer-symbolic", tile: "tile-about", command: "noros-shell --about" },
];

pub fn build_dock(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-panel");
    window.init_layer_shell();
    window.set_namespace(Some("noros-dock"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Bottom, 10);
    window.auto_exclusive_zone_enable();

    let dock = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    dock.add_css_class("dock");
    for item in DOCK_ITEMS {
        let button = gtk::Button::new();
        button.add_css_class("dock-item");
        button.add_css_class(item.tile);
        button.set_tooltip_text(Some(item.name));
        let image = gtk::Image::from_icon_name(item.icon);
        image.set_pixel_size(26);
        button.set_child(Some(&image));
        let command = item.command;
        button.connect_clicked(move |_| spawn(command));
        dock.append(&button);
    }

    window.set_child(Some(&dock));
    window.present();
}
