//! Plain windows, as many as asked for, from one small process: something
//! for a window manager to manage.
//!
//!     cargo run -p wl-probe --example tiles -- 48 ffffff
//!
//! Each is one toplevel of one flat colour, drawn as a single pixel
//! stretched to whatever size it is given (`wp_single_pixel_buffer_v1` and
//! `wp_viewporter`), so fifty of them cost what one terminal does. They do
//! nothing and take no input. A window closed by the compositor is let go
//! of, and the process ends with the last of them.
//!
//! `examples/bad-apple` uses them as the picture's pixels; they are as
//! useful for trying a layout with more windows than anybody wants to open
//! by hand.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

/// The size a window takes when the compositor leaves it to the window.
const OWN_SIZE: (i32, i32) = (320, 240);

/// One window: its surface, and the size it was last told.
struct Tile {
    surface: WlSurface,
    xdg: XdgSurface,
    toplevel: XdgToplevel,
    viewport: WpViewport,
    size: (i32, i32),
    open: bool,
}

struct Tiles {
    buffer: WlBuffer,
    tiles: Vec<Tile>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let count: usize = arguments
        .next()
        .and_then(|count| count.parse().ok())
        .ok_or("usage: tiles <count> [rrggbb]")?;
    let colour = arguments.next().unwrap_or_else(|| "ffffff".to_owned());
    let colour = u32::from_str_radix(&colour, 16).map_err(|_| "the colour is rrggbb, in hex")?;
    // Eight bits a channel, as the thirty-two the protocol takes.
    let channel = |shift: u32| ((colour >> shift) & 0xff) * 0x0101_0101;

    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Tiles>(&connection)?;
    let handle = queue.handle();
    let compositor: WlCompositor = globals.bind(&handle, 4..=6, ())?;
    let base: XdgWmBase = globals.bind(&handle, 1..=6, ())?;
    let viewporter: WpViewporter = globals.bind(&handle, 1..=1, ())?;
    let pixels: WpSinglePixelBufferManagerV1 = globals.bind(&handle, 1..=1, ())?;
    let buffer =
        pixels.create_u32_rgba_buffer(channel(16), channel(8), channel(0), u32::MAX, &handle, ());

    let mut state = Tiles {
        buffer,
        tiles: Vec::with_capacity(count),
    };
    for index in 0..count {
        let surface = compositor.create_surface(&handle, ());
        let xdg = base.get_xdg_surface(&surface, &handle, index);
        let toplevel = xdg.get_toplevel(&handle, index);
        toplevel.set_app_id("solium-tiles".to_owned());
        toplevel.set_title(format!("tile {}", index + 1));
        let viewport = viewporter.get_viewport(&surface, &handle, ());
        // The first commit has no buffer: it asks for the first configure.
        surface.commit();
        state.tiles.push(Tile {
            surface,
            xdg,
            toplevel,
            viewport,
            size: OWN_SIZE,
            open: true,
        });
    }
    while state.tiles.iter().any(|tile| tile.open) {
        queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}

impl Dispatch<XdgSurface, usize> for Tiles {
    fn event(
        state: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg.ack_configure(serial);
        let Some(tile) = state.tiles.get(*index).filter(|tile| tile.open) else {
            return;
        };
        // The one pixel, stretched to the size the window was told.
        tile.viewport.set_destination(tile.size.0, tile.size.1);
        tile.surface.attach(Some(&state.buffer), 0, 0);
        tile.surface.damage_buffer(0, 0, 1, 1);
        tile.surface.commit();
    }
}

impl Dispatch<XdgToplevel, usize> for Tiles {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(tile) = state.tiles.get_mut(*index) else {
            return;
        };
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                // Nought is "as you like": the window's own size.
                tile.size = (
                    if width > 0 { width } else { OWN_SIZE.0 },
                    if height > 0 { height } else { OWN_SIZE.1 },
                );
            }
            xdg_toplevel::Event::Close if tile.open => {
                tile.open = false;
                tile.viewport.destroy();
                tile.toplevel.destroy();
                tile.xdg.destroy();
                tile.surface.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Tiles {
    fn event(
        _: &mut Self,
        base: &XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // A ping unanswered is a client the compositor is entitled to end.
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Tiles {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Tiles: ignore WlCompositor);
delegate_noop!(Tiles: ignore WlSurface);
delegate_noop!(Tiles: ignore WlBuffer);
delegate_noop!(Tiles: ignore WpViewporter);
delegate_noop!(Tiles: ignore WpViewport);
delegate_noop!(Tiles: ignore WpSinglePixelBufferManagerV1);
