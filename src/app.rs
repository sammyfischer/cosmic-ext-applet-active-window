// SPDX-License-Identifier: MPL-2.0

use std::rc::Rc;

use cosmic::{
  applet::token::subscription::{TokenRequest, TokenUpdate, activation_token_subscription},
  cctk::{sctk::reexports::calloop, toplevel_info::ToplevelInfo},
  cosmic_config::{self, CosmicConfigEntry},
  cosmic_theme::Spacing,
  desktop::IconSourceExt,
  iced::{
    Alignment,
    Border,
    Color,
    Length,
    Rectangle,
    Subscription,
    core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping},
    platform_specific::shell::wayland::commands::popup::destroy_popup,
    window::Id,
  },
  prelude::*,
  surface::{self, action::LiveSettings},
  widget::{
    self,
    JustifyContent,
    RectangleTracker,
    column,
    icon,
    image::Handle,
    layer_container,
    rectangle_tracker::{RectangleUpdate, rectangle_tracker_subscription},
  },
};

use crate::{
  components::{
    slider_input::{SliderInput, SliderInputMessage},
    toggle_button::{TintFn, tinted_icon_class},
  },
  config::Config,
  fl,
  template::TitleTemplate,
  wayland::{ActiveApp, WaylandEvent, WaylandRequest, Window, wayland_subscription},
};

const RECTANGLE_ID: u32 = 0;

/// The application model stores app-specific state used to describe its
/// interface and drive its logic.
#[derive(Default)]
pub struct AppModel {
  /// Application state which is managed by the COSMIC runtime
  core: cosmic::Core,

  /// The popup window id
  popup: Option<Id>,

  /// The current page of the popup
  page: Page,

  /// Configuration data that persists between application runs
  config: Config,
  config_handle: Option<cosmic_config::Config>,

  max_width_slider: SliderInput,

  /// Template used to write the title text
  template: TitleTemplate,
  /// Buffer for the template text input field
  template_buf: String,

  /// The current active app
  active: Option<ActiveApp>,

  /// The tracker for the applet's bounds
  rectangle_tracker: Option<RectangleTracker<u32>>,
  /// The applet's bounds
  rectangle: Rectangle,

  /// A sender to send token requests
  token_tx: Option<calloop::channel::Sender<TokenRequest>>,
  /// A pending launch action that was requested but not executed
  pending_launch: Option<DesktopLaunch>,

  /// A sender for wayland requests (e.g. close, minimize)
  /// TODO: could combine token request handling into this
  wayland_tx: Option<calloop::channel::Sender<WaylandRequest>>,
}

#[derive(Debug, Clone)]
pub struct DesktopLaunch {
  pub name: String,
  pub app_id: String,
  pub exec: String,
  pub term: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Page {
  #[default]
  Main,
  Settings,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
  /// A no-op message
  None,

  /// The wayland request channel was opened
  InitRequestChannel(calloop::channel::Sender<WaylandRequest>),

  /// Make a wayland request
  Request(WaylandRequest),

  AppActivated(Option<ActiveApp>),
  WindowsUpdated(Vec<ToplevelInfo>),

  /// A screencopy of a window was captured
  Screencopy(ToplevelInfo, image::RgbaImage),

  /// The full-title tooltip surfaced
  TooltipSurface(surface::Action<Message>),

  /// The main popup window was toggled
  ToggleMainPopup,

  /// The settings popup window was toggled
  ToggleSettingsPopup,

  /// The popup window was closed
  PopupClosed(Id),

  /// The config was written to
  UpdateConfig(Config),

  /// The max-width value was changed in the ui
  UpdateMaxWidth(SliderInputMessage),

  /// Characters were typed in the template text input
  UpdateTemplateBuffer(String),
  /// The template text input was unfocused or submitted. No argument, commit
  /// the saved buffer
  CommitTemplate,

  /// The bounds of the applet changed
  Rectangle(RectangleUpdate<u32>),

  /// A token update was sent
  Token(TokenUpdate),

  /// Launch a desktop action
  Launch(DesktopLaunch),
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

  fn core(&self) -> &cosmic::Core { &self.core }

  fn core_mut(&mut self) -> &mut cosmic::Core { &mut self.core }

  /// Initializes the application with any given flags and startup commands.
  fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<cosmic::Action<Self::Message>>) {
    // Construct the app model with the runtime's core.
    let config_handle = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();

    let config = config_handle
      .as_ref()
      .map(|context| match Config::get_entry(context) {
        Ok(config) => config,
        Err((errors, config)) => {
          for why in errors {
            tracing::error!(%why, "error loading app config");
          }

          config
        }
      })
      .unwrap_or_default();

    let template = config
      .template
      .as_deref()
      .map_or_default(|it| TitleTemplate::parse(it).unwrap_or_default());

    let template_buf = config
      .template
      .as_deref()
      .map_or_default(ToString::to_string); // maps None to empty string, which is what we want

    let mut app = AppModel {
      core,
      config,
      config_handle,
      template,
      template_buf,
      ..Default::default()
    };
    app.track_slider_input();

    (app, Task::none())
  }

