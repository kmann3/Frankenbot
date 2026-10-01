use serenity::{builder::CreateMessage, model::channel::Message, prelude::*};

pub async fn reply_text(
    ctx: &Context,
    msg: &Message,
    content: impl Into<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = content.into();
    reply_plain_text(ctx, msg, personalized_response(&msg.author.name, &content)).await
}

pub async fn reply_plain_text(
    ctx: &Context,
    msg: &Message,
    content: impl Into<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let builder = CreateMessage::new().content(content).reference_message(msg);

    msg.channel_id.send_message(&ctx.http, builder).await?;

    Ok(())
}

pub fn personalized_put_response(username: &str) -> String {
    format!("{}\n", request_put_acknowledgement(username))
}

pub fn personalized_response(username: &str, existing_message: &str) -> String {
    format!("{}\n{existing_message}", request_acknowledgement(username))
}

fn request_put_acknowledgement(username: &str) -> &'static str {
    if username.eq_ignore_ascii_case("abbigrasso") {
        "Here, Dungeon Mommi."
    } else if username.eq_ignore_ascii_case("cptn_patty")
        || username.eq_ignore_ascii_case("mrkmann")
    {
        "Here, Dungeon Daddy."
    } else {
        "Here, CHEF!"
    }
}

fn request_acknowledgement(username: &str) -> &'static str {
    if username.eq_ignore_ascii_case("abbigrasso") {
        "Yes, Dungeon Mommi."
    } else if username.eq_ignore_ascii_case("cptn_patty")
        || username.eq_ignore_ascii_case("mrkmann")
    {
        "Yes, Dungeon Daddy."
    } else {
        "YES, CHEF!"
    }
}

pub fn mention_acknowledgement(username: &str) -> &'static str {
    if username.eq_ignore_ascii_case("abbigrasso") {
        "Yes, Dungeon Mommi!"
    } else if username.eq_ignore_ascii_case("cptn_patty")
        || username.eq_ignore_ascii_case("mrkmann")
    {
        "Yes, Dungeon Daddy!"
    } else {
        "YES, CHEF!"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personalizes_responses_by_requester() {
        assert_eq!(request_acknowledgement("abbigrasso"), "Yes, Dungeon Mommi.");
        assert_eq!(request_acknowledgement("ABBiGrasso"), "Yes, Dungeon Mommi.");
        assert_eq!(request_acknowledgement("cptn_patty"), "Yes, Dungeon Daddy.");
        assert_eq!(request_acknowledgement("mrkmann"), "Yes, Dungeon Daddy.");
        assert_eq!(request_acknowledgement("someone_else"), "YES, CHEF!");
        assert_eq!(
            personalized_response("someone_else", "Here you go!"),
            "YES, CHEF!\nHere you go!"
        );
        assert_eq!(mention_acknowledgement("abbigrasso"), "Yes, Dungeon Mommi!");
        assert_eq!(mention_acknowledgement("ABBiGrasso"), "Yes, Dungeon Mommi!");
        assert_eq!(mention_acknowledgement("cptn_patty"), "Yes, Dungeon Daddy!");
        assert_eq!(mention_acknowledgement("mrkmann"), "Yes, Dungeon Daddy!");
        assert_eq!(mention_acknowledgement("someone_else"), "YES, CHEF!");
    }
}
