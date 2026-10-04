//! The running desktop: builds wallpaper and panels from the config and rebuilds
//! them whenever the config, the user's CSS or the wallpaper folder changes.

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{gio, glib, prelude::*};

use crate::{Theme, panels, wallpaper};

struct Desktop {
    app: gtk::Application,
    theme: Theme,
    config: RefCell<lys::Config>,
    windows: RefCell<Vec<gtk::ApplicationWindow>>,
    reload_pending: RefCell<bool>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
    _hold: gio::ApplicationHoldGuard,
}

pub fn start(app: &gtk::Application, theme: Theme) {
    let config = lys::Config::load();
    // Keep GTK apps in step with the desktop (accent, light/dark, window buttons).
    if let Err(err) = lys::apply_to_apps(&config) {
        eprintln!("noros-shell: could not write app theme: {err}");
    }

    let desktop = Rc::new(Desktop {
        app: app.clone(),
        theme,
        config: RefCell::new(config),
        windows: RefCell::new(Vec::new()),
        reload_pending: RefCell::new(false),
        _monitor: RefCell::new(None),
        // Rebuilding briefly closes every window; don't let the app quit in between.
        _hold: app.hold(),
    });
    desktop.build();
    watch(&desktop);
}

impl Desktop {
    fn build(&self) {
        let config = self.config.borrow();
        let mut windows = self.windows.borrow_mut();
        windows.push(wallpaper::build(&self.app, &config));
        if config.menu_bar.visible {
            windows.push(panels::build_menu_bar(&self.app, &config));
        }
        if config.dock.visible {
            windows.push(panels::build_dock(&self.app, &config));
        }
    }

    fn reload(&self) {
        let config = lys::Config::load();
        self.theme.reload(&config);
        if let Err(err) = lys::apply_to_apps(&config) {
            eprintln!("noros-shell: could not write app theme: {err}");
        }
        *self.config.borrow_mut() = config;
        for window in self.windows.borrow_mut().drain(..) {
            window.destroy();
        }
        self.build();
    }
}

/// Watch ~/.config/noros for changes and reload, coalescing bursts of events.
fn watch(desktop: &Rc<Desktop>) {
    let dir = lys::noros_dir();
    let _ = std::fs::create_dir_all(&dir);
    let monitor = match gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        Ok(monitor) => monitor,
        Err(err) => {
            eprintln!("noros-shell: cannot watch {}: {err}", dir.display());
            return;
        }
    };

    let weak = Rc::downgrade(desktop);
    monitor.connect_changed(move |_, file, other, _event| {
        let relevant = [Some(file), other].into_iter().flatten().any(|f| {
            f.basename()
                .map(|name| {
                    let name = name.to_string_lossy();
                    name == "config.toml" || name == "theme.css" || name.starts_with("wallpaper")
                })
                .unwrap_or(false)
        });
        let Some(desktop) = weak.upgrade() else { return };
        if !relevant || *desktop.reload_pending.borrow() {
            return;
        }
        *desktop.reload_pending.borrow_mut() = true;
        let weak = Rc::downgrade(&desktop);
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if let Some(desktop) = weak.upgrade() {
                *desktop.reload_pending.borrow_mut() = false;
                desktop.reload();
            }
        });
    });
    *desktop._monitor.borrow_mut() = Some(monitor);
}
