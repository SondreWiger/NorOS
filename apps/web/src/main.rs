//! Web — the NorOS browser.
//!
//! WebKit underneath, NorOS on top: tabs, downloads into ~/Downloads, tracker
//! protection on, a blank start page that loads nothing from the network, and
//! private windows that keep nothing at all. Every page it loads goes through
//! Vakt like any other app.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};
use webkit::{prelude::*, Download, LoadEvent, NetworkSession, PermissionRequest, WebView};

const SEARCH: &str = "https://duckduckgo.com/?q=";

const APP_CSS: &str = r#"
.address { min-width: 520px; border-radius: 9px; }
.tab-label { min-width: 120px; }
.tab-close { min-width: 18px; min-height: 18px; padding: 0; border-radius: 50%; }
.downloads { padding: 8px; min-width: 340px; }
.download-row { padding: 8px 6px; }
.download-name { font-weight: 600; }
.download-detail { font-size: 12px; opacity: 0.6; }
.private-badge { font-size: 11px; font-weight: 800; padding: 2px 8px; border-radius: 99px; background: #6B4FD8; color: white; }
.load-progress trough, .load-progress progress { min-height: 2px; }
"#;

fn start_page(private: bool) -> String {
    let note = if private {
        "Private window: nothing you do here is saved — no history, no cookies, no cache."
    } else {
        "This page loads nothing from the internet. Type an address or search above."
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>New Tab</title>
<style>
  :root {{ color-scheme: dark light; }}
  body {{ margin: 0; height: 100vh; display: grid; place-items: center; font-family: Inter, sans-serif;
         background: radial-gradient(1200px 600px at 50% 0%, #13233F, #0B1220); color: #E8EEF7; }}
  .wrap {{ text-align: center; max-width: 560px; padding: 24px; }}
  h1 {{ font-size: 64px; font-weight: 900; letter-spacing: -0.04em; margin: 0 0 8px; }}
  p {{ opacity: .7; line-height: 1.5; }}
  .dot {{ color: #3DDC97; }}
</style></head>
<body><div class="wrap"><h1>Web<span class="dot">.</span></h1><p>{note}</p></div></body></html>"#
    )
}

fn waiting_page(uri: &str) -> String {
    let esc = uri.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Waiting for permission</title>
<style>body {{ margin: 0; height: 100vh; display: grid; place-items: center; font-family: Inter, sans-serif; background: #0B1220; color: #E8EEF7; }}
.wrap {{ max-width: 560px; padding: 24px; }} h1 {{ font-size: 28px; }} code {{ opacity: .7; word-break: break-all; }}
p {{ opacity: .75; line-height: 1.5; }} .dot {{ color: #3DDC97; }}</style></head>
<body><div class="wrap"><h1>Waiting for your permission<span class="dot">…</span></h1><p><code>{esc}</code></p>
<p>NorOS is asking whether Web may go online. Answer at the top of the screen — the page opens as soon as you allow it.</p></div></body></html>"#
    )
}

fn error_page(uri: &str, message: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Can't open page</title>
<style>body {{ margin: 0; height: 100vh; display: grid; place-items: center; font-family: Inter, sans-serif; background: #0B1220; color: #E8EEF7; }}
.wrap {{ max-width: 560px; padding: 24px; }} h1 {{ font-size: 28px; }} code {{ opacity: .7; word-break: break-all; }}
p {{ opacity: .75; line-height: 1.5; }}</style></head>
<body><div class="wrap"><h1>Can't open this page</h1><p><code>{}</code></p><p>{}</p>
<p>If NorOS asked whether Web may go online, check Privacy Center → App Rules.</p></div></body></html>"#,
        esc(uri),
        esc(message)
    )
}

/// Turn what was typed into an address: a URL as-is, a bare domain over HTTPS,
/// anything else as a search.
fn to_uri(input: &str) -> String {
    let input = input.trim();
    if input.contains("://") || input.starts_with("about:") || input.starts_with("file:") {
        return input.to_string();
    }
    let looks_like_host = !input.contains(' ') && (input.contains('.') || input.starts_with("localhost")) && !input.ends_with('.');
    if looks_like_host {
        format!("https://{input}")
    } else {
        let query: String = input
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                b' ' => "+".to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        format!("{SEARCH}{query}")
    }
}

fn downloads_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(|| glib::home_dir().join("Downloads"))
}

/// "file.zip", or "file (2).zip" if that's taken.
fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let name = name.replace('/', "_");
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// What Vakt (the NorOS firewall) is currently asking about this browser.
#[derive(Default)]
struct Guard {
    /// Questions about Web that haven't been answered yet.
    pending: std::collections::HashSet<u64>,
    /// Tabs whose load failed while a question was open, with the address to retry.
    waiting: Vec<(glib::WeakRef<WebView>, String)>,
}

struct Browser {
    guard: Rc<RefCell<Guard>>,
    app: gtk::Application,
    window: gtk::ApplicationWindow,
    notebook: gtk::Notebook,
    address: gtk::Entry,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
    progress: gtk::ProgressBar,
    session: NetworkSession,
    private: bool,
    downloads: gtk::Box,
    downloads_button: gtk::MenuButton,
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Web")
        .flags(gio::ApplicationFlags::NON_UNIQUE | gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(|app| {
        let private = std::env::args().any(|a| a == "--private");
        new_window(app, private).new_tab(None);
    });
    app.connect_open(|app, files, _| {
        let browser = new_window(app, false);
        for file in files {
            browser.new_tab(Some(&file.uri()));
        }
    });
    // Don't let GTK treat our --private flag as a file to open.
    let args: Vec<String> = std::env::args().filter(|a| a != "--private").collect();
    app.run_with_args(&args)
}

fn session(private: bool) -> NetworkSession {
    let session = if private {
        NetworkSession::new_ephemeral()
    } else {
        let data = glib::user_data_dir().join("noros/web");
        let cache = glib::user_cache_dir().join("noros/web");
        NetworkSession::new(data.to_str(), cache.to_str())
    };
    // WebKit's Intelligent Tracking Prevention: blocks cross-site tracking cookies.
    session.set_itp_enabled(true);
    session
}

fn new_window(app: &gtk::Application, private: bool) -> Rc<Browser> {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(APP_CSS);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(if private { "Private Window" } else { "Web" })
        .default_width(1180)
        .default_height(760)
        .build();

    let header = gtk::HeaderBar::new();
    let nav = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav.add_css_class("linked");
    let back = gtk::Button::from_icon_name("go-previous-symbolic");
    back.set_tooltip_text(Some("Back  (Alt + ←)"));
    let forward = gtk::Button::from_icon_name("go-next-symbolic");
    forward.set_tooltip_text(Some("Forward  (Alt + →)"));
    let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
    reload.set_tooltip_text(Some("Reload  (Ctrl + R)"));
    nav.append(&back);
    nav.append(&forward);
    header.pack_start(&nav);
    header.pack_start(&reload);
    if private {
        let badge = gtk::Label::new(Some("PRIVATE"));
        badge.add_css_class("private-badge");
        header.pack_start(&badge);
    }

    let address = gtk::Entry::new();
    address.add_css_class("address");
    address.set_placeholder_text(Some("Search or enter address"));
    address.set_hexpand(true);
    header.set_title_widget(Some(&address));

    let menu = gtk::MenuButton::new();
    menu.set_icon_name("open-menu-symbolic");
    let downloads_button = gtk::MenuButton::new();
    downloads_button.set_icon_name("folder-download-symbolic");
    downloads_button.set_tooltip_text(Some("Downloads"));
    let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
    new_tab.set_tooltip_text(Some("New tab  (Ctrl + T)"));
    header.pack_end(&menu);
    header.pack_end(&downloads_button);
    header.pack_end(&new_tab);
    window.set_titlebar(Some(&header));

    let downloads = gtk::Box::new(gtk::Orientation::Vertical, 4);
    downloads.add_css_class("downloads");
    let empty = gtk::Label::new(Some("No downloads yet. Files are saved to Downloads."));
    empty.add_css_class("download-detail");
    downloads.append(&empty);
    let downloads_popover = gtk::Popover::new();
    downloads_popover.set_child(Some(&downloads));
    downloads_button.set_popover(Some(&downloads_popover));

    let progress = gtk::ProgressBar::new();
    progress.add_css_class("load-progress");
    progress.set_visible(false);

    let notebook = gtk::Notebook::new();
    notebook.set_scrollable(true);
    notebook.set_vexpand(true);
    notebook.set_show_border(false);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&progress);
    layout.append(&notebook);
    window.set_child(Some(&layout));

    let browser = Rc::new(Browser {
        guard: Rc::default(),
        app: app.clone(),
        window: window.clone(),
        notebook: notebook.clone(),
        address: address.clone(),
        back: back.clone(),
        forward: forward.clone(),
        reload: reload.clone(),
        progress,
        session: session(private),
        private,
        downloads,
        downloads_button,
    });

    // Menu.
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(6);
    menu_box.set_margin_bottom(6);
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);
    let popover = gtk::Popover::new();
    for (label, action) in [
        ("New Window", "window"),
        ("New Private Window", "private"),
        ("Open Downloads Folder", "folder"),
        ("Clear Browsing Data…", "clear"),
    ] {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        if let Some(l) = item.child().and_downcast::<gtk::Label>() {
            l.set_xalign(0.0);
        }
        let b = browser.clone();
        let popover = popover.clone();
        item.connect_clicked(move |_| {
            popover.popdown();
            match action {
                "window" => {
                    new_window(&b.app, false).new_tab(None);
                }
                "private" => {
                    new_window(&b.app, true).new_tab(None);
                }
                "folder" => {
                    let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(downloads_dir()).uri(), None::<&gio::AppLaunchContext>);
                }
                _ => b.clear_data(),
            }
        });
        menu_box.append(&item);
    }
    popover.set_child(Some(&menu_box));
    menu.set_popover(Some(&popover));

    // Wiring.
    {
        let b = browser.clone();
        address.connect_activate(move |entry| {
            if let Some(view) = b.current() {
                view.load_uri(&to_uri(&entry.text()));
                view.grab_focus();
            }
        });
    }
    {
        let b = browser.clone();
        back.connect_clicked(move |_| {
            if let Some(v) = b.current() {
                v.go_back();
            }
        });
        let b = browser.clone();
        forward.connect_clicked(move |_| {
            if let Some(v) = b.current() {
                v.go_forward();
            }
        });
        let b = browser.clone();
        reload.connect_clicked(move |_| {
            if let Some(v) = b.current() {
                if v.is_loading() { v.stop_loading() } else { v.reload() }
            }
        });
        let b = browser.clone();
        new_tab.connect_clicked(move |_| {
            b.new_tab(None);
        });
    }
    {
        let b = browser.clone();
        notebook.connect_switch_page(move |_, page, _| {
            if let Some(view) = page.downcast_ref::<WebView>() {
                b.sync(view);
            }
        });
    }
    {
        let b = Rc::downgrade(&browser);
        browser.session.connect_download_started(move |_, download| {
            if let Some(b) = b.upgrade() {
                b.track_download(download);
            }
        });
    }
    add_shortcuts(&browser);
    watch_guard(&browser);
    window.present();
    browser
}

impl Browser {
    fn current(&self) -> Option<WebView> {
        self.notebook.nth_page(self.notebook.current_page()).and_downcast::<WebView>()
    }

    fn new_tab(self: &Rc<Self>, uri: Option<&str>) -> WebView {
        self.add_view(WebView::builder().network_session(&self.session).build(), uri)
    }

    fn add_view(self: &Rc<Self>, view: WebView, uri: Option<&str>) -> WebView {
        if let Some(settings) = WebViewExt::settings(&view) {
            settings.set_enable_developer_extras(true);
            // Media only plays after you click; no autoplaying videos.
            settings.set_media_playback_requires_user_gesture(true);
        }

        let label = gtk::Label::new(Some("New Tab"));
        label.add_css_class("tab-label");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(22);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.add_css_class("tab-close");
        let tab = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tab.append(&label);
        tab.append(&close);

        let index = self.notebook.append_page(&view, Some(&tab));
        self.notebook.set_tab_reorderable(&view, true);
        self.notebook.set_current_page(Some(index));

        {
            let b = Rc::downgrade(self);
            let v = view.clone();
            close.connect_clicked(move |_| {
                if let Some(b) = b.upgrade() {
                    b.close_tab(&v);
                }
            });
        }
        {
            let b = Rc::downgrade(self);
            let label = label.clone();
            view.connect_title_notify(move |v| {
                let title = v.title().filter(|t| !t.is_empty()).map(|t| t.to_string()).unwrap_or_else(|| "New Tab".into());
                label.set_text(&title);
                label.set_tooltip_text(Some(&title));
                if let Some(b) = b.upgrade() {
                    if b.current().as_ref() == Some(v) {
                        b.window.set_title(Some(&title));
                    }
                }
            });
        }
        for signal in ["uri", "estimated-load-progress", "is-loading"] {
            let b = Rc::downgrade(self);
            view.connect_notify_local(Some(signal), move |v, _| {
                if let Some(b) = b.upgrade() {
                    if b.current().as_ref() == Some(v) {
                        b.sync(v);
                    }
                }
            });
        }
        {
            // Links that open a new window become a new tab.
            let b = Rc::downgrade(self);
            view.connect_create(move |v, _| {
                let b = b.upgrade()?;
                let related = WebView::builder().related_view(v).build();
                Some(b.add_view(related, None).upcast())
            });
        }
        {
            let b = Rc::downgrade(self);
            view.connect_close(move |v| {
                if let Some(b) = b.upgrade() {
                    b.close_tab(v);
                }
            });
        }
        {
            let guard = self.guard.clone();
            view.connect_load_failed(move |v, _, uri, err| {
                // Cancelled loads (e.g. a download started) aren't failures.
                if err.matches(webkit::NetworkError::Cancelled) || err.message().contains("interrupted") {
                    return false;
                }
                // NorOS is still asking whether we may go online: wait, then retry.
                let mut g = guard.borrow_mut();
                if !g.pending.is_empty() {
                    g.waiting.push((v.downgrade(), uri.to_string()));
                    v.load_alternate_html(&waiting_page(uri), uri, None);
                    return true;
                }
                v.load_alternate_html(&error_page(uri, err.message()), uri, None);
                true
            });
        }
        {
            let b = Rc::downgrade(self);
            view.connect_permission_request(move |v, request| {
                if let Some(b) = b.upgrade() {
                    b.ask_permission(v, request);
                }
                true
            });
        }
        {
            let b = Rc::downgrade(self);
            view.connect_load_changed(move |v, event| {
                if event == LoadEvent::Committed {
                    if let Some(b) = b.upgrade() {
                        if b.current().as_ref() == Some(v) {
                            b.sync(v);
                        }
                    }
                }
            });
        }

        match uri {
            Some(uri) => view.load_uri(uri),
            None => {
                view.load_html(&start_page(self.private), Some("about:blank"));
                self.address.set_text("");
                self.address.grab_focus();
            }
        }
        view
    }

    fn close_tab(&self, view: &WebView) {
        if let Some(index) = self.notebook.page_num(view) {
            self.notebook.remove_page(Some(index));
        }
        if self.notebook.n_pages() == 0 {
            self.window.close();
        }
    }

    /// Bring the toolbar in line with the visible tab.
    fn sync(&self, view: &WebView) {
        let uri = view.uri().map(|u| u.to_string()).unwrap_or_default();
        if !self.address.has_focus() {
            self.address.set_text(if uri == "about:blank" { "" } else { &uri });
        }
        self.back.set_sensitive(view.can_go_back());
        self.forward.set_sensitive(view.can_go_forward());
        let loading = view.is_loading();
        self.reload.set_icon_name(if loading { "process-stop-symbolic" } else { "view-refresh-symbolic" });
        self.progress.set_visible(loading);
        self.progress.set_fraction(view.estimated_load_progress());
        let title = view.title().filter(|t| !t.is_empty()).map(|t| t.to_string()).unwrap_or_else(|| "Web".into());
        self.window.set_title(Some(&title));
    }

    fn ask_permission(&self, view: &WebView, request: &PermissionRequest) {
        let site = view
            .uri()
            .and_then(|u| glib::Uri::parse(&u, glib::UriFlags::NONE).ok())
            .and_then(|u| u.host())
            .map(|h| h.to_string())
            .unwrap_or_else(|| "This page".into());
        let what = match request.type_().name() {
            n if n.contains("UserMedia") => "use your camera or microphone",
            n if n.contains("Geolocation") => "know your location",
            n if n.contains("Notification") => "show notifications",
            n if n.contains("Clipboard") => "read your clipboard",
            _ => "use a device or feature",
        };
        let alert = gtk::AlertDialog::builder()
            .message(format!("{site} wants to {what}"))
            .detail("Allow it only if you trust this site. You'll be asked again next time.")
            .buttons(["Don't Allow", "Allow"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let request = request.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            if choice == Ok(1) { request.allow() } else { request.deny() }
        });
    }

    fn track_download(self: &Rc<Self>, download: &Download) {
        // Remove the "no downloads yet" text.
        if let Some(first) = self.downloads.first_child().and_downcast::<gtk::Label>() {
            self.downloads.remove(&first);
        }
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row.add_css_class("download-row");
        let name = gtk::Label::new(Some("Starting…"));
        name.add_css_class("download-name");
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let bar = gtk::ProgressBar::new();
        let detail = gtk::Label::new(Some(""));
        detail.add_css_class("download-detail");
        detail.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let open = gtk::Button::with_label("Open");
        let show = gtk::Button::with_label("Show in Files");
        let cancel = gtk::Button::with_label("Cancel");
        open.set_visible(false);
        show.set_visible(false);
        actions.append(&cancel);
        actions.append(&show);
        actions.append(&open);
        row.append(&name);
        row.append(&bar);
        row.append(&detail);
        row.append(&actions);
        self.downloads.prepend(&row);
        self.downloads_button.popup();

        let destination: Rc<RefCell<Option<PathBuf>>> = Rc::default();
        {
            let destination = destination.clone();
            let name = name.clone();
            download.connect_decide_destination(move |d, suggested| {
                let dir = downloads_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = unique_path(&dir, if suggested.is_empty() { "download" } else { suggested });
                name.set_text(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                d.set_destination(&gio::File::for_path(&path).uri());
                *destination.borrow_mut() = Some(path);
                true
            });
        }
        {
            let bar = bar.clone();
            let detail = detail.clone();
            download.connect_received_data(move |d, _| {
                bar.set_fraction(d.estimated_progress());
                let total = d.response().map(|r| r.content_length()).unwrap_or(0);
                let done = (d.estimated_progress() * total as f64) as u64;
                detail.set_text(&if total > 0 {
                    format!("{:.1} of {:.1} MB", done as f64 / 1e6, total as f64 / 1e6)
                } else {
                    "Downloading…".into()
                });
            });
        }
        {
            let (bar, detail, open, show, cancel, destination) = (bar.clone(), detail.clone(), open.clone(), show.clone(), cancel.clone(), destination.clone());
            download.connect_finished(move |_| {
                if bar.fraction() < 1.0 && detail.text().starts_with("Failed") {
                    return;
                }
                bar.set_fraction(1.0);
                detail.set_text("Saved to Downloads");
                cancel.set_visible(false);
                open.set_visible(true);
                show.set_visible(true);
                let _ = &destination;
            });
        }
        {
            let detail = detail.clone();
            let cancel = cancel.clone();
            download.connect_failed(move |_, err| {
                detail.set_text(&format!("Failed: {}", err.message()));
                cancel.set_visible(false);
            });
        }
        {
            let d = download.clone();
            cancel.connect_clicked(move |_| d.cancel());
        }
        {
            let destination = destination.clone();
            open.connect_clicked(move |_| {
                if let Some(path) = destination.borrow().as_ref() {
                    let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(path).uri(), None::<&gio::AppLaunchContext>);
                }
            });
        }
        show.connect_clicked(move |_| {
            if let Some(path) = destination.borrow().as_ref() {
                let _ = std::process::Command::new("noros-files").arg(path.parent().unwrap_or(&downloads_dir())).spawn();
            }
        });
    }

    fn clear_data(self: &Rc<Self>) {
        let alert = gtk::AlertDialog::builder()
            .message("Clear browsing data?")
            .detail("Removes cookies, site data, cache and history for all sites. Downloaded files are kept.")
            .buttons(["Cancel", "Clear"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let session = self.session.clone();
        alert.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            if choice != Ok(1) {
                return;
            }
            if let Some(manager) = session.website_data_manager() {
                manager.clear(webkit::WebsiteDataTypes::ALL, glib::TimeSpan::from_seconds(0), gio::Cancellable::NONE, |_| {});
            }
        });
    }
}

/// Follow Vakt's questions about this browser, so pages that failed while you
/// were deciding open by themselves once you allow them.
fn watch_guard(browser: &Rc<Browser>) {
    let me = std::fs::read_link("/proc/self/exe").map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let (tx, rx) = std::sync::mpsc::channel::<vakt::Event>();
    std::thread::spawn(move || loop {
        if let Ok(mut client) = vakt::Client::connect() {
            if client.send(&vakt::Request::Subscribe).is_ok() && client.wait_forever().is_ok() {
                while let Ok(event) = client.next_event() {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
    let guard = browser.guard.clone();
    let window = browser.window.downgrade();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        if window.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        while let Ok(event) = rx.try_recv() {
            match event {
                vakt::Event::Ask(ask) if ask.app.exe == me => {
                    guard.borrow_mut().pending.insert(ask.id);
                }
                vakt::Event::Answered { id, allowed } => {
                    let mut g = guard.borrow_mut();
                    if !g.pending.remove(&id) {
                        continue;
                    }
                    let waiting = std::mem::take(&mut g.waiting);
                    drop(g);
                    for (view, uri) in waiting {
                        let Some(view) = view.upgrade() else { continue };
                        if allowed {
                            view.load_uri(&uri);
                        } else {
                            view.load_alternate_html(
                                &error_page(&uri, "You chose not to let Web go online. You can change this in Privacy Center → App Rules."),
                                &uri,
                                None,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        glib::ControlFlow::Continue
    });
}

fn add_shortcuts(browser: &Rc<Browser>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let b = browser.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let view = b.current();
        match key {
            gdk::Key::t if ctrl => {
                b.new_tab(None);
            }
            gdk::Key::w if ctrl => {
                if let Some(v) = view {
                    b.close_tab(&v);
                }
            }
            gdk::Key::l if ctrl => {
                b.address.grab_focus();
                b.address.select_region(0, -1);
            }
            gdk::Key::r | gdk::Key::F5 if ctrl || key == gdk::Key::F5 => {
                if let Some(v) = view {
                    v.reload();
                }
            }
            gdk::Key::N if ctrl && shift => {
                new_window(&b.app, true).new_tab(None);
            }
            gdk::Key::n if ctrl => {
                new_window(&b.app, false).new_tab(None);
            }
            gdk::Key::Left if alt => {
                if let Some(v) = view {
                    v.go_back();
                }
            }
            gdk::Key::Right if alt => {
                if let Some(v) = view {
                    v.go_forward();
                }
            }
            gdk::Key::Tab if ctrl => {
                let n = b.notebook.n_pages();
                if n > 0 {
                    let current = b.notebook.current_page().unwrap_or(0);
                    let next = if shift { (current + n - 1) % n } else { (current + 1) % n };
                    b.notebook.set_current_page(Some(next));
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    browser.window.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_searches() {
        assert_eq!(to_uri("example.com"), "https://example.com");
        assert_eq!(to_uri("http://x.org/a"), "http://x.org/a");
        assert_eq!(to_uri("noros privacy"), format!("{SEARCH}noros+privacy"));
        assert_eq!(to_uri("æ"), format!("{SEARCH}%C3%A6"));
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join("noros-web-test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("a.zip"), "").unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a (2).zip"));
        assert_eq!(unique_path(&dir, "b.zip"), dir.join("b.zip"));
    }
}
