//! Install NorOS — the graphical installer in the live session.
//!
//! Collects choices, then hands them to `noros-install` (running with administrator
//! rights) and shows its progress. Passwords go over a pipe, never on a command line.

use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};
use serde::Deserialize;

const APP_CSS: &str = r#"
.installer { padding: 36px 48px; }
.step-title { font-size: 28px; font-weight: 800; }
.step-text { font-size: 14px; opacity: 0.75; }
.big-logo { font-size: 64px; font-weight: 900; letter-spacing: -0.03em; }
.promise { padding: 12px 14px; border-left: 3px solid @accent_bg_color; background: alpha(@accent_bg_color, 0.1); }
.disk-row { padding: 12px 14px; }
.disk-name { font-weight: 700; }
.disk-detail { font-size: 12px; opacity: 0.65; }
.warning { color: #E5484D; font-weight: 600; }
.field-label { font-weight: 600; margin-top: 10px; }
.hint { font-size: 12px; opacity: 0.6; }
.error-text { color: #E5484D; }
"#;

#[derive(Debug, Clone, Deserialize)]
struct Disk {
    path: String,
    size: u64,
    model: String,
    transport: String,
    removable: bool,
    unusable: Option<String>,
}

fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1_000_000_000.0;
    if gb >= 1000.0 { format!("{:.1} TB", gb / 1000.0) } else { format!("{gb:.0} GB") }
}

#[derive(Default)]
struct Choices {
    disk: Option<Disk>,
    full_name: String,
    username: String,
    password: String,
    hostname: String,
    encrypt: bool,
    encryption_password: String,
}

struct Ui {
    stack: gtk::Stack,
    back: gtk::Button,
    next: gtk::Button,
    choices: RefCell<Choices>,
    pages: Vec<&'static str>,
    current: RefCell<usize>,
    // Widgets read when moving on.
    disk_list: gtk::ListBox,
    disks: RefCell<Vec<Disk>>,
    name: gtk::Entry,
    username: gtk::Entry,
    password: gtk::PasswordEntry,
    password2: gtk::PasswordEntry,
    hostname: gtk::Entry,
    encrypt: gtk::Switch,
    same_password: gtk::CheckButton,
    key: gtk::PasswordEntry,
    key2: gtk::PasswordEntry,
    confirm: gtk::CheckButton,
    summary: gtk::Label,
    error: gtk::Label,
    progress: gtk::ProgressBar,
    progress_text: gtk::Label,
    process: RefCell<Option<gio::Subprocess>>,
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("no.noros.Installer")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build);
    app.run()
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}

fn page_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    b.add_css_class("installer");
    b
}

