use std::env;

use futures::lock::Mutex;
use telegram_bot::prelude::*;
use telegram_bot::{Api, ChatId, Message, MessageId, ParseMode};

use std::sync::Arc;

use crate::imdb::get_imdb_info;
use crate::jackett::{
    dispatch_from_reply, format_telegram_response, request_jackett, TelegramJackettResponse,
    TorrentLocation,
};
use crate::transmission::{
    add_torrent, delete_torrent, get_media_type_from_path, get_storage_info, get_torrents,
    stop_seeding_all, Media, Torrent,
};

const HELP: &str = "
/torrent-tv (Magnet Link)
/torrent-movie (Magnet Link)
/search (Movie or TV Show e.g. The Matrix or Simpsons s01e01)
/imdb (Imdb link). Requires omdb token set https://www.omdbapi.com/
/status - Get status of active downloads
/delete-torrent - List all downloads (reply with number to delete torrent)
/delete-tv - List TV shows files (reply with number to delete file)
/delete-movie - List movie files (reply with number to delete file)
/restructure <tv|movie> - Scan and reorganize media files
/stop-seed - Stop seeding for all downloads
/storage - Get available storage information

Reply the magnet links with:
Position of the torrent
If jackett doesn't provide a category, it's possible to force with:
tv (position)
movie (position)
";

fn allowed_groups() -> Vec<ChatId> {
    return match env::var("TELEGRAM_ALLOWED_GROUPS") {
        Ok(val) => val
            .split(',')
            .map(|x| ChatId::new(x.parse::<i64>().unwrap()))
            .collect::<Vec<ChatId>>(),
        Err(_) => Vec::new(),
    };
}

async fn dispatch_chat_id(message: Message) -> Result<String, String> {
    let chat_id = message.chat.id();
    let reply = format!("Chat ID: {}", chat_id);

    Ok(reply)
}

async fn dispatch_tv(text: Vec<String>) -> Result<String, String> {
    if text.len() <= 1 {
        return Err("Send the magnet-url after command (/torrent-tv magnet_url)".to_string());
    }

    let location = TorrentLocation {
        is_magnet: true,
        content: text[1].clone(),
    };
    add_torrent(location, Media::TV).await?;

    Ok("🧲 Added torrent".to_string())
}

async fn dispatch_movie(text: Vec<String>) -> Result<String, String> {
    if text.len() <= 1 {
        return Err("Send the magnet-url after command (/torrent-movie magnet_url)".to_string());
    }

    let location = TorrentLocation {
        is_magnet: true,
        content: text[1].clone(),
    };
    add_torrent(location, Media::Movie).await?;

    Ok("🧲 Added torrent".to_string())
}

async fn dispatch_from_imdb_url(imdb_url: String) -> Result<TelegramJackettResponse, String> {
    let title = get_imdb_info(imdb_url.clone()).await?;
    let result = request_jackett(title).await?;

    Ok(result)
}

async fn dispatch_search(text: Vec<String>) -> Result<TelegramJackettResponse, String> {
    if text.len() <= 1 {
        return Err("Pass the movie/TV after command (/search Matrix 1999)".to_string());
    }

    let search_text = text[1..].join(" ");
    let result = request_jackett(search_text).await?;

    Ok(result)
}

async fn pick_choices(
    index: u16,
    reply_text: String,
    torrents: Vec<TelegramJackettResponse>,
    mut media: Option<Media>,
) -> Result<String, String> {
    let (torrent_media, location) = dispatch_from_reply(index, reply_text, torrents).await?;

    if media.is_none() && torrent_media.is_none() {
        return Err(
            "No category for given torrent.\nReply with tv (index) or movie (index) to force it"
                .to_string(),
        );
    }

    if media.is_none() {
        media = torrent_media;
    }

    add_torrent(location, media.unwrap()).await?;

    Ok("🧲 Added torrent".to_string())
}

