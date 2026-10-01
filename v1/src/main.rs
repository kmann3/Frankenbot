mod discord;
mod handler;
mod handlers;
mod services;

use handler::Handler;

use serenity::{model::gateway::GatewayIntents, prelude::*};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let token =
        std::env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN environment variable is not set");

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating Discord client");

    println!("Starting Frankenbot...");

    if let Err(error) = client.start().await {
        eprintln!("Discord client error: {error:?}");
    }
}
