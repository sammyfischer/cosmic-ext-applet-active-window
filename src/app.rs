// SPDX-License-Identifier: MPL-2.0

use std::rc::Rc;

use cosmic::{
  cosmic_config::{self, CosmicConfigEntry},
  cosmic_theme::Spacing,
  desktop::IconSourceExt,
  iced::{
    Alignment,
    Length,
    Limits,
    Subscription,
    core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping},
    platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
    window::Id,
  },
  prelude::*,
  surface,
  widget::{self, layer_container},
};

use crate::{
  config::Config,
  fl,
  wayland::{ActiveWindow, WaylandEvent, wayland_subscription},
};

/// The application model stores app-specific state used to describe its
/// interface and drive its logic.
#[derive(Default)]
pub struct AppModel {
  /// Application state which is managed by the COSMIC runtime
  core: cosmic::Core,

  /// The popup window id
  popup: Option<Id>,

  /// Configuration data that persists between application runs
  config: Config,

  /// Example row toggler
  example_row: bool,

  /// The current active window
  active: Option<ActiveWindow>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
  /// A window was activated
  ActiveWindow(Option<ActiveWindow>),

  /// The full-title tooltip surfaced
  TooltipSurface(surface::Action<Message>),

  /// The popup window was toggled
  TogglePopup,

  /// The popup window was closed
  PopupClosed(Id),

  /// The config was written to
  UpdateConfig(Config),

  /// TODO: template code
  ToggleExampleRow(bool),
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
  /// The async executor that will be used to run your application's commands.
  type Executor = cosmic::executor::Default;

  /// Data that your application receives to its init method.
  type Flags = ();

  /// Messages which the application and its widgets will emit.
  type Message = Message;

  /// Unique identifier in RDNN (reverse domain name notation) format.
  const APP_ID: &'static str = "com.github.sammyfischer.ActiveWindowApplet";

  fn core(&self) -> &cosmic::Core {
    &self.core
  }

  fn core_mut(&mut self) -> &mut cosmic::Core {
    &mut self.core
  }

  /// Initializes the application with any given flags and startup commands.
  fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
    // Construct the app model with the runtime's core.
    let app = AppModel {
      core,
      config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
        .map(|context| match Config::get_entry(&context) {
          Ok(config) => config,
          Err((_errors, config)) => {
            // for why in errors {
            //     tracing::error!(%why, "error loading app config");
            // }

            config
          }
        })
        .unwrap_or_default(),
      ..Default::default()
    };

