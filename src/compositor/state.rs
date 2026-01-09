use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::utils::send_frames_surface_tree,
    input::{keyboard::XkbConfig, pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::{Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::LoopSignal,
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
            Display, DisplayHandle,
        },
    },
    utils::{IsAlive, Logical, Point, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, CompositorClientState,
            CompositorHandler, CompositorState, SurfaceAttributes,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface,
            XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};
use std::time::Duration;
use std::sync::{Arc, Mutex};
use wayland_server::Client;

pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct TermuiState {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub running: bool,

    // Smithay state objects
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,

    pub seat: Seat<Self>,
    pub output: Output,

    // Our window tracking
    pub toplevels: Vec<ToplevelSurface>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,

    // Frame data for terminal rendering
    pub pending_frame: Arc<Mutex<Option<FrameData>>>,

    // Terminal dimensions
    pub term_width: u32,
    pub term_height: u32,
}

#[derive(Clone)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA
}

impl TermuiState {
    pub fn new(
        display: &Display<Self>,
        loop_signal: LoopSignal,
        term_width: u32,
        term_height: u32,
    ) -> Self {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        // Create seat with keyboard and pointer
        let mut seat = seat_state.new_wl_seat(&display_handle, "termui");
        seat.add_keyboard(XkbConfig::default(), 200, 25).unwrap();
        seat.add_pointer();

        // Create output matching terminal size (in "pixels")
        let output = Output::new(
            "TERMUI-1".into(),
            PhysicalProperties {
                size: Size::from((0, 0)),
                subpixel: Subpixel::Unknown,
                make: "termui".into(),
                model: "virtual".into(),
            },
        );

        let mode = smithay::output::Mode {
            size: Size::from((term_width as i32, term_height as i32)),
            refresh: 60_000, // 60 Hz
        };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(1.0)), None);
        output.set_preferred(mode);
        output.create_global::<Self>(&display_handle);

        Self {
            display_handle,
            loop_signal,
            running: true,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            output,
            toplevels: Vec::new(),
            pointer_location: Point::from((0.0, 0.0)),
            cursor_status: CursorImageStatus::default_named(),
            pending_frame: Arc::new(Mutex::new(None)),
            term_width,
            term_height,
        }
    }

    pub fn resize_output(&mut self, width: u32, height: u32) {
        self.term_width = width;
        self.term_height = height;

        let mode = smithay::output::Mode {
            size: Size::from((width as i32, height as i32)),
            refresh: 60_000,
        };
        self.output.change_current_state(Some(mode), None, None, None);

        // Notify toplevels of new size
        for toplevel in &self.toplevels {
            toplevel.with_pending_state(|state| {
                state.size = Some(Size::from((width as i32, height as i32)));
            });
            toplevel.send_configure();
        }
    }

    pub fn surface_under_pointer(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        // For MVP, just return the first toplevel's surface
        self.toplevels.first().and_then(|tl| {
            tl.wl_surface().alive().then(|| {
                (tl.wl_surface().clone(), Point::from((0.0, 0.0)))
            })
        })
    }

    pub fn capture_frame(&self, surface: &WlSurface) -> Option<FrameData> {
        with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            let data = attrs.current();

            // Extract buffer from BufferAssignment
            let buffer = match &data.buffer {
                Some(smithay::wayland::compositor::BufferAssignment::NewBuffer(buffer)) => buffer,
                _ => return None,
            };

            // Try to read the buffer data using shm
            if let Ok(buffer_data) = smithay::wayland::shm::with_buffer_contents(
                buffer,
                |pool_ptr, pool_len, data| {
                    let width = data.width as u32;
                    let height = data.height as u32;
                    let stride = data.stride as u32;
                    let buffer_offset = data.offset as usize;

                    // The ptr is the pool base, we need to add the buffer offset
                    let ptr = unsafe { pool_ptr.add(buffer_offset) };
                    let buffer_size = (height * stride) as usize;

                    tracing::trace!(
                        "Buffer: {}x{}, stride={}, format={:?}, offset={}",
                        width, height, stride, data.format, buffer_offset
                    );

                    // Verify we're within bounds
                    if buffer_offset + buffer_size > pool_len {
                        tracing::error!("Buffer extends beyond pool!");
                        return FrameData { width: 0, height: 0, data: vec![] };
                    }

                    // Convert to RGBA
                    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

                    for y in 0..height {
                        for x in 0..width {
                            let pixel_offset = (y * stride + x * 4) as usize;
                            if pixel_offset + 4 <= buffer_size {
                                // XRGB8888 format: B, G, R, X in memory (little-endian)
                                let b = unsafe { *ptr.add(pixel_offset) };
                                let g = unsafe { *ptr.add(pixel_offset + 1) };
                                let r = unsafe { *ptr.add(pixel_offset + 2) };
                                let a = unsafe { *ptr.add(pixel_offset + 3) };
                                rgba.push(r);
                                rgba.push(g);
                                rgba.push(b);
                                rgba.push(a);
                            }
                        }
                    }

                    FrameData { width, height, data: rgba }
                },
            ) {
                return Some(buffer_data);
            }
            None
        })
    }
}

// Implement required traits
impl BufferHandler for TermuiState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for TermuiState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl CompositorHandler for TermuiState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        tracing::trace!("Surface commit");
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }

            // Capture frame when a toplevel commits
            for toplevel in &self.toplevels {
                if toplevel.wl_surface() == &root {
                    if let Some(frame_data) = self.capture_frame(&root) {
                        tracing::trace!("Captured frame: {}x{}", frame_data.width, frame_data.height);
                        *self.pending_frame.lock().unwrap() = Some(frame_data);
                    }

                    // Send frame callbacks using smithay's proper mechanism
                    let output = self.output.clone();
                    let time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap();

                    send_frames_surface_tree(
                        &root,
                        &output,
                        time,
                        Some(Duration::ZERO), // Always send callbacks
                        |_, _| Some(output.clone()),
                    );
                    break;
                }
            }
        }
    }
}


impl XdgShellHandler for TermuiState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("New toplevel surface created!");
        // Configure the surface to our terminal size
        surface.with_pending_state(|state| {
            state.size = Some(Size::from((self.term_width as i32, self.term_height as i32)));
            state.states.set(xdg_toplevel::State::Activated);
            state.states.set(xdg_toplevel::State::Maximized);
        });
        surface.send_configure();

        self.toplevels.push(surface.clone());

        // Set keyboard focus to the new toplevel
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface.wl_surface().clone()), 0.into());

        // Note: Frame callbacks are sent in the commit handler after capture
        // At this point the client hasn't committed yet, so no callbacks are registered
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // MVP: ignore popups
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.toplevels.retain(|tl| tl != &surface);

        // If no more toplevels, exit
        if self.toplevels.is_empty() {
            self.running = false;
            self.loop_signal.stop();
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: smithay::utils::Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

impl SeatHandler for TermuiState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}

impl SelectionHandler for TermuiState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for TermuiState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for TermuiState {}
impl ServerDndGrabHandler for TermuiState {}

impl OutputHandler for TermuiState {}

delegate_compositor!(TermuiState);
delegate_xdg_shell!(TermuiState);
delegate_shm!(TermuiState);
delegate_output!(TermuiState);
delegate_seat!(TermuiState);
delegate_data_device!(TermuiState);
