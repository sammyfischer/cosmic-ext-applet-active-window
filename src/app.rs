// SPDX-License-Identifier: MPL-2.0

use cosmic::{
  cosmic_config::{self, CosmicConfigEntry},
  cosmic_theme::Spacing,
  iced::{
    Alignment,
    Length,
    Limits,
    Subscription,
    core::text::Wrapping,
    futures,
    platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
    window::Id,
  },
  prelude::*,
  widget::{self, layer_container},
};
use futures::SinkExt;

use crate::{
  config::Config,
  fl,
  wayland::{ActiveWindow, WaylandEvent, wayland_subscription},
};

/// The application model stores app-specific state used to describe its
/// interface and drive its logic.
#[derive(Default)]
pub struct AppModel {
  /// Application state which is managed by the COSMIC runtime.
  core: cosmic::Core,
  /// The popup id.
  popup: Option<Id>,
  /// Configuration data that persists between application runs.
  config: Config,
  /// Example row toggler.
  example_row: bool,

  title: Option<String>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
  TogglePopup,
  PopupClosed(Id),
  SubscriptionChannel,
  UpdateConfig(Config),
  ToggleExampleRow(bool),
  ActiveWindow(Option<ActiveWindow>),
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

    let Some(title) = self.title.as_deref()
    else {
      // no active window, render zero-width
      return ctx.autosize_window(widget::text("")).into();
    };

    let Spacing { space_xxs, .. } = cosmic::theme::active().cosmic().spacing;
    let (icon_size, _) = ctx.suggested_size(true);
    let (main, cross) = ctx.suggested_padding(true);

    let is_horizontal = ctx.is_horizontal();
    let (hpadding, vpadding) = if is_horizontal {
      (main, cross)
    }
    else {
      (cross, main)
    };

    let button_height = icon_size + 2 * vpadding;

    let text = widget::container(ctx.text(title).wrapping(Wrapping::None))
      .max_width(self.config.max_width)
      .clip(true);
    let mut container = widget::Row::new();

    // if self.config.icon_position == IconPosition::Left {
    //   container = container.push(icon);
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
      .align_y(Alignment::Center);

    let button = widget::button::custom(
      layer_container(container).center_y(Length::Fixed(f32::from(button_height))),
    )
    .class(cosmic::theme::Button::AppletIcon)
    .padding([0, hpadding])
    .on_press_down(Message::TogglePopup);

    ctx.autosize_window(button).into()
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
    let active_window = wayland_subscription().map(|event| match event {
      WaylandEvent::Activate(title) => Message::ActiveWindow(title),
    });

    Subscription::batch(vec![
      active_window,
      // Create a subscription which emits updates through a channel.
      Subscription::run(|| {
        cosmic::iced::stream::channel(
          4,
          move |mut channel: futures::channel::mpsc::Sender<_>| async move {
            _ = channel.send(Message::SubscriptionChannel).await;

            futures::future::pending::<()>().await;
          },
        )
      }),
      // Watch for application configuration changes.
      self
        .core()
        .watch_config::<Config>(Self::APP_ID)
        .map(|update| {
          // for why in update.errors {
          //     tracing::error!(?why, "app config error");
          // }

          Message::UpdateConfig(update.config)
        }),
    ])
  }

  /// Handles messages emitted by the application and its widgets.
  ///
  /// Tasks may be returned for asynchronous execution of code in the background
  /// on the application's async runtime. The application will not exit until
  /// all tasks are finished.
  fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
    match message {
      Message::ActiveWindow(active) => self.title = active.map(|it| it.title.clone()),
      Message::SubscriptionChannel => {
        // For example purposes only.
      }
      Message::UpdateConfig(config) => {
        self.config = config;
      }
      Message::ToggleExampleRow(toggled) => self.example_row = toggled,
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
    }
    Task::none()
  }

  fn style(&self) -> Option<cosmic::iced::theme::Style> {
    Some(cosmic::applet::style())
  }
}
