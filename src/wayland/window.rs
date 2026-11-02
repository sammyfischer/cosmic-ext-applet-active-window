//! Window state structs and impl

use std::fmt::Display;

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
use image::RgbaImage;
use indexmap::IndexMap;

use crate::{app::DesktopLaunch, fl};

/// The currently active application
#[derive(Clone, Debug)]
pub struct ActiveApp {
  /// The appid as it appears in [`ToplevelInfo::app_id`]. This may not match
  /// the appid found in the desktop entry, so it must be obtained from the
  /// active window. Used to check if a window belongs to this app.
  id: String,

  /// The name of the app. If the desktop entry doesn't contain a name, it
  /// defaults to the window title. Instead, we resolve this during construction
  /// to default to the appid.
  name: String,

  /// Maps window ids to [`Window`]s, which are a lightweight wrapper for a
  /// [`ToplevelInfo`] and thumbnail image
  windows: WindowMap,

  /// The resolved `[Desktop Entry]` section of the desktop file. This provides
  /// direct access to fields such as name and icon without returning options or
  /// results.
  desktop: DesktopEntryData,

  /// The list of desktop actions. If the app doesn't define a "new-window"
  /// action, we create one from scratch and prepend it to the list, using the
  /// app's default `Exec` field.
  actions: Vec<DesktopLaunch>,
}

impl ActiveApp {
  /// Creates and resolves info about the currently active app.
  ///
  /// # Params
  /// - `toplevels` - a list of *all* toplevels
  /// - `desktop_cache` - cached info used to lookup desktop entries, always use
  ///   the same one
  pub(super) fn new(
    toplevels: &[&ToplevelInfo],
    desktop_cache: &mut DesktopEntryCache,
  ) -> Option<ActiveApp> {
    let active = toplevels
      .iter()
      .find(|info| info.state.contains(&State::Activated))?;
    let appid = active.app_id.as_str();

    let windows: IndexMap<_, _> = toplevels
      .iter()
      .filter_map(|info| {
        if info.app_id == appid {
          Some((info.identifier.clone(), Window::new((*info).clone())))
        }
        else {
          None
        }
      })
      .collect();

    let ctx = DesktopLookupContext::new(appid)
      .with_identifier(active.identifier.as_str())
      .with_title(active.title.as_str()); // title is used for wine/proton windows

    let opts = DesktopResolveOptions::default();
    let entry = resolve_desktop_entry(desktop_cache, &ctx, &opts);
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

      let Some(name) = entry.action_entry_localized(action, "Name", desktop_cache.locales())
      else {
        continue;
      };

      actions.push(DesktopLaunch {
        name: name.to_string(),
        app_id: appid.to_string(),
        exec: exec.to_string(),
        term: entry.terminal(),
      });
    }

    // make a custom new-window action that runs the app's main exec command.
    // this isn't guaranteed to launch a new window, some apps will refuse to do
    // this if a window already exists
    if !has_new_window && let Some(exec) = entry.exec() {
      actions.insert(0, DesktopLaunch {
        name: fl!("new-window"),
        app_id: appid.to_string(),
        exec: exec.to_string(),
        term: entry.terminal(),
      });
    }

    let data = DesktopEntryData::from_desktop_entry(desktop_cache.locales(), entry);
    let name = if is_fallback {
      // the fallback is to use the window title (which is redundant), use
      // the app id instead
      appid.to_string()
    }
    else {
      data.name.clone()
    };

    Some(Self {
      id: appid.to_string(),
      name,
      windows,
      desktop: data,
      actions,
    })
  }

  pub fn id(&self) -> &str { &self.id }
  pub fn name(&self) -> &str { &self.name }
  pub fn icon(&self) -> &IconSource { &self.desktop.icon }
  pub fn actions(&self) -> &[DesktopLaunch] { &self.actions }
  pub fn active(&self) -> Option<&Window> { self.windows.values().find(|win| win.is_active()) }
  pub fn windows(&self) -> indexmap::map::Values<'_, String, Window> { self.windows.values() }
  pub fn window_count(&self) -> usize { self.windows.len() }

  pub fn set_windows(&mut self, windows: &[ToplevelInfo]) {
    self.windows = windows
      .iter()
      .filter_map(|info| {
        if info.app_id == self.id() {
          Some((info.identifier.clone(), Window::new(info.clone())))
        }
        else {
          None
        }
      })
      .collect();
  }

  pub fn update_image(&mut self, toplevel: &ToplevelInfo, image: RgbaImage) {
    if let Some(window) = self.windows.get_mut(&toplevel.identifier) {
      window.image = Some(image);
    }
  }
}

impl Display for ActiveApp {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "ActiveApp({})", self.name())
  }
}

/// An index map of window ids to [`Window`]s. An index map is used to keep a
/// stable order for a better user experience, particularly in the window
/// switcher ui.
pub type WindowMap = IndexMap<String, Window>;

#[derive(Clone, Debug)]
pub struct Window {
  info: ToplevelInfo,
  image: Option<image::RgbaImage>,
}

impl Window {
  pub(super) fn new(info: ToplevelInfo) -> Self { Self { info, image: None } }

  pub fn id(&self) -> &str { &self.info.identifier }
  pub fn title(&self) -> &str { &self.info.title }
  pub fn foreign_toplevel(&self) -> &ToplevelHandle { &self.info.foreign_toplevel }

  pub fn is_active(&self) -> bool { self.info.state.contains(&State::Activated) }
  pub fn is_minimized(&self) -> bool { self.info.state.contains(&State::Minimized) }
  pub fn is_maximized(&self) -> bool { self.info.state.contains(&State::Maximized) }
  pub fn is_fullscreen(&self) -> bool { self.info.state.contains(&State::Fullscreen) }
  pub fn is_pinned(&self) -> bool { self.info.state.contains(&State::Sticky) }

  pub fn image(&self) -> Option<&RgbaImage> { self.image.as_ref() }
}

impl Display for Window {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "Window{{app_id: {}, title: {}}}",
      self.info.app_id,
      self.title()
    )
  }
}

impl PartialEq for Window {
  fn eq(&self, other: &Self) -> bool {
    // window id, title, and set of states
    self.id() == other.id() && self.title() == other.title() && self.info.state == other.info.state
  }
}
