//! noros-shell — the NorOS desktop: wallpaper, menu bar, dock, launcher and About window.
//!
//! Every visible piece is plain GTK styled by CSS. The default theme ships at
//! /usr/share/noros/theme/noros.css and anything in ~/.config/noros/theme.css wins.

mod about;
mod launcher;
mod panels;
mod wallpaper;

use gtk::{gdk, gio, glib, prelude::*};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASE_NAME: &str = "Første lys";

const DEFAULT_CSS: &str = include_str!("../theme/noros.css");

#[derive(Clone, Copy)]
enum Mode {
    Desktop,
    Launcher,
    About,
}

fn main() -> glib::ExitCode {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("--launcher") => Mode::Launcher,
        Some("--about") => Mode::About,
        Some("--version") => {
            println!("noros-shell {VERSION} ({RELEASE_NAME})");
            return glib::ExitCode::SUCCESS;
        }
        _ => Mode::Desktop,
    };

    if let Mode::Launcher = mode {
        // A second press of the launcher shortcut closes the open launcher.
        if launcher::close_running() {
            return glib::ExitCode::SUCCESS;
        }
    }

    let app_id = match mode {
        Mode::Desktop => "no.noros.Shell",
        Mode::Launcher => "no.noros.Launcher",
        Mode::About => "no.noros.About",
    };
    // NON_UNIQUE: no D-Bus round trip at startup, every instance is independent.
    let app = gtk::Application::builder()
        .application_id(app_id)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        load_theme();
        match mode {
            Mode::Desktop => {
                wallpaper::build(app);
                panels::build_menu_bar(app);
                panels::build_dock(app);
            }
            Mode::Launcher => launcher::build(app),
            Mode::About => about::build(app),
        }
    });

    // Don't let GTK parse our own flags.
    app.run_with_args::<&str>(&[])
}

/// Built-in theme first, then the system theme file, then the user's own CSS on top.
fn load_theme() {
    let Some(display) = gdk::Display::default() else { return };

    let builtin = gtk::CssProvider::new();
    builtin.load_from_string(DEFAULT_CSS);
    gtk::style_context_add_provider_for_display(&display, &builtin, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let system = std::path::Path::new("/usr/share/noros/theme/noros.css");
    if system.exists() {
        let provider = gtk::CssProvider::new();
        provider.load_from_path(system);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }

    let user = glib::user_config_dir().join("noros").join("theme.css");
    if user.exists() {
        let provider = gtk::CssProvider::new();
        provider.load_from_path(&user);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
}

/// Start a command detached from the shell.
pub fn spawn(command: &str) {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else { return };
    if let Err(err) = std::process::Command::new(program).args(parts).spawn() {
        eprintln!("noros-shell: failed to start {command}: {err}");
    }
}