fn build(app: &gtk::Application) {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(APP_CSS);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Install NorOS")
        .default_width(760)
        .default_height(560)
        .build();
    let header = gtk::HeaderBar::new();
    window.set_titlebar(Some(&header));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    stack.set_vexpand(true);

    // Welcome.
    let welcome = page_box();
    welcome.append(&label("NorOS", "big-logo"));
    welcome.append(&label("Install NorOS on this computer", "step-title"));
    welcome.append(&label(
        "This takes a few minutes. You'll choose a disk, create your account and set a password that encrypts the disk, so nobody can read your files without it.",
        "step-text",
    ));
    welcome.append(&label(
        "No telemetry, no accounts, no cloud. Nothing leaves this machine unless you say so.",
        "promise",
    ));
    stack.add_named(&welcome, Some("welcome"));

    // Disk.
    let disk_page = page_box();
    disk_page.append(&label("Where should NorOS go?", "step-title"));
    disk_page.append(&label("The disk you pick will be erased completely.", "warning"));
    let disk_list = gtk::ListBox::new();
    disk_list.add_css_class("boxed-list");
    disk_list.set_selection_mode(gtk::SelectionMode::Single);
    let disk_scroll = gtk::ScrolledWindow::builder().child(&disk_list).vexpand(true).build();
    disk_page.append(&disk_scroll);
    stack.add_named(&disk_page, Some("disk"));

    // Account.
    let you = page_box();
    you.append(&label("Who's using this computer?", "step-title"));
    let name = gtk::Entry::builder().placeholder_text("Your name").build();
    let username = gtk::Entry::builder().placeholder_text("username").build();
    let password = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Password").build();
    let password2 = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Password again").build();
    let hostname = gtk::Entry::builder().text("noros").build();
    you.append(&label("Name", "field-label"));
    you.append(&name);
    you.append(&label("Username", "field-label"));
    you.append(&username);
    you.append(&label("Password", "field-label"));
    you.append(&password);
    you.append(&password2);
    you.append(&label("Computer name", "field-label"));
    you.append(&hostname);
    {
        // Suggest a username from the name until the user types their own.
        let username = username.clone();
        name.connect_changed(move |name| {
            let suggestion: String = name
                .text()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                .collect();
            username.set_text(&suggestion);
        });
    }
    stack.add_named(&you, Some("you"));

    // Encryption.
    let crypt = page_box();
    crypt.append(&label("Encrypt the disk", "step-title"));
    crypt.append(&label(
        "With encryption on, everything on the disk is unreadable without a password you type when the computer starts. If the computer is lost or stolen, your files stay private.",
        "step-text",
    ));
    let encrypt_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let encrypt = gtk::Switch::new();
    encrypt.set_active(true);
    encrypt_row.append(&label("Encrypt the disk (recommended)", "field-label"));
    encrypt_row.append(&encrypt);
    crypt.append(&encrypt_row);
    let same_password = gtk::CheckButton::with_label("Use my account password");
    same_password.set_active(true);
    crypt.append(&same_password);
    let key = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Encryption password (8+ characters)").build();
    let key2 = gtk::PasswordEntry::builder().show_peek_icon(true).placeholder_text("Encryption password again").build();
    let key_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    key_box.append(&key);
    key_box.append(&key2);
    key_box.set_visible(false);
    crypt.append(&key_box);
    crypt.append(&label(
        "There is no way to recover this password. Write it down somewhere safe.",
        "hint",
    ));
    {
        let key_box = key_box.clone();
        let encrypt = encrypt.clone();
        same_password.connect_toggled(move |same| key_box.set_visible(encrypt.is_active() && !same.is_active()));
    }
    {
        let key_box = key_box.clone();
        let same = same_password.clone();
        encrypt.connect_active_notify(move |e| {
            same.set_sensitive(e.is_active());
            key_box.set_visible(e.is_active() && !same.is_active());
        });
    }
    stack.add_named(&crypt, Some("encryption"));

    // Summary.
    let review = page_box();
    review.append(&label("Ready to install", "step-title"));
    let summary = label("", "step-text");
    review.append(&summary);
    let confirm = gtk::CheckButton::with_label("I understand that everything on this disk will be erased");
    review.append(&confirm);
    stack.add_named(&review, Some("review"));

    // Progress.
    let working = page_box();
    working.set_valign(gtk::Align::Center);
    working.append(&label("Installing NorOS", "step-title"));
    let progress = gtk::ProgressBar::new();
    progress.set_margin_top(12);
    working.append(&progress);
    let progress_text = label("Starting…", "step-text");
    working.append(&progress_text);
    stack.add_named(&working, Some("installing"));

    // Done.
    let done = page_box();
    done.set_valign(gtk::Align::Center);
    done.append(&label("NorOS is installed", "step-title"));
    done.append(&label("Remove the installation USB stick or disc, then restart to start your new system.", "step-text"));
    let restart = gtk::Button::with_label("Restart Now");
    restart.add_css_class("suggested-action");
    restart.set_halign(gtk::Align::Start);
    restart.connect_clicked(|_| {
        let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
    });
    done.append(&restart);
    stack.add_named(&done, Some("done"));

    let error = label("", "error-text");
    let back = gtk::Button::with_label("Back");
    let next = gtk::Button::with_label("Continue");
    next.add_css_class("suggested-action");
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_margin_start(48);
    footer.set_margin_end(48);
    footer.set_margin_bottom(24);
    error.set_hexpand(true);
    footer.append(&error);
    footer.append(&back);
    footer.append(&next);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&stack);
    layout.append(&footer);
    window.set_child(Some(&layout));

    let ui = Rc::new(Ui {
        stack,
        back: back.clone(),
        next: next.clone(),
        choices: RefCell::new(Choices { encrypt: true, ..Default::default() }),
        pages: vec!["welcome", "disk", "you", "encryption", "review", "installing", "done"],
        current: RefCell::new(0),
        disk_list,
        disks: RefCell::default(),
        name,
        username,
        password,
        password2,
        hostname,
        encrypt,
        same_password,
        key,
        key2,
        confirm,
        summary,
        error,
        progress,
        progress_text,
        process: RefCell::new(None),
    });

    {
        let ui = ui.clone();
        back.connect_clicked(move |_| ui.go(-1));
    }
    {
        let ui = ui.clone();
        next.connect_clicked(move |_| ui.advance());
    }
    {
        // Don't let the window close mid-install.
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            if ui.page() == "installing" { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
    }
    ui.show_page();
    window.present();
}

