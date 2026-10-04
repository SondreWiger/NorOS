//! The real-hardware backend: KMS/DRM for the screen, libinput for devices,
//! libseat for permission to use them.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Format, Fourcc, Modifier,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Color32F, ImportDma, ImportMemWl,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{all_gpus, primary_gpu},
    },
    desktop::{
        layer_map_for_output,
        space::{SpaceRenderElements, space_render_elements},
        utils::{send_frames_surface_tree, surface_primary_scanout_output, update_surface_primary_scanout_output},
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{Display, backend::GlobalId},
    },
    utils::{DeviceFd, IsAlive, Point, Scale, Transform},
    wayland::{compositor::with_states, dmabuf::DmabufState},
};
use smithay::backend::renderer::element::default_primary_scanout_output_compare;
use smithay::backend::renderer::element::RenderElementStates;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{error, info, warn};

use crate::{cursor::Cursor, state::Fjord};

/// Nordic night blue, shown wherever nothing else is drawn.
const CLEAR_COLOR: Color32F = Color32F::new(0.043, 0.071, 0.125, 1.0);

smithay::backend::renderer::element::render_elements! {
    pub FjordElement<=GlesRenderer>;
    Pointer=MemoryRenderBufferRenderElement<GlesRenderer>,
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
}

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type FjordDrmOutput = DrmOutput<Allocator, Exporter, (), DrmDeviceFd>;
type FjordOutputManager = DrmOutputManager<Allocator, Exporter, (), DrmDeviceFd>;

pub struct OutputSurface {
    output: Output,
    drm_output: FjordDrmOutput,
    _global: GlobalId,
    frame_pending: bool,
    timer_pending: bool,
}

pub struct Backend {
    pub session: LibSeatSession,
    pub renderer: GlesRenderer,
    output_manager: FjordOutputManager,
    surfaces: HashMap<crtc::Handle, OutputSurface>,
    scanner: DrmScanner,
    cursor: Cursor,
    cursor_buffer: MemoryRenderBuffer,
    announced_ready: bool,
}

impl Backend {
    pub fn seat_name(&self) -> String {
        self.session.seat()
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<'static, Fjord> = EventLoop::try_new()?;
    let display: Display<Fjord> = Display::new()?;

    let (mut session, session_notifier) = LibSeatSession::new().map_err(|e| format!("no seat session: {e}"))?;
    let seat = session.seat();

    let gpu: PathBuf = std::env::var("FJORD_DRM_DEVICE")
        .ok()
        .map(PathBuf::from)
        .or_else(|| primary_gpu(&seat).ok().flatten())
        .or_else(|| all_gpus(&seat).ok().and_then(|gpus| gpus.into_iter().next()))
        .ok_or("no display device found")?;
    info!(?gpu, "using display device");

    let fd = session.open(&gpu, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd.clone())?;
    let node = DrmNode::from_path(&gpu)?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    let renderer = unsafe { GlesRenderer::new(egl_context)? };

    let mut render_formats: Vec<Format> = renderer.egl_context().dmabuf_render_formats().iter().copied().collect();
    if render_formats.is_empty() {
        // Software rendering (virtual machines) only guarantees linear buffers.
        warn!("renderer reports no dmabuf formats, assuming linear");
        render_formats = [Fourcc::Argb8888, Fourcc::Xrgb8888, Fourcc::Abgr8888, Fourcc::Xbgr8888]
            .into_iter()
            .map(|code| Format { code, modifier: Modifier::Linear })
            .collect();
    }

    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let exporter = GbmFramebufferExporter::new(gbm.clone(), node.into());
    let output_manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        [Fourcc::Argb8888, Fourcc::Abgr8888, Fourcc::Xrgb8888, Fourcc::Xbgr8888],
        render_formats,
    );

    let cursor = Cursor::load();
    let image = cursor.image();
    let cursor_buffer = MemoryRenderBuffer::from_slice(
        &image.pixels_rgba,
        Fourcc::Argb8888,
        (image.width as i32, image.height as i32),
        1,
        Transform::Normal,
        None,
    );

    let backend = Backend {
        session: session.clone(),
        renderer,
        output_manager,
        surfaces: HashMap::new(),
        scanner: DrmScanner::new(),
        cursor,
        cursor_buffer,
        announced_ready: false,
    };

    let mut state = Fjord::new(display, event_loop.handle(), event_loop.get_signal(), backend);

    // Buffer formats clients may use.
    let shm_formats: Vec<_> = state.backend.renderer.shm_formats().collect();
    state.shm_state.update_formats(shm_formats);
    let dmabuf_formats: Vec<Format> = state.backend.renderer.dmabuf_formats().iter().copied().collect();
    if !dmabuf_formats.is_empty() {
        let mut dmabuf_state = DmabufState::new();
        let global = dmabuf_state.create_global::<Fjord>(&state.display_handle, dmabuf_formats);
        state.dmabuf_state = Some((dmabuf_state, global));
    }

