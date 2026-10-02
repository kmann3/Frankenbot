use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use chrono::{DateTime, Local, Utc};
use regex::Regex;
use reqwest::{Client, Url};
use serenity::{
    builder::{
        CreateAttachment, CreateCommand, CreateCommandOption, CreateInteractionResponse,
        CreateInteractionResponseFollowup, CreateInteractionResponseMessage,
        EditInteractionResponse, GetMessages,
    },
    model::{
        Permissions,
        application::{CommandDataOptionValue, CommandInteraction, CommandOptionType},
        channel::{ChannelType, GuildChannel, Message},
        guild::{PartialMember, Role},
        id::{ChannelId, GuildId, RoleId, UserId},
        user::User,
    },
    prelude::Context,
};
use tokio::io::AsyncWriteExt as _;

use crate::discord::replies::personalized_response;

type BoxError = Box<dyn Error + Send + Sync>;

const ARCHIVE_ROOT: &str = "archives";
const SAFE_UPLOAD_LIMIT: u64 = 24 * 1024 * 1024;

static USER_MENTION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<@!?(\d+)>").expect("user mention regex should compile"));

#[derive(Debug)]
struct ArchiveSelection {
    channels: Vec<GuildChannel>,
    category: Option<GuildChannel>,
    roles: HashMap<RoleId, Role>,
}

#[derive(Debug)]
struct ArchivedMessage {
    id: u64,
    timestamp: String,
    edited: bool,
    author: String,
    author_colour: Option<(u8, u8, u8)>,
    content: String,
    attachments: Vec<ArchivedAttachment>,
    embeds: Vec<ArchivedEmbed>,
}

#[derive(Debug)]
struct ArchivedAttachment {
    name: String,
    url: String,
    size: u32,
    resource: Option<ArchivedResource>,
}

#[derive(Debug)]
struct ArchivedEmbed {
    summary: String,
    resources: Vec<ArchivedResource>,
    unavailable_resources: Vec<String>,
}

#[derive(Debug)]
struct ArchivedResource {
    source_url: String,
    relative_path: String,
    local_path: PathBuf,
    is_image: bool,
}

struct MessageArchiveState<'a> {
    ctx: &'a Context,
    guild_id: GuildId,
    roles: &'a HashMap<RoleId, Role>,
    author_colours: &'a mut HashMap<UserId, Option<(u8, u8, u8)>>,
    display_names: &'a mut HashMap<UserId, String>,
    resource_client: &'a Client,
    content_dir: &'a Path,
    used_asset_filenames: &'a mut HashSet<String>,
}

pub fn command() -> CreateCommand {
    CreateCommand::new("archive")
        .description("Archive a text/voice channel or category as HTML with resources in a ZIP")
        .default_member_permissions(Permissions::MANAGE_CHANNELS)
        .dm_permission(false)
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::Channel,
                "target",
                "The text channel, voice channel, or category to archive",
            )
            .channel_types(vec![
                ChannelType::Text,
                ChannelType::News,
                ChannelType::Voice,
                ChannelType::Category,
            ])
            .required(true),
        )
}

pub async fn handle(ctx: &Context, interaction: &CommandInteraction) {
    if let Err(error) = run(ctx, interaction).await {
        eprintln!("Archive command failed: {error}");

        let message = personalized_response(
            &interaction.user.name,
            &format!("I couldn't create the archive: {error}"),
        );
        if interaction.get_response(&ctx.http).await.is_ok() {
            let _ = interaction
                .edit_response(&ctx.http, EditInteractionResponse::new().content(message))
                .await;
        } else {
            let _ = interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(message)
                            .ephemeral(true),
                    ),
                )
                .await;
        }
    }
}

