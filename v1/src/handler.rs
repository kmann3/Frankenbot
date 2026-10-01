use serenity::{
    async_trait,
    model::{
        application::{Command, Interaction},
        channel::Message,
        gateway::Ready,
    },
    prelude::*,
};

use crate::{
    discord::replies::{mention_acknowledgement, reply_plain_text},
    handlers::{archive, facebook, instagram},
};

pub struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        //
        // Never process messages sent by bots.
        //
        if msg.author.bot {
            return;
        }

        if msg.mentions_user_id(ctx.cache.current_user().id)
            && let Err(error) =
                reply_plain_text(&ctx, &msg, mention_acknowledgement(&msg.author.name)).await
        {
            eprintln!("Unable to reply to bot mention: {error}");
        }

        // println!(
        //     "Message {} from {}: {}",
        //     msg.id, msg.author.name, msg.content
        // );

        instagram::handle_message(&ctx, &msg).await;

        facebook::handle_message(&ctx, &msg).await;

        // TODO:
        // tiktok::handle_message(...)
        //
        // TODO:
        // youtube::handle_message(...)
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("Logged in as {}!", ready.user.name);

        if let Err(error) = Command::set_global_commands(&_ctx.http, vec![archive::command()]).await
        {
            eprintln!("Unable to synchronize global commands: {error}");
        }

        for guild in &ready.guilds {
            if let Err(error) = guild.id.set_commands(&_ctx.http, Vec::new()).await {
                eprintln!(
                    "Unable to clear old guild commands for server {}: {error}",
                    guild.id
                );
            }
        }

        println!("Bot is ready.");
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction
            && command.data.name == "archive"
        {
            archive::handle(&ctx, &command).await;
        }
    }
}
