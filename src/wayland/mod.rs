use std::{
  any::TypeId,
  env,
  os::{
    fd::{FromRawFd, RawFd},
    unix::net::UnixStream,
  },
  sync::LazyLock,
};

use anyhow::Context;
use cosmic::{
  cctk::{
    self,
    cosmic_protocols::{
      toplevel_info::v1::client::zcosmic_toplevel_handle_v1::State,
      toplevel_management::v1::client::zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1 as ToplevelManagementCapabilities,
    },
    screencopy::ScreencopyState,
    sctk::{
      self,
      reexports::{calloop, calloop_wayland_source::WaylandSource},
      registry::{ProvidesRegistryState, RegistryState},
      seat::{Capability, SeatHandler, SeatState},
      shm::Shm,
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
  desktop::{DesktopEntryCache, fde},
  iced::Subscription,
};
use futures::{
  SinkExt,
  StreamExt,
  channel::mpsc::{Sender, UnboundedReceiver, UnboundedSender},
};
pub use window::*;

mod screencopy;
mod window;

/// Stores shared state of wayland handler
pub struct WaylandState {
  last_sent: Option<String>,
  tx: UnboundedSender<WaylandEvent>,
  exit: bool,

  connection: Connection,
  queue_handle: QueueHandle<Self>,
  desktop_cache: DesktopEntryCache,

  registry_state: RegistryState,
  toplevel_info_state: ToplevelInfoState,
  toplevel_mgr_state: ToplevelManagerState,
  seat_state: SeatState,
  screencopy_state: ScreencopyState,
  shm_state: Shm,
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

  /// Wayland handler thread died
  Finished,

  AppUpdate(Option<ActiveApp>),
  WindowUpdate(Vec<ToplevelInfo>),

  Image(ToplevelInfo, image::RgbaImage),
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

  /// Get a thumbnail image of the window
  Screencopy(ToplevelHandle),
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
    let active = self
      .toplevel_info_state
      .toplevels()
      .find(|info| info.state.contains(&State::Activated));

    let appid = active.map(|info| info.app_id.as_str());
    if appid != self.last_sent.as_deref() {
      // active app changed, recalc everything
      let toplevels: Vec<_> = self.toplevel_info_state.toplevels().collect();
      let active = ActiveApp::new(&toplevels, &mut self.desktop_cache);

      self.last_sent = appid.map(ToString::to_string);
      self.send_event(WaylandEvent::AppUpdate(active));
      return;
    }

    // else send new set of windows
    let toplevels: Vec<_> = self.toplevel_info_state.toplevels().cloned().collect();
    self.send_event(WaylandEvent::WindowUpdate(toplevels));
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

      WaylandRequest::Screencopy(foreign_toplevel) => {
        if let Some(info) = self.toplevel_info(&foreign_toplevel) {
          self.send_image(info.clone());
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

  let connection = if let Some(socket) = socket {
    let conn =
      Connection::from_socket(socket).context("failed to connect to wayland privileged socket")?;
    tracing::info!("connected to wayland privileged socket");
    conn
  }
  else {
    let conn = Connection::connect_to_env().context("failed to connect to wayland socket")?;
    tracing::info!("connected to wayland socket");
    conn
  };

  let (global_list, event_queue) =
    registry_queue_init(&connection).context("failed to initialize registry queue")?;
  let queue_handle = event_queue.handle();

  let mut event_loop =
    calloop::EventLoop::<WaylandState>::try_new().context("failed to initialize event loop")?;
  let loop_handle = event_loop.handle();

  // links the event queue as a source in the event loop
  //
  // this links the old implementation, which was simply listening on the
  // event queue for toplevel activations. we now need to combine that into a
  // common event loop which also handles the new requests
  WaylandSource::new(connection.clone(), event_queue)
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
  let screencopy_state = ScreencopyState::new(&global_list, &queue_handle);
  let shm_state = Shm::bind(&global_list, &queue_handle).context("failed to bind shared memory")?;

  let mut state = WaylandState {
    last_sent: None,
    tx,
    exit: false,

    connection,
    queue_handle,
    desktop_cache,

    registry_state,
    toplevel_info_state,
    toplevel_mgr_state,
    seat_state,
    screencopy_state,
    shm_state,
  };

  tracing::info!("wayland handler spawned");
  while !state.exit && !state.tx.is_closed() {
    event_loop.dispatch(None, &mut state)?;
  }
  tracing::warn!("wayland handler exited");

  Ok(())
}

static EVENT_RX: LazyLock<tokio::sync::Mutex<Option<UnboundedReceiver<WaylandEvent>>>> =
  LazyLock::new(|| tokio::sync::Mutex::new(None));

enum SubscriptionState {
  Waiting,
  Finished,
}

/// Entry point to the wayland handler. Sets up channels and spawns thread.
pub fn wayland_subscription() -> cosmic::iced::Subscription<WaylandEvent> {
  Subscription::run_with(TypeId::of::<WaylandEvent>(), |_| {
    cosmic::iced::stream::channel(50, move |mut output: Sender<WaylandEvent>| async move {
      let mut state = SubscriptionState::Waiting;

      loop {
        state = start_listening(state, &mut output).await;
      }
    })
  })
}

/// State machine impl for subscription. If the current state is
/// [`SubscriptionState::Waiting`], handle the next event. If the state is
/// [`SubscriptionState::Finished`], wait indefinitely. The state is set to
/// finished when the event receiver closes.
async fn start_listening(
  state: SubscriptionState,
  output: &mut Sender<WaylandEvent>,
) -> SubscriptionState {
  match state {
    SubscriptionState::Waiting => {
      let mut guard = EVENT_RX.lock().await;
      let event_rx = {
        // if event receiver isn't initialized
        if guard.is_none() {
          // channel to handle wayland requests (close, minimize, etc.)
          let (request_tx, request_rx) = calloop::channel::channel();

          // channel to handle events (active app/window changed)
          let (event_tx, event_rx) = futures::channel::mpsc::unbounded();

          // spawn thread once
          std::thread::spawn(move || {
            if let Err(why) = wayland_handler(event_tx, request_rx) {
              tracing::error!(?why, "wayland handler errored: {why}");
            }
          });

          *guard = Some(event_rx);

          if let Err(why) = output.send(WaylandEvent::Init(request_tx)).await {
            tracing::error!(?why, "failed to send init event: {why}");
          }
        }

        guard.as_mut().unwrap()
      };

      if let Some(event) = event_rx.next().await {
        _ = output.send(event).await;
        SubscriptionState::Waiting
      }
      else {
        _ = output.send(WaylandEvent::Finished).await;
        tracing::error!("wayland handler died");
        SubscriptionState::Finished
      }
    }

    SubscriptionState::Finished => cosmic::iced::futures::future::pending().await,
  }
}