async fn run(ctx: &Context, interaction: &CommandInteraction) -> Result<(), BoxError> {
    let guild_id = interaction
        .guild_id
        .ok_or("This command can only be used in a server.")?;

    let member_permissions = interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .unwrap_or_else(Permissions::empty);
    if !member_permissions.contains(Permissions::MANAGE_CHANNELS) {
        return Err("You need the Manage Channels permission to create an archive.".into());
    }

    let target_id = option_channel_id(interaction, "target")
        .ok_or("The target channel or category was not provided.")?;

    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    let member = interaction
        .member
        .as_deref()
        .ok_or("Your server membership could not be verified.")?;
    let selection = archive_channels(ctx, guild_id, target_id, member).await?;
    let is_category = selection.category.is_some();
    let archived_channel_count = selection.channels.len();
    let requested_target = selection
        .category
        .as_ref()
        .map(|category| format!("category {}", category.name))
        .unwrap_or_else(|| format!("channel #{}", selection.channels[0].name));
    let archive_target_name = selection
        .category
        .as_ref()
        .map(|category| category.name.as_str())
        .unwrap_or(&selection.channels[0].name);
    let archive_file_prefix = selection
        .category
        .as_ref()
        .map(|category| safe_filename(&category.name))
        .unwrap_or_else(|| archive_channel_name(&selection.channels[0]));
    let requested_by = &interaction.user.name;
    let resource_client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("Frankenbot channel archiver")
        .build()?;
    let archive_timestamp = Local::now();
    let output_dir = create_output_dir(archive_target_name, &archive_timestamp).await?;
    let upload_limit = SAFE_UPLOAD_LIMIT.min(u64::from(interaction.attachment_size_limit));
    let content_dir = output_dir.join("bundle");
    tokio::fs::create_dir_all(&content_dir).await?;
    let mut bundle_files = Vec::new();
    let mut author_colours = HashMap::new();
    let mut display_names = HashMap::new();
    let mut used_asset_filenames = HashSet::new();
    let mut unavailable_resource_count = 0_usize;

    for channel in selection.channels {
        let mut message_archive_state = MessageArchiveState {
            ctx,
            guild_id,
            roles: &selection.roles,
            author_colours: &mut author_colours,
            display_names: &mut display_names,
            resource_client: &resource_client,
            content_dir: &content_dir,
            used_asset_filenames: &mut used_asset_filenames,
        };
        let messages = fetch_all_messages(channel.id, &mut message_archive_state).await?;
        unavailable_resource_count =
            unavailable_resource_count.saturating_add(count_unavailable_resources(&messages));
        let path = content_dir.join(format!("{}.html", archive_channel_basename(&channel)));
        bundle_files.extend(archived_resource_paths(&messages));
        write_html(&path, &channel, &messages).await?;
        bundle_files.push(path);
    }

    let zip_path = output_dir.join(archive_zip_filename(
        &archive_file_prefix,
        &archive_timestamp,
    ));
    create_zip(&zip_path, &content_dir, &bundle_files).await?;
    tokio::fs::remove_dir_all(&content_dir).await?;
    let zip_size = tokio::fs::metadata(&zip_path).await?.len();
    let deliverables = [(zip_path, zip_size)];

    println!(
        "Creating archive for {requested_target}, requested by {requested_by}. File size: {:.2} MiB",
        mib(deliverables[0].1)
    );

    let oversized = deliverables
        .iter()
        .filter(|(_, size)| *size >= upload_limit)
        .collect::<Vec<_>>();
    let ready = deliverables
        .iter()
        .filter(|(_, size)| *size < upload_limit)
        .collect::<Vec<_>>();
    let mut summary = if is_category {
        format!(
            "Created one ZIP archive containing {archived_channel_count} individual HTML channel files and their referenced resource files."
        )
    } else {
        "Created one ZIP archive containing the HTML channel file and its referenced resource files."
            .to_string()
    };
    if !oversized.is_empty() {
        let names = oversized
            .iter()
            .take(10)
            .map(|(path, size)| format!("`{}` ({:.2} MiB)", display_name(path), mib(*size)))
            .collect::<Vec<_>>()
            .join(", ");
        let remainder = if oversized.len() > 10 {
            format!(" and {} more", oversized.len() - 10)
        } else {
            String::new()
        };
        write!(
            summary,
            " The following file{} exceed{} Discord's upload limit and {} been kept by the bot: {names}{remainder}. Contact the bot administrator; they can email {} to you.",
            if oversized.len() == 1 { "" } else { "s" },
            if oversized.len() == 1 { "s" } else { "" },
            if oversized.len() == 1 { "has" } else { "have" },
            if oversized.len() == 1 { "it" } else { "them" },
        )?;
    }
    if unavailable_resource_count > 0 {
        write!(
            summary,
            " {unavailable_resource_count} resource{} could not be copied into the ZIP; the archive retains the original Discord URL{} instead.",
            if unavailable_resource_count == 1 {
                ""
            } else {
                "s"
            },
            if unavailable_resource_count == 1 {
                ""
            } else {
                "s"
            },
        )?;
    }
    if ready.is_empty() && oversized.is_empty() {
        summary.push_str(" No archive files were produced.");
    }

    interaction
        .edit_response(
            &ctx.http,
            EditInteractionResponse::new().content(personalized_response(requested_by, &summary)),
        )
        .await?;

    for (path, _) in ready {
        let upload = async {
            let attachment = CreateAttachment::path(path).await?;
            interaction
                .create_followup(
                    &ctx.http,
                    CreateInteractionResponseFollowup::new()
                        .content(format!("Archive for `{}`", display_name(path)))
                        .add_file(attachment)
                        .ephemeral(true),
                )
                .await
        }
        .await;

        match upload {
            Ok(_) => {
                if let Err(error) = tokio::fs::remove_file(path).await {
                    eprintln!(
                        "Uploaded archive but could not remove local copy {}: {error}",
                        path.display()
                    );
                }
            }
            Err(error) => {
                eprintln!("Could not upload archive {}: {error}", path.display());
                interaction
                    .create_followup(
                        &ctx.http,
                        CreateInteractionResponseFollowup::new()
                            .content(format!(
                                "I created `{}`, but couldn't upload it. The file has been kept by the bot; contact the bot administrator and they can email it to you.",
                                display_name(path)
                            ))
                            .ephemeral(true),
                    )
                    .await?;
            }
        }
    }

    if oversized.is_empty()
        && let Err(error) = tokio::fs::remove_dir(&output_dir).await
        && !matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
        )
    {
        eprintln!(
            "Uploaded archive but could not remove empty directory {}: {error}",
            output_dir.display()
        );
    }

    Ok(())
}

