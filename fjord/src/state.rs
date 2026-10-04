//! Compositor state and the Wayland protocol handlers Fjord implements.

use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    process::{Child, Command},
    sync::Arc,
};

use smithay::{
    backend::renderer::{ImportDma, utils::on_commit_buffer_handler},
    delegate_dispatch2,
    desktop::{
        LayerSurface, PopupKind, PopupKeyboardGrab, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Space,
        Window, WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
    },
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, GrabType, Source},
        keyboard::{Keysym, XkbConfig},
        pointer::{CursorImageStatus, Focus, GrabStartData as PointerGrabStartData},
    },
    output::Output,
    reexports::{
        calloop::{Interest, LoopHandle, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Clock, Logical, Monotonic, Point, Rectangle, Serial, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface, with_states},
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        pointer_constraints::PointerConstraintsHandler,
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState, set_primary_focus},
        },
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
                decoration::{XdgDecorationHandler, XdgDecorationState},
            },
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData},
    },
};
use tracing::{info, warn};

use crate::{
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab, handle_resize_commit},
    udev::Backend,
};

/// Per-window bookkeeping Fjord keeps alongside each window.
#[derive(Default)]
pub struct WindowMeta {
    /// The window has been given its initial (centered) position.
    pub placed: Cell<bool>,
    /// Geometry to return to when leaving maximized / fullscreen.
    pub restore: RefCell<Option<Rectangle<i32, Logical>>>,
}

pub fn meta(window: &Window) -> &WindowMeta {
    window.user_data().insert_if_missing(WindowMeta::default);
    window.user_data().get::<WindowMeta>().unwrap()
}

pub struct Fjord {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Fjord>,
    pub loop_signal: LoopSignal,
    pub socket_name: OsString,
    pub clock: Clock<Monotonic>,
    pub backend: Backend,

    pub space: Space<Window>,
    pub popups: PopupManager,
    /// Windows the user minimized, most recent last.
    pub minimized: Vec<Window>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Fjord>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub xdg_decoration_state: XdgDecorationState,
    pub xdg_activation_state: XdgActivationState,
    pub viewporter_state: ViewporterState,
    pub dmabuf_state: Option<(DmabufState, DmabufGlobal)>,

    pub seat: Seat<Fjord>,
    pub cursor_status: CursorImageStatus,
    pub suppressed_keys: Vec<Keysym>,
    pub launcher: Option<Child>,
}