    (app, Task::none())
  }

  fn on_close_requested(&self, id: Id) -> Option<Message> {
    Some(Message::PopupClosed(id))
  }

  /// Describes the interface based on the current state of the application
  /// model.
  ///
  /// The applet's button in the panel will be drawn using the main view method.
  /// This view should emit messages to toggle the applet's popup window, which
  /// will be drawn using the `view_window` method.
  fn view(&self) -> Element<'_, Self::Message> {
    let ctx = &self.core.applet;

    let Some(active) = self.active.as_ref()
    else {
      // no active window, render nothing
      return ctx.autosize_window(widget::space()).into();
    };

    // a lot of this code is adapted from
    // [cosmic::applet::Context::icon_button_from_handle] (specifically the size
    // calculations)
    let icon_handle = active.icon_source.as_cosmic_icon();
    let symbolic = icon_handle.symbolic;

    let Spacing { space_xxs, .. } = cosmic::theme::active().cosmic().spacing;

    // the sizing looks better when `is_symbolic` is always true
    let (icon_width, icon_height) = ctx.suggested_size(true);
    let (main, cross) = ctx.suggested_padding(true);

    let is_horizontal = ctx.is_horizontal();
    let (hpadding, vpadding) = if is_horizontal {
      (main, cross)
    }
    else {
      (cross, main)
    };

    let button_height = icon_width + 2 * vpadding;

    let raw_text = format!("{} | {}", active.app_name, active.title);
    let text = widget::container(
      ctx
        .text(raw_text.clone())
        .wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
    )
    .max_width(self.config.max_width.clamp(50, 1000));

    let icon = widget::icon(icon_handle)
      .class(if symbolic {
        cosmic::theme::Svg::Custom(Rc::new(|theme| widget::svg::Style {
          color: Some(theme.cosmic().background(theme.transparent).on.into()),
        }))
      }
      else {
        cosmic::theme::Svg::default()
      })
      .width(Length::Fixed(f32::from(icon_width)))
      .height(Length::Fixed(f32::from(icon_height)));

    let mut container = widget::Row::new();

    // if self.config.icon_position == IconPosition::Left {
    container = container.push(icon);
    // }

    if is_horizontal {
      // only show text on horizontal panel layout
      container = container.push(text);
    }

    // if self.config.icon_position == IconPosition::Right {
    //   container = container.push(icon);
    // }

    container = container
      .spacing(space_xxs) // spacing between elements
      .align_y(Alignment::Center)
      .height(icon_width);

    let button = widget::button::custom(
      layer_container(container).center_y(Length::Fixed(f32::from(button_height))),
    )
    .class(cosmic::theme::Button::AppletIcon)
    .padding([0, hpadding])
    .on_press_down(Message::TogglePopup);

    let tooltip = ctx.applet_tooltip(
      button,
      textwrap::fill(&raw_text, 60),
      self.popup.is_some(),
      Message::TooltipSurface,
      None,
    );

    ctx.autosize_window(tooltip).into()
  }

  /// The applet's popup window will be drawn using this view method. If there
  /// are multiple poups, you may match the id parameter to determine which
  /// popup to create a view for.
  fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
    let content_list = widget::list_column().add(widget::settings::item(
      fl!("example-row"),
      widget::toggler(self.example_row).on_toggle(Message::ToggleExampleRow),
    ));

    self.core.applet.popup_container(content_list).into()
  }

  /// Register subscriptions for this application.
  ///
  /// Subscriptions are long-lived async tasks running in the background which
  /// emit messages to the application through a channel. They may be
  /// conditionally activated by selectively appending to the subscription
  /// batch, and will continue to execute for the duration that they remain in
  /// the batch.
  fn subscription(&self) -> Subscription<Self::Message> {
    // listen for wayland toplevel events to track the currently active window
    let active_window = wayland_subscription().map(|event| match event {
      WaylandEvent::Activate(title) => Message::ActiveWindow(title),
    });

    // watch for config changes
    let config = self
      .core()
      .watch_config::<Config>(Self::APP_ID)
      .map(|update| Message::UpdateConfig(update.config));

    Subscription::batch(vec![active_window, config])
  }

  /// Handles messages emitted by the application and its widgets.
  ///
  /// Tasks may be returned for asynchronous execution of code in the background
  /// on the application's async runtime. The application will not exit until
  /// all tasks are finished.
  fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
    match message {
      Message::ActiveWindow(active) => self.active = active,

      Message::TooltipSurface(action) => {
        // cosmic manages the tooltip action
        return cosmic::task::message(cosmic::Action::Surface(action));
      }

      Message::UpdateConfig(config) => {
        self.config = config;
      }

      Message::TogglePopup => {
        return if let Some(p) = self.popup.take() {
          destroy_popup(p)
        }
        else {
          let new_id = Id::unique();
          self.popup.replace(new_id);
          let mut popup_settings = self.core.applet.get_popup_settings(
            self.core.main_window_id().unwrap(),
            new_id,
            None,
            None,
            None,
          );
          popup_settings.positioner.size_limits = Limits::NONE
            .max_width(372.0)
            .min_width(300.0)
            .min_height(200.0)
            .max_height(1080.0);
          get_popup(popup_settings)
        };
      }

      Message::PopupClosed(id) => {
        if self.popup.as_ref() == Some(&id) {
          self.popup = None;
        }
      }

      Message::ToggleExampleRow(toggled) => self.example_row = toggled,
    }
    Task::none()
  }

  fn style(&self) -> Option<cosmic::iced::theme::Style> {
    Some(cosmic::applet::style())
  }
}
