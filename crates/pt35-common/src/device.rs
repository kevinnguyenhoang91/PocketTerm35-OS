//! Which handheld the shell runs on.
//!
//! `pt35-session` asks `pt35-device` once and exports the answer as
//! `PT35_DEVICE`. Everything sway starts inherits it. Unset is the
//! PocketTerm35, which the shell was built for.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Device {
    /// Waveshare PocketTerm35: 640x480 HDMI panel, RP2040 keyboard, touch.
    #[default]
    PocketTerm35,
    /// ClockworkPi uConsole: 1280x720 DSI panel, STM32 keyboard with a
    /// trackball and a joystick, no touch.
    UConsole,
}

impl Device {
    pub fn current() -> Self {
        Self::from_name(&std::env::var("PT35_DEVICE").unwrap_or_default())
    }

    pub fn from_name(name: &str) -> Self {
        match name.trim() {
            "uconsole" => Device::UConsole,
            _ => Device::PocketTerm35,
        }
    }

    /// The folder of shipped config merged over the defaults, beside them:
    /// `pt35/uconsole/theme.toml` over `pt35/theme.toml`. The defaults are
    /// the PocketTerm35's own.
    pub fn config_layer(self) -> Option<&'static str> {
        match self {
            Device::PocketTerm35 => None,
            Device::UConsole => Some("uconsole"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anything_but_a_known_name_is_the_pocketterm35() {
        assert_eq!(Device::from_name("uconsole\n"), Device::UConsole);
        assert_eq!(Device::from_name(""), Device::PocketTerm35);
        assert_eq!(Device::from_name("devterm"), Device::PocketTerm35);
    }
}