async fn dispatch_status() -> Result<String, String> {
    use size_format::SizeFormatterSI;
    
    let torrents = get_torrents().await?;

    if torrents.is_empty() {
        return Ok("📊 No active downloads".to_string());
    }

    let mut status = String::from("📊 Active Downloads:\n\n");

    for torrent in &torrents {
        let percent = (torrent.percent_done * 100.0) as i64;
        let status_emoji = match torrent.status {
            0 => "⏸️",  // Stopped
            1 => "⏳",   // Queued to verify
            2 => "🔍",   // Verifying
            3 => "⏳",   // Queued to download
            4 => "⬇️",   // Downloading
            5 => "⏳",   // Queued to seed
            6 => "⬆️",   // Seeding
            _ => "❓",
        };

        let size_str = SizeFormatterSI::new(torrent.total_size as u64).to_string();
        
        status.push_str(&format!(
            "{} {} ({}%)\n  Size: {}, Downloaded: {}, Uploaded: {}\n",
            status_emoji,
            torrent.name,
            percent,
            size_str,
            SizeFormatterSI::new(torrent.downloaded_ever as u64).to_string(),
            SizeFormatterSI::new(torrent.uploaded_ever as u64).to_string()
        ));
    }

    Ok(status)
}

fn format_torrent_list(torrents: &[Torrent], filter: Option<Media>) -> (String, Vec<i64>) {
    let mut list = String::new();
    let mut ids = Vec::new();

    let tv_path = env::var("TRANSMISSION_TV_PATH").unwrap_or_default();
    let movie_path = env::var("TRANSMISSION_MOVIE_PATH").unwrap_or_default();

    let mut number = 1;
    for torrent in torrents {
        let media_type = get_media_type_from_path(&torrent.download_dir, &tv_path, &movie_path);

        if let Some(filter_media) = &filter {
            if media_type.as_ref() != Some(filter_media) {
                continue;
            }
        }

        let media_label = match media_type {
            Some(Media::TV) => "📺 TV",
            Some(Media::Movie) => "🎬 Movie",
            None => "📁 Unknown",
        };

        let percent = (torrent.percent_done * 100.0) as i64;

        list.push_str(&format!(
            "{}. {} - {} ({}%)\n",
            number, media_label, torrent.name, percent
        ));
        ids.push(torrent.id);
        number += 1;
    }

    if list.is_empty() {
        list = "No downloads found".to_string();
    } else {
        list.insert_str(0, "Reply with the number to delete (torrent):\n\n");
    }

    (list, ids)
}

async fn dispatch_delete_list(filter: Option<Media>) -> Result<(String, Vec<i64>), String> {
    let torrents = get_torrents().await?;
    Ok(format_torrent_list(&torrents, filter))
}

async fn dispatch_delete(
    index: usize,
    torrent_ids: Vec<i64>,
) -> Result<String, String> {
    if index == 0 || index > torrent_ids.len() {
        return Err("Invalid index".to_string());
    }

    let id = torrent_ids[index - 1];
    delete_torrent(vec![id]).await?;

    Ok("🗑️ Torrent deleted".to_string())
}

async fn dispatch_stop_seed() -> Result<String, String> {
    stop_seeding_all().await?;
    Ok("⏹️ Stopped seeding for all downloads".to_string())
}

async fn dispatch_storage() -> Result<String, String> {
    get_storage_info()
}