fn option_channel_id(interaction: &CommandInteraction, name: &str) -> Option<ChannelId> {
    interaction.data.options.iter().find_map(|option| {
        (option.name == name)
            .then_some(&option.value)
            .and_then(|value| match value {
                CommandDataOptionValue::Channel(id) => Some(*id),
                _ => None,
            })
    })
}

async fn archive_channels(
    ctx: &Context,
    guild_id: GuildId,
    target_id: ChannelId,
    member: &serenity::model::guild::Member,
) -> Result<ArchiveSelection, BoxError> {
    let mut channels = guild_id.channels(&ctx.http).await?;
    let guild = guild_id.to_partial_guild(&ctx.http).await?;
    let target = channels
        .remove(&target_id)
        .ok_or("The selected channel no longer exists.")?;

    if target.guild_id != guild_id {
        return Err("The selected channel does not belong to this server.".into());
    }
    let required = Permissions::MANAGE_CHANNELS
        | Permissions::VIEW_CHANNEL
        | Permissions::READ_MESSAGE_HISTORY;
    if !guild
        .user_permissions_in(&target, member)
        .contains(required)
    {
        return Err(
            "You need Manage Channels, View Channel, and Read Message History permissions for the selected target."
                .into(),
        );
    }

    match target.kind {
        ChannelType::Text | ChannelType::News | ChannelType::Voice => Ok(ArchiveSelection {
            channels: vec![target],
            category: None,
            roles: guild.roles,
        }),
        ChannelType::Category => {
            let mut children = channels
                .into_values()
                .filter(|channel| {
                    channel.parent_id == Some(target_id)
                        && matches!(
                            channel.kind,
                            ChannelType::Text | ChannelType::News | ChannelType::Voice
                        )
                        && guild
                            .user_permissions_in(channel, member)
                            .contains(Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY)
                })
                .collect::<Vec<_>>();
            children.sort_by_key(|channel| (channel.position, channel.id));
            if children.is_empty() {
                Err("That category has no text or voice channels to archive.".into())
            } else {
                Ok(ArchiveSelection {
                    channels: children,
                    category: Some(target),
                    roles: guild.roles,
                })
            }
        }
        _ => Err("Select a text channel, announcement channel, voice channel, or category.".into()),
    }
}

async fn fetch_all_messages(
    channel_id: ChannelId,
    state: &mut MessageArchiveState<'_>,
) -> Result<Vec<ArchivedMessage>, BoxError> {
    let mut before = None;
    let mut all = Vec::new();

    loop {
        let mut request = GetMessages::new().limit(100);
        if let Some(message_id) = before {
            request = request.before(message_id);
        }
        let batch = channel_id.messages(&state.ctx.http, request).await?;
        if batch.is_empty() {
            break;
        }

        before = batch.last().map(|message| message.id);
        let count = batch.len();
        for message in batch {
            all.push(archive_message(message, state).await);
        }
        if count < 100 {
            break;
        }
    }

    all.reverse();
    Ok(all)
}