  fn on_close_requested(&self, id: Id) -> Option<Message> {
    tracing::info!("close requested: {id}");
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

    let nothing = ctx.autosize_window(widget::space());
    let Some(app) = self.active.as_ref()
    else {
      return nothing.into();
    };
    let Some(window) = app.active()
    else {
      return nothing.into();
    };

    // a lot of this code is adapted from
    // [cosmic::applet::Context::icon_button_from_handle] (specifically the size
    // calculations)
    let icon_handle = app.icon().as_cosmic_icon();
    let symbolic = icon_handle.symbolic;

    let Spacing { space_xxs, .. } = cosmic::theme::active().cosmic().spacing;

    // the sizing looks better when `is_symbolic` is always true
    let (icon_width, icon_height) = ctx.suggested_size(true);
    let (main, cross) = ctx.suggested_padding(true);

    let is_horizontal = ctx.is_horizontal();
    let (hpadding, vpadding) = if is_horizontal { (main, cross) } else { (cross, main) };

    let button_height = icon_width + 2 * vpadding;

    #[allow(clippy::cast_possible_truncation)]
    let raw_text = self
      .template
      .replace(app.name(), window.title(), (app.window_count()) as u32);

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
    // left click to open main page
    .on_press_down(Message::ToggleMainPopup);

    let tooltip = ctx.applet_tooltip(
      if let Some(tracker) = self.rectangle_tracker.as_ref() {
        Element::from(tracker.container(RECTANGLE_ID, button))
      }
      else {
        Element::from(button)
      },
      textwrap::fill(&raw_text, 60),
      self.popup.is_some(),
      Message::TooltipSurface,
      None,
    );

    // right click to open settings page
    let mouse_area = widget::mouse_area(tooltip).on_right_release(Message::ToggleSettingsPopup);

    ctx.autosize_window(mouse_area).into()
  }

