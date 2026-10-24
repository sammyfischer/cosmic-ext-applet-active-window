// SPDX-License-Identifier: MPL-2.0

use std::rc::Rc;

use cosmic::{
  cosmic_config::{self, CosmicConfigEntry},
  cosmic_theme::Spacing,
  desktop::IconSourceExt,
  iced::{
    Alignment,
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
    RectangleTracker,
    column,
    layer_container,
    rectangle_tracker::{RectangleUpdate, rectangle_tracker_subscription},
  },
};

use crate::{
  components::slider_input::{SliderInput, SliderInputMessage},
  config::Config,
  fl,
  template::TitleTemplate,
  wayland::{ActiveWindow, WaylandEvent, wayland_subscription},
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

  /// The current active window
  active: Option<ActiveWindow>,

  /// The tracker for the applet's bounds
  rectangle_tracker: Option<RectangleTracker<u32>>,
  /// The applet's bounds
  rectangle: Rectangle,
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
  /// A window was activated
  ActiveWindow(Option<ActiveWindow>),

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

  /// A no-op message
  ///
  /// TODO: temporary
  None,
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

    let raw_text = self.template.replace(&active.app_name, &active.title);
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
      WaylandEvent::Activate(title) => Message::ActiveWindow(title),
    });

    // watch for config changes
    let config = self
      .core()
      .watch_config::<Config>(Self::APP_ID)
      .map(|update| Message::UpdateConfig(update.config));

    // track the applet's bounds
    let rectangle =
      rectangle_tracker_subscription(RECTANGLE_ID).map(|(_, update)| Message::Rectangle(update));

    Subscription::batch(vec![active_window, config, rectangle])
  }

  /// Handles messages emitted by the application and its widgets.
  ///
  /// Tasks may be returned for asynchronous execution of code in the background
  /// on the application's async runtime. The application will not exit until
  /// all tasks are finished.
  fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
    match message {
      Message::None => {}

      Message::ActiveWindow(active) => self.active = active,

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
    }

    Task::none()
  }

  fn style(&self) -> Option<cosmic::iced::theme::Style> {
    Some(cosmic::applet::style())
  }
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

    let Some(active) = self.active.as_ref()
    else {
      // this will most likely never be visible, but it's here to avoid ever
      // having an empty popup
      return widget::text("No active window").into();
    };

    let Spacing {
      space_s, space_xxs, ..
    } = cosmic::theme::spacing();
    let ctx = &self.core.applet;
    let mut content = widget::menu::menu_column::MenuColumn::new().align_x(Alignment::Center);

    // the full title, since the tooltip is never visible while the popup is
    let title = widget::text(&active.title).wrapping(Wrapping::WordOrGlyph);
    content = content.push(
      widget::container(title)
        .padding([space_xxs, space_s])
        .width(Length::Fill),
    );

    let actions = &active.actions;
    if !actions.is_empty() {
      content = content.push(widget::divider::horizontal::light());
    }

    for action in actions {
      let name =
        widget::text::body(&action.name).ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));

      let button = menu_item_button(name).on_press(Message::None);

      content = content.push(button);
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
}
