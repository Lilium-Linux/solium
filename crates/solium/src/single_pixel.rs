//! `wp_single_pixel_buffer_v1`: a buffer that is a colour rather than pixels.
//!
//! A client that wants a rectangle of one colour — a background, a surface
//! held blank while what goes in it loads — otherwise has to allocate shared
//! memory, write the same value into every pixel of it, and hand that over for
//! the compositor to upload. With this global it names the colour instead,
//! gets back a buffer one pixel in size, and stretches it with a `wp_viewport`,
//! which Solium already advertises.
//!
//! ## Why there is nothing else in this file
//!
//! Smithay 0.7 implements the protocol and draws what it produces, so Solium's
//! part is the global. That was read in smithay 0.7.0's source rather than
//! assumed, and the pieces are worth separating by how they are known.
//!
//! **Pinned by the test below**, because it all happens on commit and a test
//! can reach a commit: the buffer is recognised as `BufferType::SinglePixel`
//! and carries the colour the client asked for, and `Solium::commit`, through
//! `on_commit_buffer_handler`, keeps it on the surface as a 1x1 buffer
//! stretched to the viewport and opaque across all of it. Keeping it is the
//! step that could have failed without a sound: smithay's `update_buffer`
//! drops any buffer `buffer_dimensions` cannot size and draws the surface
//! empty.
//!
//! **Read, not tested**, because the test binary has no renderer:
//!
//! - `import_surface` skips a single-pixel buffer rather than uploading it
//!   (`backend/renderer/utils/wayland.rs:507`), and
//!   `WaylandSurfaceRenderElement::from_state` draws it as
//!   `WaylandSurfaceTexture::SolidColor` (`backend/renderer/element/surface.rs:237`).
//!   Solium reaches client surfaces only through those elements: the one place
//!   in this crate that looks at a surface's renderer state itself is
//!   `Solium::has_content`, which asks only whether a buffer is attached, and a
//!   single-pixel buffer is one.
//! - On the hardware the DRM compositor cannot scan one out, because the GBM
//!   exporter takes only dmabuf and EGL buffers
//!   (`backend/drm/exporter/gbm.rs:100-114`), so it is composited like any
//!   other buffer that cannot go on a plane.

use smithay::{
    reexports::wayland_server::DisplayHandle, wayland::single_pixel_buffer::SinglePixelBufferState,
};

use crate::state::Solium;

/// Registers `wp_single_pixel_buffer_manager_v1`.
///
/// Every client may use it. A buffer of one colour grants nothing that
/// `wl_shm` does not already, and it asks less of the compositor than the
/// shared-memory buffer it replaces.
pub(crate) fn state(display: &DisplayHandle) -> SinglePixelBufferState {
    SinglePixelBufferState::new::<Solium>(display)
}

