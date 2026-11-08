//! #52: a clipboard manager can watch and set the clipboard without focus.

use super::*;
use smithay::reexports::wayland_server::Display;
use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1,
    ext_data_control_source_v1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1,
    zwlr_data_control_source_v1,
};

const TEXT: &str = "text/plain;charset=utf-8";

/// One clipboard manager. It has a seat and the two managers and no
/// surface at all, which is the point: nothing of it can ever be focused.
#[derive(Debug, Default)]
struct Client {
    seat: Option<wl_seat::WlSeat>,
    wlr: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    ext: Option<ext_data_control_manager_v1::ExtDataControlManagerV1>,
    /// The mime types each offer advertised, by the offer's protocol id.
    mimes: std::collections::HashMap<u32, Vec<String>>,
    /// The clipboard as the last `selection` event left it.
    wlr_selection: Option<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1>,
    ext_selection: Option<ext_data_control_offer_v1::ExtDataControlOfferV1>,
    /// What a source of this client was asked to write, and where.
    sends: Vec<(String, std::os::fd::OwnedFd)>,
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
            "wl_seat" => state.seat = Some(registry.bind(name, 1, qh, ())),
            "zwlr_data_control_manager_v1" => state.wlr = Some(registry.bind(name, 2, qh, ())),
            "ext_data_control_manager_v1" => state.ext = Some(registry.bind(name, 1, qh, ())),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Client: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(
    Client: ignore zwlr_data_control_manager_v1::ZwlrDataControlManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore ext_data_control_manager_v1::ExtDataControlManagerV1
);

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for Client {
    fn event(
        state: &mut Self,
        _device: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_device_v1::Event::Selection { id } = event {
            state.wlr_selection = id;
        }
    }

    wayland_client::event_created_child!(Client, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ext_data_control_device_v1::ExtDataControlDeviceV1, ()> for Client {
    fn event(
        state: &mut Self,
        _device: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_data_control_device_v1::Event::Selection { id } = event {
            state.ext_selection = id;
        }
    }

    wayland_client::event_created_child!(Client, ext_data_control_device_v1::ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ext_data_control_offer_v1::ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()> for Client {
    fn event(
        state: &mut Self,
        offer: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            let id = offer.id().protocol_id();
            state.mimes.entry(id).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ext_data_control_offer_v1::ExtDataControlOfferV1, ()> for Client {
    fn event(
        state: &mut Self,
        offer: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            let id = offer.id().protocol_id();
            state.mimes.entry(id).or_default().push(mime_type);
        }
    }
}

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for Client {
    fn event(
        state: &mut Self,
        _source: &zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_source_v1::Event::Send { mime_type, fd } = event {
            state.sends.push((mime_type, fd));
        }
    }
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, ()> for Client {
    fn event(
        state: &mut Self,
        _source: &ext_data_control_source_v1::ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_data_control_source_v1::Event::Send { mime_type, fd } = event {
            state.sends.push((mime_type, fd));
        }
    }
}

/// A client on its own socket, with the registry read.
struct Connected {
    conn: Connection,
    queue: EventQueue<Client>,
    client: Client,
}

impl Connected {
    fn new(display: &mut Display<Solium>, state: &mut Solium) -> Self {
        let (server_side, client_side) =
            UnixStream::pair().expect("a socket pair for the test client");
        display
            .handle()
            .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let queue = conn.new_event_queue::<Client>();
        conn.display().get_registry(&queue.handle(), ());
        let mut connected = Self {
            conn,
            queue,
            client: Client::default(),
        };
        connected.round_trip(display, state);
        connected
    }

    /// Send what this client asked, let the server answer, read the
    /// answer. Blocking is safe for the reason `drag_icon` gives: the
    /// server has written everything before the read, and nothing but
    /// this thread drives it. Every request here gets at least a reply
    /// the server writes, so the read never waits on an empty socket --
    /// except where the caller says otherwise with `settle`.
    fn round_trip(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
        self.settle(display, state);
        self.queue
            .blocking_dispatch(&mut self.client)
            .expect("reading the server's answer");
    }

    /// The same without the read, for a request with no answer to this
    /// client.
    fn settle(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
        self.conn.flush().expect("flushing the client's requests");
        display
            .dispatch_clients(state)
            .expect("dispatching the client's requests");
        display.flush_clients().expect("flushing to the clients");
    }

    fn dispatch_pending(&mut self) {
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        self.queue
            .dispatch_pending(&mut self.client)
            .expect("dispatching what arrived");
    }
}

/// Write what a source was asked for, and close the pipe so the reader
/// sees the end.
fn answer(client: &mut Client, mime: &str, bytes: &[u8]) {
    let (asked, fd) = client
        .sends
        .pop()
        .expect("the source was asked for its data");
    assert_eq!(asked, mime);
    std::fs::File::from(fd)
        .write_all(bytes)
        .expect("writing the selection into the pipe");
}

/// **Issue #52.** Copy through one data-control protocol and read it
/// back through the other, between two clients that have no surface.
///
/// Both directions, wlroots to ext and ext to wlroots, because each half
/// proves something the other does not: the first that a manager joining
/// late is told what is on the clipboard now, the second that a manager
/// already watching is told when it changes. On `stage` before #52 there
/// is no such global, and the first `expect` below fails.
#[test]
fn a_clipboard_manager_reads_and_sets_the_clipboard_without_focus() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    let mut copier = Connected::new(&mut display, &mut state);
    let seat = copier.client.seat.clone().expect("wl_seat is advertised");
    let wlr = copier
        .client
        .wlr
        .clone()
        .expect("zwlr_data_control_manager_v1 is advertised");
    let qh = copier.queue.handle();
    let copier_device = wlr.get_data_device(&seat, &qh, ());
    let source = wlr.create_data_source(&qh, ());
    source.offer(TEXT.to_owned());
    copier_device.set_selection(Some(&source));
    copier.settle(&mut display, &mut state);

    // A manager that starts now: getting its device is enough to be
    // told the clipboard, with no focus and no surface to focus.
    let mut manager = Connected::new(&mut display, &mut state);
    let seat = manager.client.seat.clone().expect("wl_seat is advertised");
    let ext = manager
        .client
        .ext
        .clone()
        .expect("ext_data_control_manager_v1 is advertised");
    let qh = manager.queue.handle();
    let manager_device = ext.get_data_device(&seat, &qh, ());
    manager.round_trip(&mut display, &mut state);
    let offer = manager
        .client
        .ext_selection
        .clone()
        .expect("the manager is told what is on the clipboard");
    assert_eq!(
        manager.client.mimes.get(&offer.id().protocol_id()),
        Some(&vec![TEXT.to_owned()]),
    );

    // And it can read it: the request goes to the copier's source.
    let (mut reader, writer) = std::io::pipe().expect("a pipe for the transfer");
    offer.receive(TEXT.to_owned(), writer.as_fd());
    drop(writer);
    manager.settle(&mut display, &mut state);
    copier
        .queue
        .blocking_dispatch(&mut copier.client)
        .expect("reading the send request");
    answer(&mut copier.client, TEXT, b"copied in one");
    let mut read = String::new();
    reader
        .read_to_string(&mut read)
        .expect("reading the transfer");
    assert_eq!(read, "copied in one");

    // Forget anything the copier was told so far -- smithay may tell a
    // manager about its own selection -- so what follows is news.
    copier.client.wlr_selection = None;

    // The other way: the manager puts something on the clipboard, as
    // one restoring a history entry does, and the copier, already
    // watching, is told.
    let source = ext.create_data_source(&qh, ());
    source.offer(TEXT.to_owned());
    manager_device.set_selection(Some(&source));
    manager.settle(&mut display, &mut state);
    copier.dispatch_pending();
    let offer = copier
        .client
        .wlr_selection
        .clone()
        .expect("the copier is told the clipboard changed");
    assert_eq!(
        copier.client.mimes.get(&offer.id().protocol_id()),
        Some(&vec![TEXT.to_owned()]),
    );
    let (mut reader, writer) = std::io::pipe().expect("a pipe for the transfer");
    offer.receive(TEXT.to_owned(), writer.as_fd());
    drop(writer);
    copier.settle(&mut display, &mut state);
    manager
        .queue
        .blocking_dispatch(&mut manager.client)
        .expect("reading the send request");
    answer(&mut manager.client, TEXT, b"restored from history");
    let mut read = String::new();
    reader
        .read_to_string(&mut read)
        .expect("reading the transfer");
    assert_eq!(read, "restored from history");
}
