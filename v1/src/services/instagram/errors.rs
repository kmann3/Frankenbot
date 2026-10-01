use std::{error::Error, fmt};

#[derive(Debug)]
pub enum InstagramDownloadError {
    PostProbablyGone(String),

    LoginRequired(String),

    YtDlp(String),

    GalleryDl(String),

    Other(String),
}

impl fmt::Display for InstagramDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PostProbablyGone(error)
            | Self::LoginRequired(error)
            | Self::YtDlp(error)
            | Self::GalleryDl(error)
            | Self::Other(error) => {
                write!(f, "{error}")
            }
        }
    }
}

impl Error for InstagramDownloadError {}

impl From<std::io::Error> for InstagramDownloadError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.to_string())
    }
}

pub(super) fn classify_download_error(details: String, downloader: &str) -> InstagramDownloadError {
    //
    // Highest priority.
    //
    if looks_like_post_gone_error(&details) {
        return InstagramDownloadError::PostProbablyGone(details);
    }

    //
    // Only check authentication after ruling out
    // an empty-media response.
    //
    if looks_like_login_error(&details) {
        return InstagramDownloadError::LoginRequired(details);
    }

    match downloader {
        "yt-dlp" => InstagramDownloadError::YtDlp(details),

        "gallery-dl" => InstagramDownloadError::GalleryDl(details),

        _ => InstagramDownloadError::Other(details),
    }
}

fn looks_like_post_gone_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();

    text.contains("instagram sent an empty media response") || text.contains("empty media response")
}

fn looks_like_login_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();

    text.contains("redirect to login page")
        || text.contains("/accounts/login")
        || text.contains("login required")
        || text.contains("authentication required")
        || text.contains("not logged in")
        || text.contains("please log in")
        || text.contains("please login")
        || text.contains("log in to instagram")
        || text.contains("login to instagram")
        || text.contains("checkpoint_required")
        || text.contains("challenge_required")
        || text.contains("--cookies-from-browser")
        || text.contains("you need to log in")
}
