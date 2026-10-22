use cosmic::{
  Element,
  iced::{Alignment, Length},
  theme,
  widget,
};

#[derive(Clone, Debug, PartialEq)]
pub enum SliderInputMessage {
  /// Slider value changed. Fires repeatedly when the slider is dragged.
  SliderChange(u32),

  /// Slider was released
  SliderRelease,

  /// Text input value changed. Fires repeatedly when typing.
  TextChanged(String),

  /// Text input was "committed". Fires when input is unfocused or submitted.
  TextCommit,
}

/// A numeric slider and text input that are linked. They share the same
/// underlying value, and either can be used to edit it. Changes in one are
/// immediately reflected in the other.
#[derive(Clone, Debug, Default)]
pub struct SliderInput {
  /// The current live value as it changes
  value: u32,

  /// The stable value, after the slider is released or the text input is
  /// unfocused/submitted
  committed: u32,

  min: u32,
  max: u32,
  step: u32,

  text_buf: String,
}

impl SliderInput {
  pub fn new(initial: u32, min: u32, max: u32, step: u32) -> Self {
    let value = initial.clamp(min, max);
    Self {
      value,
      committed: value,
      min,
      max,
      step,
      text_buf: value.to_string(),
    }
  }

  fn commit(&mut self) -> Option<u32> {
    // if value changed -> Some, else -> None
    (self.value != self.committed).then(|| {
      self.committed = self.value;
      self.value
    })
  }

  pub fn update(&mut self, msg: SliderInputMessage) -> Option<u32> {
    match msg {
      SliderInputMessage::SliderChange(val) => {
        let val = val.clamp(self.min, self.max);
        self.value = val;
        self.text_buf = val.to_string();
        None
      }

      SliderInputMessage::SliderRelease => self.commit(),

      SliderInputMessage::TextChanged(mut text) => {
        // prevent non-ascii digits from being typed
        text.retain(|c| c.is_ascii_digit());

        // don't allow typing of characters past [self.max]'s number of digits
        let max_digits = self.max.to_string().len();
        text.truncate(max_digits);

        // update the text input with the cleaned value
        self.text_buf.clone_from(&text);

        // if parse fails, return None and don't set any values
        let val = text.parse::<u32>().ok()?;
        if (self.min..=self.max).contains(&val) {
          self.value = val; // update slider live for valid values
        }

        None
      }

      SliderInputMessage::TextCommit => {
        let val = self.text_buf.parse::<u32>().ok()?.clamp(self.min, self.max);
        self.value = val;
        self.text_buf = val.to_string();
        self.commit()
      }
    }
  }

  /// Builds the view for this component.
  ///
  /// # Params
  /// - `callback` - a function that maps [`SliderInputMessage`] to your message
  ///   type
  pub fn view<Msg: Clone + 'static>(
    &self,
    callback: fn(SliderInputMessage) -> Msg,
  ) -> Element<'_, Msg> {
    let space_xxs = theme::active().cosmic().spacing.space_xxs;

    let slider = widget::slider(self.min..=self.max, self.value, move |v| {
      callback(SliderInputMessage::SliderChange(v))
    })
    .step(self.step)
    .shift_step(self.step / 2)
    .on_release(callback(SliderInputMessage::SliderRelease))
    .width(Length::Fill);

    #[allow(clippy::cast_precision_loss)]
    let digits = self.max.to_string().len() as f32;

    let input = widget::text_input("", &self.text_buf)
      .on_input(move |text| callback(SliderInputMessage::TextChanged(text)))
      .on_unfocus(callback(SliderInputMessage::TextCommit))
      .on_submit(move |_| callback(SliderInputMessage::TextCommit))
      .width(Length::Fixed(digits * 14.0)); // about 14px per digit

    widget::Row::new()
      .push(slider)
      .push(input)
      .spacing(space_xxs)
      .align_y(Alignment::Center)
      .into()
  }
}
