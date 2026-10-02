use serenity::{model::channel::Message, prelude::*};

use crate::{
    discord::{replies::reply_text, uploads::reply_with_downloaded_files},
    services::instagram::{self, InstagramDownloadError},
};

pub async fn handle_message(ctx: &Context, msg: &Message) {
    let media_items = instagram::find_media(&msg.content);

    if !media_items.is_empty() {
        super::log_media_message(ctx, msg).await;
    }

    for media in media_items {
        if let Err(error) = reply_text(
            ctx,
            msg,
            format!("Instagram {:?} detected. Downloading...", media.media_type),
        )
        .await
        {
            eprintln!("Unable to send Instagram status reply: {error}");
        }

        match instagram::download_media(&media).await {
            Ok(media_paths) => {
                if let Err(error) = reply_with_downloaded_files(ctx, msg, &media_paths).await {
                    eprintln!("Instagram upload failed: {error}");

                    if let Err(send_error) = reply_text(
                        ctx,
                        msg,
                        format!(
                            "I downloaded the Instagram media, \
                                 but couldn't upload it to Discord. \
                                 Error: {error}"
                        ),
                    )
                    .await
                    {
                        eprintln!(
                            "Unable to send Instagram upload \
                             error reply: {send_error}"
                        );
                    }
                }

                if let Some(first_path) = media_paths.first()
                    && let Err(error) = instagram::cleanup_download(first_path).await
                {
                    eprintln!("Instagram cleanup failed: {error}");
                }
            }

            Err(error) => {
                eprintln!("Instagram download failed:\n{error}");

                let message = match &error {
                    InstagramDownloadError::PostProbablyGone(_) => {
                        "This Instagram post appears to be gone \
                             or is no longer available."
                            .to_string()
                    }

                    InstagramDownloadError::LoginRequired(_) => {
                        "Instagram requires a login to download \
                             this media."
                            .to_string()
                    }

                    InstagramDownloadError::YtDlp(_) => "yt-dlp failed while downloading this \
                             Instagram media."
                        .to_string(),

                    InstagramDownloadError::GalleryDl(_) => "gallery-dl failed while downloading \
                             this Instagram post."
                        .to_string(),

                    InstagramDownloadError::Other(_) => {
                        format!(
                            "I couldn't download the Instagram \
                                 media. Error: {error}"
                        )
                    }
                };

                if let Err(send_error) = reply_text(ctx, msg, message).await {
                    eprintln!(
                        "Unable to send Instagram error reply: \
                         {send_error}"
                    );
                }
            }
        }
    }
}