impl Fjord {
    pub fn new(display: Display<Fjord>, loop_handle: LoopHandle<'static, Fjord>, loop_signal: LoopSignal, backend: Backend) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let viewporter_state = ViewporterState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, backend.seat_name());
        let xkb = XkbConfig {
            layout: &std::env::var("XKB_DEFAULT_LAYOUT").unwrap_or_default(),
            ..XkbConfig::default()
        };
        seat.add_keyboard(xkb, 300, 30)
            .or_else(|_| seat.add_keyboard(XkbConfig::default(), 300, 30))
            .expect("failed to initialize keyboard");
        seat.add_pointer();

        let socket = ListeningSocketSource::new_auto().expect("failed to create wayland socket");
        let socket_name = socket.socket_name().to_os_string();
        loop_handle
            .insert_source(socket, |stream, _, state| {
                if let Err(err) = state.display_handle.insert_client(stream, Arc::new(ClientState::default())) {
                    warn!("failed to add client: {err}");
                }
            })
            .expect("failed to listen on wayland socket");
        loop_handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state| {
                // Safety: the display is never dropped while the loop runs.
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                }
                Ok(PostAction::Continue)
            })
            .expect("failed to add display to event loop");
        info!(socket = ?socket_name, "listening");

        Self {
            display_handle: dh,
            loop_handle,
            loop_signal,
            socket_name,
            clock: Clock::new(),
            backend,
            space: Space::default(),
            popups: PopupManager::default(),
            minimized: Vec::new(),
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            xdg_decoration_state,
            xdg_activation_state,
            viewporter_state,
            dmabuf_state: None,
            seat,
            cursor_status: CursorImageStatus::default_named(),
            suppressed_keys: Vec::new(),
            launcher: None,
        }
    }

    // ── Queries ────────────────────────────────────────────────────────

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false))
            .cloned()
    }

    pub fn primary_output(&self) -> Option<Output> {
        self.space.outputs().next().cloned()
    }

    /// The part of the screen windows may use (screen minus menu bar, dock…).
    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        let Some(output) = self.primary_output() else {
            return Rectangle::from_size((1280, 720).into());
        };
        let output_geo = self.space.output_geometry(&output).unwrap_or_default();
        let mut zone = layer_map_for_output(&output).non_exclusive_zone();
        zone.loc += output_geo.loc;
        zone
    }

    /// The surface under a point, in stacking order: overlays and panels on top,
    /// then windows, then desktop background layers.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self
            .space
            .outputs()
            .find(|o| self.space.output_geometry(o).map(|g| g.to_f64().contains(pos)).unwrap_or(false))?;
        let output_geo = self.space.output_geometry(output)?;
        let layers = layer_map_for_output(output);

        let layer_hit = |kind: Layer| {
            layers
                .layers_on(kind)
                .rev()
                .find_map(|layer| {
                    let geo = layers.layer_geometry(layer)?;
                    let local = pos - output_geo.loc.to_f64() - geo.loc.to_f64();
                    layer
                        .surface_under(local, WindowSurfaceType::ALL)
                        .map(|(surface, loc)| (surface, (loc + geo.loc + output_geo.loc).to_f64()))
                })
        };

        if let Some(hit) = layer_hit(Layer::Overlay).or_else(|| layer_hit(Layer::Top)) {
            return Some(hit);
        }
        if let Some((window, loc)) = self.space.element_under(pos) {
            if let Some((surface, surface_loc)) = window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((surface, (surface_loc + loc).to_f64()));
            }
        }
        layer_hit(Layer::Bottom).or_else(|| layer_hit(Layer::Background))
    }

    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<LayerSurface> {
        self.space.outputs().find_map(|o| {
            layer_map_for_output(o)
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
        })
    }

    // ── Focus & stacking ──────────────────────────────────────────────

    pub fn focus_window(&mut self, window: Option<Window>, serial: Serial) {
        let keyboard = self.seat.get_keyboard().unwrap();
        if let Some(window) = &window {
            self.space.raise_element(window, true);
        }
        for w in self.space.elements() {
            let active = Some(w) == window.as_ref();
            w.set_activated(active);
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        let surface = window.and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
        keyboard.set_focus(self, surface, serial);
    }

    /// Focus whatever window ends up on top, e.g. after a window closes.
    pub fn focus_topmost(&mut self) {
        let top = self.space.elements().last().cloned();
        self.focus_window(top, SERIAL_COUNTER.next_serial());
    }

    /// Alt+Tab: bring the bottom-most window (or a minimized one) to the front.
    pub fn cycle_windows(&mut self) {
        let next = if let Some(window) = self.minimized.pop() {
            let loc = window_restore_loc(&window);
            self.space.map_element(window.clone(), loc, false);
            Some(window)
        } else {
            self.space.elements().next().cloned()
        };
        if next.is_some() {
            self.focus_window(next, SERIAL_COUNTER.next_serial());
        }
    }

    pub fn close_focused(&mut self) {
        let keyboard = self.seat.get_keyboard().unwrap();
        if let Some(focus) = keyboard.current_focus() {
            if let Some(window) = self.window_for_surface(&focus) {
                window.toplevel().unwrap().send_close();
            }
        }
    }

    pub fn toggle_maximize_focused(&mut self) {
        let keyboard = self.seat.get_keyboard().unwrap();
        if let Some(window) = keyboard.current_focus().and_then(|f| self.window_for_surface(&f)) {
            let toplevel = window.toplevel().unwrap().clone();
            if is_maximized(&toplevel) {
                self.unmaximize(&toplevel);
            } else {
                self.maximize(&toplevel);
            }
        }
    }

    fn maximize(&mut self, surface: &ToplevelSurface) {
        let Some(window) = self.window_for_surface(surface.wl_surface()) else { return };
        let zone = self.usable_area();
        if let Some(loc) = self.space.element_location(&window) {
            meta(&window).restore.replace(Some(Rectangle::new(loc, window.geometry().size)));
        }
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(zone.size);
        });
        self.space.map_element(window, zone.loc, true);
        surface.send_pending_configure();
    }

    fn unmaximize(&mut self, surface: &ToplevelSurface) {
        let Some(window) = self.window_for_surface(surface.wl_surface()) else { return };
        let restore = meta(&window).restore.take();
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = restore.map(|r| r.size);
        });
        if let Some(r) = restore {
            self.space.map_element(window, r.loc, true);
        }
        surface.send_pending_configure();
    }

    // ── Processes ─────────────────────────────────────────────────────

    pub fn spawn(&self, command: &str) -> Option<Child> {
        info!(command, "spawning");
        let mut parts = command.split_whitespace();
        let program = parts.next()?;
        Command::new(program)
            .args(parts)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env_remove("DISPLAY")
            .spawn()
            .inspect_err(|err| warn!(command, "failed to spawn: {err}"))
            .ok()
    }

    pub fn toggle_launcher(&mut self) {
        if let Some(child) = &mut self.launcher {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
                self.launcher = None;
                return;
            }
        }
        self.launcher = self.spawn("noros-shell --launcher");
    }

    // ── Commit handling ───────────────────────────────────────────────

    fn on_toplevel_commit(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_surface(surface) else { return };
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
            return;
        }

        let meta = meta(&window);
        let size = window.geometry().size;
        if !meta.placed.get() && size.w > 0 && size.h > 0 {
            meta.placed.set(true);
            let zone = self.usable_area();
            // Center new windows, with a slight cascade so stacks stay readable.
            let cascade = (self.space.elements().count() as i32 - 1).clamp(0, 8) * 28;
            let x = zone.loc.x + ((zone.size.w - size.w) / 2).max(0) + cascade;
            let y = zone.loc.y + ((zone.size.h - size.h) / 3).max(0) + cascade;
            self.space.map_element(window.clone(), (x, y), true);
            self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
        }
    }

    fn on_popup_commit(&mut self, surface: &WlSurface) {
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
        }
    }

    fn on_layer_commit(&mut self, surface: &WlSurface) {
        let Some(output) = self
            .space
            .outputs()
            .find(|o| layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some())
            .cloned()
        else {
            return;
        };

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        let layer = {
            let mut map = layer_map_for_output(&output);
            map.arrange();
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned()
        };
        let Some(layer) = layer else { return };

        if !initial_configure_sent {
            layer.layer_surface().send_configure();
            return;
        }

        // Launchers and other exclusive overlays take the keyboard as soon as they appear.
        let wants_keyboard = layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive;
        let keyboard = self.seat.get_keyboard().unwrap();
        if wants_keyboard && keyboard.current_focus().as_ref() != Some(surface) {
            keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else { return };
        let Some(output) = self.primary_output() else { return };
        let Some(output_geo) = self.space.output_geometry(&output) else { return };

        // Position of the popup's root (window or panel) in global coordinates.
        let parent_loc = if let Some(window) = self.window_for_surface(&root) {
            match self.space.element_geometry(&window) {
                Some(geo) => geo.loc,
                None => return,
            }
        } else if let Some(layer) = self.layer_for_surface(&root) {
            match layer_map_for_output(&output).layer_geometry(&layer) {
                Some(geo) => geo.loc + output_geo.loc,
                None => return,
            }
        } else {
            return;
        };

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= parent_loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

fn is_maximized(surface: &ToplevelSurface) -> bool {
    surface.with_pending_state(|state| state.states.contains(xdg_toplevel::State::Maximized))
}

fn window_restore_loc(window: &Window) -> Point<i32, Logical> {
    meta(window).restore.borrow().map(|r| r.loc).unwrap_or_default()
}

fn check_grab(seat: &Seat<Fjord>, surface: &WlSurface, serial: Serial) -> Option<PointerGrabStartData<Fjord>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// ── Protocol handlers ────────────────────────────────────────────────

impl CompositorHandler for Fjord {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
        }
        self.popups.commit(surface);
        self.on_toplevel_commit(surface);
        self.on_popup_commit(surface);
        self.on_layer_commit(surface);
        handle_resize_commit(&mut self.space, surface);
    }
}