  /// The applet's popup window will be drawn using this view method. If there
  /// are multiple poups, you may match the id parameter to determine which
  /// popup to create a view for.
  fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
    match self.page {
      Page::Main => self.popup_main_page(),
      Page::Settings => self.popup_settings_page(),
    }
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
      WaylandEvent::Init(tx) => Message::InitRequestChannel(tx),
      WaylandEvent::AppUpdate(app) => Message::AppActivated(app),
      WaylandEvent::WindowUpdate(windows) => Message::WindowsUpdated(windows),
      WaylandEvent::Image(toplevel, image) => Message::Screencopy(toplevel, image),
    });

    // watch for config changes
    let config = self
      .core()
      .watch_config::<Config>(Self::APP_ID)
      .map(|update| Message::UpdateConfig(update.config));

    // track the applet's bounds
    let applet_rectangle =
      rectangle_tracker_subscription(RECTANGLE_ID).map(|(_, update)| Message::Rectangle(update));

    // something to do with activating windows when they spawn
    let token = activation_token_subscription(0).map(Message::Token);

    Subscription::batch(vec![active_window, config, applet_rectangle, token])
  }

  /// Handles messages emitted by the application and its widgets.
  ///
  /// Tasks may be returned for asynchronous execution of code in the background
  /// on the application's async runtime. The application will not exit until
  /// all tasks are finished.
  fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
    match message {
      Message::None => {}

      Message::InitRequestChannel(tx) => {
        self.wayland_tx = Some(tx);
      }

      Message::Request(req) => {
        if let Some(tx) = self.wayland_tx.as_ref() {
          let _ = tx.send(req);
        }
        return self.close_popup();
      }

      Message::AppActivated(active) => {
        self.active = active;
      }

      Message::WindowsUpdated(windows) => {
        if let Some(app) = self.active.as_mut() {
          app.set_windows(&windows);
        }
      }

      Message::Screencopy(toplevel, image) => {
        if let Some(app) = self.active.as_mut() {
          app.update_image(&toplevel, image);
        }
      }

      Message::TooltipSurface(action) => {
        // cosmic manages the tooltip action
        return cosmic::task::message(cosmic::Action::Surface(action));
      }

      Message::UpdateConfig(config) => {
        self.config = config;
        self.track_slider_input();
        self.template_buf = self.config.template.clone().unwrap_or_default();

        self.template = self.config.template.as_deref().map_or_default(|it| {
          TitleTemplate::parse(it)
            .inspect_err(|why| tracing::error!(?why, "failed to parse template '{it}'"))
            .unwrap_or_default()
        });
      }

      Message::UpdateMaxWidth(msg) => {
        if let Some(width) = self.max_width_slider.update(msg)
          && let Some(handle) = &self.config_handle
        {
          if let Err(why) = self.config.set_max_width(handle, width) {
            tracing::error!(?why, "failed to write max width config to disk");
          }
          else {
            self.config.max_width = width;
          }
        }
      }

      Message::UpdateTemplateBuffer(template) => {
        self.template_buf.clone_from(&template);
      }

      Message::CommitTemplate => {
        // get template but set empty string to None
        let template = Some(self.template_buf.clone()).filter(|it| !it.is_empty());

        let parsed = match template.as_deref() {
          Some(template) => match TitleTemplate::parse(template) {
            Ok(it) => it,

            // if parse fails, refuse to set template
            Err(why) => {
              tracing::error!(?why, "failed to parse template '{template}'");
              return Task::none();
            }
          },

          None => TitleTemplate::default(),
        };

        if let Some(handle) = self.config_handle.as_ref() {
          // writes to config file (if value changed) and sets it in-memory
          let _ = self
            .config
            .set_template(handle, template.clone())
            .inspect_err(|why| tracing::error!(?why, "failed to set template '{:?}'", template));
        }
        else {
          // there was an error resolving the config handle, write in-memory
          self.config.template.clone_from(&template);
        }

        self.template = parsed;
      }

      Message::ToggleMainPopup => {
        return if let Some(p) = self.popup.take() {
          destroy_popup(p)
        }
        else {
          self.page = Page::Main;
          if let Some(active) = self.active.as_ref()
            && let Some(tx) = self.wayland_tx.as_ref()
          {
            // request thumbnail image for each window
            for win in active.windows() {
              if let Err(why) = tx.send(WaylandRequest::Screencopy(win.foreign_toplevel().clone()))
              {
                tracing::error!(%why, "failed to send screencopy request");
              }
            }
          }
          Self::build_popup()
        };
      }

      Message::ToggleSettingsPopup => {
        return if let Some(p) = self.popup.take() {
          destroy_popup(p)
        }
        else {
          self.page = Page::Settings;
          Self::build_popup()
        };
      }

      Message::PopupClosed(id) => {
        if self.popup.as_ref() == Some(&id) {
          self.popup = None;
        }
      }

      Message::Rectangle(update) => match update {
        RectangleUpdate::Init(tracker) => self.rectangle_tracker = Some(tracker),
        RectangleUpdate::Rectangle((_, rect)) => self.rectangle = rect,
      },

      Message::Token(update) => match update {
        TokenUpdate::Init(sender) => self.token_tx = Some(sender),
        TokenUpdate::Finished => self.token_tx = None,

        TokenUpdate::ActivationToken { token, .. } => {
          if let Some(launch) = self.pending_launch.take() {
            return launch_action(launch, token);
          }
        }
      },

      Message::Launch(launch) => {
        if let Some(tx) = self.token_tx.as_ref() {
          let _ = tx.send(TokenRequest {
            app_id: launch.app_id.clone(),
            exec: launch.exec.clone(),
          });
          self.pending_launch = Some(launch);
        }
        else {
          // just try anyway without a token (the launch will work, but it
          // probably won't grab focus)
          return launch_action(launch, None);
        }
      }
    }

    Task::none()
  }

  fn style(&self) -> Option<cosmic::iced::theme::Style> { Some(cosmic::applet::style()) }
}

impl AppModel {
  /// Updates the stored value of [`Self::max_width_slider`]
  fn track_slider_input(&mut self) {
    self.max_width_slider = SliderInput::new(self.config.max_width, 50, 1000, 50);
  }

