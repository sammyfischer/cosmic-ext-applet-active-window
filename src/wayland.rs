use std::{
  any::TypeId,
  collections::HashSet,
  env,
  fmt::Display,
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
    DesktopEntryData,
    DesktopLookupContext,
    DesktopResolveOptions,
    fde::{self, IconSource},
    resolve_desktop_entry,
  },
  iced::Subscription,
};
use futures::{
  SinkExt,
  StreamExt,
  channel::mpsc::{Sender, UnboundedSender},
};

use crate::{app::DesktopLaunch, fl};

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

/// Used to check equality between window activations. If any field changes,
/// that triggers refetching of info and redrawing the ui.
#[derive(Debug, PartialEq)]
struct WindowKey {
  id: String,
  app_id: String,
  title: String,
  state: HashSet<State>,
}

impl From<&ToplevelInfo> for WindowKey {
  fn from(value: &ToplevelInfo) -> Self {
    Self {
      id: value.identifier.clone(),
      app_id: value.app_id.clone(),
      title: value.title.clone(),
      state: value.state.clone(),
    }
  }
}

sctk::delegate_registry!(WaylandState);
cctk::delegate_toplevel_info!(WaylandState);
cctk::delegate_toplevel_manager!(WaylandState);
sctk::delegate_seat!(WaylandState);

/// Stores toplevel and desktop entry info of a particular window
#[derive(Clone, Debug)]
pub struct WindowData {
  info: ToplevelInfo,

  // stores [DesktopEntryData] bc after construction, it contains everything we need.
  // [DesktopEntry] is no longer needed
  desktop: DesktopEntryData,

  // stored separately from `desktop` bc it may fallback to the window title. instead resolve it
  // upon construction to fallback to the app id
  app_name: String,

  // stored separately bc we add a custom new-window action *only if* one isn't already defined. we
  // need the [DesktopEntry] to determine that.
  actions: Vec<DesktopLaunch>,
}

impl WindowData {
  pub fn new(info: ToplevelInfo, cache: &mut DesktopEntryCache) -> Self {
    let app_id = info.app_id.as_str();

    let ctx = DesktopLookupContext::new(info.app_id.as_str())
      .with_identifier(info.identifier.as_str())
      .with_title(info.title.as_str()); // title is used for wine/proton windows

    let opts = DesktopResolveOptions::default();
    let entry = resolve_desktop_entry(cache, &ctx, &opts);
    let is_fallback = entry.path.is_empty(); // the fallback for an unmatched appid can be detected if the path is empty

    // whether there's a custom new-window action in the desktop file. if there
    // is, we don't need to make our own
    let mut has_new_window = false;
    let mut actions = Vec::new();
    for action in entry.actions().into_iter().flatten() {
      if action == "new-window" {
        has_new_window = true;
      }

      let Some(exec) = entry.action_entry(action, "Exec")
      else {
        // no exec, nothing to run
        continue;
      };

      let Some(name) = entry.action_entry_localized(action, "Name", cache.locales())
      else {
        continue;
      };

      actions.push(DesktopLaunch {
        name: name.to_string(),
        app_id: app_id.to_string(),
        exec: exec.to_string(),
        term: entry.terminal(),
      });
    }

    // make a custom new-window action that runs the app's main exec command.
    // this isn't guaranteed to launch a new window, some apps will refuse to do
    // this if a window already exists
    if !has_new_window && let Some(exec) = entry.exec() {
      actions.insert(0, DesktopLaunch {
        name: fl!("launch-new-window"),
        app_id: app_id.to_string(),
        exec: exec.to_string(),
        term: entry.terminal(),
      });
    }

    let data = DesktopEntryData::from_desktop_entry(cache.locales(), entry);
    let app_name = if is_fallback {
      // the fallback is to use the window title (which is redundant), use
      // the app id instead
      app_id.to_string()
    }
    else {
      data.name.clone()
    };

    Self {
      info,
      desktop: data,
      app_name,
      actions,
    }
  }

  // TOPLEVEL STUFF

  /// The displayed title text of the window. This can be arbitrarily set by the
  /// application.
  pub fn title(&self) -> &str {
    &self.info.title
  }

  /// Gets the foreign toplevel handle. Use this to make [requests] on the
  /// window.
  ///
  /// [requests]: WaylandRequest
  pub fn foreign_toplevel(&self) -> &ToplevelHandle {
    &self.info.foreign_toplevel
  }

  pub fn is_minimized(&self) -> bool {
    self.info.state.contains(&State::Minimized)
  }

