//! Whether movement animates: the saved setting, and the desktop's own
//! animation switch for when it follows the system. Reduced motion snaps
//! what moves and keeps what only fades.

use iced::Subscription;
use iced::futures::{SinkExt, Stream, StreamExt};
use wineo_core::library::Motion as Setting;

use crate::Message;

const NAMESPACE: &str = "org.gnome.desktop.interface";
const KEY: &str = "enable-animations";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Motion {
    /// The Appearance draft's while Settings shows it, the saved one
    /// otherwise.
    pub(crate) setting: Setting,
    /// The desktop turned animations off, as the portal last said.
    pub(crate) system_reduced: bool,
}

impl Motion {
    pub(crate) fn reduced(self) -> bool {
        match self.setting {
            Setting::System => self.system_reduced,
            Setting::Full => false,
            Setting::Reduced => true,
        }
    }
}

/// GNOME's switch as the portal gave it. Anything but a readable `false`
/// means full motion.
pub(crate) fn reduced_from<E>(enabled: Result<bool, E>) -> bool {
    matches!(enabled, Ok(false))
}

/// The desktop's switch now and whenever it changes. A subscription, not a
/// boot task, so tests that run `boot` and `update` never reach the bus.
pub(crate) fn subscription() -> Subscription<Message> {
    Subscription::run(portal).map(Message::SystemMotion)
}

fn portal() -> impl Stream<Item = bool> {
    iced::stream::channel(1, async |mut output| {
        let Ok(settings) = ashpd::desktop::settings::Settings::new().await else {
            return;
        };
        // Listening first, so a flip between the read and the match rule
        // isn't missed.
        let changes = settings
            .receive_setting_changed_with_args::<bool>(NAMESPACE, KEY)
            .await;
        let now = settings.read::<bool>(NAMESPACE, KEY).await;
        let _ = output.send(reduced_from(now)).await;
        let Ok(mut changes) = changes else {
            return;
        };
        while let Some(change) = changes.next().await {
            if output.send(reduced_from(change)).await.is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests;
