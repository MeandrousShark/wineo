use std::fmt;

/// The request that failed, which is all an error says about where it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// Building the HTTP client, before any request.
    Setup,
    Token,
    Owned,
    Product,
    Link,
    Builds,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Setup => "GOG client setup",
            Self::Token => "GOG token request",
            Self::Owned => "GOG owned games request",
            Self::Product => "GOG product request",
            Self::Link => "GOG download link request",
            Self::Builds => "GOG build list request",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorKind {
    #[error("can't reach GOG")]
    Connect,
    #[error("GOG didn't answer in time")]
    Timeout,
    #[error("the request didn't go through")]
    Request,
    /// The token endpoint's `invalid_grant`: the code or refresh token is
    /// used up, expired, or revoked.
    #[error("GOG no longer accepts this login")]
    InvalidGrant,
    #[error("GOG rejected the login")]
    Unauthorized,
    #[error("GOG is limiting requests")]
    RateLimited,
    /// A 5xx.
    #[error("GOG answered with status {0}")]
    Server(u16),
    /// Any other status that isn't a success.
    #[error("GOG answered with status {0}")]
    Status(u16),
    #[error("GOG's answer couldn't be read")]
    Body,
    #[error("GOG's answer is too large")]
    TooLarge,
    #[error("GOG's answer isn't valid JSON")]
    Json,
    #[error("GOG's answer isn't shaped as expected")]
    Malformed,
    /// A download link pointing somewhere the access token must not go.
    #[error("the download link doesn't point at GOG")]
    UntrustedLink,
}

/// How a caller should treat a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The token endpoint refused the code or refresh token, so the saved
    /// login is no good.
    LoggedOut,
    /// Worth trying again later: the network, a timeout, a 429, or a 5xx.
    Temporary,
    /// GOG answered, but not with what was expected.
    Malformed,
    /// Anything else, like a 404 or an access token that was refused.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{endpoint} failed: {kind}")]
pub struct Error {
    endpoint: Endpoint,
    kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(endpoint: Endpoint, kind: ErrorKind) -> Self {
        Self { endpoint, kind }
    }

    pub fn endpoint(&self) -> Endpoint {
        self.endpoint
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn class(&self) -> Class {
        match self.kind {
            ErrorKind::InvalidGrant | ErrorKind::Unauthorized
                if self.endpoint == Endpoint::Token =>
            {
                Class::LoggedOut
            }
            ErrorKind::Connect
            | ErrorKind::Timeout
            | ErrorKind::Request
            | ErrorKind::Body
            | ErrorKind::RateLimited
            | ErrorKind::Server(_) => Class::Temporary,
            ErrorKind::Json | ErrorKind::Malformed | ErrorKind::TooLarge => Class::Malformed,
            ErrorKind::InvalidGrant
            | ErrorKind::Unauthorized
            | ErrorKind::Status(_)
            | ErrorKind::UntrustedLink => Class::Other,
        }
    }
}