impl Ui {
    fn page(&self) -> &'static str {
        self.pages[*self.current.borrow()]
    }

    fn go(&self, step: i32) {
        let index = (*self.current.borrow() as i32 + step).clamp(0, self.pages.len() as i32 - 1) as usize;
        *self.current.borrow_mut() = index;
        self.show_page();
    }

    fn show_page(&self) {
        let page = self.page();
        self.error.set_text("");
        self.stack.set_visible_child_name(page);
        self.back.set_visible(!matches!(page, "welcome" | "installing" | "done"));
        self.next.set_visible(!matches!(page, "installing" | "done"));
        self.next.set_label(if page == "review" { "Erase Disk and Install" } else { "Continue" });
        if page == "review" {
            self.next.add_css_class("destructive-action");
            self.next.remove_css_class("suggested-action");
        } else {
            self.next.remove_css_class("destructive-action");
            self.next.add_css_class("suggested-action");
        }
        match page {
            "disk" => self.load_disks(),
            "review" => self.fill_summary(),
            _ => {}
        }
    }

    fn fail(&self, message: &str) {
        self.error.set_text(message);
    }

    /// Check the current page, store its answers, and move on.
    fn advance(self: &Rc<Self>) {
        match self.page() {
            "disk" => {
                let Some(row) = self.disk_list.selected_row() else { return self.fail("Choose a disk.") };
                let disk = self.disks.borrow().get(row.index() as usize).cloned();
                match disk {
                    Some(d) if d.unusable.is_none() => self.choices.borrow_mut().disk = Some(d),
                    Some(d) => return self.fail(&format!("Can't use this disk: {}", d.unusable.unwrap())),
                    None => return,
                }
            }
            "you" => {
                let username = self.username.text().to_string();
                let valid = !username.is_empty()
                    && username.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
                if !valid {
                    return self.fail("Usernames use lowercase letters and digits, starting with a letter.");
                }
                if username == "root" || username == "noros" {
                    return self.fail("That username is reserved.");
                }
                if self.password.text().is_empty() {
                    return self.fail("Choose a password.");
                }
                if self.password.text() != self.password2.text() {
                    return self.fail("The passwords don't match.");
                }
                let hostname = self.hostname.text().trim().to_string();
                if hostname.is_empty() || !hostname.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return self.fail("Computer names use letters, digits and -.");
                }
                let mut c = self.choices.borrow_mut();
                c.full_name = self.name.text().trim().to_string();
                if c.full_name.is_empty() {
                    c.full_name = username.clone();
                }
                c.username = username;
                c.password = self.password.text().to_string();
                c.hostname = hostname;
            }
            "encryption" => {
                let mut c = self.choices.borrow_mut();
                c.encrypt = self.encrypt.is_active();
                if c.encrypt {
                    let key = if self.same_password.is_active() {
                        c.password.clone()
                    } else {
                        if self.key.text() != self.key2.text() {
                            drop(c);
                            return self.fail("The encryption passwords don't match.");
                        }
                        self.key.text().to_string()
                    };
                    if key.chars().count() < 8 {
                        drop(c);
                        return self.fail("The encryption password needs at least 8 characters. Use a longer one here, or a longer account password.");
                    }
                    c.encryption_password = key;
                }
            }
            "review" => {
                if !self.confirm.is_active() {
                    return self.fail("Tick the box to confirm the disk may be erased.");
                }
                self.go(1);
                self.start_install();
                return;
            }
            _ => {}
        }
        self.go(1);
    }

    fn load_disks(&self) {
        while let Some(child) = self.disk_list.first_child() {
            self.disk_list.remove(&child);
        }
        let disks: Vec<Disk> = std::process::Command::new("noros-install")
            .arg("--list-disks")
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice(&o.stdout).ok())
            .unwrap_or_default();
        if disks.is_empty() {
            self.fail("No disks found.");
        }
        for disk in &disks {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            row.add_css_class("disk-row");
            let kind = if disk.removable { "Removable" } else { "Internal" };
            row.append(&label(&format!("{} — {}", disk.model, human_size(disk.size)), "disk-name"));
            let detail = match &disk.unusable {
                Some(why) => format!("{} · {} · not available: {why}", disk.path, kind),
                None => format!("{} · {} {}", disk.path, kind, disk.transport.to_uppercase()),
            };
            row.append(&label(&detail, "disk-detail"));
            let list_row = gtk::ListBoxRow::new();
            list_row.set_child(Some(&row));
            list_row.set_sensitive(disk.unusable.is_none());
            self.disk_list.append(&list_row);
        }
        if let Some(first) = disks.iter().position(|d| d.unusable.is_none()) {
            self.disk_list.select_row(self.disk_list.row_at_index(first as i32).as_ref());
        }
        *self.disks.borrow_mut() = disks;
    }

    fn fill_summary(&self) {
        let c = self.choices.borrow();
        let disk = c.disk.as_ref().map(|d| format!("{} ({}, {})", d.model, human_size(d.size), d.path)).unwrap_or_default();
        self.summary.set_text(&format!(
            "Disk: {disk}\nAccount: {} ({})\nComputer name: {}\nEncryption: {}",
            c.full_name,
            c.username,
            c.hostname,
            if c.encrypt { "on" } else { "off — anyone with the disk can read your files" },
        ));
        self.confirm.set_active(false);
    }

    fn start_install(self: &Rc<Self>) {
        let c = self.choices.borrow();
        let Some(disk) = &c.disk else { return };
        let mut argv: Vec<&str> = vec![
            "sudo", "-n", "noros-install", "--progress-lines", "--yes",
            "--disk", &disk.path, "--user", &c.username, "--name", &c.full_name, "--hostname", &c.hostname,
        ];
        if !c.encrypt {
            argv.push("--no-encryption");
        }
        let argv: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
        let process = match gio::Subprocess::newv(
            &argv,
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
        ) {
            Ok(p) => p,
            Err(err) => return self.install_failed(&err.to_string()),
        };

        // Passwords travel over the pipe.
        let mut secrets = format!("{}\n", c.password);
        if c.encrypt {
            secrets.push_str(&format!("{}\n", c.encryption_password));
        }
        if let Some(stdin) = process.stdin_pipe() {
            let _ = stdin.write_all(secrets.as_bytes(), gio::Cancellable::NONE);
            let _ = stdin.close(gio::Cancellable::NONE);
        }
        drop(c);

        let stdout = gio::DataInputStream::new(&process.stdout_pipe().unwrap());
        read_progress(self.clone(), stdout, Rc::new(RefCell::new(Vec::new())));
        // Keep the process alive as long as we're reading from it.
        *self.process.borrow_mut() = Some(process);
    }

    fn install_failed(&self, message: &str) {
        *self.current.borrow_mut() = self.pages.iter().position(|p| *p == "review").unwrap();
        self.show_page();
        self.fail(&format!("Installation failed: {message}"));
    }
}

