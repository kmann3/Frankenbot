use std::sync::LazyLock;

use regex::Regex;

static INSTAGRAM_REEL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:www\.)?instagram\.com/reel/([A-Za-z0-9_-]+)(?:/[^\s]*)?")
        .expect("Invalid Instagram Reel regex")
});

static INSTAGRAM_POST_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:www\.)?instagram\.com/p/([A-Za-z0-9_-]+)(?:/[^\s]*)?")
        .expect("Invalid Instagram Post regex")
});

static INSTAGRAM_STORY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:www\.)?instagram\.com/stories/([A-Za-z0-9._]+)/([0-9]+)(?:/[^\s]*)?")
        .expect("Invalid Instagram Story regex")
});

#[derive(Debug, Clone)]
pub enum InstagramMediaType {
    Reel,
    Post,
    Story,
}

#[derive(Debug, Clone)]
pub struct InstagramMedia {
    pub media_type: InstagramMediaType,

    pub id: String,

    pub url: String,
    //pub username: Option<String>,
}

pub fn find_media(message: &str) -> Vec<InstagramMedia> {
    let mut results = Vec::new();

    for captures in INSTAGRAM_REEL_REGEX.captures_iter(message) {
        let Some(full_match) = captures.get(0) else {
            continue;
        };

        let Some(id) = captures.get(1) else {
            continue;
        };

        results.push(InstagramMedia {
            media_type: InstagramMediaType::Reel,

            id: id.as_str().to_string(),

            url: full_match.as_str().to_string(),
            //username: None,
        });
    }

    for captures in INSTAGRAM_POST_REGEX.captures_iter(message) {
        let Some(full_match) = captures.get(0) else {
            continue;
        };

        let Some(id) = captures.get(1) else {
            continue;
        };

        results.push(InstagramMedia {
            media_type: InstagramMediaType::Post,

            id: id.as_str().to_string(),

            url: full_match.as_str().to_string(),
            //username: None,
        });
    }

    for captures in INSTAGRAM_STORY_REGEX.captures_iter(message) {
        let Some(full_match) = captures.get(0) else {
            continue;
        };

        // let Some(username) = captures.get(1) else {
        //     continue;
        // };

        let Some(id) = captures.get(2) else {
            continue;
        };

        results.push(InstagramMedia {
            media_type: InstagramMediaType::Story,

            id: id.as_str().to_string(),

            url: full_match.as_str().to_string(),
            //username: Some(username.as_str().to_string()),
        });
    }

    results
}
