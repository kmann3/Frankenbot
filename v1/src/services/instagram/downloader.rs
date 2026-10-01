use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::process::Command;

use super::{
    errors::{InstagramDownloadError, classify_download_error},
    parser::{InstagramMedia, InstagramMediaType},
};

pub async fn download_media(
    media: &InstagramMedia,
) -> Result<Vec<PathBuf>, InstagramDownloadError> {
    match media.media_type {
        InstagramMediaType::Post => download_post(media).await,

        InstagramMediaType::Reel | InstagramMediaType::Story => download_with_ytdlp(media).await,
    }
}

async fn download_post(media: &InstagramMedia) -> Result<Vec<PathBuf>, InstagramDownloadError> {
    let download_dir = create_download_directory("post", &media.id).await?;

    let canonical_url = format!("https://www.instagram.com/p/{}/", media.id);

    println!("Downloading Instagram post with gallery-dl...");

    let mut command = Command::new("gallery-dl");
    command.arg("--no-input").arg("-D").arg(&download_dir);
    add_cookie_arguments(&mut command, true);
    let output = command.arg(&canonical_url).output().await?;

    if !output.status.success() {
        let details = command_error_details(&output.stdout, &output.stderr);

        return Err(classify_download_error(details, "gallery-dl"));
    }

    let files = find_completed_files(&download_dir).await?;

    if files.is_empty() {
        return Err(InstagramDownloadError::Other(format!(
            "gallery-dl completed successfully, \
                     but no media files were found in {}",
            download_dir.display()
        )));
    }

    Ok(files)
}

async fn download_with_ytdlp(
    media: &InstagramMedia,
) -> Result<Vec<PathBuf>, InstagramDownloadError> {
    let prefix = match media.media_type {
        InstagramMediaType::Reel => "reel",

        InstagramMediaType::Story => "story",

        InstagramMediaType::Post => {
            return Err(InstagramDownloadError::Other(
                "Instagram posts should use gallery-dl.".to_string(),
            ));
        }
    };

    let download_dir = create_download_directory(prefix, &media.id).await?;

    let output_template = download_dir.join("%(id)s.%(ext)s");

    println!(
        "Downloading Instagram {:?} with yt-dlp...",
        media.media_type
    );

    let mut command = Command::new("yt-dlp");
    command
        .arg("--no-playlist")
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("-o")
        .arg(&output_template);
    add_cookie_arguments(&mut command, false);
    let output = command.arg(&media.url).output().await?;

    if !output.status.success() {
        let details = command_error_details(&output.stdout, &output.stderr);

        return Err(classify_download_error(details, "yt-dlp"));
    }

    let files = find_completed_files(&download_dir).await?;

    if files.is_empty() {
        return Err(InstagramDownloadError::Other(format!(
            "yt-dlp completed successfully, \
                     but no completed media file was found in {}",
            download_dir.display()
        )));
    }

    Ok(files)
}

fn add_cookie_arguments(command: &mut Command, load_all_firefox_containers: bool) {
    let Ok(cookie_source) = std::env::var("INSTAGRAM_COOKIES_FROM_BROWSER") else {
        return;
    };
    let cookie_source = cookie_source.trim();
    if !cookie_source.is_empty() {
        command
            .arg("--cookies-from-browser")
            .arg(cookie_source_for_tool(
                cookie_source,
                load_all_firefox_containers,
            ));
    }
}

fn cookie_source_for_tool(cookie_source: &str, load_all_firefox_containers: bool) -> String {
    if load_all_firefox_containers
        && cookie_source.starts_with("firefox:")
        && !cookie_source.contains("::")
    {
        format!("{cookie_source}::all")
    } else {
        cookie_source.to_string()
    }
}

fn command_error_details(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);

    let stderr = String::from_utf8_lossy(stderr);

    format!(
        "stdout:\n{stdout}\n\
         stderr:\n{stderr}"
    )
}

async fn create_download_directory(
    prefix: &str,
    media_id: &str,
) -> Result<PathBuf, InstagramDownloadError> {
    let base = PathBuf::from("downloads");

    tokio::fs::create_dir_all(&base).await?;

    let directory = base.join(format!("{prefix}_{media_id}_{}", unique_download_id()));

    tokio::fs::create_dir_all(&directory).await?;

    Ok(directory)
}

async fn find_completed_files(download_dir: &Path) -> Result<Vec<PathBuf>, InstagramDownloadError> {
    let mut entries = tokio::fs::read_dir(download_dir).await?;

    let mut files = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.ends_with(".part")
            || file_name.contains(".part-")
            || file_name.contains(".temp.")
            || file_name.contains(".fdash-")
            || file_name.contains(".fhttp-")
        {
            continue;
        }

        files.push(path);
    }

    files.sort();

    Ok(files)
}

pub async fn cleanup_download(media_path: &Path) -> Result<(), InstagramDownloadError> {
    let Some(download_dir) = media_path.parent() else {
        return Ok(());
    };

    let downloads_dir = PathBuf::from("downloads");

    if !download_dir.starts_with(&downloads_dir) {
        return Err(InstagramDownloadError::Other(format!(
            "Refusing to delete unexpected directory: {}",
            download_dir.display()
        )));
    }

    if tokio::fs::try_exists(download_dir).await? {
        tokio::fs::remove_dir_all(download_dir).await?;
    }

    Ok(())
}

fn unique_download_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_dl_loads_all_firefox_containers_without_changing_ytdlp_source() {
        let source = "firefox:/Users/test/LibreWolf/Profile";
        assert_eq!(
            cookie_source_for_tool(source, true),
            format!("{source}::all")
        );
        assert_eq!(cookie_source_for_tool(source, false), source);
        assert_eq!(
            cookie_source_for_tool("firefox:/Users/test/LibreWolf/Profile::6", true),
            "firefox:/Users/test/LibreWolf/Profile::6"
        );
    }
}
