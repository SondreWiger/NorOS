//! Loads the pointer image from the installed cursor theme.

use std::io::Read;

use xcursor::{CursorTheme, parser::{Image, parse_xcursor}};

pub struct Cursor {
    image: Image,
}

impl Cursor {
    pub fn load() -> Self {
        let theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "Adwaita".into());
        let size: u32 = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
        let image = load_theme_image(&theme, size).unwrap_or_else(|| {
            tracing::warn!(theme, "cursor theme not found, using built-in arrow");
            fallback_arrow()
        });
        Self { image }
    }

    pub fn image(&self) -> &Image {
        &self.image
    }
}

fn load_theme_image(theme: &str, size: u32) -> Option<Image> {
    let path = CursorTheme::load(theme).load_icon("default").or_else(|| CursorTheme::load(theme).load_icon("left_ptr"))?;
    let mut data = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut data).ok()?;
    let images = parse_xcursor(&data)?;
    images.into_iter().min_by_key(|img| (img.size as i32 - size as i32).abs())
}

/// A small white arrow with a dark outline, drawn in code so Fjord never runs without a pointer.
fn fallback_arrow() -> Image {
    const W: u32 = 16;
    const H: u32 = 24;
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        // The arrow widens one pixel per row, then the tail narrows.
        let width = if y < 16 { y + 1 } else { 0 };
        for x in 0..W {
            let inside = x < width || (y >= 16 && y < 22 && x >= 5 + (y - 16) / 2 && x < 8 + (y - 16) / 2);
            let edge = x == 0 || x + 1 == width || y == 15 && x < 16;
            let idx = ((y * W + x) * 4) as usize;
            if inside {
                let v = if edge { 20 } else { 250 };
                pixels[idx..idx + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
    }
    Image {
        size: 24,
        width: W,
        height: H,
        xhot: 0,
        yhot: 0,
        delay: 0,
        pixels_rgba: pixels,
        pixels_argb: vec![],
    }
}
