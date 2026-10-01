use serenity::{model::channel::Message, prelude::*};

use crate::{
    discord::{replies::reply_text, uploads::reply_with_downloaded_file},
    services::facebook,
};

pub async fn handle_message(ctx: &Context, msg: &Message) {
    let media_items = facebook::find_media(&msg.content);

    if !media_items.is_empty() {
        super::log_media_message(ctx, msg).await;
    }

    for media in media_items {
        if let Err(error) = reply_text(
            ctx,
            msg,
            format!("Facebook {:?} detected. Downloading...", media.media_type),
        )
        .await
        {
            eprintln!("Unable to send Facebook status reply: {error}");
        }

        match facebook::download_media(&media).await {
            Ok(media_path) => {
                if let Err(error) = reply_with_downloaded_file(ctx, msg, &media_path).await {
                    eprintln!("Facebook upload failed: {error}");

                    if let Err(send_error) = reply_text(
                        ctx,
                        msg,
                        format!(
                            "I downloaded the Facebook media, \
                                 but couldn't upload it to Discord. \
                                 Error: {error}"
                        ),
                    )
                    .await
                    {
                        eprintln!(
                            "Unable to send Facebook upload error \
                             reply: {send_error}"
                        );
                    }
                }

                if let Err(error) = tokio::fs::remove_file(&media_path).await {
                    eprintln!(
                        "Failed to delete temporary Facebook file \
                         {}: {error}",
                        media_path.display()
                    );
                }
            }

            Err(error) => {
                eprintln!("Facebook download failed:\n{error}");

                if let Err(send_error) = reply_text(
                    ctx,
                    msg,
                    format!(
                        "I couldn't download the Facebook media. \
                             Error: {error}"
                    ),
                )
                .await
                {
                    eprintln!(
                        "Unable to send Facebook error reply: \
                         {send_error}"
                    );
                }
            }
        }
    }
}
