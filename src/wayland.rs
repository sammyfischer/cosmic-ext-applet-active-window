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
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::State,
    sctk::{
      self,
      registry::{ProvidesRegistryState, RegistryState},
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    wayland_client::{Connection, QueueHandle, globals::registry_queue_init},
    wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1 as ToplevelHandle,
  },
  iced::Subscription,
};
use futures::{
  SinkExt,
  channel::mpsc::{Sender, UnboundedSender},
};

pub struct WindowData {
  exit: bool,
  tx: UnboundedSender<WaylandEvent>,
  registry_state: RegistryState,
  toplevel_state: ToplevelInfoState,
  last_sent: Option<ActiveWindow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveWindow {
  /// The displayed title text of the window. This can be arbitrarily set by the
  /// application.
  pub title: String,

  /// The name of the app. This may be a name or an RDNN.
  pub app_id: String,
}

impl From<&ToplevelInfo> for ActiveWindow {
  fn from(value: &ToplevelInfo) -> Self {
    Self {
      title: value.title.clone(),
      app_id: value.app_id.clone(),
    }
  }
}

#[derive(Clone, Debug)]
pub enum WaylandEvent {
  // currently only one event, might need more later
  Activate(Option<ActiveWindow>),
}

impl ProvidesRegistryState for WindowData {
  fn registry(&mut self) -> &mut RegistryState {
    &mut self.registry_state
  }

  sctk::registry_handlers!();
}

impl ToplevelInfoHandler for WindowData {
  fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
    &mut self.toplevel_state
  }

  fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}
  fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ToplevelHandle) {}

  fn info_done(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>) {
    let active: Option<ActiveWindow> = self
      .toplevel_state
      .toplevels()
      .find(|info| info.state.contains(&State::Activated))
      .map(Into::into);

    // send title if it changed
    if active != self.last_sent {
      self.last_sent.clone_from(&active);

      // set exit flag if it errors (it only errors when the receiver is
      // dropped/closed)
      if self
        .tx
        .unbounded_send(WaylandEvent::Activate(active))
        .is_err()
      {
        self.exit = true;
      }
    }
  }
}

/// Subscription handler for wayland events
pub fn wayland_handler(tx: UnboundedSender<WaylandEvent>) -> anyhow::Result<()> {
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

  let (global_list, mut event_queue) =
    registry_queue_init(&conn).context("failed to initialize registry queue")?;
  let queue_handle = event_queue.handle();

  // let mut event_loop = calloop::EventLoop::<WindowData>::try_new()?;
  // let loop_handle = event_loop.handle();

  // let wayland_source = WaylandSource::new(conn, event_queue);
  // wayland_source
  //   .insert(loop_handle.clone())
  //   .context("failed to add wayland source into event loop")?;

  // if loop_handle
  //   .insert_source(rx, |event, _, window_data| match event {
  //     calloop::channel::Event::Msg(()) => {}
  //     calloop::channel::Event::Closed => window_data.exit = true,
  //   })
  //   .is_err()
  // {
  //   bail!("failed to insert loop handle source");
  // };

  let registry_state = RegistryState::new(&global_list);
  let toplevel_state = ToplevelInfoState::new(&registry_state, &queue_handle);
  let mut window_data = WindowData {
    exit: false,
    tx,
    registry_state,
    toplevel_state,
    last_sent: None,
  };

  while !window_data.exit {
    event_queue.blocking_dispatch(&mut window_data)?;
    // event_loop
    //   .dispatch(None, &mut window_data)
    //   .context("failed to dispatch an event")?;
  }

  Ok(())
}

pub fn wayland_subscription() -> cosmic::iced::Subscription<WaylandEvent> {
  Subscription::run_with(TypeId::of::<WaylandEvent>(), |_| {
    cosmic::iced::stream::channel(10, move |mut output: Sender<WaylandEvent>| async move {
      let (tx, mut rx) = futures::channel::mpsc::unbounded();
      std::thread::spawn(move || wayland_handler(tx));

      while let Ok(event) = rx.recv().await {
        let _ = output.send(event).await;
      }
      futures::future::pending::<()>().await;
    })
  })
}

cctk::delegate_toplevel_info!(WindowData); // implements Dispatch for compatibility with ToplevelInfoState::new
sctk::delegate_registry!(WindowData); // implements Dispatch for compatibility with registry_queue_init