async fn archive_message(message: Message, state: &mut MessageArchiveState<'_>) -> ArchivedMessage {
    let message_id = message.id.get();
    let author = resolve_display_name(
        state.ctx,
        state.guild_id,
        &message.author,
        message.member.as_deref(),
        state.display_names,
    )
    .await;
    let mut mention_names = HashMap::with_capacity(message.mentions.len());
    for user in &message.mentions {
        let display_name = resolve_display_name(
            state.ctx,
            state.guild_id,
            user,
            user.member.as_deref(),
            state.display_names,
        )
        .await;
        mention_names.insert(user.id.get(), display_name);
    }
    let content = replace_user_mentions(&message.content, &mention_names);
    let author_colour = resolve_author_colour(
        state.ctx,
        state.guild_id,
        message.author.id,
        message.member.as_deref(),
        state.roles,
        state.author_colours,
    )
    .await;
    let mut attachments = Vec::with_capacity(message.attachments.len());
    for attachment in message.attachments {
        let filename = unique_asset_filename(
            &safe_resource_filename(&attachment.filename, "attachment"),
            state.used_asset_filenames,
        );
        let filename_key = filename.to_lowercase();
        let relative_path = PathBuf::from("assets").join(filename);
        let resource = archive_resource(
            state.resource_client,
            &attachment.url,
            state.content_dir,
            relative_path,
            is_image_attachment(&attachment.filename, attachment.content_type.as_deref()),
        )
        .await;
        if resource.is_none() {
            state.used_asset_filenames.remove(&filename_key);
        }
        attachments.push(ArchivedAttachment {
            name: attachment.filename,
            url: attachment.url,
            size: attachment.size,
            resource,
        });
    }

    let mut embeds = Vec::with_capacity(message.embeds.len());
    for embed in message.embeds {
        let summary = embed
            .title
            .clone()
            .or(embed.description.clone())
            .or(embed.url.clone())
            .unwrap_or_else(|| "Rich embed".to_string());
        let mut resources = Vec::new();
        let mut unavailable_resources = Vec::new();
        let resource_urls = [
            embed.image.as_ref().map(|image| {
                (
                    image.proxy_url.as_deref().unwrap_or(image.url.as_str()),
                    true,
                )
            }),
            embed.thumbnail.as_ref().map(|thumbnail| {
                (
                    thumbnail
                        .proxy_url
                        .as_deref()
                        .unwrap_or(thumbnail.url.as_str()),
                    true,
                )
            }),
            embed.author.as_ref().and_then(|author| {
                author
                    .proxy_icon_url
                    .as_deref()
                    .or(author.icon_url.as_deref())
                    .map(|url| (url, true))
            }),
            embed.footer.as_ref().and_then(|footer| {
                footer
                    .proxy_icon_url
                    .as_deref()
                    .or(footer.icon_url.as_deref())
                    .map(|url| (url, true))
            }),
            embed.video.as_ref().map(|video| {
                (
                    video.proxy_url.as_deref().unwrap_or(video.url.as_str()),
                    false,
                )
            }),
        ];
        for (url, is_image) in resource_urls.into_iter().flatten() {
            if resources
                .iter()
                .any(|resource: &ArchivedResource| resource.source_url == url)
            {
                continue;
            }
            let filename = unique_asset_filename(
                &resource_filename_from_url(url, "embed-resource"),
                state.used_asset_filenames,
            );
            let filename_key = filename.to_lowercase();
            let relative_path = PathBuf::from("assets").join(filename);
            if let Some(resource) = archive_resource(
                state.resource_client,
                url,
                state.content_dir,
                relative_path,
                is_image,
            )
            .await
            {
                resources.push(resource);
            } else {
                state.used_asset_filenames.remove(&filename_key);
                unavailable_resources.push(url.to_string());
            }
        }
        embeds.push(ArchivedEmbed {
            summary,
            resources,
            unavailable_resources,
        });
    }

    ArchivedMessage {
        id: message_id,
        timestamp: format_archive_timestamp(message.timestamp.unix_timestamp()),
        edited: message.edited_timestamp.is_some(),
        author,
        author_colour,
        content,
        attachments,
        embeds,
    }
}

async fn resolve_display_name(
    ctx: &Context,
    guild_id: GuildId,
    user: &User,
    partial_member: Option<&PartialMember>,
    display_names: &mut HashMap<UserId, String>,
) -> String {
    if let Some(name) = display_names.get(&user.id) {
        return name.clone();
    }

    let name = if partial_member.is_some() {
        preferred_display_name(user, partial_member).to_string()
    } else if let Ok(member) = guild_id.member(&ctx.http, user.id).await {
        member.display_name().to_string()
    } else {
        user.display_name().to_string()
    };
    display_names.insert(user.id, name.clone());
    name
}

fn preferred_display_name<'a>(user: &'a User, member: Option<&'a PartialMember>) -> &'a str {
    preferred_display_name_parts(
        &user.name,
        user.global_name.as_deref(),
        member.and_then(|member| member.nick.as_deref()),
    )
}

fn preferred_display_name_parts<'a>(
    username: &'a str,
    global_name: Option<&'a str>,
    server_nickname: Option<&'a str>,
) -> &'a str {
    server_nickname.or(global_name).unwrap_or(username)
}

fn replace_user_mentions(content: &str, mention_names: &HashMap<u64, String>) -> String {
    USER_MENTION_PATTERN
        .replace_all(content, |captures: &regex::Captures<'_>| {
            captures
                .get(1)
                .and_then(|capture| capture.as_str().parse::<u64>().ok())
                .and_then(|user_id| mention_names.get(&user_id))
                .map(|username| format!("@{username}"))
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

fn format_archive_timestamp(unix_timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix_timestamp, 0)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%Y.%m.%d-%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "0000.00.00-00:00:00".to_string())
}