    // Input devices.
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|_| "failed to assign libinput seat")?;
    event_loop
        .handle()
        .insert_source(LibinputInputBackend::new(libinput.clone()), |event, _, state| {
            state.process_input_event(event)
        })?;

    // Vblank notifications from the display.
    event_loop.handle().insert_source(drm_notifier, |event, _, state| match event {
        DrmEvent::VBlank(crtc) => state.frame_finished(crtc),
        DrmEvent::Error(err) => error!("drm error: {err:?}"),
    })?;

    // Switching to another virtual terminal and back.
    event_loop.handle().insert_source(session_notifier, move |event, _, state| match event {
        SessionEvent::PauseSession => {
            info!("session paused");
            libinput.suspend();
            state.backend.output_manager.pause();
        }
        SessionEvent::ActivateSession => {
            info!("session resumed");
            if libinput.resume().is_err() {
                error!("failed to resume libinput");
            }
            if let Err(err) = state.backend.output_manager.lock().activate(false) {
                error!("failed to reactivate display: {err}");
            }
            for surface in state.backend.surfaces.values_mut() {
                surface.frame_pending = false;
            }
            state.render_all();
        }
    })?;

    state.scan_connectors();
    if state.space.outputs().next().is_none() {
        return Err("no connected display".into());
    }

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::set_var("XDG_CURRENT_DESKTOP", "NorOS");
        std::env::remove_var("DISPLAY");
    }
    let autostart = std::env::var("FJORD_AUTOSTART").unwrap_or_else(|_| "noros-shell".into());
    for command in autostart.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        state.spawn(command);
    }

    state.render_all();

    event_loop.run(None, &mut state, |state| {
        state.space.refresh();
        state.popups.cleanup();
        for output in state.space.outputs() {
            layer_map_for_output(output).cleanup();
        }
        let _ = state.display_handle.flush_clients();
    })?;

    info!("fjord exiting");
    Ok(())
}

impl Fjord {
    fn scan_connectors(&mut self) {
        let result = match self.backend.scanner.scan_connectors(self.backend.output_manager.device()) {
            Ok(result) => result,
            Err(err) => {
                warn!("failed to scan connectors: {err}");
                return;
            }
        };
        for event in result {
            match event {
                DrmScanEvent::Connected { connector, crtc: Some(crtc) } => self.connector_connected(connector, crtc),
                DrmScanEvent::Disconnected { crtc: Some(crtc), .. } => {
                    if let Some(surface) = self.backend.surfaces.remove(&crtc) {
                        self.space.unmap_output(&surface.output);
                    }
                }
                _ => {}
            }
        }
    }

