//! Lys — the NorOS theme engine.
//!
//! All look-and-layout choices live in one human-readable file,
//! `~/.config/noros/config.toml`. The Settings app writes it, the desktop shell
//! watches it, and this crate turns it into CSS:
//!
//! * shell CSS (menu bar, dock, launcher), loaded by noros-shell, and
//! * app CSS + GTK settings (`~/.config/gtk-4.0/`), picked up by every GTK app.
//!
//! The user's own `~/.config/noros/theme.css` always loads last and wins.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

// ── Paths ────────────────────────────────────────────────────────────

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".config"))
}

pub fn noros_dir() -> PathBuf {
    config_home().join("noros")
}

pub fn config_path() -> PathBuf {
    noros_dir().join("config.toml")
}

/// The user's own CSS, loaded after everything else.
pub fn user_css_path() -> PathBuf {
    noros_dir().join("theme.css")
}

/// Where a chosen wallpaper image is copied to.
pub fn wallpaper_dir() -> PathBuf {
    noros_dir().join("wallpapers")
}

/// Running from the live ISO (nothing is saved)?
pub fn is_live() -> bool {
    fs::read_to_string("/proc/cmdline").map(|c| c.contains("NOROS_LIVE")).unwrap_or(false)
}

// ── Configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockPosition {
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ButtonStyle {
    /// Red, yellow and green circles.
    Traffic,
    /// The toolkit's plain icons.
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub mode: Mode,
    /// Accent color as `#RRGGBB`.
    pub accent: String,
    /// Base corner radius in pixels for panels, menus and dialogs.
    pub corner_radius: u32,
    /// How opaque the menu bar and dock are, 0.3–1.0.
    pub panel_opacity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Desktop {
    /// A built-in wallpaper name (see [`WALLPAPERS`]) or a path to an image.
    pub wallpaper: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dock {
    pub visible: bool,
    pub position: DockPosition,
    /// Size of each dock tile in pixels.
    pub icon_size: u32,
    /// Desktop file IDs (e.g. `foot.desktop`) or `noros-search`.
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuBar {
    pub visible: bool,
    pub position: BarPosition,
    pub show_date: bool,
    pub clock_24h: bool,
    pub show_seconds: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Windows {
    pub buttons_side: Side,
    pub button_style: ButtonStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Privacy {
    /// Keep a local list of which apps you opened. Off unless you turn it on.
    pub activity_history: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub appearance: Appearance,
    pub desktop: Desktop,
    pub dock: Dock,
    pub menu_bar: MenuBar,
    pub windows: Windows,
    pub privacy: Privacy,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: Mode::Dark,
            accent: "#3DDC97".into(),
            corner_radius: 10,
            panel_opacity: 0.78,
        }
    }
}

impl Default for Desktop {
    fn default() -> Self {
        Self { wallpaper: "aurora".into() }
    }
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            visible: true,
            position: DockPosition::Bottom,
            icon_size: 52,
            apps: DEFAULT_DOCK_APPS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            visible: true,
            position: BarPosition::Top,
            show_date: true,
            clock_24h: true,
            show_seconds: false,
        }
    }
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            buttons_side: Side::Left,
            button_style: ButtonStyle::Traffic,
        }
    }
}

pub const DEFAULT_DOCK_APPS: &[&str] = &[
    "noros-search",
    "no.noros.Files.desktop",
    "foot.desktop",
    "no.noros.Text.desktop",
    "no.noros.Privacy.desktop",
    "no.noros.Settings.desktop",
];

/// Accent colors offered in Settings. Any `#RRGGBB` works in the file.
pub const ACCENTS: &[(&str, &str)] = &[
    ("Aurora", "#3DDC97"),
    ("Glacier", "#7FD1FF"),
    ("Fjord", "#4C8DFF"),
    ("Heather", "#B07CFF"),
    ("Lingonberry", "#E5484D"),
    ("Cloudberry", "#F5A524"),
    ("Moss", "#8BC34A"),
    ("Granite", "#9AA8BC"),
];

/// Built-in drawn wallpapers: (id, display name).
pub const WALLPAPERS: &[(&str, &str)] = &[
    ("aurora", "Aurora"),
    ("aurora-violet", "Violet Night"),
    ("glacier", "Glacier"),
    ("midnight", "Midnight Sun"),
];

impl Config {
    /// Reads the config file; anything missing or unreadable falls back to defaults.
    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .map(Config::sanitized)
            .unwrap_or_default()
    }

    /// Writes atomically, so watchers never see a half-written file.
    pub fn save(&self) -> io::Result<()> {
        let path = config_path();
        fs::create_dir_all(path.parent().unwrap())?;
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, format!("# NorOS settings. Edit here or in the Settings app.\n\n{text}"))?;
        fs::rename(tmp, path)
    }

    fn sanitized(mut self) -> Self {
        if parse_hex(&self.appearance.accent).is_none() {
            self.appearance.accent = Appearance::default().accent;
        }
        self.appearance.corner_radius = self.appearance.corner_radius.min(28);
        self.appearance.panel_opacity = self.appearance.panel_opacity.clamp(0.3, 1.0);
        self.dock.icon_size = self.dock.icon_size.clamp(32, 96);
        self
    }

    pub fn accent_rgb(&self) -> (u8, u8, u8) {
        parse_hex(&self.appearance.accent).unwrap_or((0x3D, 0xDC, 0x97))
    }
}

