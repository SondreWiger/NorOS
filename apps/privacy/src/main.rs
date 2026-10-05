//! NorOS Privacy Center: see what leaves this machine, and decide.

use std::{cell::RefCell, rc::Rc, sync::mpsc, time::Duration};

use gtk::{gdk, gio, glib, prelude::*};
use vakt::{Client, Connection, Event, Mode, Policy, Request, Rule, Status};

const APP_CSS: &str = r#"
.privacy-page { padding: 24px 32px 32px; }
.page-title { font-size: 26px; font-weight: 800; margin-bottom: 4px; }
.page-subtitle { opacity: 0.65; margin-bottom: 14px; }
.group-title { font-weight: 700; font-size: 13px; opacity: 0.7; margin: 18px 4px 6px; }
.group { border-radius: 12px; background: alpha(currentColor, 0.05); border: 1px solid alpha(currentColor, 0.08); }
.item { padding: 10px 14px; min-height: 34px; }
.item + .item { border-top: 1px solid alpha(currentColor, 0.07); }
.item-title { font-weight: 600; }
.item-detail { font-size: 12px; opacity: 0.6; }
.hero { padding: 18px 20px; border-radius: 14px; background: alpha(#3DDC97, 0.12); border: 1px solid alpha(#3DDC97, 0.35); }
.hero.watching { background: alpha(#F5A524, 0.12); border-color: alpha(#F5A524, 0.4); }
.hero.offline { background: alpha(#E5484D, 0.12); border-color: alpha(#E5484D, 0.4); }
.hero-title { font-size: 20px; font-weight: 800; }
.number { font-size: 30px; font-weight: 800; font-feature-settings: "tnum"; }
.badge { font-size: 11px; font-weight: 700; padding: 2px 8px; border-radius: 99px; }
.badge.allowed { background: alpha(#3DDC97, 0.2); color: #3DDC97; }
.badge.blocked { background: alpha(#E5484D, 0.2); color: #E5484D; }
.notice { font-size: 12px; opacity: 0.7; padding: 10px 14px; }
.error-banner { padding: 10px 14px; border-radius: 10px; background: alpha(#E5484D, 0.15); }
"#;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Privacy")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build);
    app.run()
}

/// One request, one answer, on a fresh connection.
fn ask(request: Request) -> Option<Event> {
    Client::connect().ok()?.call(&request).ok()
}

fn fmt_time(secs: u64, with_date: bool) -> String {
    let format = if with_date { "%e %b %H:%M" } else { "%H:%M:%S" };
    glib::DateTime::from_unix_local(secs as i64)
        .and_then(|d| d.format(format))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

// ── Layout helpers ───────────────────────────────────────────────────

fn page(title: &str, subtitle: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("privacy-page");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("page-title");
    t.set_xalign(0.0);
    content.append(&t);
    let s = gtk::Label::new(Some(subtitle));
    s.add_css_class("page-subtitle");
    s.set_xalign(0.0);
    s.set_wrap(true);
    content.append(&s);
    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&content).build();
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

fn item(title: &str, detail: Option<&str>, end: Option<&gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("item");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("item-title");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&t);
    if let Some(detail) = detail {
        let d = gtk::Label::new(Some(detail));
        d.add_css_class("item-detail");
        d.set_xalign(0.0);
        d.set_wrap(true);
        text.append(&d);
    }
    row.append(&text);
    if let Some(end) = end {
        end.set_valign(gtk::Align::Center);
        row.append(end);
    }
    row
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn badge(allowed: bool) -> gtk::Widget {
    let label = gtk::Label::new(Some(if allowed { "Allowed" } else { "Blocked" }));
    label.add_css_class("badge");
    label.add_css_class(if allowed { "allowed" } else { "blocked" });
    label.upcast()
}

// ── Window ───────────────────────────────────────────────────────────

fn build(app: &gtk::Application) {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(APP_CSS);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Privacy Center")
        .default_width(940)
        .default_height(660)
        .build();
    window.set_titlebar(Some(&gtk::HeaderBar::new()));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_hexpand(true);

    let overview = overview_page();
    stack.add_titled(&overview.0, Some("overview"), "Overview");
    stack.add_titled(&network_page(), Some("network"), "Network Activity");
    let rules = rules_page();
    stack.add_titled(&rules.0, Some("rules"), "App Rules");
    stack.add_titled(&devices_page(&overview.1), Some("devices"), "Camera & Devices");
    stack.add_titled(&activity_page(), Some("activity"), "Activity History");
    stack.add_titled(&promise_page(), Some("promise"), "What NorOS Sends");

    // Refresh the pages that summarize state whenever they're shown.
    {
        let refresh_overview = overview.1.clone();
        let refresh_rules = rules.1.clone();
        stack.connect_visible_child_name_notify(move |stack| match stack.visible_child_name().as_deref() {
            Some("overview") => refresh_overview(),
            Some("rules") => refresh_rules(),
            _ => {}
        });
    }

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_width_request(210);
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.append(&sidebar);
    layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    layout.append(&stack);
    window.set_child(Some(&layout));
    window.present();
}

type Refresh = Rc<dyn Fn()>;

fn overview_page() -> (gtk::ScrolledWindow, Refresh) {
    let (scroller, content) = page("Overview", "Vakt guards every connection your apps try to make.");
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&body);

    let refresh: Refresh = {
        let body = body.clone();
        Rc::new(move || {
            clear(&body);
            let Some(Event::Status(status)) = ask(Request::Status) else {
                let banner = gtk::Label::new(Some(
                    "The Vakt guard isn't running. While it's off, apps can't reach the internet at all — NorOS stays closed rather than unguarded.",
                ));
                banner.add_css_class("error-banner");
                banner.set_wrap(true);
                banner.set_xalign(0.0);
                body.append(&banner);
                return;
            };
            fill_overview(&body, &status);
        })
    };
    refresh();
    // Keep the numbers current while the window is open.
    {
        let refresh = refresh.clone();
        let weak = body.downgrade();
        glib::timeout_add_seconds_local(4, move || {
            let Some(body) = weak.upgrade() else { return glib::ControlFlow::Break };
            if body.is_mapped() {
                refresh();
            }
            glib::ControlFlow::Continue
        });
    }
    (scroller, refresh)
}

fn fill_overview(body: &gtk::Box, status: &Status) {
    let mode = status.mode.unwrap_or(Mode::Ask);
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 4);
    hero.add_css_class("hero");
    let (title, text) = match mode {
        Mode::Ask => ("Protected", "Apps must ask before they go online for the first time."),
        Mode::AllowAll => ("Watching", "Every app may go online. Connections are still logged here."),
        Mode::Offline => ("Offline", "Nothing leaves this machine. Every app is blocked from the network."),
    };
    match mode {
        Mode::Offline => hero.add_css_class("offline"),
        Mode::AllowAll => hero.add_css_class("watching"),
        Mode::Ask => {}
    }
    let t = gtk::Label::new(Some(title));
    t.add_css_class("hero-title");
    t.set_xalign(0.0);
    let d = gtk::Label::new(Some(text));
    d.set_xalign(0.0);
    hero.append(&t);
    hero.append(&d);
    body.append(&hero);

    let numbers = gtk::Box::new(gtk::Orientation::Horizontal, 36);
    numbers.set_margin_top(18);
    for (value, label) in [(status.allowed_today, "connections allowed today"), (status.blocked_today, "blocked today"), (status.rules as u64, "app rules")] {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let n = gtk::Label::new(Some(&value.to_string()));
        n.add_css_class("number");
        n.set_xalign(0.0);
        let l = gtk::Label::new(Some(label));
        l.add_css_class("item-detail");
        l.set_xalign(0.0);
        column.append(&n);
        column.append(&l);
        numbers.append(&column);
    }
    body.append(&numbers);

    let g = group(body, "Network Mode");
    let dropdown = gtk::DropDown::from_strings(&["Ask for each new app", "Allow every app", "Offline — block everything"]);
    dropdown.set_selected(match mode {
        Mode::Ask => 0,
        Mode::AllowAll => 1,
        Mode::Offline => 2,
    });
    let body_weak = body.downgrade();
    dropdown.connect_selected_notify(move |d| {
        let mode = match d.selected() {
            1 => Mode::AllowAll,
            2 => Mode::Offline,
            _ => Mode::Ask,
        };
        if let Some(Event::Error { message }) = ask(Request::SetMode { mode }) {
            eprintln!("noros-privacy: {message}");
        }
        // Re-render on the next tick (we're inside the dropdown's own signal).
        let weak = body_weak.clone();
        glib::idle_add_local_once(move || {
            if let (Some(body), Some(Event::Status(status))) = (weak.upgrade(), ask(Request::Status)) {
                clear(&body);
                fill_overview(&body, &status);
            }
        });
    });
    g.append(&item("When an app wants to go online", Some("System services are always listed under “What NorOS Sends”."), Some(dropdown.upcast_ref())));

    let camera = if !status.camera_present {
        "No camera connected".to_string()
    } else if !status.camera_enabled {
        "Off for every app".to_string()
    } else if status.camera_in_use.is_empty() {
        "On · not in use".to_string()
    } else {
        format!("In use by {}", status.camera_in_use.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "))
    };
    let g = group(body, "Devices");
    g.append(&item("Camera", Some(&camera), None));
    g.append(&item("Microphone", Some("No sound system yet — arrives with NorOS 0.5"), None));
}

fn network_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page("Network Activity", "Every connection your apps tried to make, newest first. This list lives only in memory.");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("group");
    content.append(&list);

    let add = {
        let list = list.clone();
        move |c: &Connection, at_top: bool| {
            let row = item(
                &format!("{}  →  {}", c.app.name, c.destination.label()),
                Some(&format!("{} · {} · {} · {}", fmt_time(c.time, false), c.destination.protocol.to_uppercase(), c.reason, c.app.exe)),
                Some(&badge(c.allowed)),
            );
            if at_top {
                list.prepend(&row);
            } else {
                list.append(&row);
            }
            // Keep the view light.
            let mut count = 0;
            let mut child = list.first_child();
            while let Some(c) = child {
                count += 1;
                child = c.next_sibling();
                if count > 300 {
                    list.remove(&c);
                }
            }
        }
    };

    match ask(Request::Log) {
        Some(Event::Log { entries }) if !entries.is_empty() => {
            for c in &entries {
                add(c, false);
            }
        }
        _ => list.append(&item("Nothing yet", Some("Connections show up here as they happen."), None)),
    }

    // Live updates.
    let (tx, rx) = mpsc::channel::<Connection>();
    std::thread::spawn(move || {
        let Ok(mut client) = Client::connect() else { return };
        if client.send(&Request::Subscribe).is_err() || client.wait_forever().is_err() {
            return;
        }
        while let Ok(event) = client.next_event() {
            if let Event::Connection(c) = event {
                if tx.send(c).is_err() {
                    return;
                }
            }
        }
    });
    let weak = list.downgrade();
    glib::timeout_add_local(Duration::from_millis(300), move || {
        if weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        while let Ok(c) = rx.try_recv() {
            add(&c, true);
        }
        glib::ControlFlow::Continue
    });
    scroller
}

fn rules_page() -> (gtk::ScrolledWindow, Refresh) {
    let (scroller, content) = page("App Rules", "Your decisions about which apps may go online. Change or forget them here.");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("group");
    content.append(&list);

    let refresh: Rc<RefCell<Option<Refresh>>> = Rc::new(RefCell::new(None));
    let render: Refresh = {
        let list = list.clone();
        let refresh = refresh.clone();
        Rc::new(move || {
            clear(&list);
            let rules: Vec<Rule> = match ask(Request::Rules) {
                Some(Event::Rules { rules }) => rules,
                _ => Vec::new(),
            };
            if rules.is_empty() {
                list.append(&item("No rules yet", Some("When you answer “Always Allow” or “Always Block”, the rule appears here."), None));
            }
            for rule in rules {
                let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let policy = gtk::DropDown::from_strings(&["Allow", "Block", "Ask"]);
                policy.set_selected(match rule.policy {
                    Policy::Allow => 0,
                    Policy::Block => 1,
                    Policy::Ask => 2,
                });
                {
                    let exe = rule.exe.clone();
                    let name = rule.name.clone();
                    policy.connect_selected_notify(move |d| {
                        let policy = match d.selected() {
                            0 => Policy::Allow,
                            1 => Policy::Block,
                            _ => Policy::Ask,
                        };
                        let _ = ask(Request::SetRule { exe: exe.clone(), name: name.clone(), policy });
                    });
                }
                let forget = gtk::Button::from_icon_name("user-trash-symbolic");
                forget.set_tooltip_text(Some("Forget this rule (the app will ask again)"));
                {
                    let exe = rule.exe.clone();
                    let refresh = refresh.clone();
                    forget.connect_clicked(move |_| {
                        let _ = ask(Request::DeleteRule { exe: exe.clone() });
                        if let Some(r) = refresh.borrow().clone() {
                            glib::idle_add_local_once(move || r());
                        }
                    });
                }
                controls.append(&policy);
                controls.append(&forget);
                list.append(&item(&rule.name, Some(&format!("{} · since {}", rule.exe, fmt_time(rule.created, true))), Some(controls.upcast_ref())));
            }
        })
    };
    *refresh.borrow_mut() = Some(render.clone());
    render();
    (scroller, render)
}

fn devices_page(refresh_overview: &Refresh) -> gtk::ScrolledWindow {
    let (scroller, content) = page(
        "Camera & Devices",
        "Turning the camera off removes access for every app at the system level — no app can switch it back on.",
    );
    let status = match ask(Request::Status) {
        Some(Event::Status(s)) => s,
        _ => Status::default(),
    };
    let g = group(&content, "Camera");
    let switch = gtk::Switch::new();
    switch.set_active(status.camera_enabled);
    let in_use = gtk::Label::new(None);
    in_use.add_css_class("item-detail");
    {
        let refresh = refresh_overview.clone();
        switch.connect_active_notify(move |s| {
            if let Some(Event::Error { message }) = ask(Request::SetCamera { enabled: s.is_active() }) {
                eprintln!("noros-privacy: {message}");
            }
            refresh();
        });
    }
    let detail = if status.camera_present { "Allow apps to use the camera" } else { "No camera is connected right now" };
    g.append(&item("Camera access", Some(detail), Some(switch.upcast_ref())));
    for app in &status.camera_in_use {
        g.append(&item(&format!("{} is using the camera", app.name), Some(&app.exe), None));
    }

    let g = group(&content, "Coming Later");
    g.append(&item("Microphone", Some("NorOS doesn't have a sound system yet. Microphone control arrives with it in 0.5."), None));
    g.append(&item("Files, location and screen", Some("Per-app permissions need app sandboxing, which arrives with Flatpak apps in 0.5."), None));
    scroller
}

fn activity_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page(
        "Activity History",
        "Off unless you turn it on. When on, NorOS keeps a list of apps you open — only in your home folder, on your encrypted disk. It is never sent anywhere.",
    );
    let config = lys::Config::load();
    let g = group(&content, "History");
    let switch = gtk::Switch::new();
    switch.set_active(config.privacy.activity_history);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("group");

    let render = {
        let list = list.clone();
        Rc::new(move || {
            clear(&list);
            let entries = lys::read_activity();
            if entries.is_empty() {
                list.append(&item("Nothing recorded", None, None));
            }
            for (secs, what) in entries.into_iter().take(200) {
                list.append(&item(&what, Some(&fmt_time(secs, true)), None));
            }
        })
    };
    switch.connect_active_notify(move |s| {
        let mut config = lys::Config::load();
        config.privacy.activity_history = s.is_active();
        if let Err(err) = config.save() {
            eprintln!("noros-privacy: {err}");
        }
    });
    g.append(&item("Keep a history of the apps I open", None, Some(switch.upcast_ref())));
    let wipe = gtk::Button::with_label("Delete History");
    wipe.add_css_class("destructive-action");
    {
        let render = render.clone();
        wipe.connect_clicked(move |_| {
            let _ = lys::clear_activity();
            render();
        });
    }
    g.append(&item("Delete everything recorded so far", None, Some(wipe.upcast_ref())));

    let label = gtk::Label::new(Some("Recent"));
    label.add_css_class("group-title");
    label.set_xalign(0.0);
    content.append(&label);
    content.append(&list);
    render();
    scroller
}

fn promise_page() -> gtk::ScrolledWindow {
    let (scroller, content) = page(
        "What NorOS Sends",
        "NorOS has no telemetry, no accounts and no cloud. These are the only parts of the system itself that can use the network, and when.",
    );
    let g = group(&content, "System Services");
    for (name, when) in [
        ("Network setup (systemd-networkd)", "Asks your router for an address when you connect a cable or Wi-Fi."),
        ("Name lookups (systemd-resolved)", "Looks up names like example.com — only when one of your apps asks."),
        ("Updates (apt)", "Only when you press “Update Now” in Settings. A snapshot is taken first."),
    ] {
        g.append(&item(name, Some(when), None));
    }
    let g = group(&content, "Never");
    for (what, why) in [
        ("Usage statistics and crash reports", "Not collected. Crash logs stay on this machine."),
        ("Accounts and sign-in", "NorOS has no online account."),
        ("Update checks in the background", "NorOS never checks for updates on its own."),
    ] {
        g.append(&item(what, Some(why), None));
    }
    let note = gtk::Label::new(Some("These services are allowed without asking and don't appear under Network Activity. Everything your apps do does."));
    note.add_css_class("notice");
    note.set_wrap(true);
    note.set_xalign(0.0);
    content.append(&note);
    scroller
}
