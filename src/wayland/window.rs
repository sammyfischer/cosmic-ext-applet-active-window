//! Window state structs and impl

use std::{collections::HashSet, fmt::Display};

use cosmic::{
  cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::State,
    toplevel_info::ToplevelInfo,
    wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1 as ToplevelHandle,
  },
  desktop::{
    DesktopEntryCache,
    DesktopEntryData,
    DesktopLookupContext,
    DesktopResolveOptions,
    fde::IconSource,
    resolve_desktop_entry,
  },
};

use crate::{app::DesktopLaunch, fl};

#[derive(Debug, PartialEq)]
pub(super) struct ActiveKey {
  pub(super) id: String,
  pub(super) app_id: String,
  pub(super) title: String,
  pub(super) state: HashSet<State>,
}

/// Used to check equality between window activations. If any field changes,
/// that triggers refetching of info and redrawing the ui.
#[derive(Debug, PartialEq)]
pub(super) struct WindowKey {
  pub(super) active: ActiveKey,
  pub(super) siblings: Vec<SiblingWindowData>,
}

impl WindowKey {
  pub fn new(info: &ToplevelInfo, siblings: Vec<SiblingWindowData>) -> Self {
    Self {
      active: ActiveKey {
        id: info.identifier.clone(),
        app_id: info.app_id.clone(),
        title: info.title.clone(),
        state: info.state.clone(),
      },
      siblings,
    }
  }
}

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

  // list of other windows of the same app
  // TODO: recalc siblings when any window is minimized
  siblings: Vec<SiblingWindowData>,

  // stored separately bc we add a custom new-window action *only if* one isn't already defined. we
  // need the [DesktopEntry] to determine that
  actions: Vec<DesktopLaunch>,
}

impl WindowData {
  pub fn new(
    info: ToplevelInfo,
    siblings: Vec<SiblingWindowData>,
    cache: &mut DesktopEntryCache,
  ) -> Self {
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
      siblings,
      actions,
    }
  }

  // TOPLEVEL STUFF

  /// The displayed title text of the window. This can be arbitrarily set by the
  /// application.
  pub fn title(&self) -> &str { &self.info.title }

  /// The list of other windows of the same app, not including this window.
  pub fn siblings(&self) -> &Vec<SiblingWindowData> { &self.siblings }
  pub fn set_siblings(&mut self, siblings: Vec<SiblingWindowData>) { self.siblings = siblings; }

  /// Gets the foreign toplevel handle. Use this to make [requests] on the
  /// window.
  ///
  /// [requests]: WaylandRequest
  pub fn foreign_toplevel(&self) -> &ToplevelHandle { &self.info.foreign_toplevel }

  pub fn is_minimized(&self) -> bool { self.info.state.contains(&State::Minimized) }
  pub fn is_maximized(&self) -> bool { self.info.state.contains(&State::Maximized) }
  pub fn is_fullscreen(&self) -> bool { self.info.state.contains(&State::Fullscreen) }
  pub fn is_sticky(&self) -> bool { self.info.state.contains(&State::Sticky) }

  // DESKTOP STUFF

  /// The user-friendly app name, which may not be unique
  pub fn app_name(&self) -> &str { &self.app_name }

  /// The app icon
  pub fn icon(&self) -> &IconSource { &self.desktop.icon }

  /// The list of desktop actions the app may have
  pub fn actions(&self) -> &Vec<DesktopLaunch> { &self.actions }
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

/// Minimum amount of window data needed to activate a sibling window and
/// display the activation buttons.
#[derive(Clone, Debug)]
pub struct SiblingWindowData {
  // store id for equality comparison
  id: String,
  title: String,
  is_minimized: bool,
  foreign_toplevel: ToplevelHandle,
}

impl SiblingWindowData {
  pub fn new(info: &ToplevelInfo) -> Self {
    Self {
      id: info.identifier.clone(),
      title: info.title.clone(),
      is_minimized: info.state.contains(&State::Minimized),
      foreign_toplevel: info.foreign_toplevel.clone(),
    }
  }

  pub fn title(&self) -> &str { &self.title }
  pub fn is_minimized(&self) -> bool { self.is_minimized }
  pub fn foreign_toplevel(&self) -> &ToplevelHandle { &self.foreign_toplevel }
}

impl PartialEq for SiblingWindowData {
  fn eq(&self, other: &Self) -> bool {
    // don't compare the entire foreign toplevel handle
    self.id == other.id && self.title == other.title && self.is_minimized == other.is_minimized
  }
}
