//! Steam's client log, `logs/console_log.txt`. Its `GameAction` lines say
//! what Steam is doing for a launch, which the game's manifest doesn't:
//!
//! ```text
//! [2026-10-30 11:11:47] GameAction [AppID 427520, ActionID 1] : LaunchApp changed task to ProcessingShaderCache with ""
//! ```
//!
//! After `-applaunch`, Steam may process Vulkan shaders for the game before it
//! starts it, for longer than a launch waits.

use super::Install;
use super::log_tail::{Event, Follower, message};

const SHADER_TASK: &str = "ProcessingShaderCache";

/// Follows the install's console log from the moment it is made, for one
/// app's shader pass. Made just before a launch is requested, so lines
/// written earlier, like a pass an earlier launch never finished, don't
/// count.
pub struct ShaderWatch {
    log: Follower,
    app_id: String,
    /// Whether the newest `LaunchApp` line for the app names the pass.
    processing: bool,
}

impl ShaderWatch {
    pub fn start(install: &Install, app_id: &str) -> Self {
        Self {
            log: Follower::at_end(install.root().join("logs/console_log.txt")),
            app_id: app_id.to_owned(),
            processing: false,
        }
    }

    /// Whether Steam is processing shaders for the app right now: the newest
    /// `LaunchApp` line for it, whatever its ActionID, names the pass. A log
    /// that can't be read or isn't understood says no, so a launch waits no
    /// longer than it did before.
    pub fn processing(&mut self) -> bool {
        let Self {
            log,
            app_id,
            processing,
        } = self;
        let read = log.read(|event| match event {
            Event::Restarted => *processing = false,
            Event::Line(line) => {
                if let Some(task) = launch_task(message(line), app_id) {
                    *processing = task == SHADER_TASK;
                }
            }
        });
        read.is_ok() && *processing
    }
}

/// The task a `LaunchApp changed task to` line for `app_id` names.
fn launch_task<'a>(message: &'a str, app_id: &str) -> Option<&'a str> {
    let rest = message.strip_prefix("GameAction [AppID ")?;
    let (id, rest) = rest.split_once(", ActionID ")?;
    if id != app_id {
        return None;
    }
    let (action, rest) = rest.split_once("] : LaunchApp changed task to ")?;
    if action.is_empty() || !action.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(rest.split_whitespace().next().unwrap_or(""))
}

#[cfg(test)]
mod tests;
