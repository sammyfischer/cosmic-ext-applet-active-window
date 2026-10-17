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
    toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
    wayland_client::{Connection, QueueHandle, globals::registry_queue_init},
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
  channel::mpsc::{Sender, UnboundedSender},
};

pub struct WindowData {
  exit: bool,
  tx: UnboundedSender<WaylandEvent>,
  registry_state: RegistryState,
  toplevel_state: ToplevelInfoState,
  desktop_cache: DesktopEntryCache,
  last_sent: Option<ActiveWindow>,
}

#[derive(Clone, Debug)]
pub struct ActiveWindow {
  /// The displayed title text of the window. This can be arbitrarily set by the
  /// application.
  pub title: String,

  /// The name of the app. This may be a name or an RDNN.
  pub app_id: String,

  /// The user-friendly app name, which doesn't need to be unique.
  pub app_name: String,

  /// The app icon
  pub icon_source: IconSource,
}

impl PartialEq for ActiveWindow {
  fn eq(&self, other: &Self) -> bool {
    self.app_id == other.app_id && self.title == other.title
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
    let active = self
      .toplevel_state
      .toplevels()
      .find(|info| info.state.contains(&State::Activated))
      .map(|info| {
        let title = info.title.clone();
        let app_id = info.app_id.clone();

        let ctx = DesktopLookupContext::new(info.app_id.as_str())
          .with_identifier(info.identifier.as_str())
          .with_title(info.title.as_str());

        let opts = DesktopResolveOptions::default();
        let entry = resolve_desktop_entry(&mut self.desktop_cache, &ctx, &opts);
        let is_fallback = entry.path.is_empty(); // the fallback for an unmatched appid can be detected if the path is empty

        let data = DesktopEntryData::from_desktop_entry(self.desktop_cache.locales(), entry);
        let app_name = if is_fallback {
          // the fallback is to use the window title (which is redundant), use
          // the appid instead
          app_id.clone()
        }
        else {
          data.name
        };
        let icon_source = data.icon;

        ActiveWindow {
          title,
          app_id,
          app_name,
          icon_source,
        }
      });

    match (active, &self.last_sent) {
      // didn't change
      (None, None) => {}

      // definitely changed
      (None, Some(_)) => {
        self.last_sent = None;

        if self
          .tx
          .unbounded_send(WaylandEvent::Activate(None))
          .is_err()
        {
          self.exit = true;
        }
      }

      // definitely changed
      (Some(active), None) => {
        self.last_sent = Some(active.clone());

        if self
          .tx
          .unbounded_send(WaylandEvent::Activate(Some(active)))
          .is_err()
        {
          self.exit = true;
        }
      }

      // might've changed
      (Some(active), Some(last_sent)) => {
        // if changed
        if active.app_id != last_sent.app_id || active.title != last_sent.title {
          self.last_sent = Some(active.clone());

          if self
            .tx
            .unbounded_send(WaylandEvent::Activate(Some(active)))
            .is_err()
          {
            self.exit = true;
          }
        }
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

  let registry_state = RegistryState::new(&global_list);
  let toplevel_state = ToplevelInfoState::new(&registry_state, &queue_handle);
  let desktop_cache = DesktopEntryCache::new(fde::get_languages_from_env());

  let mut window_data = WindowData {
    exit: false,
    tx,
    registry_state,
    toplevel_state,
    desktop_cache,
    last_sent: None,
  };

  while !window_data.exit {
    event_queue.blocking_dispatch(&mut window_data)?;
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
