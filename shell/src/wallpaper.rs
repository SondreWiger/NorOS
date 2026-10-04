//! The desktop background: a user image if one is set, otherwise a drawn
//! aurora over Nordic mountains.

use std::f64::consts::PI;

use gtk::{cairo, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub fn build(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(app);
    window.add_css_class("noros-wallpaper");
    window.init_layer_shell();
    window.set_namespace(Some("noros-wallpaper"));
    window.set_layer(Layer::Background);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(-1);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }

    let user_image = ["png", "jpg", "jpeg", "webp"]
        .iter()
        .map(|ext| gtk::glib::user_config_dir().join("noros").join(format!("wallpaper.{ext}")))
        .find(|p| p.exists());

    if let Some(path) = user_image {
        let picture = gtk::Picture::for_filename(path);
        picture.set_content_fit(gtk::ContentFit::Cover);
        window.set_child(Some(&picture));
    } else {
        let area = gtk::DrawingArea::new();
        area.set_draw_func(|_, cr, w, h| draw_aurora(cr, w as f64, h as f64));
        window.set_child(Some(&area));
    }
    window.present();
}

/// Tiny deterministic random source so the sky looks the same on every boot.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f64 / (1u32 << 24) as f64
    }
}

fn draw_aurora(cr: &cairo::Context, w: f64, h: f64) {
    // Night sky.
    let sky = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    sky.add_color_stop_rgb(0.0, 0.020, 0.031, 0.067);
    sky.add_color_stop_rgb(0.55, 0.055, 0.094, 0.180);
    sky.add_color_stop_rgb(1.0, 0.090, 0.161, 0.267);
    let _ = cr.set_source(&sky);
    let _ = cr.paint();

    // Stars, denser near the top.
    let mut rng = Lcg(0x4e6f_724f);
    for _ in 0..((w * h) / 9000.0) as usize {
        let x = rng.next() * w;
        let y = rng.next().powf(1.6) * h * 0.7;
        let r = 0.4 + rng.next() * 1.1;
        let a = 0.25 + rng.next() * 0.6;
        cr.set_source_rgba(0.85, 0.92, 1.0, a);
        cr.arc(x, y, r, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }

    // Aurora: soft ribbons, each a vertical gradient fading upward.
    let ribbons: [(f64, f64, (f64, f64, f64), f64); 4] = [
        (0.30, 0.16, (0.24, 0.86, 0.59), 0.55),
        (0.38, 0.12, (0.20, 0.78, 0.82), 0.40),
        (0.25, 0.10, (0.45, 0.95, 0.65), 0.30),
        (0.44, 0.09, (0.52, 0.45, 0.95), 0.22),
    ];
    for (i, (base, height, (r, g, b), alpha)) in ribbons.into_iter().enumerate() {
        let base_y = h * base;
        let top_y = base_y - h * height;
        let phase = i as f64 * 1.7;
        let wave = |x: f64| (x / w * 2.0 * PI * 1.3 + phase).sin() * h * 0.045 + (x / w * 2.0 * PI * 0.4 + phase).cos() * h * 0.03;

        cr.new_path();
        let steps = 48;
        for s in 0..=steps {
            let x = w * s as f64 / steps as f64;
            let y = base_y + wave(x);
            if s == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        for s in (0..=steps).rev() {
            let x = w * s as f64 / steps as f64;
            cr.line_to(x, top_y + wave(x) * 1.4);
        }
        cr.close_path();

        let glow = cairo::LinearGradient::new(0.0, top_y, 0.0, base_y + h * 0.05);
        glow.add_color_stop_rgba(0.0, r, g, b, 0.0);
        glow.add_color_stop_rgba(0.75, r, g, b, alpha * 0.6);
        glow.add_color_stop_rgba(1.0, r, g, b, 0.0);
        let _ = cr.set_source(&glow);
        let _ = cr.fill();
    }

    // Mountains: three ranges, darker toward the viewer.
    let ranges: [(f64, f64, (f64, f64, f64), u32); 3] = [
        (0.70, 0.20, (0.075, 0.129, 0.208), 11),
        (0.78, 0.17, (0.047, 0.086, 0.149), 9),
        (0.88, 0.14, (0.024, 0.043, 0.082), 7),
    ];
    for (layer, (base, peak, (r, g, b), peaks)) in ranges.into_iter().enumerate() {
        let mut rng = Lcg(0x6672_6a00 + layer as u32);
        cr.new_path();
        cr.move_to(0.0, h);
        let base_y = h * base;
        cr.line_to(0.0, base_y);
        let step = w / peaks as f64;
        for p in 0..peaks {
            let x0 = p as f64 * step;
            let summit_x = x0 + step * (0.3 + rng.next() * 0.4);
            let summit_y = base_y - h * peak * (0.35 + rng.next() * 0.65);
            let valley_y = base_y - h * peak * rng.next() * 0.25;
            cr.line_to(summit_x, summit_y);
            cr.line_to(x0 + step, valley_y);
        }
        cr.line_to(w, h);
        cr.close_path();
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill();
    }

    // Still water at the bottom of the fjord.
    let water = cairo::LinearGradient::new(0.0, h * 0.93, 0.0, h);
    water.add_color_stop_rgba(0.0, 0.10, 0.25, 0.32, 0.0);
    water.add_color_stop_rgba(1.0, 0.10, 0.25, 0.32, 0.35);
    cr.rectangle(0.0, h * 0.93, w, h * 0.07);
    let _ = cr.set_source(&water);
    let _ = cr.fill();
}