async fn resolve_author_colour(
    ctx: &Context,
    guild_id: GuildId,
    user_id: UserId,
    partial_member: Option<&PartialMember>,
    roles: &HashMap<RoleId, Role>,
    cache: &mut HashMap<UserId, Option<(u8, u8, u8)>>,
) -> Option<(u8, u8, u8)> {
    if let Some(colour) = cache.get(&user_id) {
        return *colour;
    }

    let member_roles = if let Some(member) = partial_member {
        Some(member.roles.clone())
    } else {
        guild_id
            .member(&ctx.http, user_id)
            .await
            .ok()
            .map(|member| member.roles)
    };
    let colour = member_roles.and_then(|member_roles| {
        member_roles
            .iter()
            .filter_map(|role_id| roles.get(role_id))
            .filter_map(|role| {
                let colour = if role.colours.primary_colour.0 != 0 {
                    role.colours.primary_colour
                } else {
                    role.colour
                };
                (colour.0 != 0).then_some((role, colour))
            })
            .max_by_key(|(role, _)| (role.position, role.id))
            .map(|(_, colour)| colour.tuple())
    });

    cache.insert(user_id, colour);
    colour
}

async fn archive_resource(
    client: &Client,
    url: &str,
    content_dir: &Path,
    relative_path: PathBuf,
    is_image: bool,
) -> Option<ArchivedResource> {
    let local_path = content_dir.join(&relative_path);
    match try_download_resource(client, url, &local_path).await {
        Ok(()) => Some(ArchivedResource {
            source_url: url.to_string(),
            relative_path: path_for_archive(&relative_path),
            local_path,
            is_image,
        }),
        Err(error) => {
            eprintln!("Could not archive resource {url}: {error}");
            None
        }
    }
}

async fn try_download_resource(client: &Client, url: &str, path: &Path) -> Result<(), BoxError> {
    let parsed = Url::parse(url)?;
    if parsed.scheme() != "https" || !is_allowed_resource_host(&parsed) {
        return Err("resource URL is not on Discord's HTTPS CDN/proxy".into());
    }

    let mut response = client.get(parsed).send().await?.error_for_status()?;
    if !is_allowed_resource_host(response.url()) {
        return Err("resource download redirected away from Discord's CDN/proxy".into());
    }

    let parent = path
        .parent()
        .ok_or("resource path has no parent directory")?;
    tokio::fs::create_dir_all(parent).await?;
    let partial_path = parent.join(format!(
        ".{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("resource")
    ));
    let result = async {
        let mut file = tokio::fs::File::create(&partial_path).await?;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&partial_path, path).await?;
        Ok::<(), BoxError>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial_path).await;
    }
    result
}

fn is_allowed_resource_host(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host == "cdn.discordapp.com"
            || host == "media.discordapp.net"
            || host == "cdn.discord.com"
            || host == "media.discord.com"
            || host.ends_with(".discordapp.com")
            || host.ends_with(".discordapp.net")
            || host.ends_with(".discord.com")
    })
}

fn safe_resource_filename(filename: &str, fallback: &str) -> String {
    let filename = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let filename = filename.trim();
    if filename.is_empty() || matches!(filename, "." | "..") {
        fallback.to_string()
    } else {
        filename.to_string()
    }
}

fn resource_filename_from_url(url: &str, fallback: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(|filename| safe_resource_filename(filename, fallback))
        })
        .filter(|filename| !filename.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn unique_asset_filename(filename: &str, used_filenames: &mut HashSet<String>) -> String {
    if used_filenames.insert(filename.to_lowercase()) {
        return filename.to_string();
    }

    let (stem, extension) = split_filename_extension(filename);
    for suffix in 1_u64.. {
        let candidate = if extension.is_empty() {
            format!("{stem}_{suffix}")
        } else {
            format!("{stem}_{suffix}.{extension}")
        };
        if used_filenames.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("the numeric filename suffix space is inexhaustible")
}

fn split_filename_extension(filename: &str) -> (&str, &str) {
    filename
        .rfind('.')
        .filter(|index| *index > 0 && *index + 1 < filename.len())
        .map(|index| (&filename[..index], &filename[index + 1..]))
        .unwrap_or((filename, ""))
}

fn path_for_archive(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn supported_image_mime(value: &str) -> Option<&'static str> {
    match value
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/bmp" => Some("image/bmp"),
        "image/tiff" => Some("image/tiff"),
        _ => None,
    }
}

fn is_image_attachment(filename: &str, content_type: Option<&str>) -> bool {
    content_type.and_then(supported_image_mime).is_some()
        || Path::new(filename)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
                )
            })
}