/// Read `PROGRESS n text` / `DONE` / `ERROR text` lines until the installer finishes.
fn read_progress(ui: Rc<Ui>, stream: gio::DataInputStream, other_output: Rc<RefCell<Vec<String>>>) {
    let stream2 = stream.clone();
    stream.read_line_utf8_async(glib::Priority::DEFAULT, gio::Cancellable::NONE, move |result| {
        let line = match result {
            Ok(Some(line)) => line.to_string(),
            _ => {
                let tail = other_output.borrow().iter().rev().take(3).cloned().collect::<Vec<_>>().join(" ");
                return ui.install_failed(if tail.is_empty() { "the installer stopped unexpectedly" } else { &tail });
            }
        };
        if let Some(rest) = line.strip_prefix("PROGRESS ") {
            let (pct, text) = rest.split_once(' ').unwrap_or((rest, ""));
            ui.progress.set_fraction(pct.parse::<f64>().unwrap_or(0.0) / 100.0);
            ui.progress_text.set_text(text);
        } else if line == "DONE" {
            ui.go(1);
            return;
        } else if let Some(message) = line.strip_prefix("ERROR ") {
            return ui.install_failed(message);
        } else if !line.trim().is_empty() {
            other_output.borrow_mut().push(line);
        }
        read_progress(ui, stream2, other_output);
    });
}