    fn connector_connected(&mut self, connector: connector::Info, crtc: crtc::Handle) {
        let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        let modes = connector.modes();

        // FJORD_MODE=1440x900 picks a specific resolution; otherwise use the display's preferred one.
        let wanted = std::env::var("FJORD_MODE").ok().and_then(|m| {
            let (w, h) = m.split_once('x')?;
            Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?))
        });
        let mode = wanted
            .and_then(|(w, h)| modes.iter().find(|m| m.size() == (w, h)))
            .or_else(|| modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)))
            .or_else(|| modes.first())
            .copied();
        let Some(mode) = mode else {
            warn!(name, "connector has no modes");
            return;
        };

        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "NorOS".into(),
                model: name.clone(),
                serial_number: String::new(),
            },
        );
        let global = output.create_global::<Fjord>(&self.display_handle);
        let wl_mode = WlMode::from(mode);
        let scale = std::env::var("FJORD_SCALE").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
        let x = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).map(|g| g.size.w).sum::<i32>();
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), Some(Transform::Normal), Some(OutputScale::Fractional(scale)), Some((x, 0).into()));
        self.space.map_output(&output, (x, 0));

        let backend = &mut self.backend;
        let drm_output = backend.output_manager.lock().initialize_output::<_, FjordElement>(
            crtc,
            mode,
            &[connector.handle()],
            &output,
            None,
            &mut backend.renderer,
            &DrmOutputRenderElements::default(),
        );
        let drm_output = match drm_output {
            Ok(o) => o,
            Err(err) => {
                error!(name, "failed to initialize output: {err}");
                self.space.unmap_output(&output);
                return;
            }
        };
        info!(name, width = mode.size().0, height = mode.size().1, "output ready");

        backend.surfaces.insert(
            crtc,
            OutputSurface {
                output,
                drm_output,
                _global: global,
                frame_pending: false,
                timer_pending: false,
            },
        );
    }

    pub fn render_all(&mut self) {
        let crtcs: Vec<_> = self.backend.surfaces.keys().copied().collect();
        for crtc in crtcs {
            self.render_output(crtc);
        }
    }

    fn frame_finished(&mut self, crtc: crtc::Handle) {
        let Some(surface) = self.backend.surfaces.get_mut(&crtc) else { return };
        surface.frame_pending = false;
        if let Err(err) = surface.drm_output.frame_submitted() {
            warn!("frame submission error: {err}");
        }
        self.render_output(crtc);
    }

    /// Try again in about one frame; used when nothing changed on screen.
    fn schedule_render(&mut self, crtc: crtc::Handle) {
        let Some(surface) = self.backend.surfaces.get_mut(&crtc) else { return };
        if surface.timer_pending {
            return;
        }
        surface.timer_pending = true;
        let refresh = surface.output.current_mode().map(|m| m.refresh).filter(|r| *r > 0).unwrap_or(60_000);
        let delay = Duration::from_micros(1_000_000_000 / refresh as u64);
        let _ = self.loop_handle.insert_source(Timer::from_duration(delay), move |_, _, state| {
            if let Some(surface) = state.backend.surfaces.get_mut(&crtc) {
                surface.timer_pending = false;
            }
            state.render_output(crtc);
            TimeoutAction::Drop
        });
    }

    fn render_output(&mut self, crtc: crtc::Handle) {
        let Some(surface) = self.backend.surfaces.get(&crtc) else { return };
        if surface.frame_pending {
            return;
        }
        let output = surface.output.clone();

        if let CursorImageStatus::Surface(cursor) = &self.cursor_status {
            if !cursor.alive() {
                self.cursor_status = CursorImageStatus::default_named();
            }
        }

        let elements = self.output_elements(&output);
        let surface = self.backend.surfaces.get_mut(&crtc).unwrap();
        let result = surface
            .drm_output
            .render_frame(&mut self.backend.renderer, &elements, CLEAR_COLOR, FrameFlags::DEFAULT);

        let (rendered, states) = match result {
            Ok(frame) => (!frame.is_empty, frame.states),
            Err(err) => {
                warn!("render error: {err}");
                self.schedule_render(crtc);
                return;
            }
        };

        if rendered {
            match surface.drm_output.queue_frame(()) {
                Ok(()) => {
                    surface.frame_pending = true;
                    if !self.backend.announced_ready {
                        self.backend.announced_ready = true;
                        info!("fjord: first frame on screen");
                    }
                }
                Err(err) => {
                    warn!("failed to queue frame: {err}");
                    self.schedule_render(crtc);
                }
            }
        } else {
            self.schedule_render(crtc);
        }

        self.post_repaint(&output, &states);
    }

    fn output_elements(&mut self, output: &Output) -> Vec<FjordElement> {
        let renderer = &mut self.backend.renderer;
        let mut elements = Vec::new();
        let Some(output_geo) = self.space.output_geometry(output) else { return elements };
        let scale = Scale::from(output.current_scale().fractional_scale());
        let pointer = self.seat.get_pointer().unwrap().current_location();

        // The pointer goes first: elements are listed front to back.
        if output_geo.to_f64().contains(pointer) {
            let local = pointer - output_geo.loc.to_f64();
            match &self.cursor_status {
                CursorImageStatus::Hidden => {}
                CursorImageStatus::Surface(surface) => {
                    let hotspot = with_states(surface, |states| {
                        states
                            .data_map
                            .get::<CursorImageSurfaceData>()
                            .map(|attrs| attrs.lock().unwrap().hotspot)
                            .unwrap_or_default()
                    });
                    let pos = (local - hotspot.to_f64()).to_physical(scale).to_i32_round();
                    let surface_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                        render_elements_from_surface_tree(renderer, surface, pos, scale, 1.0, Kind::Cursor);
                    elements.extend(surface_elements.into_iter().map(FjordElement::Surface));
                }
                CursorImageStatus::Named(_) => {
                    let image = self.backend.cursor.image();
                    let hotspot = Point::<f64, smithay::utils::Logical>::from((image.xhot as f64, image.yhot as f64));
                    let pos = (local - hotspot).to_physical(scale);
                    match MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        pos,
                        &self.backend.cursor_buffer,
                        None,
                        None,
                        None,
                        Kind::Cursor,
                    ) {
                        Ok(element) => elements.push(FjordElement::Pointer(element)),
                        Err(err) => warn!("cursor upload failed: {err}"),
                    }
                }
            }
        }

        if let Ok(space_elements) = space_render_elements(renderer, [&self.space], output, 1.0) {
            elements.extend(space_elements.into_iter().map(FjordElement::Space));
        }
        elements
    }

    /// Tell clients their frame was shown so they can draw the next one.
    fn post_repaint(&mut self, output: &Output, states: &RenderElementStates) {
        let time: Duration = self.clock.now().into();
        let throttle = Some(Duration::from_secs(1));

        for window in self.space.elements() {
            window.with_surfaces(|surface, data| {
                update_surface_primary_scanout_output(surface, output, data, None, states, default_primary_scanout_output_compare);
            });
            if self.space.outputs_for_element(window).contains(output) {
                window.send_frame(output, time, throttle, surface_primary_scanout_output);
            }
        }

        let map = layer_map_for_output(output);
        for layer in map.layers() {
            layer.with_surfaces(|surface, data| {
                update_surface_primary_scanout_output(surface, output, data, None, states, default_primary_scanout_output_compare);
            });
            layer.send_frame(output, time, throttle, surface_primary_scanout_output);
        }
        drop(map);

        if let CursorImageStatus::Surface(surface) = &self.cursor_status {
            send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
    }
}