fn list_files_in_directory(dir_path: &str) -> Result<Vec<String>, String> {
    use std::fs;
    
    let entries = fs::read_dir(dir_path)
        .map_err(|e| format!("Failed to read directory {}: {}", dir_path, e))?;
    
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        
        if path.is_dir() || path.is_file() {
            if let Some(file_name) = path.file_name() {
                if let Some(name_str) = file_name.to_str() {
                    // Skip hidden files
                    if !name_str.starts_with('.') {
                        files.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    
    // Sort files alphabetically
    files.sort();
    Ok(files)
}

fn format_file_list(files: &[String], _base_path: &str) -> (String, Vec<String>) {
    let mut list = String::new();
    let mut paths = Vec::new();

    let mut number = 1;
    for file_path in files {
        // Get just the file/folder name
        let display_name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);
        
        list.push_str(&format!(
            "{}. {}\n",
            number, display_name
        ));
        paths.push(file_path.clone());
        number += 1;
    }

    if list.is_empty() {
        list = "No files found".to_string();
    } else {
        list.insert_str(0, "Reply with the number to delete (file):\n\n");
    }

    (list, paths)
}

async fn dispatch_delete_file_list(media: Media) -> Result<(String, Vec<String>), String> {
    let path = match media {
        Media::TV => env::var("TRANSMISSION_TV_PATH")
            .map_err(|_| "TRANSMISSION_TV_PATH env var is not set".to_string())?,
        Media::Movie => env::var("TRANSMISSION_MOVIE_PATH")
            .map_err(|_| "TRANSMISSION_MOVIE_PATH env var is not set".to_string())?,
    };

    let files = list_files_in_directory(&path)?;
    Ok(format_file_list(&files, &path))
}

async fn dispatch_delete_file(
    index: usize,
    file_paths: Vec<String>,
) -> Result<String, String> {
    use std::fs;
    use std::path::Path;
    
    if index == 0 || index > file_paths.len() {
        return Err("Invalid index".to_string());
    }

    let file_path = &file_paths[index - 1];
    let path = Path::new(file_path);

    if path.is_dir() {
        fs::remove_dir_all(path)
            .map_err(|e| format!("Failed to delete directory: {}", e))?;
        Ok(format!("🗑️ Directory deleted: {}", path.file_name().unwrap_or_default().to_string_lossy()))
    } else if path.is_file() {
        fs::remove_file(path)
            .map_err(|e| format!("Failed to delete file: {}", e))?;
        Ok(format!("🗑️ File deleted: {}", path.file_name().unwrap_or_default().to_string_lossy()))
    } else {
        Err("Path does not exist".to_string())
    }
}

pub async fn send_message(api: &Api, message: &Message, text: String) -> Result<Option<MessageId>, ()> {
    let mut reply = message.text_reply(text);

    let result = api.send(reply.parse_mode(ParseMode::Html)).await;
    match result {
        Ok(sent_msg) => {
            use telegram_bot::MessageOrChannelPost;
            let msg_id = match sent_msg {
                MessageOrChannelPost::Message(m) => m.id,
                MessageOrChannelPost::ChannelPost(cp) => cp.id,
            };
            println!("Reply sent with id: {:?}", msg_id);
            Ok(Some(msg_id))
        }
        Err(err) => {
            println!("Error when sending telegram message: {}", err);
            Ok(None)
        }
    }
}
// Holds a pending list to be stored after message is sent and message ID is known
enum PendingList {
    Torrent(Vec<i64>),
    File(Vec<String>),
    Restructure(crate::restructure::RestructurePlan),
}

async fn add_response(
    response: Result<TelegramJackettResponse, String>,
    responses: &mut Arc<Mutex<Vec<TelegramJackettResponse>>>,
) -> Result<String, String> {
    match response {
        Ok(response) => {
            let mut r = responses.lock().await;

            let reply_text = format_telegram_response(response.clone());
            r.push(response);
            Ok(reply_text)
        }
        Err(err) => Err(err),
    }
}

async fn add_torrent_list(
    text: String,
    torrent_ids: Vec<i64>,
    torrent_lists: &mut Arc<Mutex<Vec<(Vec<i64>, String, MessageId)>>>,
    message_id: MessageId,
) -> String {
    let mut lists = torrent_lists.lock().await;
    lists.push((torrent_ids, text.clone(), message_id));
    // Keep only last 100 lists to avoid memory issues
    if lists.len() > 100 {
        lists.remove(0);
    }
    text
}

async fn add_file_list(
    text: String,
    file_paths: Vec<String>,
    file_lists: &mut Arc<Mutex<Vec<(Vec<String>, String, MessageId)>>>,
    message_id: MessageId,
) -> String {
    let mut lists = file_lists.lock().await;
    lists.push((file_paths, text.clone(), message_id));
    // Keep only last 100 lists to avoid memory issues
    if lists.len() > 100 {
        lists.remove(0);
    }
    text
}

async fn add_restructure_plan(
    text: String,
    plan: crate::restructure::RestructurePlan,
    plans: &mut Arc<Mutex<Vec<(crate::restructure::RestructurePlan, String, MessageId)>>>,
    message_id: MessageId,
) -> String {
    let mut p = plans.lock().await;
    p.push((plan, text.clone(), message_id));
    // Keep only last 100 plans to avoid memory issues
    if p.len() > 100 {
        p.remove(0);
    }
    text
}

fn transmission_path(env_var: String) -> Result<String, String> {
    env::var(&env_var).map_err(|_| format!("{} env var is not set", env_var))
}

pub async fn handle_message(
    api: &Api,
    message: &Message,
    text: Vec<String>,
    responses: &mut Arc<Mutex<Vec<TelegramJackettResponse>>>,
    torrent_lists: &mut Arc<Mutex<Vec<(Vec<i64>, String, MessageId)>>>,
    file_lists: &mut Arc<Mutex<Vec<(Vec<String>, String, MessageId)>>>,
    restructure_plans: &mut Arc<Mutex<Vec<(crate::restructure::RestructurePlan, String, MessageId)>>>,
) -> Result<(), ()> {
    let chat_id = message.chat.id();
    let mut result: Result<String, String> = Err("🤷🏻‍I didn't get it!".to_string());
    let mut pending_list: Option<PendingList> = None;

    let prefix = text.first().unwrap();
    let suffix = text.last().unwrap();

    if prefix.as_str() == "/chat-id" {
        result = dispatch_chat_id(message.clone()).await;
    }

    if allowed_groups().is_empty() || allowed_groups().contains(&chat_id) {
        if let Some(reply) = message.reply_to_message.clone() {
            let num: Option<u16>;
            let mut media: Option<Media> = None;

            match prefix.as_str() {
                "tv" => {
                    media = Some(Media::TV);
                    num = suffix.parse::<u16>().ok();
                }
                "movie" => {
                    media = Some(Media::Movie);
                    num = suffix.parse::<u16>().ok();
                }
                _ => {
                    num = prefix.parse::<u16>().ok();
                }
            }

            if let Some(num) = num {
                if let Some(reply_text) = reply.text() {
                    let mut matched = false;
                    
                    // 1) check FILE lists only if reply_text exactly matches a stored FILE list text
                    {
                        let file_lists_guard = file_lists.lock().await;
                        for (file_paths, _list_text, stored_id) in file_lists_guard.iter() {
                            let reply_msg_id = match *reply {
                                telegram_bot::MessageOrChannelPost::Message(ref m) => m.id,
                                telegram_bot::MessageOrChannelPost::ChannelPost(ref cp) => cp.id,
                            };
                            if reply_msg_id == *stored_id {
                                let paths = file_paths.clone();
                                drop(file_lists_guard);
                                result = dispatch_delete_file(num as usize, paths).await;
                                matched = true;
                                break;
                            }
                        }
                    }

                    // 2) if not matched, check TORRENT lists only if reply_text exactly matches a stored TORRENT list text
                    if !matched {
                        let lists = torrent_lists.lock().await;
                        for (torrent_ids, _list_text, stored_id) in lists.iter() {
                            let reply_msg_id = match *reply {
                                telegram_bot::MessageOrChannelPost::Message(ref m) => m.id,
                                telegram_bot::MessageOrChannelPost::ChannelPost(ref cp) => cp.id,
                            };
                            if reply_msg_id == *stored_id {
                                result = dispatch_delete(num as usize, torrent_ids.clone()).await;
                                matched = true;
                                break;
                            }
                        }
                        drop(lists);
                    }

                    // 3) if not matched, check RESTRUCTURE plans
                    if !matched {
                        let restructure_guard = restructure_plans.lock().await;
                        for (plan, _list_text, stored_id) in restructure_guard.iter() {
                            let reply_msg_id = match *reply {
                                telegram_bot::MessageOrChannelPost::Message(ref m) => m.id,
                                telegram_bot::MessageOrChannelPost::ChannelPost(ref cp) => cp.id,
                            };
                            if reply_msg_id == *stored_id {
                                // Check for cancel
                                if prefix.to_lowercase().trim() == "cancel" {
                                    result = Ok("❌ Restructure cancelled".to_string());
                                    matched = true;
                                    break;
                                }

                                // Parse reply and execute
                                let full_reply = text.join(" ");
                                match crate::restructure::parse_restructure_reply(&full_reply, plan) {
                                    Ok(operations) => {
                                        drop(restructure_guard);
                                        result = crate::restructure::execute_moves(&operations).await;
                                        matched = true;
                                    }
                                    Err(e) => {
                                        result = Err(e);
                                        matched = true;
                                    }
                                }
                                break;
                            }
                        }
                    }

                    // If not a delete reply, try Jackett response
                    if !matched {
                        let r = responses.lock().await;
                        result = pick_choices(num, reply_text, r.clone(), media).await;
                    }
                }
            } else {
                result = Err(
                    "Not a number.\nPossible solutions: (index), movie (index) or tv (index) "
                        .to_string(),
                )
            }
        }

        // TODO: Move to const
        let imdb_url = "https://www.imdb.com";
        if prefix.starts_with(imdb_url)
            || suffix.starts_with(imdb_url)
            || (prefix == "/imdb" || suffix.starts_with(imdb_url))
        {
            let mut url = suffix;

            if prefix.starts_with(imdb_url) {
                url = prefix;
            }

            let response = dispatch_from_imdb_url(url.clone()).await;
            result = add_response(response, responses).await;
        };

        result = match prefix.as_str() {
            "/torrent-tv" => dispatch_tv(text).await,
            "/torrent-movie" => dispatch_movie(text).await,
            "/help" => Ok(HELP.to_string()),
            "/search" => {
                let response = dispatch_search(text).await;
                add_response(response, responses).await
            }
            "/status" => dispatch_status().await,
            "/delete-torrent" => {
                match dispatch_delete_list(None).await {
                    Ok((text, ids)) => {
                        pending_list = Some(PendingList::Torrent(ids));
                        Ok(text)
                    }
                    Err(e) => Err(e),
                }
            }
            "/delete-tv" => {
                match dispatch_delete_file_list(Media::TV).await {
                    Ok((text, paths)) => {
                        pending_list = Some(PendingList::File(paths));
                        Ok(text)
                    }
                    Err(e) => Err(e),
                }
            }
            "/delete-movie" => {
                match dispatch_delete_file_list(Media::Movie).await {
                    Ok((text, paths)) => {
                        pending_list = Some(PendingList::File(paths));
                        Ok(text)
                    }
                    Err(e) => Err(e),
                }
            }
            "/restructure" => {
                if text.len() < 2 {
                    Err("Usage: /restructure <tv|movie>".to_string())
                } else {
                    let media = match text[1].to_lowercase().as_str() {
                        "tv" => Some(Media::TV),
                        "movie" => Some(Media::Movie),
                        _ => None,
                    };

                    match media {
                        Some(m) => {
                            let actual_env_var = match m {
                                Media::TV => "ACTUAL_TV_PATH",
                                Media::Movie => "ACTUAL_MOVIE_PATH",
                            };
                            let transmission_env_var = match m {
                                Media::TV => "TRANSMISSION_TV_PATH".to_string(),
                                Media::Movie => "TRANSMISSION_MOVIE_PATH".to_string(),
                            };

                            let base_path_result = env::var(actual_env_var)
                                .ok()
                                .map(Ok)
                                .unwrap_or_else(|| transmission_path(transmission_env_var));

                            match base_path_result {
                                Ok(base_path) => {
                                    match crate::restructure::generate_restructure_plan(m, &base_path).await {
                                        Ok(plan) => {
                                            if plan.operations.is_empty() && plan.unparseable_files.is_empty() {
                                                Ok("✅ Nothing to restructure".to_string())
                                            } else {
                                                let text = crate::restructure::format_restructure_plan(&plan);
                                                pending_list = Some(PendingList::Restructure(plan));
                                                Ok(text)
                                            }
                                        }
                                        Err(e) => Err(e),
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        }
                        None => Err("Invalid media type. Use 'tv' or 'movie'".to_string()),
                    }
                }
            }
            "/stop-seed" => dispatch_stop_seed().await,
            "/storage" => dispatch_storage().await,
            _ => result,
        };
    }

    println!("{:?}", result);
    match result {
        Ok(text) => {
            if !text.is_empty() {
                if let Ok(sent_id_opt) = send_message(api, message, text.clone()).await {
                    if let (Some(sent_id), Some(pending)) = (sent_id_opt, pending_list) {
                        match pending {
                            PendingList::Torrent(ids) => {
                                // store mapping for replies to this message
                                let _ = add_torrent_list(text, ids, torrent_lists, sent_id).await;
                            }
                            PendingList::File(paths) => {
                                let _ = add_file_list(text, paths, file_lists, sent_id).await;
                            }
                            PendingList::Restructure(plan) => {
                                let _ = add_restructure_plan(text, plan, restructure_plans, sent_id).await;
                            }
                        }
                    }
                }
            }
        }
        Err(text) => {
            let _ = send_message(api, message, format!("❌ {}", text.clone())).await?;
        }
    };
    Ok(())
}
