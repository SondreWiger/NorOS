//! Keyboard and pointer input, including Fjord's global shortcuts.

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, InputTime, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        session::Session,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
};
use tracing::info;

use crate::{grabs::MoveSurfaceGrab, state::Fjord};

const BTN_LEFT: u32 = 0x110;
const TERMINAL: &str = "foot";

#[derive(Debug, Clone, Copy)]
enum Action {
    None,
    Quit,
    VtSwitch(i32),
    Launcher,
    Terminal,
    CloseWindow,
    CycleWindows,
    ToggleMaximize,
}

impl Fjord {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => self.on_keyboard::<I>(event),
            InputEvent::PointerMotion { event, .. } => self.on_pointer_motion::<I>(event),
            InputEvent::PointerMotionAbsolute { event, .. } => self.on_pointer_motion_absolute::<I>(event),
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<I>(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<I>(event),
            _ => {}
        }
    }

    fn on_keyboard<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time(&event);
        let keycode = event.key_code();
        let state = event.state();
        let keyboard = self.seat.get_keyboard().unwrap();
        let mut suppressed = std::mem::take(&mut self.suppressed_keys);

        let action = keyboard
            .input(self, keycode, state, serial, time, |_, mods, handle| {
                let keysym = handle.modified_sym();
                if state == KeyState::Pressed {
                    let action = shortcut(mods.logo, mods.ctrl, mods.alt, keysym);
                    if let Some(action) = action {
                        suppressed.push(keysym);
                        return FilterResult::Intercept(action);
                    }
                    FilterResult::Forward
                } else if suppressed.contains(&keysym) {
                    suppressed.retain(|k| *k != keysym);
                    FilterResult::Intercept(Action::None)
                } else {
                    FilterResult::Forward
                }
            })
            .unwrap_or(Action::None);
        self.suppressed_keys = suppressed;

        match action {
            Action::None => {}
            Action::Quit => {
                info!("quit requested");
                self.loop_signal.stop();
            }
            Action::VtSwitch(vt) => {
                if let Err(err) = self.backend.session.change_vt(vt) {
                    tracing::warn!("vt switch failed: {err}");
                }
            }
            Action::Launcher => self.toggle_launcher(),
            Action::Terminal => {
                self.spawn(TERMINAL);
            }
            Action::CloseWindow => self.close_focused(),
            Action::CycleWindows => self.cycle_windows(),
            Action::ToggleMaximize => self.toggle_maximize_focused(),
        }
    }

    fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let Some(geo) = self.primary_output().and_then(|o| self.space.output_geometry(&o)) else {
            return pos;
        };
        let max_x = (geo.loc.x + geo.size.w) as f64 - 1.0;
        let max_y = (geo.loc.y + geo.size.h) as f64 - 1.0;
        (pos.x.clamp(geo.loc.x as f64, max_x), pos.y.clamp(geo.loc.y as f64, max_y)).into()
    }

    fn move_pointer_to(&mut self, location: Point<f64, Logical>, time: InputTime) {
        let pointer = self.seat.get_pointer().unwrap();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        let pointer = self.seat.get_pointer().unwrap();
        let location = self.clamp_to_outputs(pointer.current_location() + event.delta());
        self.move_pointer_to(location, event.time());
    }

    fn on_pointer_motion_absolute<I: InputBackend>(&mut self, event: I::PointerMotionAbsoluteEvent) {
        let Some(geo) = self.primary_output().and_then(|o| self.space.output_geometry(&o)) else {
            return;
        };
        let location = event.position_transformed(geo.size) + geo.loc.to_f64();
        self.move_pointer_to(location, event.time());
    }

    fn on_pointer_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = event.state();

        if state == ButtonState::Pressed && !pointer.is_grabbed() {
            let location = pointer.current_location();
            let super_held = keyboard.modifier_state().logo;

            if let Some(layer) = self.layer_at(location) {
                // Panels that accept the keyboard (search fields, menus) get focus on click.
                if layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None {
                    keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
                }
            } else if let Some((window, loc)) = self.space.element_under(location).map(|(w, l)| (w.clone(), l)) {
                self.focus_window(Some(window.clone()), serial);
                // Super + drag moves a window from anywhere inside it.
                if super_held && button == BTN_LEFT {
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };
                    let grab = MoveSurfaceGrab {
                        start_data,
                        window,
                        initial_window_location: loc,
                    };
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                    return;
                }
            } else {
                self.focus_window(None, serial);
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    /// The top/overlay layer surface (menu bar, dock, launcher) under a point, if any.
    fn layer_at(&self, pos: Point<f64, Logical>) -> Option<smithay::desktop::LayerSurface> {
        let output = self.primary_output()?;
        let geo = self.space.output_geometry(&output)?;
        let map = smithay::desktop::layer_map_for_output(&output);
        let local = pos - geo.loc.to_f64();
        map.layer_under(Layer::Overlay, local)
            .or_else(|| map.layer_under(Layer::Top, local))
            .cloned()
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();
        let horizontal = event
            .amount(Axis::Horizontal)
            .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
        let vertical = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);

        let mut frame = AxisFrame::new(event.time()).source(source);
        if horizontal != 0.0 {
            frame = frame.value(Axis::Horizontal, horizontal);
            if let Some(discrete) = event.amount_v120(Axis::Horizontal) {
                frame = frame.v120(Axis::Horizontal, discrete as i32);
            }
        }
        if vertical != 0.0 {
            frame = frame.value(Axis::Vertical, vertical);
            if let Some(discrete) = event.amount_v120(Axis::Vertical) {
                frame = frame.v120(Axis::Vertical, discrete as i32);
            }
        }
        if source == AxisSource::Finger {
            if event.amount(Axis::Horizontal) == Some(0.0) {
                frame = frame.stop(Axis::Horizontal);
            }
            if event.amount(Axis::Vertical) == Some(0.0) {
                frame = frame.stop(Axis::Vertical);
            }
        }

        let pointer = self.seat.get_pointer().unwrap();
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

/// Global shortcuts. Super is the Command key on Mac keyboards.
fn shortcut(logo: bool, ctrl: bool, alt: bool, keysym: Keysym) -> Option<Action> {
    let raw = keysym.raw();
    // Ctrl+Alt+F1..F12 arrives as XF86Switch_VT_1..12.
    if (0x1008FE01..=0x1008FE0C).contains(&raw) {
        return Some(Action::VtSwitch((raw - 0x1008FE01 + 1) as i32));
    }
    if ctrl && alt && keysym == Keysym::BackSpace {
        return Some(Action::Quit);
    }
    if alt && !logo && keysym == Keysym::Tab {
        return Some(Action::CycleWindows);
    }
    if !logo {
        return None;
    }
    match keysym {
        Keysym::space => Some(Action::Launcher),
        Keysym::Return => Some(Action::Terminal),
        Keysym::q | Keysym::w => Some(Action::CloseWindow),
        Keysym::Tab => Some(Action::CycleWindows),
        Keysym::Up | Keysym::m => Some(Action::ToggleMaximize),
        _ => None,
    }
}
