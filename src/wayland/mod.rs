use std::{
  any::TypeId,
  env,
  os::{
    fd::{FromRawFd, RawFd},
    unix::net::UnixStream,
  },
};

use anyhow::Context;
use cosmic::{
  cctk::{
    self,
    cosmic_protocols::{
      toplevel_info::v1::client::zcosmic_toplevel_handle_v1::State,
      toplevel_management::v1::client::zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1 as ToplevelManagementCapabilities,
    },
    sctk::{
      self,
      reexports::{calloop, calloop_wayland_source::WaylandSource},
      registry::{ProvidesRegistryState, RegistryState},
      seat::{Capability, SeatHandler, SeatState},
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    toplevel_management::{ToplevelManagerHandler, ToplevelManagerState},
    wayland_client::{
      Connection,
      QueueHandle,
      WEnum,
      globals::registry_queue_init,
      protocol::wl_seat::WlSeat,
    },
    wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1 as ToplevelHandle,
  },
  desktop::{
    DesktopEntryCache,
    fde::{self},
  },
  iced::Subscription,
};
use futures::{
  SinkExt,
  StreamExt,
  channel::mpsc::{Sender, UnboundedSender},
};
pub use window::*;

mod window;

/// Stores shared state of wayland handling
pub struct WaylandState {
  exit: bool,
  tx: UnboundedSender<WaylandEvent>,
  desktop_cache: DesktopEntryCache,
  last_sent: Option<WindowKey>,

  registry_state: RegistryState,
  toplevel_info_state: ToplevelInfoState,
  toplevel_mgr_state: ToplevelManagerState,
  seat_state: SeatState,
}

sctk::delegate_registry!(WaylandState);
cctk::delegate_toplevel_info!(WaylandState);
cctk::delegate_toplevel_manager!(WaylandState);
sctk::delegate_seat!(WaylandState);

// [WindowData] significantly increases the size of the enum compared to the
// Init variant. However, it's by far the most common event that will be sent,
// so it's not necessary to box it.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WaylandEvent {
  /// Initialize the channel to send wayland requests
  Init(calloop::channel::Sender<WaylandRequest>),

  /// A window was activated
  Activated(Option<WindowData>),

  /// The activated window is unchanged, but its list of siblings changed
  SiblingsChanged(Vec<SiblingWindowData>),
}

#[derive(Clone, Debug)]
pub enum WaylandRequest {
  /// Close the window
  Close(ToplevelHandle),

  /// Minimize the window
  ///
  /// This can't be a toggle, because the window loses focus after being
  /// minimized, thus the applet will switch to displaying the new active window
  Minimize(ToplevelHandle),

  /// Toggle maximization state of a window
  Maximize(ToplevelHandle),

  /// Toggle fullscreen state of a window
  Fullscreen(ToplevelHandle),

  /// Toggles sticky state of a window
  Sticky(ToplevelHandle),

  /// Grabs window focus
  Activate(ToplevelHandle),
}

impl ProvidesRegistryState for WaylandState {
  fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }

  sctk::registry_handlers!();
}

impl ToplevelInfoHandler for WaylandState {
  fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState { &mut self.toplevel_info_state }

  fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}

  fn info_done(&mut self, _: &Connection, _: &QueueHandle<Self>) {
    let active_info = self
      .toplevel_info_state
      .toplevels()
      .find(|info| info.state.contains(&State::Activated));

    let siblings: Vec<_> = self
      .toplevel_info_state
      .toplevels()
      .filter_map(|info| {
        if active_info.is_some_and(|it| info.app_id == it.app_id)
          && !info.state.contains(&State::Activated)
        {
          Some(SiblingWindowData::new(info))
        }
        else {
          None
        }
      })
      .collect();

    // check equality based on key first, resolve desktop info later
    let new_key = active_info.map(|info| WindowKey::new(info, siblings.clone()));
    if new_key == self.last_sent {
      return;
    }

    match (new_key.as_ref(), self.last_sent.as_ref()) {
      // no change
      (None, None) => {}

      // both exist, and the active window's data is unchanged
      (Some(new), Some(old)) if new.active == old.active => {
        // sibling data is unchanged => whole key is unchanged, no update
        if new.siblings == old.siblings {
          return;
        }
        // sibling data is changed => just update siblings
        tracing::info!(?new_key, "detected sibling window change");
        self.last_sent = new_key;
        self.send_event(WaylandEvent::SiblingsChanged(siblings));
      }

      // full update
      _ => {
        tracing::info!(?new_key, "detected active window change");
        self.last_sent = new_key;
        let active = active_info
          .map(|info| WindowData::new(info.to_owned(), siblings, &mut self.desktop_cache));

        self.send_event(WaylandEvent::Activated(active));
      }
    }
  }
}

impl ToplevelManagerHandler for WaylandState {
  fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState { &mut self.toplevel_mgr_state }

  fn capabilities(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: Vec<WEnum<ToplevelManagementCapabilities>>,
  ) {
  }
}

impl SeatHandler for WaylandState {
  fn seat_state(&mut self) -> &mut SeatState { &mut self.seat_state }

  fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
  fn new_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}
  fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {
  }
  fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl WaylandState {
  /// Sends a wayland event. If the channel is closed, traces the error and sets
  /// the exit flag.
  fn send_event(&mut self, event: WaylandEvent) {
    if let Err(why) = self.tx.unbounded_send(event) {
      tracing::error!(%why, "failed to send wayland event");
      self.exit = true;
    }
  }

  /// Gets [`ToplevelInfo`] from a foreign toplevel handle
  fn toplevel_info(&self, handle: &ToplevelHandle) -> Option<&ToplevelInfo> {
    self.toplevel_info_state.info(handle)
  }

  /// Handles an incoming wayland request (e.g. close window, minimize)
  fn handle_request(&mut self, req: WaylandRequest) {
    let mgr = &self.toplevel_mgr_state.manager;

    match req {
      WaylandRequest::Close(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          mgr.close(cosmic_toplevel);
        }
      }

      WaylandRequest::Minimize(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Minimized) {
            mgr.unset_minimized(cosmic_toplevel);
          }
          else {
            mgr.set_minimized(cosmic_toplevel);
          }
        }
      }

      WaylandRequest::Maximize(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Maximized) {
            mgr.unset_maximized(cosmic_toplevel);
          }
          else {
            mgr.set_maximized(cosmic_toplevel);
          }
        }
      }

      WaylandRequest::Fullscreen(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Fullscreen) {
            mgr.unset_fullscreen(cosmic_toplevel);
          }
          else {
            // None tells the compositor to choose the output
            // (display/monitor)
            mgr.set_fullscreen(cosmic_toplevel, None);
          }
        }
      }

      WaylandRequest::Sticky(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Sticky) {
            mgr.unset_sticky(cosmic_toplevel);
          }
          else {
            mgr.set_sticky(cosmic_toplevel);
          }
        }
      }

      WaylandRequest::Activate(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
          && let Some(seat) = self.seat_state.seats().next()
        {
          mgr.activate(cosmic_toplevel, &seat);
        }
      }
    }
  }
}

/// Wayland event/request handler thread. Creates the event loop/queue,
/// initializes state, and listens for events until shutdown.
pub fn wayland_handler(
  tx: UnboundedSender<WaylandEvent>,
  rx: calloop::channel::Channel<WaylandRequest>,
) -> anyhow::Result<()> {
  let socket = env::var("X_PRIVILEGED_WAYLAND_SOCKET").ok().and_then(|fd| {
    fd.parse::<RawFd>()
      .ok()
      .map(|fd| unsafe { UnixStream::from_raw_fd(fd) })
  });

  let conn = if let Some(socket) = socket {
    Connection::from_socket(socket)
  }
  else {
    Connection::connect_to_env()
  }
  .context("failed to initialize wayland connection")?;

  let (global_list, event_queue) =
    registry_queue_init(&conn).context("failed to initialize registry queue")?;
  let queue_handle = event_queue.handle();

  let mut event_loop =
    calloop::EventLoop::<WaylandState>::try_new().context("failed to initialize event loop")?;
  let loop_handle = event_loop.handle();

  // links the event queue as a source in the event loop
  //
  // this links the old implementation, which was simply listening on the
  // event queue for toplevel activations. we now need to combine that into a
  // common event loop which also handles the new requests
  WaylandSource::new(conn.clone(), event_queue)
    .insert(loop_handle.clone())
    .context("failed to add wayland source to event loop")?;

  // add our own source to handle our requests
  loop_handle
    .insert_source(rx, |event, (), state| match event {
      calloop::channel::Event::Msg(req) => state.handle_request(req),
      calloop::channel::Event::Closed => state.exit = true,
    })
    .map_err(|why| why.error)
    .context("failed to insert request channel to event loop")?;

  let desktop_cache = DesktopEntryCache::new(fde::get_languages_from_env());
  let registry_state = RegistryState::new(&global_list);
  let toplevel_info_state = ToplevelInfoState::new(&registry_state, &queue_handle);
  let toplevel_mgr_state = ToplevelManagerState::new(&registry_state, &queue_handle);
  let seat_state = SeatState::new(&global_list, &queue_handle);

  let mut state = WaylandState {
    exit: false,
    tx,
    desktop_cache,
    last_sent: None,
    registry_state,
    toplevel_info_state,
    toplevel_mgr_state,
    seat_state,
  };

  tracing::info!("wayland handler spawned");
  while !state.exit && !state.tx.is_closed() {
    event_loop.dispatch(None, &mut state)?;
  }
  tracing::warn!("wayland handler exited");

  Ok(())
}

/// Entry point to the wayland handler. Sets up channels and spawns thread.
pub fn wayland_subscription() -> cosmic::iced::Subscription<WaylandEvent> {
  Subscription::run_with(TypeId::of::<WaylandEvent>(), |_| {
    cosmic::iced::stream::channel(10, move |mut output: Sender<WaylandEvent>| async move {
      // channel to handle wayland requests (close, minimize, etc.)
      let (request_tx, request_rx) = calloop::channel::channel();

      // channel to handle events (window activation state changed)
      let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();

      std::thread::spawn(move || {
        if let Err(why) = wayland_handler(event_tx, request_rx) {
          tracing::error!(?why, "wayland handler errored");
        }
      });

      if let Err(why) = output.send(WaylandEvent::Init(request_tx)).await {
        tracing::error!(?why, "failed to send init event");
      }
      while let Some(event) = event_rx.next().await {
        if let Err(why) = output.send(event).await {
          tracing::error!(?why, "failed to forward an event");
        }
      }

      futures::future::pending::<()>().await;
    })
  })
}