  /// Creates the view of the popup main page
  fn popup_main_page(&self) -> Element<'_, Message> {
    // from https://github.com/pop-os/cosmic-applets/blob/master/cosmic-app-list/src/app.rs#L2100

    /// A menu item in the list that is entirely clickable
    fn menu_item_button<'elem>(
      content: impl Into<Element<'elem, Message>>,
    ) -> widget::Button<'elem, Message> {
      let Spacing {
        space_s, space_xxs, ..
      } = cosmic::theme::spacing();

      widget::button::custom(content)
        .height(20 + 2 * space_xxs)
        .class(cosmic::theme::Button::MenuItem)
        .padding([space_xxs, space_s])
        .width(Length::Fill)
    }

    let placeholder =
      cosmic::applet::padded_control(widget::text("No active window")).center_x(Length::Fill);
    let Some(app) = self.active.as_ref()
    else {
      return placeholder.into();
    };
    let Some(active) = app.active()
    else {
      return placeholder.into();
    };

    let Spacing {
      space_s, space_xxs, ..
    } = cosmic::theme::spacing();
    let ctx = &self.core.applet;
    let mut content = widget::menu::menu_column::MenuColumn::new().align_x(Alignment::Center);

    // the full title, since the tooltip is never visible while the popup is
    let title = widget::text(active.title()).wrapping(Wrapping::WordOrGlyph);
    content = content.push(
      widget::container(title)
        .padding([space_xxs, space_s])
        .width(Length::Fill),
    );

    content = content.push(widget::divider::horizontal::light());

    let controls = widget::flex_row(window_control_buttons(active))
      .width(Length::Fill)
      .align_items(Alignment::Center)
      .justify_items(Alignment::Center)
      .justify_content(Some(JustifyContent::SpaceAround));

    content = content.push(
      widget::container(controls)
        .padding([space_xxs, space_s])
        .width(Length::Fill),
    );

    let launchers = app.actions();
    if !launchers.is_empty() {
      content = content.push(widget::divider::horizontal::light());

      for launch in launchers {
        let name = widget::text::body(launch.name.clone())
          .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        let button = menu_item_button(name).on_press(Message::Launch(launch.clone()));
        content = content.push(button);
      }
    }

    let windows: Vec<_> = app.windows().collect();
    if !windows.is_empty() {
      const THUMBNAIL_HEIGHT: f32 = 120.0;
      const IMAGE_HEIGHT: f32 = 90.0;

      content = content.push(widget::divider::horizontal::light());

      let mut thumbnails = Vec::with_capacity(windows.len());

      for win in windows {
        let img: Element<'_, Message> = if let Some(img) = win.image() {
          widget::Image::new(Handle::from_rgba(img.width(), img.height(), img.to_vec())).into()
        }
        else {
          // fill with empty space
          widget::space()
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        let img = img
          // wrap with container to create rounded rectangle border
          .apply(widget::container)
          .class(cosmic::theme::Container::custom(move |theme| {
            widget::container::Style {
              border: Border {
                color: theme.cosmic().bg_divider().into(),
                width: 1.0,
                radius: 1.0.into(),
              },
              ..Default::default()
            }
          }))
          .padding(1u16)
          // wrap with container to constrain the size
          .apply(widget::container)
          .width(Length::Fill)
          .height(Length::Fixed(IMAGE_HEIGHT))
          .center(Length::Fill);

        let mut title = widget::text::body(win.title())
          .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
          .width(Length::Fill)
          .center();

        if win.is_minimized() {
          let color: Color = cosmic::theme::active().cosmic().on_bg_color().into();
          title = title.class(cosmic::theme::Text::Color(Color {
            a: color.a * 0.6,
            ..color
          }));
        }

        let mut col = widget::Column::new().spacing(space_xxs);
        col = col.push(img).push(title);

        let button = widget::button::custom(col)
          .class(thumbnail_button_style(win.is_active()))
          .width(Length::Fill)
          .height(Length::Fixed(THUMBNAIL_HEIGHT))
          .padding(space_xxs)
          .selected(win.is_active())
          .on_press(Message::Request(WaylandRequest::Activate(
            win.foreign_toplevel().clone(),
          )));

        thumbnails.push(button.into());
      }

      content = content.push(
        widget::container(thumbnail_grid(thumbnails, space_xxs))
          .padding(space_xxs)
          .width(Length::Fill),
      );
    }

    ctx.popup_container(content).into()
  }

  /// Creates the view of the popup settings page
  fn popup_settings_page(&self) -> Element<'_, Message> {
    /// A field in the settings form. This is just a pair of the field's label
    /// and input element.
    struct Field<'elem> {
      label: String,
      input: Element<'elem, Message>,
    }

    fn row<'elem>(content: impl Into<Element<'elem, Message>>) -> Element<'elem, Message> {
      let Spacing {
        space_xxs, space_s, ..
      } = cosmic::theme::spacing();

      widget::container(content.into())
        .padding([space_xxs, space_s])
        .width(Length::Fill)
        .into()
    }

    let ctx = &self.core.applet;
    let Spacing { space_xxs, .. } = cosmic::theme::spacing();

    let set_max_width = Field {
      label: fl!("config-max-width"),

      input: self.max_width_slider.view(Message::UpdateMaxWidth),
    };

    let set_template = Field {
      label: fl!("config-template"),

      input: widget::text_input("template", &self.template_buf)
        .on_input(Message::UpdateTemplateBuffer)
        .on_unfocus(Message::CommitTemplate)
        .on_submit(|_| Message::CommitTemplate)
        .into(),
    };

    let content = column![
      // max width slider
      row(ctx.text(set_max_width.label)),
      row(set_max_width.input),
      // template text input
      row(ctx.text(set_template.label)),
      row(set_template.input),
    ]
    .align_x(Alignment::Start)
    .padding([/* vertical: */ space_xxs, 0]);

    ctx.popup_container(content).into()
  }

  /// Build the popup window
  #[allow(clippy::cast_possible_truncation)]
  fn build_popup() -> cosmic::Task<cosmic::Action<Message>> {
    // needs to be sent as a task for frosted glass blur to work
    cosmic::surface::surface_task(cosmic::surface::action::app_popup(
      |_| LiveSettings::default(),
      |app: &mut Self| {
        let new_id = Id::unique();
        app.popup.replace(new_id);

        let mut settings = app.core.applet.get_popup_settings(
          app.core.main_window_id().unwrap(),
          new_id,
          None,
          None,
          None,
        );

        // from cosmic-applet-time
        // the popup is designed for a single icon, and doesn't center
        // properly for an applet that has a higher width
        let Rectangle {
          x,
          y,
          width,
          height,
        } = app.rectangle;

        settings.positioner.anchor_rect = Rectangle::<i32> {
          x: x.max(1.0) as i32,
          y: y.max(1.0) as i32,
          width: width.max(1.0) as i32,
          height: height.max(1.0) as i32,
        };
        settings.positioner.size = None; // autosize

        settings
      },
      None,
    ))
  }

  fn close_popup(&mut self) -> Task<cosmic::Action<Message>> {
    match self.popup.take() {
      Some(id) => destroy_popup(id),
      None => Task::none(),
    }
  }
}

