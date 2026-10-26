use cosmic::{
  iced::Color,
  theme,
  widget::button::{Catalog, Style},
};

pub type TintFn = fn(&cosmic::Theme) -> Color;

/// Creates a button theme that is tinted when toggled on, and default when
/// toggled off.
pub fn tinted_icon_class(tint: TintFn) -> theme::Button {
  // modify style to apply the right colors
  let apply = move |theme: &cosmic::Theme, mut style: Style| {
    let c = tint(theme);
    style.icon_color = Some(c);
    style.text_color = Some(c);
    style
  };

  theme::Button::Custom {
    active: Box::new(move |focused, theme| {
      apply(theme, theme.active(focused, false, &theme::Button::Icon))
    }),
    disabled: Box::new(move |theme| apply(theme, theme.disabled(&theme::Button::Icon))),
    hovered: Box::new(move |focused, theme| {
      apply(theme, theme.hovered(focused, false, &theme::Button::Icon))
    }),
    pressed: Box::new(move |focused, theme| {
      apply(theme, theme.pressed(focused, false, &theme::Button::Icon))
    }),
  }
}
