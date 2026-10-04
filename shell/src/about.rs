//! "About NorOS": what this machine is, and the privacy promise.

use gtk::prelude::*;

use crate::{RELEASE_NAME, VERSION};

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn cpu_model() -> String {
    let info = read("/proc/cpuinfo");
    info.lines()
        .find(|l| l.starts_with("model name") || l.starts_with("Model") || l.starts_with("Hardware"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            let cores = info.lines().filter(|l| l.starts_with("processor")).count().max(1);
            format!("{cores}-core {}", std::env::consts::ARCH)
        })
}

fn memory() -> String {
    read("/proc/meminfo")
        .lines()
        .find(|l| l.starts_with("MemTotal"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map(|kb| format!("{:.1} GB", kb / 1024.0 / 1024.0))
        .unwrap_or_else(|| "unknown".into())
}

pub fn build(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("About NorOS"));
    window.set_default_size(520, 0);
    window.set_resizable(false);
    window.add_css_class("noros-about");

    let header = gtk::HeaderBar::new();
    header.set_show_title_buttons(true);
    header.set_title_widget(Some(&gtk::Label::new(None)));
    window.set_titlebar(Some(&header));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.add_css_class("about");

    let logo = gtk::Label::new(Some("NorOS"));
    logo.add_css_class("about-logo");
    logo.set_xalign(0.0);
    content.append(&logo);

    let version = gtk::Label::new(Some(&format!("Version {VERSION} — {RELEASE_NAME}")));
    version.add_css_class("about-version");
    version.set_xalign(0.0);
    content.append(&version);

    let grid = gtk::Grid::new();
    grid.add_css_class("about-grid");
    grid.set_column_spacing(24);
    grid.set_row_spacing(6);
    let kernel = read("/proc/sys/kernel/osrelease").trim().to_string();
    let rows = [
        ("Processor", cpu_model()),
        ("Memory", memory()),
        ("Architecture", std::env::consts::ARCH.to_string()),
        ("Kernel", format!("Linux {kernel}")),
        ("Desktop", "Fjord compositor".to_string()),
    ];
    for (i, (key, value)) in rows.into_iter().enumerate() {
        let k = gtk::Label::new(Some(key));
        k.add_css_class("about-key");
        k.set_xalign(0.0);
        let v = gtk::Label::new(Some(&value));
        v.add_css_class("about-value");
        v.set_xalign(0.0);
        v.set_selectable(true);
        grid.attach(&k, 0, i as i32, 1, 1);
        grid.attach(&v, 1, i as i32, 1, 1);
    }
    content.append(&grid);

    let promise = gtk::Label::new(Some(
        "No telemetry. No accounts. No cloud.\nNothing leaves this machine unless you say so.",
    ));
    promise.add_css_class("about-promise");
    promise.set_xalign(0.0);
    content.append(&promise);

    window.set_child(Some(&content));
    window.present();
}