  pub fn is_maximized(&self) -> bool {
    self.info.state.contains(&State::Maximized)
  }

  pub fn is_fullscreen(&self) -> bool {
    self.info.state.contains(&State::Fullscreen)
  }

  pub fn is_sticky(&self) -> bool {
    self.info.state.contains(&State::Sticky)
  }

  // DESKTOP STUFF

  /// The user-friendly app name, which may not be unique
  pub fn app_name(&self) -> &str {
    &self.app_name
  }

  /// The app icon
  pub fn icon(&self) -> &IconSource {
    &self.desktop.icon
  }

  /// The list of desktop actions the app may have
  pub fn actions(&self) -> &Vec<DesktopLaunch> {
    &self.actions
  }
}

impl Display for WindowData {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "WindowData {{ name: {}, title: {} }}",
      self.app_name(),
      self.title()
    )
  }
}

// [WindowData] significantly increases the size of the enum compared to the
// Init variant. However, it's by far the most common event that will be sent,
// so it's not necessary to box it.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WaylandEvent {
  /// Initialize the channel to send wayland requests
  Init(calloop::channel::Sender<WaylandRequest>),

  /// A window was activated
  Activate(Option<WindowData>),
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
}

impl ProvidesRegistryState for WaylandState {
  fn registry(&mut self) -> &mut RegistryState {
    &mut self.registry_state
  }

  sctk::registry_handlers!();
}

impl ToplevelInfoHandler for WaylandState {
  fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
    &mut self.toplevel_info_state
  }

  fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}

  fn info_done(&mut self, _: &Connection, _: &QueueHandle<Self>) {
    let active_info = self
      .toplevel_info_state
      .toplevels()
      .find(|info| info.state.contains(&State::Activated));

    // check equality based on key first, resolve desktop info later
    let active_key = active_info.map(WindowKey::from);
    if active_key == self.last_sent {
      return;
    }

    tracing::info!(?active_key, "detected active window change");
    self.last_sent = active_key;
    let active = active_info.map(|info| WindowData::new(info.to_owned(), &mut self.desktop_cache));

    if let Err(why) = self.tx.unbounded_send(WaylandEvent::Activate(active)) {
      tracing::error!(?why, "failed to send window activation event");
      self.exit = true;
    }
  }
}

impl ToplevelManagerHandler for WaylandState {
  fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
    &mut self.toplevel_mgr_state
  }

  fn capabilities(
    &mut self,
    _: &Connection,
    _: &QueueHandle<Self>,
    _: Vec<WEnum<ToplevelManagementCapabilities>>,
  ) {
  }
}

impl SeatHandler for WaylandState {
  fn seat_state(&mut self) -> &mut SeatState {
    &mut self.seat_state
  }

  fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
  fn new_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}
  fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {
  }
  fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl WaylandState {
  /// Converts a wayland toplevel handle into a cosmic toplevel handle
  fn toplevel_info(&self, handle: &ToplevelHandle) -> Option<&ToplevelInfo> {
    self.toplevel_info_state.info(handle)
  }

  fn handle_request(&mut self, req: WaylandRequest) {
    let mgr = &self.toplevel_mgr_state.manager;

    match req {
      WaylandRequest::Close(handle) => {
        if let Some(info) = self.toplevel_info(&handle)
          && let Some(cosmic_toplevel) = info.cosmic_toplevel.as_ref()
        {
          mgr.close(cosmic_toplevel);
        }
      }

      WaylandRequest::Minimize(handle) => {
        if let Some(info) = self.toplevel_info(&handle)
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

      WaylandRequest::Maximize(handle) => {
        if let Some(info) = self.toplevel_info(&handle)
          && let Some(ct) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Maximized) {
            mgr.unset_maximized(ct);
          }
          else {
            mgr.set_maximized(ct);
          }
        }
      }

      WaylandRequest::Fullscreen(handle) => {
        if let Some(info) = self.toplevel_info(&handle)
          && let Some(ct) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Fullscreen) {
            mgr.unset_fullscreen(ct);
          }
          else {
            // None tells the compositor to choose the output
            // (display/monitor)
            mgr.set_fullscreen(ct, None);
          }
        }
      }

      WaylandRequest::Sticky(handle) => {
        if let Some(info) = self.toplevel_info(&handle)
          && let Some(ct) = info.cosmic_toplevel.as_ref()
        {
          if info.state.contains(&State::Sticky) {
            mgr.unset_sticky(ct);
          }
          else {
            mgr.set_sticky(ct);
          }
        }
      }
    }
  }
}

/// Subscription handler for wayland events
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