async fn create_output_dir(
    target_name: &str,
    timestamp: &DateTime<Local>,
) -> Result<PathBuf, BoxError> {
    let path = archive_output_path(target_name, timestamp);
    tokio::fs::create_dir_all(ARCHIVE_ROOT).await?;
    tokio::fs::create_dir(&path).await?;
    Ok(path)
}

fn archive_output_path(target_name: &str, timestamp: &DateTime<Local>) -> PathBuf {
    PathBuf::from(ARCHIVE_ROOT).join(format!(
        "{}-{}",
        safe_filename(target_name),
        format_directory_timestamp(timestamp)
    ))
}

fn format_directory_timestamp(timestamp: &DateTime<Local>) -> String {
    timestamp.format("%Y.%m.%d-%H.%M.%S%.3f").to_string()
}

fn archive_zip_filename(prefix: &str, timestamp: &DateTime<Local>) -> String {
    format!("{prefix}-{}.zip", format_directory_timestamp(timestamp))
}

async fn create_zip(
    zip_path: &Path,
    base_dir: &Path,
    archive_paths: &[PathBuf],
) -> Result<(), BoxError> {
    let zip_path = zip_path.to_owned();
    let base_dir = base_dir.to_owned();
    let archive_paths = archive_paths.to_vec();
    tokio::task::spawn_blocking(move || write_zip(&zip_path, &base_dir, &archive_paths)).await??;
    Ok(())
}

fn write_zip(zip_path: &Path, base_dir: &Path, archive_paths: &[PathBuf]) -> Result<(), BoxError> {
    let file = std::fs::File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for path in archive_paths {
        let filename = path
            .strip_prefix(base_dir)?
            .to_string_lossy()
            .replace('\\', "/");
        zip.start_file(filename, options)?;
        let mut input = std::fs::File::open(path)?;
        std::io::copy(&mut input, &mut zip)?;
    }

    zip.finish()?.sync_all()?;
    Ok(())
}

async fn write_html(
    path: &Path,
    channel: &GuildChannel,
    messages: &[ArchivedMessage],
) -> Result<(), BoxError> {
    let mut output = String::with_capacity(messages.len().saturating_mul(300));
    write!(
        output,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>#{}</title><style>{}</style></head><body><main><header><h1>#{}</h1><p>{} messages • channel ID {}</p></header>",
        html_escape(&channel.name),
        HTML_STYLE,
        html_escape(&channel.name),
        messages.len(),
        channel.id.get(),
    )?;

    for message in messages {
        let author_colour = message
            .author_colour
            .map(|(red, green, blue)| format!("#{red:02X}{green:02X}{blue:02X}"))
            .unwrap_or_else(|| "#F2F3F5".to_string());
        write!(
            output,
            "<article id=\"message-{}\"><div class=\"message-line\"><time>[{}]</time> <strong style=\"color:{}\">{}:</strong> <span class=\"content\">{}</span>{}</div>",
            message.id,
            html_escape(&message.timestamp),
            author_colour,
            html_escape(&message.author),
            html_escape(&message.content),
            if message.edited {
                " <span class=\"edited\">(edited)</span>"
            } else {
                ""
            },
        )?;
        if !message.attachments.is_empty() {
            output.push_str("<ul class=\"attachments\">");
            for attachment in &message.attachments {
                let href = attachment
                    .resource
                    .as_ref()
                    .map(|resource| resource.relative_path.as_str())
                    .unwrap_or(&attachment.url);
                write!(
                    output,
                    "<li>Attachment: <a href=\"{}\">{}</a> ({:.2} MiB)",
                    html_escape(href),
                    html_escape(&attachment.name),
                    mib(u64::from(attachment.size)),
                )?;
                if let Some(resource) = &attachment.resource
                    && resource.is_image
                {
                    write_html_image(&mut output, &attachment.name, &resource.relative_path)?;
                } else if attachment.resource.is_none() {
                    output.push_str(
                        " <span class=\"resource-unavailable\">(local copy unavailable)</span>",
                    );
                }
                output.push_str("</li>");
            }
            output.push_str("</ul>");
        }
        for embed in &message.embeds {
            write!(
                output,
                "<div class=\"embed\">Embed: {}</div>",
                html_escape(&embed.summary)
            )?;
            for resource in &embed.resources {
                if resource.is_image {
                    write_html_image(&mut output, "Embedded image", &resource.relative_path)?;
                } else {
                    write!(
                        output,
                        "<div class=\"embed-resource\">Embed resource: <a href=\"{}\">{}</a></div>",
                        html_escape(&resource.relative_path),
                        html_escape(&resource.relative_path),
                    )?;
                }
            }
            for url in &embed.unavailable_resources {
                write!(
                    output,
                    "<div class=\"resource-unavailable\">Resource could not be archived: <a href=\"{}\">{}</a></div>",
                    html_escape(url),
                    html_escape(url),
                )?;
            }
        }
        output.push_str("</article>");
    }
    output.push_str("</main></body></html>");
    tokio::fs::write(path, output).await?;
    Ok(())
}

