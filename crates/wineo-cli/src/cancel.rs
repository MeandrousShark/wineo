use std::process::ExitCode;

use crate::prompt::Canceled;

/// The exit status of a command the user ended early, as a shell reports Ctrl+C.
pub const EXIT: u8 = 130;

pub fn exit_code() -> ExitCode {
    ExitCode::from(EXIT)
}

/// Whether a guided flow was ended from a prompt.
pub fn prompted(error: &anyhow::Error) -> bool {
    error.is::<Canceled>()
}

/// Whether a core operation stopped because its token was canceled.
pub fn core(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<wineo_core::Error>(),
        Some(wineo_core::Error::Canceled(_))
    )
}