impl BufferHandler for Fjord {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Fjord {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Fjord {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: smithay::backend::allocator::dmabuf::Dmabuf, notifier: ImportNotifier) {
        if self.backend.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Fjord>();
        } else {
            notifier.failed();
        }
    }
}

impl XdgShellHandler for Fjord {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let zone = self.usable_area();
        surface.with_pending_state(|state| {
            state.bounds = Some(zone.size);
        });
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window, zone.loc, false);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.minimized.retain(|w| w.toplevel().map(|t| t != &surface).unwrap_or(false));
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.space.unmap_elem(&window);
        }
        self.focus_topmost();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            warn!("failed to track popup: {err}");
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let wl_surface = surface.wl_surface();
        let Some(start_data) = check_grab(&seat, wl_surface, serial) else { return };
        let Some(window) = self.window_for_surface(wl_surface) else { return };

        // Dragging a maximized window pops it back to its normal size.
        if is_maximized(&surface) {
            self.unmaximize(&surface);
        }
        let initial_window_location = self.space.element_location(&window).unwrap_or_default();
        let grab = MoveSurfaceGrab { start_data, window, initial_window_location };
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        let seat = Seat::from_resource(&seat).unwrap();
        let wl_surface = surface.wl_surface();
        let Some(start_data) = check_grab(&seat, wl_surface, serial) else { return };
        let Some(window) = self.window_for_surface(wl_surface) else { return };

        let loc = self.space.element_location(&window).unwrap_or_default();
        let size = window.geometry().size;
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();
        let grab = ResizeSurfaceGrab::start(start_data, window, edges.into(), Rectangle::new(loc, size));
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<Fjord> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else { return };

        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial) || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial) || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.maximize(&surface);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.unmaximize(&surface);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let Some(window) = self.window_for_surface(surface.wl_surface()) else { return };
        let Some(output) = self.primary_output() else { return };
        let Some(geo) = self.space.output_geometry(&output) else { return };
        if let Some(loc) = self.space.element_location(&window) {
            meta(&window).restore.replace(Some(Rectangle::new(loc, window.geometry().size)));
        }
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geo.size);
            state.fullscreen_output = surface.wl_surface().client().and_then(|c| output.client_outputs(&c).next());
        });
        self.space.map_element(window, geo.loc, true);
        surface.send_pending_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.unmaximize(&surface);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let Some(window) = self.window_for_surface(surface.wl_surface()) else { return };
        if let Some(loc) = self.space.element_location(&window) {
            meta(&window).restore.replace(Some(Rectangle::new(loc, window.geometry().size)));
        }
        self.space.unmap_elem(&window);
        self.minimized.push(window);
        self.focus_topmost();
    }
}

