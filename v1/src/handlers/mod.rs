pub mod archive;
pub mod facebook;
pub mod instagram;

use chrono::{DateTime, Local, Utc};
use serenity::{
    model::channel::{ChannelType, Message},
    prelude::Context,
};

async fn log_media_message(ctx: &Context, msg: &Message) {
    let name = if let Some(member) = msg.member.as_deref() {
        member
            .nick
            .as_deref()
            .unwrap_or_else(|| msg.author.display_name())
            .to_string()
    } else if let Some(guild_id) = msg.guild_id {
        guild_id
            .member(&ctx.http, msg.author.id)
            .await
            .map(|member| member.display_name().to_string())
            .unwrap_or_else(|_| msg.author.display_name().to_string())
    } else {
        msg.author.display_name().to_string()
    };
    let timestamp = DateTime::<Utc>::from_timestamp(msg.timestamp.unix_timestamp(), 0)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%b %d @ %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| msg.timestamp.to_string());
    let location = media_message_location(ctx, msg).await;
    println!("-----------------------");
    println!("{timestamp} - {name}: {}", msg.content);
    println!("{location}");
}

async fn media_message_location(ctx: &Context, msg: &Message) -> String {
    if msg.guild_id.is_none() {
        return "Category: None | Channel: Direct message".to_string();
    }

    let channel = match msg.channel_id.to_channel(ctx).await {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("Could not resolve media message channel: {error}");
            return format!("Category: Unknown | Channel ID: {}", msg.channel_id);
        }
    };
    let Some(channel) = channel.guild() else {
        return "Category: None | Channel: Direct message".to_string();
    };

    let mut category = "None".to_string();
    if let Some(parent_id) = channel.parent_id {
        category = "Unknown".to_string();
        if let Ok(parent) = parent_id.to_channel(ctx).await
            && let Some(parent) = parent.guild()
        {
            if parent.kind == ChannelType::Category {
                category = parent.name;
            } else {
                // A thread's parent is its containing channel, whose parent is the category.
                category = "None".to_string();
                if let Some(category_id) = parent.parent_id {
                    category = "Unknown".to_string();
                    if let Ok(parent_category) = category_id.to_channel(ctx).await
                        && let Some(parent_category) = parent_category.guild()
                        && parent_category.kind == ChannelType::Category
                    {
                        category = parent_category.name;
                    }
                }
            }
        }
    }

    format!("Category: {category} | Channel: #{}", channel.name)
}
