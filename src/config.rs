// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
  pub icon_position: IconPosition,
  pub max_width: u32,
  pub template: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconPosition {
  #[default]
  Left,
  Right,
}

impl IconPosition {
  pub fn index(self) -> u16 {
    match self {
      IconPosition::Left => 0,
      IconPosition::Right => 1,
    }
  }
}

impl Default for Config {
  fn default() -> Self {
    Self {
      icon_position: IconPosition::default(),
      max_width: 400,
      template: Option::default(),
    }
  }
}