// ── Activity history ─────────────────────────────────────────────────

/// Where the opt-in activity history lives. It never leaves this file.
pub fn history_path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".local/share"));
    data.join("noros/activity.log")
}

/// Note that an app was opened — only if the user turned history on.
pub fn record_activity(what: &str) {
    if !Config::load().privacy.activity_history {
        return;
    }
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{secs}\t{}\n", what.replace(['\t', '\n'], " "));
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// (seconds since epoch, what), newest first.
pub fn read_activity() -> Vec<(u64, String)> {
    let mut entries: Vec<(u64, String)> = fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (secs, what) = l.split_once('\t')?;
            Some((secs.parse().ok()?, what.to_string()))
        })
        .collect();
    entries.reverse();
    entries
}

pub fn clear_activity() -> io::Result<()> {
    match fs::remove_file(history_path()) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

// ── Colors ───────────────────────────────────────────────────────────

pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

pub fn to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Black or white, whichever reads better on top of `rgb`.
fn readable_on((r, g, b): (u8, u8, u8)) -> &'static str {
    let lum = 0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64;
    if lum > 150.0 { "#0B1220" } else { "#FFFFFF" }
}

struct Palette {
    panel: &'static str,
    surface: &'static str,
    text: &'static str,
    muted: &'static str,
    line: &'static str,
}

const DARK: Palette = Palette {
    panel: "#0B1220",
    surface: "#13233F",
    text: "#E8EEF7",
    muted: "#9AA8BC",
    line: "rgba(255,255,255,0.09)",
};

const LIGHT: Palette = Palette {
    panel: "#F4F7FB",
    surface: "#FFFFFF",
    text: "#0B1220",
    muted: "#4A5B78",
    line: "rgba(11,18,32,0.12)",
};

fn palette(mode: Mode) -> &'static Palette {
    match mode {
        Mode::Dark => &DARK,
        Mode::Light => &LIGHT,
    }
}

// ── Generated CSS ────────────────────────────────────────────────────

/// CSS for the desktop shell. Loaded on top of the static NorOS theme.
pub fn shell_css(config: &Config) -> String {
    let p = palette(config.appearance.mode);
    let accent = config.accent_rgb();
    let r = config.appearance.corner_radius;
    let opacity = config.appearance.panel_opacity;
    let tile = config.dock.icon_size;
    let tile_radius = (tile as f64 * 0.27).round() as u32;

    format!(
        "/* Generated by Lys from ~/.config/noros/config.toml. Do not edit; use theme.css. */
@define-color noros_accent {accent_hex};
@define-color noros_accent_fg {accent_fg};
@define-color noros_panel {panel};
@define-color noros_surface {surface};
@define-color noros_text {text};
@define-color noros_muted {muted};
@define-color noros_line {line};

.menubar {{ background: alpha(@noros_panel, {opacity}); }}
.dock {{ background: alpha(@noros_surface, {dock_opacity}); border-radius: {dock_radius}px; }}
.dock-item {{ min-width: {tile}px; min-height: {tile}px; border-radius: {tile_radius}px; }}
popover.noros-menu > contents {{ border-radius: {r}px; }}
.menu-item, .launcher-row {{ border-radius: {inner}px; }}
.launcher {{ border-radius: {outer}px; }}
.launcher-search {{ border-radius: {inner}px; }}
",
        accent_hex = to_hex(accent),
        accent_fg = readable_on(accent),
        panel = p.panel,
        surface = p.surface,
        text = p.text,
        muted = p.muted,
        line = p.line,
        dock_opacity = (opacity - 0.06).max(0.3),
        dock_radius = tile_radius + 8,
        inner = r.saturating_sub(4).max(2),
        outer = r + 6,
    )
}

