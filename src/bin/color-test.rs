use wayland_client::{
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use std::fs::File;
use std::os::unix::io::AsFd;

struct State {
    running: bool,
    configured: bool,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
    buffer: Option<wl_buffer::WlBuffer>,
    width: u32,
    height: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let display = conn.display();

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let _registry = display.get_registry(&qh, ());

    let mut state = State {
        running: true,
        configured: false,
        compositor: None,
        shm: None,
        xdg_wm_base: None,
        surface: None,
        xdg_surface: None,
        xdg_toplevel: None,
        buffer: None,
        width: 640,
        height: 480,
    };

    // Roundtrip to get globals
    event_queue.roundtrip(&mut state)?;

    // Create surface
    let compositor = state.compositor.as_ref().expect("No compositor");
    let surface = compositor.create_surface(&qh, ());
    state.surface = Some(surface.clone());

    // Create xdg_surface
    let xdg_wm_base = state.xdg_wm_base.as_ref().expect("No xdg_wm_base");
    let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &qh, ());
    state.xdg_surface = Some(xdg_surface.clone());

    // Create xdg_toplevel
    let xdg_toplevel = xdg_surface.get_toplevel(&qh, ());
    xdg_toplevel.set_title("Color Test".to_string());
    state.xdg_toplevel = Some(xdg_toplevel);

    // Initial commit to signal we're ready
    surface.commit();

    // Wait for configure
    while !state.configured {
        event_queue.blocking_dispatch(&mut state)?;
    }

    // Create buffer and draw
    create_buffer_and_draw(&mut state, &qh)?;

    // Commit the buffer
    if let (Some(surface), Some(buffer)) = (state.surface.as_ref(), state.buffer.as_ref()) {
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, state.width as i32, state.height as i32);
        surface.commit();
    }

    println!("Color test running - should display a red gradient");
    println!("Press Ctrl+C to exit");

    // Main loop
    while state.running {
        event_queue.blocking_dispatch(&mut state)?;
    }

    Ok(())
}

fn create_buffer_and_draw(
    state: &mut State,
    qh: &QueueHandle<State>,
) -> Result<(), Box<dyn std::error::Error>> {
    let shm = state.shm.as_ref().expect("No shm");
    let width = state.width;
    let height = state.height;
    let stride = width * 4;
    let size = (stride * height) as usize;

    // Create shared memory file
    let file = File::from(rustix::fs::memfd_create(
        "color-test-buffer",
        rustix::fs::MemfdFlags::CLOEXEC,
    )?);
    rustix::fs::ftruncate(&file, size as u64)?;

    // Memory map and draw
    let data = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            0,
        )
    };

    if data == libc::MAP_FAILED {
        return Err("mmap failed".into());
    }

    // Draw a red gradient
    let pixels = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, size) };
    for y in 0..height {
        for x in 0..width {
            let idx = (y * stride + x * 4) as usize;
            // XRGB8888 format (BGRX in memory on little-endian)
            pixels[idx] = 0;                              // Blue
            pixels[idx + 1] = ((y * 255) / height) as u8; // Green (gradient)
            pixels[idx + 2] = 255;                        // Red (full)
            pixels[idx + 3] = 255;                        // Alpha/X
        }
    }

    // Create pool and buffer
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Xrgb8888,
        qh,
        (),
    );

    state.buffer = Some(buffer);

    // Unmap (buffer still valid because pool holds the fd)
    unsafe {
        libc::munmap(data, size);
    }

    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor = registry.bind::<wl_compositor::WlCompositor, _, _>(name, 4, qh, ());
                    state.compositor = Some(compositor);
                }
                "wl_shm" => {
                    let shm = registry.bind::<wl_shm::WlShm, _, _>(name, 1, qh, ());
                    state.shm = Some(shm);
                }
                "xdg_wm_base" => {
                    let xdg_wm_base = registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qh, ());
                    state.xdg_wm_base = Some(xdg_wm_base);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            println!("Buffer released");
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        xdg_wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            xdg_wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configured = true;
            println!("Surface configured!");
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    println!("Toplevel configured: {}x{}", width, height);
                    state.width = width as u32;
                    state.height = height as u32;
                }
            }
            xdg_toplevel::Event::Close => {
                println!("Close requested");
                state.running = false;
            }
            _ => {}
        }
    }
}
