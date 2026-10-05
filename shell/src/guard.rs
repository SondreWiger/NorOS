//! Vakt prompts: when an app tries to go online for the first time, ask the user.
//!
//! A background thread stays subscribed to vaktd; questions arrive here and are
//! shown one at a time as a panel at the top of the screen.

use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::mpsc, time::Duration};

use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use vakt::{Answer, Ask, Client, Event, Request};

struct Prompts {
    app: gtk::Application,
    waiting: VecDeque<Ask>,
    current: Option<(u64, gtk::ApplicationWindow)>,
}

pub fn start(app: &gtk::Application) {
    let (tx, rx) = mpsc::channel::<Event>();
    std::thread::spawn(move || loop {
        if let Ok(mut client) = Client::connect() {
            if client.send(&Request::Subscribe).is_ok() && client.wait_forever().is_ok() {
                while let Ok(event) = client.next_event() {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
        }
        // vaktd not (yet) running: try again shortly.
        std::thread::sleep(Duration::from_secs(3));
    });

    let prompts = Rc::new(RefCell::new(Prompts { app: app.clone(), waiting: VecDeque::new(), current: None }));
    glib::timeout_add_local(Duration::from_millis(150), move || {
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Ask(ask) => {
                    let mut p = prompts.borrow_mut();
                    let known = p.current.as_ref().is_some_and(|(id, _)| *id == ask.id) || p.waiting.iter().any(|a| a.id == ask.id);
                    if !known {
                        p.waiting.push_back(ask);
                    }
                }
                Event::Answered { id, .. } => {
                    let mut p = prompts.borrow_mut();
                    p.waiting.retain(|a| a.id != id);
                    if p.current.as_ref().is_some_and(|(current, _)| *current == id) {
                        if let Some((_, window)) = p.current.take() {
                            window.destroy();
                        }
                    }
                }
                _ => {}
            }
        }
        show_next(&prompts);
        glib::ControlFlow::Continue
    });
}

fn answer(id: u64, answer: Answer) {
    std::thread::spawn(move || {
        if let Ok(mut client) = Client::connect() {
            let _ = client.call(&Request::Answer { id, answer });
        }
    });
}

fn show_next(prompts: &Rc<RefCell<Prompts>>) {
    let mut p = prompts.borrow_mut();
    if p.current.is_some() {
        return;
    }
    let Some(ask) = p.waiting.pop_front() else { return };

    let window = gtk::ApplicationWindow::new(&p.app);
    window.add_css_class("noros-panel");
    window.init_layer_shell();
    window.set_namespace(Some("noros-vakt"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, 70);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("vakt-prompt");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let shield = gtk::Image::from_icon_name("security-high-symbolic");
    shield.set_pixel_size(32);
    shield.add_css_class("vakt-shield");
    top.append(&shield);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(Some(&format!("{} wants to go online", ask.app.name)));
    title.add_css_class("vakt-title");
    title.set_xalign(0.0);
    let destination = gtk::Label::new(Some(&format!("Connecting to {} ({})", ask.destination.label(), ask.destination.protocol.to_uppercase())));
    destination.add_css_class("vakt-detail");
    destination.set_xalign(0.0);
    destination.set_selectable(false);
    let program = gtk::Label::new(Some(&ask.app.exe));
    program.add_css_class("vakt-program");
    program.set_xalign(0.0);
    program.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    text.append(&title);
    text.append(&destination);
    text.append(&program);
    top.append(&text);
    card.append(&top);

    let countdown = gtk::Label::new(None);
    countdown.add_css_class("vakt-countdown");
    countdown.set_xalign(0.0);
    card.append(&countdown);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let choices = [
        ("Always Block", Answer::BlockAlways, "vakt-block"),
        ("Block", Answer::BlockOnce, "vakt-block"),
        ("Allow Once", Answer::AllowOnce, "vakt-allow"),
        ("Always Allow", Answer::AllowAlways, "vakt-allow-always"),
    ];
    for (label, choice, class) in choices {
        let button = gtk::Button::with_label(label);
        button.add_css_class("vakt-button");
        button.add_css_class(class);
        let id = ask.id;
        let prompts = prompts.clone();
        button.connect_clicked(move |_| {
            answer(id, choice);
            close_current(&prompts);
        });
        buttons.append(&button);
    }
    card.append(&buttons);

    // Escape means "not now".
    let keys = gtk::EventControllerKey::new();
    {
        let id = ask.id;
        let prompts = prompts.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                answer(id, Answer::BlockOnce);
                close_current(&prompts);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);

    // Show how long until silence counts as "block".
    let remaining = Rc::new(RefCell::new(ask.timeout));
    countdown.set_text(&format!("Blocked automatically in {}s if you don't answer.", ask.timeout));
    {
        let weak = countdown.downgrade();
        glib::timeout_add_seconds_local(1, move || {
            let Some(label) = weak.upgrade() else { return glib::ControlFlow::Break };
            let mut left = remaining.borrow_mut();
            *left = left.saturating_sub(1);
            label.set_text(&format!("Blocked automatically in {}s if you don't answer.", *left));
            if *left == 0 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }

    window.set_child(Some(&card));
    window.present();
    p.current = Some((ask.id, window));
}

fn close_current(prompts: &Rc<RefCell<Prompts>>) {
    if let Some((_, window)) = prompts.borrow_mut().current.take() {
        window.destroy();
    }
}