/// CSS shared by every GTK app: accent color and the window buttons.
pub fn app_css(config: &Config) -> String {
    let accent = config.accent_rgb();
    let mut css = format!(
        "/* Generated by Lys from ~/.config/noros/config.toml. Do not edit; use gtk.css. */
@define-color accent_bg_color {accent};
@define-color accent_color {accent};
@define-color accent_fg_color {fg};
@define-color theme_selected_bg_color {accent};
@define-color theme_selected_fg_color {fg};

/* GTK 4.16+ themes read the accent from CSS variables. */
:root {{
  --accent-bg-color: {accent};
  --accent-color: {accent};
  --accent-fg-color: {fg};
}}

button.suggested-action {{ background: {accent}; background-image: none; color: {fg}; }}
button.suggested-action:hover {{ background: shade({accent}, 1.08); background-image: none; }}
button.suggested-action:active {{ background: shade({accent}, 0.9); background-image: none; }}
switch:checked {{ background-color: {accent}; }}
scale highlight, progressbar progress, levelbar block.filled {{ background-color: {accent}; }}
checkbutton check:checked, checkbutton radio:checked {{ background-color: {accent}; color: {fg}; border-color: {accent}; }}
.navigation-sidebar row:selected, stacksidebar row:selected {{ background-color: alpha({accent}, 0.28); }}
selection, text selection, entry selection {{ background-color: alpha({accent}, 0.4); }}
entry:focus-within, passwordentry:focus-within {{ outline-color: alpha({accent}, 0.6); }}
",
        accent = to_hex(accent),
        fg = readable_on(accent),
    );

    if config.windows.button_style == ButtonStyle::Traffic {
        // The circle is drawn on the icon, not the button, so it never stretches
        // with the height of a title bar.
        css.push_str(
            "
windowcontrols > button {
  min-width: 22px;
  min-height: 22px;
  margin: 0 1px;
  padding: 0;
  border: none;
  background: none;
  box-shadow: none;
}
windowcontrols > button > image {
  min-width: 14px;
  min-height: 14px;
  padding: 0;
  border-radius: 50%;
  -gtk-icon-size: 10px;
  color: transparent;
  box-shadow: inset 0 0 0 1px rgba(0,0,0,0.18);
}
windowcontrols:hover > button > image { color: rgba(0,0,0,0.6); }
windowcontrols > button.close > image    { background-color: #FF5F57; }
windowcontrols > button.minimize > image { background-color: #FEBC2E; }
windowcontrols > button.maximize > image { background-color: #28C840; }
windowcontrols > button:backdrop > image { background-color: rgba(127,127,127,0.35); }
",
        );
    }
    css
}

/// GTK's own settings: light/dark preference and which side the window buttons go.
pub fn gtk_settings_ini(config: &Config) -> String {
    let layout = match config.windows.buttons_side {
        Side::Left => "close,minimize,maximize:",
        Side::Right => ":minimize,maximize,close",
    };
    format!(
        "# Generated by NorOS Settings.\n[Settings]\ngtk-application-prefer-dark-theme={}\ngtk-decoration-layout={}\ngtk-font-name=Inter 11\ngtk-icon-theme-name=Adwaita\ngtk-cursor-theme-name=Adwaita\ngtk-cursor-theme-size=24\n",
        config.appearance.mode == Mode::Dark,
        layout,
    )
}

/// Writes everything GTK apps read at startup. Apps opened afterwards use the new look.
pub fn apply_to_apps(config: &Config) -> io::Result<()> {
    let gtk_dir = config_home().join("gtk-4.0");
    fs::create_dir_all(&gtk_dir)?;
    write_if_changed(&gtk_dir.join("settings.ini"), &gtk_settings_ini(config))?;
    write_if_changed(&gtk_dir.join("noros.css"), &app_css(config))?;

    // Make sure the user's gtk.css pulls in ours, without touching anything they wrote.
    let gtk_css = gtk_dir.join("gtk.css");
    let existing = fs::read_to_string(&gtk_css).unwrap_or_default();
    if !existing.contains("noros.css") {
        fs::write(
            &gtk_css,
            format!("@import url(\"noros.css\");\n/* Your own CSS for all apps goes below. */\n\n{existing}"),
        )?;
    }
    Ok(())
}

/// GTK on Wayland takes the window-button layout and color scheme from GSettings,
/// and updates running apps the moment they change.
#[cfg(feature = "gsettings")]
pub fn apply_gsettings(config: &Config) {
    use gio::prelude::*;

    let set = |schema: &str, key: &str, value: &str| {
        let Some(source) = gio::SettingsSchemaSource::default() else { return };
        let Some(found) = source.lookup(schema, true) else { return };
        if !found.has_key(key) {
            return;
        }
        let settings = gio::Settings::new(schema);
        if settings.string(key) != value {
            let _ = settings.set_string(key, value);
        }
    };
    let layout = match config.windows.buttons_side {
        Side::Left => "close,minimize,maximize:",
        Side::Right => ":minimize,maximize,close",
    };
    set("org.gnome.desktop.wm.preferences", "button-layout", layout);
    let scheme = match config.appearance.mode {
        Mode::Dark => "prefer-dark",
        Mode::Light => "prefer-light",
    };
    set("org.gnome.desktop.interface", "color-scheme", scheme);
    gio::Settings::sync();
}

fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read_to_string(path).map(|old| old == content).unwrap_or(false) {
        return Ok(());
    }
    fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let config: Config = toml::from_str("[dock]\nposition = \"left\"\n").unwrap();
        assert_eq!(config.dock.position, DockPosition::Left);
        assert_eq!(config.menu_bar, MenuBar::default());
    }

    #[test]
    fn bad_values_are_sanitized() {
        let config: Config = toml::from_str("[appearance]\naccent = \"red\"\ncorner_radius = 400\n").unwrap();
        let config = config.sanitized();
        assert_eq!(config.appearance.accent, "#3DDC97");
        assert_eq!(config.appearance.corner_radius, 28);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#3DDC97"), Some((0x3D, 0xDC, 0x97)));
        assert_eq!(parse_hex("3DDC97"), None);
        assert_eq!(to_hex((1, 2, 255)), "#0102FF");
    }
}