/// Launch a desktop action
fn launch_action(launch: DesktopLaunch, token: Option<String>) -> Task<cosmic::Action<Message>> {
  let mut env = Vec::new();

  if let Some(token) = token {
    env.push(("XDG_ACTIVATION_TOKEN".to_string(), token.clone()));
    env.push(("DESKTOP_STARTUP_ID".to_string(), token));
  }

  cosmic::task::future(async move {
    cosmic::desktop::spawn_desktop_exec(&launch.exec, env, Some(&launch.app_id), launch.term).await;
    Message::None
  })
  .map(cosmic::Action::App)
}

/// Builds a list of window control buttons
fn window_control_buttons<'elem>(active: &Window) -> Vec<Element<'elem, Message>> {
  /// Common config for each button
  ///
  /// TODO: check capabilities of window, make `on_press` an option to disable
  /// button if incapable. Store capabilities in [`WindowData`] to access here
  /// easily.
  fn icon_button<'elem>(
    icon: &'static str,
    name: String,
    tint: Option<TintFn>,
    on_press: WaylandRequest,
  ) -> Element<'elem, Message> {
    const SIZE: u16 = 24;

    let mut button = widget::button::icon(icon::from_name(icon).symbolic(true).size(SIZE))
      .icon_size(SIZE)
      .tooltip(name)
      .on_press(Message::Request(on_press));

    if let Some(tint) = tint {
      button = button.class(tinted_icon_class(tint));
    }

    button.into()
  }

  // callbacks to get button colors from theme
  let accent_color: TintFn = |theme| theme.cosmic().accent_color().into();
  let destructive_color: TintFn = |theme| theme.cosmic().destructive_color().into();

  let min = icon_button(
    "window-minimize-symbolic",
    fl!("minimize"),
    active.is_minimized().then_some(accent_color),
    WaylandRequest::Minimize(active.foreign_toplevel().clone()),
  );

  let max = if active.is_maximized() {
    icon_button(
      "window-restore-symbolic",
      fl!("restore"),
      Some(accent_color),
      WaylandRequest::Maximize(active.foreign_toplevel().clone()),
    )
  }
  else {
    icon_button(
      "window-maximize-symbolic",
      fl!("maximize"),
      None,
      WaylandRequest::Maximize(active.foreign_toplevel().clone()),
    )
  };

  let full = icon_button(
    "view-fullscreen-symbolic",
    fl!("fullscreen"),
    active.is_fullscreen().then_some(accent_color),
    WaylandRequest::Fullscreen(active.foreign_toplevel().clone()),
  );

  let pin = if active.is_pinned() {
    icon_button(
      "pin-symbolic",
      fl!("unpin"),
      Some(accent_color),
      WaylandRequest::Sticky(active.foreign_toplevel().clone()),
    )
  }
  else {
    icon_button(
      "pin-symbolic",
      fl!("pin"),
      None,
      WaylandRequest::Sticky(active.foreign_toplevel().clone()),
    )
  };

  let close = icon_button(
    "window-close-symbolic",
    fl!("close"),
    // impossible to be closed and active at the same time
    Some(destructive_color),
    WaylandRequest::Close(active.foreign_toplevel().clone()),
  );

  vec![min, max, full, pin, close]
}