fn write_html_image(
    output: &mut String,
    alt: &str,
    relative_path: &str,
) -> Result<(), std::fmt::Error> {
    write!(
        output,
        "<a class=\"archived-image-link\" href=\"{}\"><img class=\"archived-image\" loading=\"lazy\" alt=\"{}\" src=\"{}\"></a>",
        html_escape(relative_path),
        html_escape(alt),
        html_escape(relative_path),
    )
}

fn archived_resource_paths(messages: &[ArchivedMessage]) -> Vec<PathBuf> {
    messages
        .iter()
        .flat_map(|message| {
            message
                .attachments
                .iter()
                .filter_map(|attachment| attachment.resource.as_ref())
                .chain(message.embeds.iter().flat_map(|embed| &embed.resources))
        })
        .map(|resource| resource.local_path.clone())
        .collect()
}

fn count_unavailable_resources(messages: &[ArchivedMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            message
                .attachments
                .iter()
                .filter(|attachment| attachment.resource.is_none())
                .count()
                + message
                    .embeds
                    .iter()
                    .map(|embed| embed.unavailable_resources.len())
                    .sum::<usize>()
        })
        .sum()
}

fn archive_channel_basename(channel: &GuildChannel) -> String {
    format!("{}-{}", archive_channel_name(channel), channel.id.get())
}

fn archive_channel_name(channel: &GuildChannel) -> String {
    format!(
        "{}{}",
        channel_filename_prefix(channel.kind),
        safe_filename(&channel.name)
    )
}

fn channel_filename_prefix(kind: ChannelType) -> &'static str {
    if kind == ChannelType::Voice {
        "VC-"
    } else {
        ""
    }
}

