//! noros-shell — the NorOS desktop: wallpaper, menu bar, dock, launcher and About window.
//!
//! Every visible piece is plain GTK styled by CSS: the base theme, then the CSS Lys
//! generates from ~/.config/noros/config.toml, then the user's own theme.css.
//! The desktop watches those files and rebuilds itself when they change.

mod about;
mod desktop;
mod guard;
mod launcher;
mod panels;
mod wallpaper;

use gtk::{gdk, gio, glib, prelude::*};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASE_NAME: &str = "Bro";

const BASE_CSS: &str = include_str!("../theme/noros.css");

#[derive(Clone, Copy)]
enum Mode {
    Desktop,
    Launcher,
    About,
    /// `noros-shell --message TITLE TEXT`: a small notice window.
    Message,
}

fn main() -> glib::ExitCode {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("--launcher") => Mode::Launcher,
        Some("--about") => Mode::About,
        Some("--message") => Mode::Message,
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
        Mode::Message => "no.noros.Message",
    };
    // NON_UNIQUE: no D-Bus round trip at startup, every instance is independent.
    let app = gtk::Application::builder()
        .application_id(app_id)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let theme = Theme::install();
        match mode {
            Mode::Desktop => desktop::start(app, theme),
            Mode::Launcher => launcher::build(app),
            Mode::About => about::build(app),
            Mode::Message => {
                let args: Vec<String> = std::env::args().skip(2).collect();
                let window = gtk::ApplicationWindow::builder().application(app).default_width(440).resizable(false).build();
                window.set_title(Some(args.first().map(String::as_str).unwrap_or("NorOS")));
                let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
                content.set_margin_top(24);
                content.set_margin_bottom(20);
                content.set_margin_start(24);
                content.set_margin_end(24);
                let title = gtk::Label::new(args.first().map(String::as_str));
                title.add_css_class("about-version");
                title.set_markup(&format!("<b><big>{}</big></b>", glib::markup_escape_text(args.first().map(String::as_str).unwrap_or(""))));
                title.set_xalign(0.0);
                let body = gtk::Label::new(args.get(1).map(String::as_str));
                body.set_wrap(true);
                body.set_xalign(0.0);
                body.set_max_width_chars(52);
                let ok = gtk::Button::with_label("OK");
                ok.add_css_class("suggested-action");
                ok.set_halign(gtk::Align::End);
                let w = window.clone();
                ok.connect_clicked(move |_| w.close());
                content.append(&title);
                content.append(&body);
                content.append(&ok);
                window.set_child(Some(&content));
                window.present();
            }
        }
    });

    // Don't let GTK parse our own flags.
    app.run_with_args::<&str>(&[])
}

/// The three CSS layers. The generated and user layers can be reloaded at any time.
pub struct Theme {
    generated: gtk::CssProvider,
    user: gtk::CssProvider,
}

impl Theme {
    fn install() -> Self {
        let display = gdk::Display::default().expect("no display");
        let add = |provider: &gtk::CssProvider, priority: u32| {
            gtk::style_context_add_provider_for_display(&display, provider, priority);
        };

        let base = gtk::CssProvider::new();
        base.load_from_string(BASE_CSS);
        add(&base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let theme = Self {
            generated: gtk::CssProvider::new(),
            user: gtk::CssProvider::new(),
        };
        add(&theme.generated, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        add(&theme.user, gtk::STYLE_PROVIDER_PRIORITY_USER);
        theme.reload(&lys::Config::load());
        theme
    }

    pub fn reload(&self, config: &lys::Config) {
        self.generated.load_from_string(&lys::shell_css(config));
        let user_css = std::fs::read_to_string(lys::user_css_path()).unwrap_or_default();
        self.user.load_from_string(&user_css);
    }
}

/// Start a command detached from the shell.
pub fn spawn(command: &str) {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else { return };
    lys::record_activity(program);
    if let Err(err) = std::process::Command::new(program).args(parts).spawn() {
        eprintln!("noros-shell: failed to start {command}: {err}");
    }
}