smithay::delegate_single_pixel_buffer!(Solium);

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use smithay::{
        backend::renderer::{BufferType, buffer_type, utils::with_renderer_surface_state},
        reexports::wayland_server::{
            Display,
            protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
        },
        utils::{Logical, Rectangle, Size},
        wayland::single_pixel_buffer::get_single_pixel_buffer,
    };
    use wayland_client::{
        Connection, Dispatch, Proxy as _, QueueHandle,
        protocol::{wl_buffer, wl_compositor, wl_registry, wl_surface},
    };
    use wayland_protocols::wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    };

    use crate::state::{ClientState, Solium};

    /// The client side: the three globals a client needs to put a solid colour
    /// on screen at a size of its choosing, and nothing else.
    ///
    /// **No toplevel**, for the reason `state::tests::drag_icon` gives: a
    /// window mapped in a process that already holds a raw libwayland
    /// connection builds a Qt scene for its decoration, and that aborts the
    /// whole test binary. A bare `wl_surface` never reaches `new_toplevel`,
    /// and it still goes through `Solium::commit`, which is what this needs.
    #[derive(Debug, Default)]
    struct Client {
        compositor: Option<wl_compositor::WlCompositor>,
        viewporter: Option<WpViewporter>,
        single_pixel: Option<WpSinglePixelBufferManagerV1>,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_registry::Event::Global {
                name, interface, ..
            } = event
            else {
                return;
            };
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
                "wp_viewporter" => state.viewporter = Some(registry.bind(name, 1, qh, ())),
                "wp_single_pixel_buffer_manager_v1" => {
                    state.single_pixel = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
    wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
    wayland_client::delegate_noop!(Client: ignore WpViewporter);
    wayland_client::delegate_noop!(Client: ignore WpViewport);
    wayland_client::delegate_noop!(Client: ignore WpSinglePixelBufferManagerV1);

    /// **Issue #74: a client can ask for a colour and have it on a surface.**
    ///
    /// One test rather than two because each half is the other's
    /// precondition: the buffer has to exist before it can be attached, and
    /// "the colour is right" says nothing about whether a commit keeps it.
    ///
    /// Opaque red at a size that is not 1x1 on purpose. A colour with every
    /// channel equal would pass with the channels in the wrong order, and a
    /// viewport of 1x1 would pass without the viewport being applied at all.
    #[test]
    fn a_colour_is_a_buffer_and_a_committed_one_fills_its_viewport() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        let (server_side, client_side) =
            UnixStream::pair().expect("a socket pair for the test client");
        let served = display
            .handle()
            .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let mut event_queue = conn.new_event_queue::<Client>();
        let qh = event_queue.handle();
        let mut client = Client::default();

        conn.display().get_registry(&qh, ());
        conn.flush().expect("flushing get_registry");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching get_registry");
        display
            .flush_clients()
            .expect("flushing the registry snapshot");
        // Safe to block: the server wrote the whole registry on the line
        // above and nothing but this thread drives it, so these bytes are
        // already in the kernel buffer. The same argument as `drag_icon`'s.
        event_queue
            .blocking_dispatch(&mut client)
            .expect("reading the registry snapshot");

        // The defect itself: on `stage` before #74 the registry had no such
        // global, so a client that looks for it found nothing to bind.
        let manager = client
            .single_pixel
            .clone()
            .expect("wp_single_pixel_buffer_manager_v1 is advertised");
        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let viewporter = client.viewporter.clone().expect("wp_viewporter bound");

        // Each channel is a u32 scaled to the full range, so u32::MAX is 1.0.
        let asked = manager.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        viewport.set_destination(64, 32);
        surface.attach(Some(&asked), 0, 0);
        surface.damage(0, 0, 64, 32);
        surface.commit();
        conn.flush().expect("flushing the buffer and the commit");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching the buffer and the commit");

        // The compositor's own handles. The protocol id is the same number on
        // both sides of one connection, so these are exact lookups rather
        // than a search for the only buffer around.
        let buffer: WlBuffer = served
            .object_from_protocol_id(&display.handle(), asked.id().protocol_id())
            .expect("the compositor made a wl_buffer for the request");
        let committed: WlSurface = served
            .object_from_protocol_id(&display.handle(), surface.id().protocol_id())
            .expect("the compositor made a wl_surface for the request");

        // `matches!` because `BufferType` has no `PartialEq`.
        assert!(
            matches!(buffer_type(&buffer), Some(BufferType::SinglePixel)),
            "the renderer tells buffer types apart with this, and it is what \
             makes smithay draw a colour instead of looking for a texture"
        );
        assert_eq!(
            get_single_pixel_buffer(&buffer)
                .expect("the buffer carries single-pixel data")
                .rgba8888(),
            [255, 0, 0, 255],
            "the colour the client named, channel for channel"
        );

        let (kept, buffer_size, surface_size, opaque) =
            with_renderer_surface_state(&committed, |surface| {
                (
                    surface.buffer().map(|kept| {
                        // Smithay's `Buffer` wraps the `WlBuffer`; the
                        // comparison below is with the protocol object.
                        let kept: &WlBuffer = kept;
                        kept.clone()
                    }),
                    surface.buffer_size(),
                    surface.surface_size(),
                    surface.opaque_regions().map(<[_]>::to_vec),
                )
            })
            .expect("Solium::commit gave the surface renderer state");
        assert_eq!(
            kept.as_ref(),
            Some(&buffer),
            "the commit kept the buffer on the surface; a buffer the renderer \
             cannot size is dropped here and the surface draws nothing"
        );
        assert_eq!(buffer_size, Some(Size::<i32, Logical>::from((1, 1))));
        assert_eq!(
            surface_size,
            Some(Size::<i32, Logical>::from((64, 32))),
            "one pixel stretched to the size the viewport asked for"
        );
        assert_eq!(
            opaque,
            Some(vec![Rectangle::<i32, Logical>::from_size((64, 32).into())]),
            "an opaque colour covers what is under it, so the damage tracker \
             can stop drawing there"
        );
    }
}