impl WlrLayerShellHandler for Fjord {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<WlOutput>, _layer: Layer, namespace: String) {
        let Some(output) = wl_output.as_ref().and_then(Output::from_resource).or_else(|| self.primary_output()) else {
            warn!("layer surface created without any output");
            surface.send_close();
            return;
        };
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            warn!("failed to map layer surface: {err}");
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut had_focus = false;
        for output in self.space.outputs() {
            let mut map = layer_map_for_output(output);
            if let Some(layer) = map.layers().find(|l| l.layer_surface() == &surface).cloned() {
                had_focus = self.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(layer.wl_surface());
                map.unmap_layer(&layer);
            }
        }
        if had_focus {
            self.focus_topmost();
        }
    }
}

impl SeatHandler for Fjord {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Fjord> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}

impl PointerConstraintsHandler for Fjord {}

impl SelectionHandler for Fjord {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Fjord {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl PrimarySelectionHandler for Fjord {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl DndGrabHandler for Fjord {}

impl WaylandDndGrabHandler for Fjord {
    fn dnd_requested<S: Source>(&mut self, source: S, _icon: Option<WlSurface>, seat: Seat<Self>, serial: Serial, type_: GrabType) {
        match type_ {
            GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let Some(start_data) = pointer.grab_start_data() else {
                    source.cancel();
                    return;
                };
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => source.cancel(),
        }
    }
}

impl OutputHandler for Fjord {}

impl XdgDecorationHandler for Fjord {
    // Apps draw their own title bars, styled by the NorOS theme.
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ClientSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ClientSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ClientSide);
    }
}

impl XdgActivationHandler for Fjord {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        if data.timestamp.elapsed().as_secs() < 10 {
            if let Some(window) = self.window_for_surface(&surface) {
                self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
            }
        }
    }
}

delegate_dispatch2!(Fjord);
