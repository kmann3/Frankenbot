use std::sync::LazyLock;

use regex::Regex;

static FACEBOOK_URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:(?:www|m)\.)?(?:facebook\.com|fb\.watch)/[^\s]+")
        .expect("Invalid Facebook URL regex")
});

static FACEBOOK_CDN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\s]*fbcdn\.net/[^\s]+").expect("Invalid Facebook CDN regex")
});

#[derive(Debug, Clone)]
pub enum FacebookMediaType {
    Video,
    Photo,
    Post,
    DirectImage,
}

#[derive(Debug, Clone)]
pub struct FacebookMedia {
    pub media_type: FacebookMediaType,

    pub url: String,
}

pub fn find_media(message: &str) -> Vec<FacebookMedia> {
    let mut media_items = Vec::new();

    for url_match in FACEBOOK_URL_REGEX.find_iter(message) {
        let url = url_match.as_str().to_string();

        media_items.push(FacebookMedia {
            media_type: classify_facebook_url(&url),

            url,
        });
    }

    for url_match in FACEBOOK_CDN_REGEX.find_iter(message) {
        media_items.push(FacebookMedia {
            media_type: FacebookMediaType::DirectImage,

            url: url_match.as_str().to_string(),
        });
    }

    media_items
}

fn classify_facebook_url(url: &str) -> FacebookMediaType {
    let lowercase = url.to_ascii_lowercase();

    if lowercase.contains("/reel/")
        || lowercase.contains("/videos/")
        || lowercase.contains("/watch")
        || lowercase.contains("fb.watch/")
        || lowercase.contains("/share/v/")
    {
        FacebookMediaType::Video
    } else if lowercase.contains("/photo")
        || lowercase.contains("/photos/")
        || lowercase.contains("fbid=")
    {
        FacebookMediaType::Photo
    } else {
        FacebookMediaType::Post
    }
}
