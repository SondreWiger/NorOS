//! Pointer grabs: moving and resizing windows with the mouse.

use std::cell::RefCell;

use smithay::{
    desktop::{Space, Window},
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor, shell::xdg::SurfaceCachedState},
};

use crate::state::Fjord;

const BTN_LEFT: u32 = 0x110;

/// Forwards everything a grab doesn't care about straight to the client.
macro_rules! forward_rest {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut Fjord,
            handle: &mut PointerInnerHandle<'_, Fjord>,
            focus: Option<(WlSurface, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }
        fn axis(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, details: AxisFrame) {
            handle.axis(data, details)
        }
        fn frame(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>) {
            handle.frame(data);
        }
        fn gesture_swipe_begin(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GestureSwipeBeginEvent) {
            handle.gesture_swipe_begin(data, event)
        }
        fn gesture_swipe_update(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GestureSwipeUpdateEvent) {
            handle.gesture_swipe_update(data, event)
        }
        fn gesture_swipe_end(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GestureSwipeEndEvent) {
            handle.gesture_swipe_end(data, event)
        }
        fn gesture_pinch_begin(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GesturePinchBeginEvent) {
            handle.gesture_pinch_begin(data, event)
        }
        fn gesture_pinch_update(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GesturePinchUpdateEvent) {
            handle.gesture_pinch_update(data, event)
        }
        fn gesture_pinch_end(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GesturePinchEndEvent) {
            handle.gesture_pinch_end(data, event)
        }
        fn gesture_hold_begin(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GestureHoldBeginEvent) {
            handle.gesture_hold_begin(data, event)
        }
        fn gesture_hold_end(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &GestureHoldEndEvent) {
            handle.gesture_hold_end(data, event)
        }
        fn start_data(&self) -> &PointerGrabStartData<Fjord> {
            &self.start_data
        }
        fn unset(&mut self, _data: &mut Fjord) {}
    };
}

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Fjord>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Fjord> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Fjord,
        handle: &mut PointerInnerHandle<'_, Fjord>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;
        let mut new_location = (self.initial_window_location.to_f64() + delta).to_i32_round();
        // Never let a title bar slide under the menu bar.
        let top = data.usable_area().loc.y;
        new_location.y = new_location.y.max(top);
        data.space.map_element(self.window.clone(), new_location, true);
    }

    fn button(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_rest!();
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
        const TOP    = 0b0001;
        const BOTTOM = 0b0010;
        const LEFT   = 0b0100;
        const RIGHT  = 0b1000;
        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(x: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits(x as u32).unwrap_or(ResizeEdge::empty())
    }
}

pub struct ResizeSurfaceGrab {
    start_data: PointerGrabStartData<Fjord>,
    window: Window,
    edges: ResizeEdge,
    initial_rect: Rectangle<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: PointerGrabStartData<Fjord>,
        window: Window,
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    ) -> Self {
        ResizeSurfaceState::with(window.toplevel().unwrap().wl_surface(), |state| {
            *state = ResizeSurfaceState::Resizing { edges, initial_rect };
        });
        Self {
            start_data,
            window,
            edges,
            initial_rect,
            last_window_size: initial_rect.size,
        }
    }
}

impl PointerGrab<Fjord> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Fjord,
        handle: &mut PointerInnerHandle<'_, Fjord>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let mut delta = event.location - self.start_data.location;
        let mut w = self.initial_rect.size.w;
        let mut h = self.initial_rect.size.h;

        if self.edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                delta.x = -delta.x;
            }
            w = (self.initial_rect.size.w as f64 + delta.x) as i32;
        }
        if self.edges.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
            if self.edges.intersects(ResizeEdge::TOP) {
                delta.y = -delta.y;
            }
            h = (self.initial_rect.size.h as f64 + delta.y) as i32;
        }

        let (min_size, max_size) = compositor::with_states(self.window.toplevel().unwrap().wl_surface(), |states| {
            let mut guard = states.cached_state.get::<SurfaceCachedState>();
            let data = guard.current();
            (data.min_size, data.max_size)
        });
        let max_w = if max_size.w == 0 { i32::MAX } else { max_size.w };
        let max_h = if max_size.h == 0 { i32::MAX } else { max_size.h };
        self.last_window_size = Size::from((
            w.max(min_size.w.max(1)).min(max_w),
            h.max(min_size.h.max(1)).min(max_h),
        ));

        let xdg = self.window.toplevel().unwrap();
        xdg.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(self.last_window_size);
        });
        xdg.send_pending_configure();
    }

    fn button(&mut self, data: &mut Fjord, handle: &mut PointerInnerHandle<'_, Fjord>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            let xdg = self.window.toplevel().unwrap();
            xdg.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
                state.size = Some(self.last_window_size);
            });
            xdg.send_pending_configure();
            ResizeSurfaceState::with(xdg.wl_surface(), |state| {
                *state = ResizeSurfaceState::WaitingForLastCommit {
                    edges: self.edges,
                    initial_rect: self.initial_rect,
                };
            });
        }
    }

    forward_rest!();
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
enum ResizeSurfaceState {
    #[default]
    Idle,
    Resizing {
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    },
    WaitingForLastCommit {
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    },
}

impl ResizeSurfaceState {
    fn with<F, T>(surface: &WlSurface, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        compositor::with_states(surface, |states| {
            states.data_map.insert_if_missing(RefCell::<Self>::default);
            let state = states.data_map.get::<RefCell<Self>>().unwrap();
            cb(&mut state.borrow_mut())
        })
    }

    fn commit(&mut self) -> Option<(ResizeEdge, Rectangle<i32, Logical>)> {
        match *self {
            Self::Resizing { edges, initial_rect } => Some((edges, initial_rect)),
            Self::WaitingForLastCommit { edges, initial_rect } => {
                *self = Self::Idle;
                Some((edges, initial_rect))
            }
            Self::Idle => None,
        }
    }
}

/// Keeps the opposite edge anchored while resizing from the top or left.
pub fn handle_resize_commit(space: &mut Space<Window>, surface: &WlSurface) -> Option<()> {
    let window = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()?;
    let mut loc = space.element_location(&window)?;
    let geometry = window.geometry();

    let new_loc: Point<Option<i32>, Logical> = ResizeSurfaceState::with(surface, |state| {
        state
            .commit()
            .and_then(|(edges, initial)| {
                edges.intersects(ResizeEdge::TOP_LEFT).then(|| {
                    let x = edges
                        .intersects(ResizeEdge::LEFT)
                        .then_some(initial.loc.x + (initial.size.w - geometry.size.w));
                    let y = edges
                        .intersects(ResizeEdge::TOP)
                        .then_some(initial.loc.y + (initial.size.h - geometry.size.h));
                    (x, y).into()
                })
            })
            .unwrap_or_default()
    });

    if let Some(x) = new_loc.x {
        loc.x = x;
    }
    if let Some(y) = new_loc.y {
        loc.y = y;
    }
    if new_loc.x.is_some() || new_loc.y.is_some() {
        space.map_element(window, loc, false);
    }
    Some(())
}
