#[cfg(test)]
mod explicit_sync {
    use super::*;
    use smithay::backend::{
        allocator::{
            Allocator, Buffer as _, Fourcc, Modifier,
            dmabuf::AsDmabuf,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::DrmDeviceFd,
        renderer::utils::with_renderer_surface_state,
    };
    use smithay::reexports::{
        calloop::EventLoop,
        drm::control::{Device as _, syncobj},
    };
    use smithay::utils::{DeviceFd, Transform};
    use wayland_protocols::wp::{
        linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1,
        linux_drm_syncobj::v1::client::{
            wp_linux_drm_syncobj_surface_v1, wp_linux_drm_syncobj_timeline_v1,
        },
    };
    pub(super) use wayland_protocols::wp::{
        linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
    };
    wayland_client::delegate_noop!(Client: ignore WpLinuxDrmSyncobjManagerV1);
    wayland_client::delegate_noop!(Client: ignore wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1);
    wayland_client::delegate_noop!(Client: ignore wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1);
    wayland_client::delegate_noop!(Client: ignore ZwpLinuxDmabufV1);
    wayland_client::delegate_noop!(Client: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);
    // These cases exercise buffer ownership, not QML frames. Qt scenes must
    // remain on its dedicated test thread when the workspace runs in parallel.
    fn test_state(display: smithay::reexports::wayland_server::DisplayHandle) -> Solium {
        let mut state = Solium::new(display);
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        state
    }

    struct Gpu {
        fd: DrmDeviceFd,
        allocator: GbmAllocator<DrmDeviceFd>,
        handle: syncobj::Handle,
    }
    impl Gpu {
        fn new() -> Self {
            let path =
                std::env::var("SOLIUM_TEST_RENDER_NODE").expect("set SOLIUM_TEST_RENDER_NODE");
            assert!(
                std::path::Path::new(&path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("renderD")
            );
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
            assert!(smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&fd));
            let allocator = GbmAllocator::new(
                GbmDevice::new(fd.clone()).unwrap(),
                GbmBufferFlags::RENDERING,
            );
            let handle = fd.create_syncobj(false).unwrap();
            Self {
                fd,
                allocator,
                handle,
            }
        }
        fn timeline(
            &self,
            client: &Client,
            qh: &QueueHandle<Client>,
        ) -> wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1 {
            let fd = self.fd.syncobj_to_fd(self.handle, false).unwrap();
            client
                .syncobj
                .as_ref()
                .expect("explicit sync advertised")
                .import_timeline(fd.as_fd(), qh, ())
        }
        fn signal(&self, point: u64) {
            self.fd
                .syncobj_timeline_signal(&[self.handle], &[point])
                .unwrap();
        }
        fn point(&self) -> u64 {
            let mut p = [0];
            self.fd
                .syncobj_timeline_query(&[self.handle], &mut p, false)
                .unwrap();
            p[0]
        }
        fn buffer(&mut self, client: &Client, qh: &QueueHandle<Client>) -> wl_buffer::WlBuffer {
            let bo = self
                .allocator
                .create_buffer(80, 80, Fourcc::Argb8888, &[Modifier::Linear])
                .unwrap();
            let dma = bo.export().unwrap();
            let params = client.dmabuf.as_ref().unwrap().create_params(qh, ());
            let m = u64::from(dma.format().modifier);
            for (i, ((fd, offset), stride)) in dma
                .handles()
                .zip(dma.offsets())
                .zip(dma.strides())
                .enumerate()
            {
                params.add(fd, i as u32, offset, stride, (m >> 32) as u32, m as u32);
            }
            let buffer = params.create_immed(
                80,
                80,
                Fourcc::Argb8888 as u32,
                zwp_linux_buffer_params_v1::Flags::empty(),
                qh,
                (),
            );
            params.destroy();
            buffer
        }
        fn setup(&self) -> (Display<Solium>, Solium, EventLoop<'static, Solium>) {
            let display = Display::new().unwrap();
            let mut state = test_state(display.handle());
            let event_loop = EventLoop::try_new().unwrap();
            state.explicit_sync.sender =
                Some(crate::explicit_sync::install(&event_loop.handle(), |s| s).unwrap());
            state.dmabuf_global = Some(state.dmabuf_state.create_global::<Solium>(
                &display.handle(),
                [smithay::backend::allocator::Format {
                    code: Fourcc::Argb8888,
                    modifier: Modifier::Linear,
                }],
            ));
            state
                .explicit_sync
                .enable(&display.handle(), self.fd.clone(), true);
            (display, state, event_loop)
        }
    }
    impl Drop for Gpu {
        fn drop(&mut self) {
            let _ = self.fd.destroy_syncobj(self.handle);
        }
    }
    fn points(
        sync: &wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        t: &wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
        a: u32,
        r: u32,
    ) {
        sync.set_acquire_point(t, 0, a);
        sync.set_release_point(t, 0, r);
    }
    fn drive(event_loop: &mut EventLoop<'static, Solium>, state: &mut Solium) {
        for _ in 0..4 {
            event_loop
                .dispatch(Some(Duration::from_millis(5)), state)
                .unwrap();
        }
    }
    fn current(s: &WlSurface) -> Option<smithay::backend::renderer::utils::Buffer> {
        with_renderer_surface_state(s, |r| r.buffer().cloned()).flatten()
    }
    #[test]
    fn explicit_sync_is_not_advertised_without_a_device() {
        let mut display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        let (_, _, client) = connect(&mut display, &mut state);
        assert!(client.syncobj.is_none());
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_waits_without_blocking_other_clients_and_releases_the_replaced_buffer() {
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let server = window.wl_surface().unwrap().into_owned();
        let previous = current(&server).unwrap();
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&surface, &qh, ());
        let timeline = gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        points(&sync, &timeline, 1, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        assert_eq!(
            current(&server).unwrap().id(),
            previous.id(),
            "unready buffer became visible"
        );
        let (_, _, other) = connect(&mut display, &mut state);
        assert!(other.compositor.is_some());
        assert_eq!(gpu.point(), 0);
        gpu.signal(1);
        drive(&mut event_loop, &mut state);
        let held = current(&server).unwrap();
        assert_ne!(
            held.id(),
            previous.id(),
            "signaled commit was never applied"
        );
        let snapshot = crate::explicit_sync::snapshot(&mut state);
        assert!(snapshot.iter().any(|buffer| buffer.id() == held.id()));
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(gpu.point(), 1, "retained buffer was released");
        drop(held);
        assert_eq!(gpu.point(), 1, "snapshot did not retain the explicit root");
        drop(snapshot);
        assert_eq!(gpu.point(), 2);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_same_buffer_gets_fresh_points() {
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let server = window.wl_surface().unwrap().into_owned();
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&surface, &qh, ());
        let timeline = gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        let mut previous_commit = None;
        for (a, r) in [(1, 2), (3, 4)] {
            gpu.signal(u64::from(a));
            points(&sync, &timeline, a, r);
            surface.attach(Some(&buffer), 0, 0);
            surface.damage(0, 0, 80, 80);
            surface.commit();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            drive(&mut event_loop, &mut state);
            let commit = with_renderer_surface_state(&server, |r| r.current_commit()).unwrap();
            assert_ne!(
                Some(commit),
                previous_commit,
                "same-buffer damage was hidden from caches"
            );
            previous_commit = Some(commit);
        }
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(current(&server).is_none());
        assert_eq!(
            gpu.point(),
            4,
            "same wl_buffer retained the first release point"
        );
    }
    #[derive(Debug)]
    struct ReadFence {
        ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
        read: Option<OwnedFd>,
    }
    impl smithay::backend::renderer::sync::Fence for ReadFence {
        fn is_signaled(&self) -> bool {
            self.ready.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn wait(&self) -> Result<(), smithay::backend::renderer::sync::Interrupted> {
            if self.is_signaled() {
                Ok(())
            } else {
                Err(smithay::backend::renderer::sync::Interrupted)
            }
        }
        fn is_exportable(&self) -> bool {
            self.read.is_some()
        }
        fn export(&self) -> Option<OwnedFd> {
            self.read.as_ref()?.try_clone().ok()
        }
    }
    #[test]
    fn explicit_sync_release_waits_for_gpu_readiness_even_after_unmap() {
        use std::io::Write;
        let mut display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        let mut event_loop = EventLoop::try_new().unwrap();
        state.explicit_sync.sender =
            Some(crate::explicit_sync::install(&event_loop.handle(), |s| s).unwrap());
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let buffer = commit_buffer(&client, &qh, &surface, 80, 80);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let held = current(&window.wl_surface().unwrap()).unwrap();
        let (read, mut write) = UnixStream::pair().unwrap();
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.explicit_sync.retire(
            ReadFence {
                ready: ready.clone(),
                read: Some(read.into()),
            }
            .into(),
            vec![held],
        );
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            !client
                .released
                .contains(&wayland_client::Proxy::id(&buffer)),
            "GPU is still reading"
        );
        ready.store(true, std::sync::atomic::Ordering::SeqCst);
        write.write_all(&[1]).unwrap();
        drive(&mut event_loop, &mut state);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            client
                .released
                .contains(&wayland_client::Proxy::id(&buffer)),
            "readiness never released the buffer"
        );
    }
    #[test]
    fn explicit_sync_failed_gpu_wait_does_not_release_storage() {
        let mut display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let buffer = commit_buffer(&client, &qh, &surface, 80, 80);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let held = current(&window.wl_surface().unwrap()).unwrap();
        state.explicit_sync.retire(
            ReadFence {
                ready: Default::default(),
                read: None,
            }
            .into(),
            vec![held],
        );
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            !client
                .released
                .contains(&wayland_client::Proxy::id(&buffer))
        );
    }
    #[test]
    fn explicit_sync_disabled_dmabuf_never_advertises_the_protocol() {
        let mut display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        let event_loop = EventLoop::try_new().unwrap();
        state.explicit_sync.sender =
            Some(crate::explicit_sync::install(&event_loop.handle(), |s| s).unwrap());
        let file = std::fs::File::open("/dev/null").unwrap();
        state.explicit_sync.enable(
            &display.handle(),
            DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file))),
            false,
        );
        let (_, _, client) = connect(&mut display, &mut state);
        assert!(client.syncobj.is_none());
    }

    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_synchronized_children_wait_with_the_parent_transaction() {
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, parent) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let root = window.wl_surface().unwrap().into_owned();
        let child = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let _sub = client
            .subcompositor
            .as_ref()
            .unwrap()
            .get_subsurface(&child, &parent, &qh, ());
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&child, &qh, ());
        let timeline = gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        points(&sync, &timeline, 1, 2);
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        parent.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        let server = smithay::wayland::compositor::get_children(&root)
            .pop()
            .unwrap();
        assert!(
            current(&server).is_none(),
            "synchronized child escaped its acquire blocker"
        );
        gpu.signal(1);
        drive(&mut event_loop, &mut state);
        assert!(
            current(&server).is_some(),
            "parent transaction did not import the ready child"
        );
        let child_id = current(&server).unwrap().id();
        let snapshot = crate::explicit_sync::snapshot(&mut state);
        assert!(snapshot.iter().any(|buffer| buffer.id() == child_id));
        drop(snapshot);
        gpu.signal(3);
        points(&sync, &timeline, 3, 4);
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        parent.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        child.attach(None, 0, 0);
        child.commit();
        parent.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(
            gpu.point(),
            4,
            "child reattachment kept the first release point"
        );
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_invalid_commits_disconnect_only_the_offending_client() {
        use wp_linux_drm_syncobj_surface_v1::Error as E;
        for (case, expected) in [
            (0, E::NoAcquirePoint),
            (1, E::NoReleasePoint),
            (2, E::NoAcquirePoint),
            (3, E::ConflictingPoints),
            (4, E::NoBuffer),
            (5, E::UnsupportedBuffer),
        ] {
            let mut gpu = Gpu::new();
            let (mut display, mut state, _event_loop) = gpu.setup();
            let (conn, mut queue, mut client) = connect(&mut display, &mut state);
            let qh = queue.handle();
            let (_, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
            let sync = client
                .syncobj
                .as_ref()
                .unwrap()
                .get_surface(&surface, &qh, ());
            let timeline = gpu.timeline(&client, &qh);
            let buffer = gpu.buffer(&client, &qh);
            match case {
                0 => {}
                1 => sync.set_acquire_point(&timeline, 0, 1),
                2 => sync.set_release_point(&timeline, 0, 2),
                3 => points(&sync, &timeline, 2, 1),
                _ => points(&sync, &timeline, 1, 2),
            }
            match case {
                4 => {}
                5 => {
                    commit_buffer(&client, &qh, &surface, 80, 80);
                }
                _ => surface.attach(Some(&buffer), 0, 0),
            }
            if case != 5 {
                surface.commit();
            }
            conn.display().sync(&qh, ());
            conn.flush().unwrap();
            display.dispatch_clients(&mut state).unwrap();
            display.flush_clients().unwrap();
            assert!(
                queue.blocking_dispatch(&mut client).is_err(),
                "case {case} was accepted"
            );
            let error = conn.protocol_error().expect("protocol error");
            assert_eq!(error.code, expected as u32, "case {case}: {error}");
            let (_, _, other) = connect(&mut display, &mut state);
            assert!(other.compositor.is_some());
        }
    }

    #[test]
    fn explicit_sync_registration_failure_keeps_storage_until_completion() {
        let mut display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        let mut event_loop = EventLoop::try_new().unwrap();
        state.explicit_sync.sender =
            Some(crate::explicit_sync::install(&event_loop.handle(), |s| s).unwrap());
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let buffer = commit_buffer(&client, &qh, &surface, 80, 80);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let held = current(&window.wl_surface().unwrap()).unwrap();
        // epoll cannot watch a regular character device. Export succeeds,
        // source registration fails, and the completion wait also fails.
        let invalid = std::fs::File::open("/dev/null").unwrap();
        state.explicit_sync.retire(
            ReadFence {
                ready: Default::default(),
                read: Some(invalid.into()),
            }
            .into(),
            vec![held],
        );
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(
            !client
                .released
                .contains(&wayland_client::Proxy::id(&buffer))
        );
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_abandoned_acquires_do_not_keep_eventfds() {
        fn descriptors() -> usize {
            std::fs::read_dir("/proc/self/fd").unwrap().count()
        }
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let mut before = None;
        for iteration in 0..12 {
            let (conn, mut queue, mut client) = connect(&mut display, &mut state);
            let qh = queue.handle();
            let (_, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
            let sync = client
                .syncobj
                .as_ref()
                .unwrap()
                .get_surface(&surface, &qh, ());
            let timeline = gpu.timeline(&client, &qh);
            let buffer = gpu.buffer(&client, &qh);
            points(&sync, &timeline, iteration * 2 + 1, iteration * 2 + 2);
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            drive(&mut event_loop, &mut state);
            drop((surface, sync, timeline, buffer, qh, queue, client, conn));
            display.dispatch_clients(&mut state).unwrap();
            drive(&mut event_loop, &mut state);
            // Qt and GBM lazily create process-wide descriptors on first use.
            // Subsequent client lifetimes must not keep accumulating sources.
            if iteration == 0 {
                before = Some(descriptors());
            }
        }
        let before = before.unwrap();
        let after = descriptors();
        assert!(
            after <= before + 2,
            "abandoned acquires leaked descriptors: {before} before, {after} after"
        );
    }
    fn protocol_only() -> (Display<Solium>, Solium) {
        let display = Display::new().unwrap();
        let mut state = test_state(display.handle());
        // No ioctls are used: these cases create/destroy protocol objects and
        // reject missing points before touching a DRM timeline.
        let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(
            std::fs::File::open("/dev/null").unwrap(),
        )));
        state.explicit_sync.protocol = Some(smithay::wayland::drm_syncobj::DrmSyncobjState::new::<
            Solium,
        >(&display.handle(), fd));
        (display, state)
    }
    fn error_roundtrip(
        display: &mut Display<Solium>,
        state: &mut Solium,
        conn: &Connection,
        queue: &mut wayland_client::EventQueue<Client>,
        client: &mut Client,
    ) -> Option<wayland_client::backend::protocol::ProtocolError> {
        conn.display().sync(&queue.handle(), ());
        let _ = conn.flush();
        display.dispatch_clients(state).unwrap();
        display.flush_clients().unwrap();
        let _ = queue.blocking_dispatch(client);
        conn.protocol_error()
    }
    #[test]
    fn explicit_sync_recreated_object_requires_points() {
        let (mut display, mut state) = protocol_only();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (_, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let manager = client.syncobj.clone().unwrap();
        let first = manager.get_surface(&surface, &qh, ());
        first.destroy();
        let second = manager.get_surface(&surface, &qh, ());
        commit_buffer(&client, &qh, &surface, 80, 80);
        let error = error_roundtrip(&mut display, &mut state, &conn, &mut queue, &mut client)
            .expect("recreated object accepted missing points");
        assert_eq!(
            error.code,
            wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32
        );
        assert_eq!(
            error.object_id,
            wayland_client::Proxy::id(&second).protocol_id()
        );
    }
    #[test]
    fn explicit_sync_recreated_object_prevents_a_duplicate() {
        let (mut display, mut state) = protocol_only();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let manager = client.syncobj.clone().unwrap();
        let first = manager.get_surface(&surface, &qh, ());
        first.destroy();
        let _second = manager.get_surface(&surface, &qh, ());
        let _third = manager.get_surface(&surface, &qh, ());
        let error = error_roundtrip(&mut display, &mut state, &conn, &mut queue, &mut client)
            .expect("duplicate recreated object accepted");
        assert_eq!(error.code, wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_manager_v1::Error::SurfaceExists as u32);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_recreated_object_uses_a_new_timeline() {
        let gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let server = window.wl_surface().unwrap().into_owned();
        let previous = current(&server).unwrap().id();
        let manager = client.syncobj.clone().unwrap();
        let first = manager.get_surface(&surface, &qh, ());
        first.destroy();
        let second = manager.get_surface(&surface, &qh, ());
        let mut fresh = Gpu::new();
        let timeline = fresh.timeline(&client, &qh);
        let buffer = fresh.buffer(&client, &qh);
        points(&second, &timeline, 1, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        assert_eq!(current(&server).unwrap().id(), previous);
        fresh.signal(1);
        drive(&mut event_loop, &mut state);
        assert_ne!(current(&server).unwrap().id(), previous);
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert_eq!(fresh.point(), 2);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_composed_frame_retires_its_source_after_gpu_completion() {
        use smithay::backend::{
            egl::{EGLContext, EGLDisplay},
            renderer::{
                Bind as _, Offscreen as _,
                damage::OutputDamageTracker,
                element::{
                    Kind,
                    surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                },
                gles::{GlesRenderbuffer, GlesRenderer},
            },
        };
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let server = window.wl_surface().unwrap().into_owned();
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&surface, &qh, ());
        let timeline = gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        gpu.signal(1);
        points(&sync, &timeline, 1, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, 80, 80);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        let gbm = GbmDevice::new(gpu.fd.clone()).unwrap();
        #[expect(unsafe_code, reason = "the GBM device outlives the test EGL display")]
        let egl = unsafe { EGLDisplay::new(gbm) }.unwrap();
        let context = EGLContext::new(&egl).unwrap();
        #[expect(
            unsafe_code,
            reason = "the test renderer owns its current GLES context"
        )]
        let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
        let size: smithay::utils::Size<i32, smithay::utils::Physical> = (80, 80).into();
        // Sample client storage through the real surface element path. A
        // linear client dmabuf need not itself support framebuffer binding;
        // only the compositor's renderbuffer below is a render target.
        let sources = crate::explicit_sync::snapshot(&mut state);
        assert!(!sources.is_empty());
        let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(
                &mut renderer,
                &server,
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            );
        assert!(!elements.is_empty());
        let mut image: GlesRenderbuffer = renderer
            .create_buffer(Fourcc::Abgr8888, (80, 80).into())
            .unwrap();
        {
            let mut target = renderer.bind(&mut image).unwrap();
            let mut damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
            let rendered = damage
                .render_output(
                    &mut renderer,
                    &mut target,
                    0,
                    &elements,
                    [0.0, 0.0, 0.0, 1.0],
                )
                .unwrap();
            drop(rendered);
        }
        drop(elements);
        crate::explicit_sync::submitted(&mut state, &mut renderer, sources);
        surface.attach(None, 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        for _ in 0..100 {
            if gpu.point() == 2 {
                break;
            }
            drive(&mut event_loop, &mut state);
        }
        assert_eq!(
            gpu.point(),
            2,
            "completed GPU source was never returned to the client"
        );
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_destroy_returns_the_same_buffer_to_implicit_sync() {
        let mut gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let server = window.wl_surface().unwrap().into_owned();
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&surface, &qh, ());
        let timeline = gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        gpu.signal(1);
        points(&sync, &timeline, 1, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        assert_eq!(gpu.point(), 1);
        sync.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        assert!(current(&server).is_some());
        assert_eq!(
            gpu.point(),
            2,
            "implicit reattachment retained the old explicit release point"
        );
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_destroying_an_unready_child_releases_its_parent_transaction() {
        destroy_queued_child(true, false);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_destroying_child_before_source_registration_releases_buffer() {
        destroy_queued_child(false, false);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_destroying_child_as_acquire_signals_releases_buffer() {
        destroy_queued_child(true, true);
    }
    fn destroy_queued_child(register: bool, ready: bool) {
        let mut gpu = Gpu::new();
        let release_gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, parent) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let root = window.wl_surface().unwrap().into_owned();
        let previous = current(&root).unwrap().id();
        let child = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let sub = client
            .subcompositor
            .as_ref()
            .unwrap()
            .get_subsurface(&child, &parent, &qh, ());
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&child, &qh, ());
        let acquire = gpu.timeline(&client, &qh);
        let release = release_gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        // Separate timelines ensure discarding release cannot signal acquire.
        sync.set_acquire_point(&acquire, 0, 1);
        sync.set_release_point(&release, 0, 1);
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        let _parent_buffer = commit_buffer(&client, &qh, &parent, 80, 80);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        if register {
            drive(&mut event_loop, &mut state);
        }
        assert_eq!(current(&root).unwrap().id(), previous);
        if ready {
            gpu.signal(1);
        }
        child.destroy();
        sub.destroy();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        drive(&mut event_loop, &mut state);
        assert_eq!(
            gpu.point(),
            u64::from(ready),
            "discarding release must not change acquire readiness"
        );
        assert_ne!(
            current(&root).unwrap().id(),
            previous,
            "destroyed child left its parent transaction blocked"
        );
        assert_eq!(
            release_gpu.point(),
            1,
            "destroyed child did not release its discarded buffer"
        );
    }

    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_detached_cached_child_releases_discarded_buffer() {
        replace_cached_child(false);
    }
    #[test]
    #[ignore = "requires SOLIUM_TEST_RENDER_NODE; run dev/explicit-sync-check.sh"]
    fn explicit_sync_implicit_cached_child_does_not_inherit_discarded_points() {
        replace_cached_child(true);
    }
    fn replace_cached_child(implicit: bool) {
        let mut gpu = Gpu::new();
        let release_gpu = Gpu::new();
        let (mut display, mut state, mut event_loop) = gpu.setup();
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, _, parent) = open_surface(&mut display, &mut state, &conn, &client, &qh);
        let root = window.wl_surface().unwrap().into_owned();
        let child = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let _sub = client
            .subcompositor
            .as_ref()
            .unwrap()
            .get_subsurface(&child, &parent, &qh, ());
        let sync = client
            .syncobj
            .as_ref()
            .unwrap()
            .get_surface(&child, &qh, ());
        let acquire = gpu.timeline(&client, &qh);
        let release = release_gpu.timeline(&client, &qh);
        let buffer = gpu.buffer(&client, &qh);
        sync.set_acquire_point(&acquire, 0, 1);
        sync.set_release_point(&release, 0, 1);
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        sync.destroy();
        let replacement = implicit.then(|| commit_buffer(&client, &qh, &child, 40, 40));
        if !implicit {
            child.attach(None, 0, 0);
            child.commit();
        }
        parent.commit();
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        gpu.signal(1);
        drive(&mut event_loop, &mut state);
        assert_eq!(
            release_gpu.point(),
            1,
            "superseded cached buffer was not released"
        );
        if replacement.is_some() {
            let children = smithay::wayland::compositor::get_children(&root);
            let child = children.first().unwrap();
            assert!(current(child).is_some(), "implicit child was not imported");
        }
    }
}
