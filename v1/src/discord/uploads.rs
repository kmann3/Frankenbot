use std::path::{Path, PathBuf};

use serenity::{
    builder::{CreateAttachment, CreateMessage},
    model::channel::Message,
    prelude::*,
};

use crate::discord::replies::{
    personalized_put_response,
    personalized_response,
    reply_text,
};

const MAX_UPLOAD_SIZE: u64 = 24 * 1024 * 1024;

pub async fn reply_with_downloaded_file(
    ctx: &Context,
    msg: &Message,
    file_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let metadata = tokio::fs::metadata(file_path).await?;

    let file_size = metadata.len();

    let file_size_mb = file_size as f64 / 1024.0 / 1024.0;

    println!("{}: {:.2} MiB", file_path.display(), file_size_mb);

    if file_size >= MAX_UPLOAD_SIZE {
        reply_text(
            ctx,
            msg,
            format!(
                "I downloaded this, but it's {:.2} MB, \
                 which is too big for me to upload here.",
                file_size_mb
            ),
        )
        .await?;

        return Ok(());
    }

    let attachment = CreateAttachment::path(file_path).await?;

    let builder = CreateMessage::new()
        .content(personalized_put_response(&msg.author.name))
        .reference_message(msg)
        .add_file(attachment);

    msg.channel_id.send_message(&ctx.http, builder).await?;

    Ok(())
}

pub async fn reply_with_downloaded_files(
    ctx: &Context,
    msg: &Message,
    file_paths: &[PathBuf],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut attachments = Vec::new();

    let mut oversized_files = Vec::new();

    for file_path in file_paths {
        let metadata = tokio::fs::metadata(file_path).await?;

        let file_size = metadata.len();

        let file_size_mb = file_size as f64 / 1024.0 / 1024.0;

        println!("{}: {:.2} MiB", file_path.display(), file_size_mb);

        if file_size >= MAX_UPLOAD_SIZE {
            oversized_files.push((file_path.clone(), file_size_mb));

            continue;
        }

        attachments.push(CreateAttachment::path(file_path).await?);
    }

    if !attachments.is_empty() {
        let builder = CreateMessage::new()
            .content(personalized_response(&msg.author.name, "Here you go!"))
            .reference_message(msg)
            .add_files(attachments);

        msg.channel_id.send_message(&ctx.http, builder).await?;
    }

    if !oversized_files.is_empty() {
        let descriptions = oversized_files
            .iter()
            .map(|(path, size)| {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file");

                format!("`{name}` ({size:.2} MB)")
            })
            .collect::<Vec<_>>()
            .join(", ");

        reply_text(
            ctx,
            msg,
            format!(
                "I downloaded the post, but the following \
                 file(s) are too large for me to repost \
                 right now: {descriptions}"
            ),
        )
        .await?;
    }

    Ok(())
}