fn safe_filename(name: &str) -> String {
    let safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches('-');
    if safe.is_empty() {
        "channel".to_string()
    } else {
        safe.to_string()
    }
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn display_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("archive file")
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

const HTML_STYLE: &str = r#"
:root{color-scheme:light dark;font:15px/1.45 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
body{margin:0;background:#313338;color:#dbdee1}main{max-width:920px;margin:auto;background:#313338;min-height:100vh}
header{position:sticky;top:0;padding:20px 24px;background:#2b2d31;border-bottom:1px solid #1e1f22;z-index:1}h1{margin:0}header p{margin:.25rem 0 0;color:#b5bac1}
article{padding:4px 24px;break-inside:avoid}article:hover{background:#2e3035}.message-line{white-space:normal;overflow-wrap:anywhere}.message-line time{color:#949ba4;font-variant-numeric:tabular-nums}.message-line strong{font-weight:600}.content{white-space:pre-wrap;color:#dbdee1}.edited{font-size:.75rem;color:#949ba4}
.attachments{margin:.35rem 0 .5rem 2rem;padding-left:1.3rem}.embed,.embed-resource,.resource-unavailable{margin:.35rem 0 .5rem 2rem;padding:.35rem .65rem;border-left:4px solid #5865f2;background:#2b2d31;overflow-wrap:anywhere}a{color:#00a8fc}.archived-image-link{display:block;margin:.65rem 0 1rem 2rem}.archived-image{display:block;max-width:min(100%,760px);max-height:720px;border-radius:8px;object-fit:contain;background:#1e1f22}
@media print{header{position:static}article:hover{background:transparent}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_content_and_attributes() {
        assert_eq!(html_escape("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
    }

    #[test]
    fn creates_safe_channel_filenames() {
        assert_eq!(safe_filename("general chat!"), "general-chat");
        assert_eq!(safe_filename("🔥"), "channel");
        assert_eq!(channel_filename_prefix(ChannelType::Voice), "VC-");
        assert_eq!(channel_filename_prefix(ChannelType::Text), "");
        assert_eq!(channel_filename_prefix(ChannelType::News), "");
    }

    #[test]
    fn formats_timestamps_in_the_bot_local_timezone() {
        let formatted = format_archive_timestamp(1_750_680_673);
        assert_eq!(formatted.len(), 19);
        assert_eq!(&formatted[4..5], ".");
        assert_eq!(&formatted[7..8], ".");
        assert_eq!(&formatted[10..11], "-");
        assert_eq!(&formatted[13..14], ":");
        assert_eq!(&formatted[16..17], ":");
    }

    #[test]
    fn formats_archive_directory_timestamps_to_milliseconds() {
        let now = Local::now();
        let formatted = format_directory_timestamp(&now);
        assert_eq!(formatted.len(), 23);
        assert_eq!(&formatted[4..5], ".");
        assert_eq!(&formatted[7..8], ".");
        assert_eq!(&formatted[10..11], "-");
        assert_eq!(&formatted[13..14], ".");
        assert_eq!(&formatted[16..17], ".");
        assert_eq!(&formatted[19..20], ".");
        assert_eq!(
            archive_output_path("general chat!", &now)
                .file_name()
                .and_then(|name| name.to_str()),
            Some(format!("general-chat-{formatted}").as_str())
        );
        assert_eq!(
            archive_zip_filename("general-chat", &now),
            format!("general-chat-{formatted}.zip")
        );
    }

    #[test]
    fn replaces_user_ids_with_discord_usernames() {
        let mention_names = HashMap::from([
            (4_024_920, "Nazadus".to_string()),
            (9_999, "AnotherUser".to_string()),
        ]);

        assert_eq!(
            replace_user_mentions(
                "Replying to <@4024920> and <@!9999>; unknown: <@1234>",
                &mention_names,
            ),
            "Replying to @Nazadus and @AnotherUser; unknown: <@1234>"
        );
    }

    #[test]
    fn prefers_discord_display_names_over_account_usernames() {
        assert_eq!(
            preferred_display_name_parts("account-name", Some("Global Name"), Some("Server Nick")),
            "Server Nick"
        );
        assert_eq!(
            preferred_display_name_parts("account-name", Some("Global Name"), None),
            "Global Name"
        );
        assert_eq!(
            preferred_display_name_parts("account-name", None, None),
            "account-name"
        );
    }

    #[test]
    fn references_separate_html_image_files() {
        let mut html = String::new();
        write_html_image(&mut html, "test image", "assets/general/test.png")
            .expect("HTML should render");
        assert!(html.contains("src=\"assets/general/test.png\""));
        assert!(html.contains("href=\"assets/general/test.png\""));
        assert!(!html.contains("base64"));
        assert!(html.contains("test image"));
    }

    #[test]
    fn creates_one_zip_with_each_channel_archive() {
        use std::io::Read as _;

        let directory =
            std::env::temp_dir().join(format!("frankenbot-zip-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("test directory should exist");
        let first = directory.join("general.html");
        let second = directory.join("random.html");
        let asset_dir = directory.join("assets");
        let image = asset_dir.join("image.png");
        let attachment = asset_dir.join("original guide.pdf");
        let zip_path = directory.join("category.zip");
        std::fs::create_dir_all(&asset_dir).expect("asset directory should exist");
        std::fs::write(&first, "general archive").expect("first archive should write");
        std::fs::write(&second, "random archive").expect("second archive should write");
        std::fs::write(&image, b"image asset fixture").expect("image asset should write");
        std::fs::write(&attachment, b"%PDF-test").expect("PDF attachment should write");

        write_zip(
            &zip_path,
            &directory,
            &[
                first.clone(),
                second.clone(),
                image.clone(),
                attachment.clone(),
            ],
        )
        .expect("ZIP should render");
        let mut zip =
            zip::ZipArchive::new(std::fs::File::open(&zip_path).expect("ZIP should be readable"))
                .expect("ZIP should be valid");
        assert_eq!(zip.len(), 4);
        let mut content = String::new();
        zip.by_name("general.html")
            .expect("general archive should be present")
            .read_to_string(&mut content)
            .expect("general archive should be readable");
        assert_eq!(content, "general archive");
        assert!(zip.by_name("assets/image.png").is_ok());
        assert!(zip.by_name("assets/original guide.pdf").is_ok());
        drop(zip);

        std::fs::remove_file(first).expect("first test file should be removable");
        std::fs::remove_file(second).expect("second test file should be removable");
        std::fs::remove_file(image).expect("test image should be removable");
        std::fs::remove_file(attachment).expect("test attachment should be removable");
        std::fs::remove_file(zip_path).expect("test ZIP should be removable");
        std::fs::remove_dir(asset_dir).expect("asset directory should be removable");
        std::fs::remove_dir(directory).expect("test directory should be removable");
    }

    #[test]
    fn preserves_safe_resource_filenames() {
        assert_eq!(
            safe_resource_filename("Quarterly Report.pdf", "attachment"),
            "Quarterly Report.pdf"
        );
        assert_eq!(
            safe_resource_filename("../unsafe.pdf", "attachment"),
            "unsafe.pdf"
        );
        assert_eq!(safe_resource_filename("..", "attachment"), "attachment");

        let mut used = HashSet::new();
        assert_eq!(unique_asset_filename("foo.pdf", &mut used), "foo.pdf");
        assert_eq!(unique_asset_filename("foo.pdf", &mut used), "foo_1.pdf");
        assert_eq!(unique_asset_filename("FOO.PDF", &mut used), "FOO_2.PDF");
        assert_eq!(unique_asset_filename("README", &mut used), "README");
        assert_eq!(unique_asset_filename("README", &mut used), "README_1");
    }
}
