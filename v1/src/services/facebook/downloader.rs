use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, header::CONTENT_TYPE};

use scraper::{Html, Selector};

use tokio::process::Command;

use super::{FacebookMedia, FacebookMediaType};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub async fn download_media(media: &FacebookMedia) -> Result<PathBuf, BoxError> {
    match media.media_type {
        FacebookMediaType::Video => download_video(&media.url).await,

        FacebookMediaType::Photo => download_photo_page(&media.url).await,

        FacebookMediaType::DirectImage => download_direct_image(&media.url).await,

        FacebookMediaType::Post => download_post(&media.url).await,
    }
}

async fn download_post(url: &str) -> Result<PathBuf, BoxError> {
    match download_video(url).await {
        Ok(path) => {
            return Ok(path);
        }

        Err(video_error) => {
            println!("Facebook post was not downloadable as video:");

            println!("  {video_error}");
        }
    }

    match download_photo_page(url).await {
        Ok(path) => Ok(path),

        Err(photo_error) => Err(format!(
            "Facebook post could not be downloaded \
                     as video or image. \
                     Image error: {photo_error}"
        )
        .into()),
    }
}

async fn download_video(url: &str) -> Result<PathBuf, BoxError> {
    let output_dir = PathBuf::from("downloads");

    tokio::fs::create_dir_all(&output_dir).await?;

    let unique_id = unique_id();

    let output_template = output_dir.join(format!("facebook_video_{unique_id}_%(id)s.%(ext)s"));

    let output = Command::new("yt-dlp")
        .arg("--no-playlist")
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("-o")
        .arg(&output_template)
        .arg(url)
        .output()
        .await?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);

        let stderr = String::from_utf8_lossy(&output.stderr);

        return Err(format!(
            "yt-dlp failed.\n\
                 stdout:\n{stdout}\n\
                 stderr:\n{stderr}"
        )
        .into());
    }

    find_downloaded_file(&output_dir, &format!("facebook_video_{unique_id}_")).await
}

async fn download_photo_page(url: &str) -> Result<PathBuf, BoxError> {
    let client = build_http_client()?;

    let html = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    //
    // Html is not Send, so make sure it is dropped
    // before the next await.
    //
    let image_url = {
        let document = Html::parse_document(&html);

        let selector = Selector::parse(r#"meta[property="og:image"]"#)
            .map_err(|error| format!("Could not create og:image selector: {error:?}"))?;

        document
            .select(&selector)
            .find_map(|element| element.value().attr("content"))
            .ok_or("Facebook page did not contain an og:image URL")?
            .to_string()
    };

    download_direct_image(&image_url).await
}

async fn download_direct_image(url: &str) -> Result<PathBuf, BoxError> {
    let output_dir = PathBuf::from("downloads");

    tokio::fs::create_dir_all(&output_dir).await?;

    let client = build_http_client()?;

    let response = client.get(url).send().await?.error_for_status()?;

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let extension = extension_for_content_type(&content_type);

    let output_path = output_dir.join(format!("facebook_photo_{}.{}", unique_id(), extension));

    let bytes = response.bytes().await?;

    tokio::fs::write(&output_path, &bytes).await?;

    Ok(output_path)
}

fn build_http_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/140 Safari/537.36",
        )
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
}

fn extension_for_content_type(content_type: &str) -> &'static str {
    if content_type.contains("image/png") {
        "png"
    } else if content_type.contains("image/webp") {
        "webp"
    } else if content_type.contains("image/gif") {
        "gif"
    } else {
        "jpg"
    }
}

async fn find_downloaded_file(directory: &Path, prefix: &str) -> Result<PathBuf, BoxError> {
    let mut entries = tokio::fs::read_dir(directory).await?;

    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name();

        let file_name = file_name.to_string_lossy();

        if file_name.starts_with(prefix) {
            return Ok(entry.path());
        }
    }

    Err(format!(
        "Download completed but no output file \
             beginning with '{prefix}' was found"
    )
    .into())
}

fn unique_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
