// Simple test client to verify compositor is working
use wayland_client::{protocol::wl_registry, Connection, Dispatch, QueueHandle};

struct AppData {
    globals: Vec<(u32, String, u32)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            println!("Global: {} v{} (name={})", interface, version, name);
            state.globals.push((name, interface, version));
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");
    let display = conn.display();

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let _registry = display.get_registry(&qh, ());

    let mut app_data = AppData { globals: vec![] };

    // Do a roundtrip to get all globals
    event_queue.roundtrip(&mut app_data).expect("Roundtrip failed");

    println!("\nFound {} globals", app_data.globals.len());

    // Check for xdg_wm_base
    if app_data.globals.iter().any(|(_, name, _)| name == "xdg_wm_base") {
        println!("✓ xdg_wm_base found!");
    } else {
        println!("✗ xdg_wm_base NOT found!");
    }
}
