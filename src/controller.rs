//! Launcher navigation only. Never inject input into the desktop or game.
use gilrs::{Axis, Button, EventType, Gilrs};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Layout {
    deadzone: f32,
    left_stick: BTreeMap<String, String>,
    buttons: BTreeMap<String, String>,
}

impl Layout {
    fn load() -> Result<Self, String> {
        // One source of truth. Changes to the existing bridge layout rebuild in.
        let layout: Self = serde_json::from_str(include_str!("../assets/gamepad.json"))
            .map_err(|_| "Controller layout could not be read.".to_owned())?;
        if !layout.deadzone.is_finite() || !(0.0..1.0).contains(&layout.deadzone) {
            return Err("Controller deadzone must be between zero and one.".into());
        }
        for name in ["up", "down", "left", "right"] {
            if !layout.left_stick.contains_key(name) {
                return Err("Controller layout is missing a stick direction.".into());
            }
        }
        Ok(layout)
    }

    fn button(&self, button: Button) -> Option<Navigation> {
        match button {
            Button::DPadUp => Some(Navigation::Previous),
            Button::DPadDown => Some(Navigation::Next),
            Button::DPadLeft => Some(Navigation::Left),
            Button::DPadRight => Some(Navigation::Right),
            _ => {
                let name = match button {
                    Button::South => "A",
                    Button::East => "B",
                    Button::LeftTrigger => "LB",
                    Button::Start => "START",
                    _ => return None,
                };
                match self.buttons.get(name)?.as_str() {
                    "SPACE" | "ENTER" => Some(Navigation::Confirm),
                    "X" => Some(Navigation::Back),
                    "TAB" => Some(Navigation::Next),
                    "ESCAPE" => Some(Navigation::Settings),
                    _ => None,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Navigation {
    Previous,
    Next,
    Left,
    Right,
    Confirm,
    Back,
    Settings,
}

pub struct Controller {
    gilrs: Gilrs,
    layout: Layout,
    held: Option<Navigation>,
    next_repeat: Instant,
    pub connected: bool,
}

impl Controller {
    pub fn new() -> Result<Self, String> {
        let layout = Layout::load()?;
        let gilrs = Gilrs::new().map_err(|_| {
            "Controller input is unavailable; keyboard and mouse still work.".to_owned()
        })?;
        Ok(Self {
            gilrs,
            layout,
            held: None,
            next_repeat: Instant::now(),
            connected: false,
        })
    }

    pub fn poll(&mut self, enabled: bool, focused: bool) -> Vec<Navigation> {
        let mut actions = Vec::new();
        // (R) Bounded drain prevents a noisy controller from starving the UI.
        for _ in 0..64 {
            let Some(event) = self.gilrs.next_event() else {
                break;
            };
            if enabled
                && focused
                && let EventType::ButtonPressed(button, _) = event.event
                && let Some(action) = self.layout.button(button)
            {
                actions.push(action);
            }
        }
        let pad = self.gilrs.gamepads().next().map(|(_, pad)| pad);
        self.connected = pad.is_some();
        let direction = if enabled && focused {
            pad.and_then(|pad| {
                stick(
                    pad.value(Axis::LeftStickX),
                    pad.value(Axis::LeftStickY),
                    self.layout.deadzone,
                )
            })
        } else {
            None
        };
        if let Some(direction) = direction {
            let now = Instant::now();
            if self.held != Some(direction) {
                actions.push(direction);
                self.next_repeat = now + Duration::from_millis(350);
            } else if now >= self.next_repeat {
                actions.push(direction);
                self.next_repeat = now + Duration::from_millis(120);
            }
        }
        self.held = direction;
        actions
    }
}

fn stick(x: f32, y: f32, deadzone: f32) -> Option<Navigation> {
    if !x.is_finite() || !y.is_finite() || x.abs().max(y.abs()) <= deadzone {
        return None;
    }
    if x.abs() > y.abs() {
        Some(if x > 0.0 {
            Navigation::Right
        } else {
            Navigation::Left
        })
    } else {
        Some(if y > 0.0 {
            Navigation::Previous
        } else {
            Navigation::Next
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_layout_and_contextual_navigation() {
        let layout = Layout::load().unwrap();
        assert_eq!(layout.deadzone, 0.25);
        for (name, key) in [("up", "W"), ("down", "S"), ("left", "Q"), ("right", "E")] {
            assert_eq!(layout.left_stick[name], key);
        }
        assert_eq!(layout.button(Button::South), Some(Navigation::Confirm));
        assert_eq!(layout.button(Button::East), Some(Navigation::Back));
        assert_eq!(layout.button(Button::LeftTrigger), Some(Navigation::Next));
        assert_eq!(layout.button(Button::Start), Some(Navigation::Settings));
        assert_eq!(stick(0.2, 0.2, layout.deadzone), None);
        assert_eq!(stick(-1.0, 0.0, layout.deadzone), Some(Navigation::Left));
        assert_eq!(stick(0.0, 1.0, layout.deadzone), Some(Navigation::Previous));
    }
}
