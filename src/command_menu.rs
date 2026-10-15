//! Native Telegram menu via the existing client's request extension API.
//! The pinned telegram-bot version predates these two Bot API methods.
use serde::Serialize;
use telegram_bot::types::requests::{
    Error as RequestError, HttpRequest, JsonRequestType, JsonTrueToUnitResponse,
    Request, RequestType, RequestUrl,
};
use telegram_bot::Api;

#[derive(Serialize)]
struct BotCommand {
    command: &'static str,
    description: &'static str,
}

const COMMANDS: &[BotCommand] = &[
    BotCommand { command: "help", description: "Show all commands and usage" },
    BotCommand { command: "search", description: "Search movies or TV: /search title" },
    BotCommand { command: "status", description: "Show download status and progress" },
    BotCommand { command: "storage", description: "Show available disk storage" },
    BotCommand { command: "torrent_tv", description: "Add TV download: /torrent_tv magnet-link" },
    BotCommand { command: "torrent_movie", description: "Add movie download: /torrent_movie magnet-link" },
    BotCommand { command: "imdb", description: "Search by IMDb link (requires OMDB token)" },
    BotCommand { command: "delete_torrent", description: "List torrents to remove (keeps files)" },
    BotCommand { command: "delete_tv", description: "List TV files to permanently delete" },
    BotCommand { command: "delete_movie", description: "List movie files to permanently delete" },
    BotCommand { command: "stop_seed", description: "Stop seeding all downloads" },
    BotCommand { command: "restructure", description: "Preview media reorganization: /restructure tv or movie" },
    BotCommand { command: "chat_id", description: "Show this chat ID" },
];

#[derive(Serialize)]
struct SetMyCommands {
    commands: &'static [BotCommand],
}

impl Request for SetMyCommands {
    type Type = JsonRequestType<Self>;
    type Response = JsonTrueToUnitResponse;
    fn serialize(&self) -> Result<HttpRequest, RequestError> {
        <Self::Type as RequestType>::serialize(RequestUrl::method("setMyCommands"), self)
    }
}

#[derive(Serialize)]
struct MenuButton {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct SetChatMenuButton {
    menu_button: MenuButton,
}

impl Request for SetChatMenuButton {
    type Type = JsonRequestType<Self>;
    type Response = JsonTrueToUnitResponse;
    fn serialize(&self) -> Result<HttpRequest, RequestError> {
        <Self::Type as RequestType>::serialize(RequestUrl::method("setChatMenuButton"), self)
    }
}

pub async fn register(api: &Api) -> Result<(), telegram_bot::Error> {
    api.send(SetMyCommands { commands: COMMANDS }).await?;
    api.send(SetChatMenuButton { menu_button: MenuButton { kind: "commands" } }).await?;
    println!("Registered {} Telegram commands and native menu button", COMMANDS.len());
    Ok(())
}

/// Telegram command menus require underscores; retain legacy hyphen commands.
/// Group menus may append @bot_username to the command.
pub fn normalize_command(text: &str) -> String {
    if text.starts_with('/') {
        text.split('@').next().unwrap_or(text).replace('_', "-")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_commands_are_valid_unique_and_routable() {
        let handlers = ["/help", "/search", "/status", "/storage", "/torrent-tv",
            "/torrent-movie", "/imdb", "/delete-torrent", "/delete-tv",
            "/delete-movie", "/stop-seed", "/restructure", "/chat-id"];
        let mut names = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!((1..=32).contains(&command.command.len()));
            assert!(command.command.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
            assert!((1..=256).contains(&command.description.chars().count()));
            assert!(names.insert(command.command));
            assert!(handlers.contains(&normalize_command(&format!("/{}", command.command)).as_str()));
        }
    }

    #[test]
    fn normalizes_menu_and_legacy_commands_without_changing_arguments() {
        assert_eq!(normalize_command("/torrent_tv@ExampleBot"), "/torrent-tv");
        assert_eq!(normalize_command("/torrent-tv"), "/torrent-tv");
        assert_eq!(normalize_command("/status@ExampleBot"), "/status");
        assert_eq!(normalize_command("https://example.com/a_b"), "https://example.com/a_b");
        assert_eq!(normalize_command("tv"), "tv");
    }

    #[test]
    fn serializes_native_menu_payloads() {
        let commands = serde_json::to_value(SetMyCommands { commands: COMMANDS }).unwrap();
        assert_eq!(commands["commands"].as_array().unwrap().len(), 13);
        let menu = serde_json::to_value(SetChatMenuButton { menu_button: MenuButton { kind: "commands" } }).unwrap();
        assert_eq!(menu, serde_json::json!({"menu_button": {"type": "commands"}}));
    }
}