/// Style for one of the clickable window thumbnails in the window grid. Taken
/// from [cosmic-app-list] in order to match it exactly.
///
/// [cosmic-app-list]: https://github.com/pop-os/cosmic-applets/blob/master/cosmic-app-list/src/app.rs#L531
fn thumbnail_button_style(selected: bool) -> cosmic::theme::Button {
  let radius = cosmic::theme::active()
    .cosmic()
    .radius_m()
    .map(|x| if x < 8.0 { x } else { x - 4.0 });

  cosmic::theme::Button::Custom {
    active: Box::new(move |focused, theme| {
      let a = widget::button::Catalog::active(
        theme,
        focused,
        selected,
        &cosmic::theme::Button::AppletMenu,
      );
      widget::button::Style {
        background: if selected {
          Some(cosmic::iced::Background::Color(
            theme.cosmic().icon_button.selected_state_color().into(),
          ))
        }
        else {
          a.background
        },
        border_radius: radius.into(),
        outline_width: 0.0,
        ..a
      }
    }),
    hovered: Box::new(move |focused, theme| {
      let focused = selected || focused;
      let text = widget::button::Catalog::hovered(
        theme,
        focused,
        focused,
        &cosmic::theme::Button::AppletMenu,
      );
      widget::button::Style {
        border_radius: radius.into(),
        outline_width: 0.0,
        ..text
      }
    }),
    disabled: Box::new(move |theme| {
      let text = widget::button::Catalog::disabled(theme, &cosmic::theme::Button::AppletMenu);
      widget::button::Style {
        border_radius: radius.into(),
        outline_width: 0.0,
        ..text
      }
    }),
    pressed: Box::new(move |focused, theme| {
      let focused = selected || focused;
      let text = widget::button::Catalog::pressed(
        theme,
        focused,
        focused,
        &cosmic::theme::Button::AppletMenu,
      );
      widget::button::Style {
        border_radius: radius.into(),
        outline_width: 0.0,
        ..text
      }
    }),
  }
}

/// Builds a thumbnail grid layout, filling it with the elements in `cells`.
fn thumbnail_grid(cells: Vec<Element<'_, Message>>, gap: u16) -> Element<'_, Message> {
  let mut rows = widget::Column::new().spacing(gap);
  let mut iter = cells.into_iter();

  // grap two items at a time and build a row each
  while let Some(first) = iter.next() {
    // if no second element, fill with horizontal space
    let second = iter
      .next()
      .unwrap_or_else(|| widget::space::horizontal().width(Length::Fill).into());

    rows = rows.push(widget::Row::new().push(first).push(second).spacing(gap));
  }

  rows.into()
}
