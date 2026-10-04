//! The desktop background: a user image if one is set, otherwise a drawn
//! aurora over Nordic mountains.

use std::f64::consts::PI;

use gtk::{cairo, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub fn build(app: &gtk::Application, config: &lys::Config) -> gtk::ApplicationWindow {
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

    let choice = config.desktop.wallpaper.as_str();
    let image = std::path::Path::new(choice);
    if image.is_absolute() && image.exists() {
        let picture = gtk::Picture::for_filename(image);
        picture.set_content_fit(gtk::ContentFit::Cover);
        window.set_child(Some(&picture));
    } else {
        let scene = Scene::named(choice);
        let area = gtk::DrawingArea::new();
        area.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, &scene));
        window.set_child(Some(&area));
    }
    window.present();
    window
}

type Rgb = (f64, f64, f64);

/// Colors for one of the drawn wallpapers.
struct Scene {
    sky: [Rgb; 3],
    ribbons: [Rgb; 4],
    mountains: [Rgb; 3],
    stars: bool,
}

impl Scene {
    fn named(name: &str) -> Self {
        match name {
            "aurora-violet" => Scene {
                sky: [(0.035, 0.020, 0.075), (0.090, 0.055, 0.180), (0.180, 0.100, 0.290)],
                ribbons: [(0.69, 0.49, 1.0), (0.52, 0.45, 0.95), (0.90, 0.45, 0.85), (0.24, 0.86, 0.80)],
                mountains: [(0.110, 0.075, 0.200), (0.075, 0.050, 0.145), (0.040, 0.025, 0.085)],
                stars: true,
            },
            "glacier" => Scene {
                sky: [(0.55, 0.72, 0.88), (0.72, 0.84, 0.93), (0.90, 0.95, 0.98)],
                ribbons: [(1.0, 1.0, 1.0), (0.80, 0.92, 1.0), (0.95, 0.98, 1.0), (0.70, 0.85, 0.95)],
                mountains: [(0.62, 0.72, 0.82), (0.45, 0.56, 0.68), (0.28, 0.36, 0.47)],
                stars: false,
            },
            "midnight" => Scene {
                sky: [(0.16, 0.12, 0.30), (0.62, 0.36, 0.42), (0.98, 0.70, 0.42)],
                ribbons: [(1.0, 0.80, 0.50), (0.98, 0.60, 0.50), (1.0, 0.90, 0.70), (0.85, 0.50, 0.60)],
                mountains: [(0.30, 0.18, 0.30), (0.20, 0.12, 0.22), (0.10, 0.06, 0.12)],
                stars: false,
            },
            _ => Scene {
                sky: [(0.020, 0.031, 0.067), (0.055, 0.094, 0.180), (0.090, 0.161, 0.267)],
                ribbons: [(0.24, 0.86, 0.59), (0.20, 0.78, 0.82), (0.45, 0.95, 0.65), (0.52, 0.45, 0.95)],
                mountains: [(0.075, 0.129, 0.208), (0.047, 0.086, 0.149), (0.024, 0.043, 0.082)],
                stars: true,
            },
        }
    }
}

/// Tiny deterministic random source so the sky looks the same on every boot.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f64 / (1u32 << 24) as f64
    }
}

fn draw(cr: &cairo::Context, w: f64, h: f64, scene: &Scene) {
    // Night sky.
    let sky = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    for (stop, (r, g, b)) in [0.0, 0.55, 1.0].into_iter().zip(scene.sky) {
        sky.add_color_stop_rgb(stop, r, g, b);
    }
    let _ = cr.set_source(&sky);
    let _ = cr.paint();

    // Stars, denser near the top.
    let mut rng = Lcg(0x4e6f_724f);
    let star_count = if scene.stars { ((w * h) / 9000.0) as usize } else { 0 };
    for _ in 0..star_count {
        let x = rng.next() * w;
        let y = rng.next().powf(1.6) * h * 0.7;
        let r = 0.4 + rng.next() * 1.1;
        let a = 0.25 + rng.next() * 0.6;
        cr.set_source_rgba(0.85, 0.92, 1.0, a);
        cr.arc(x, y, r, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }

    // Aurora: soft ribbons, each a vertical gradient fading upward.
    let shapes: [(f64, f64, f64); 4] = [(0.30, 0.16, 0.55), (0.38, 0.12, 0.40), (0.25, 0.10, 0.30), (0.44, 0.09, 0.22)];
    let ribbons = shapes.into_iter().zip(scene.ribbons).map(|((base, height, alpha), color)| (base, height, color, alpha));
    for (i, (base, height, (r, g, b), alpha)) in ribbons.enumerate() {
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
    let ranges: [(f64, f64, u32); 3] = [(0.70, 0.20, 11), (0.78, 0.17, 9), (0.88, 0.14, 7)];
    for (layer, ((base, peak, peaks), (r, g, b))) in ranges.into_iter().zip(scene.mountains).enumerate() {
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
