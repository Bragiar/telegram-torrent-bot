use std::env;

use futures::lock::Mutex;
use telegram_bot::prelude::*;
use telegram_bot::{Api, ChatId, Message, MessageId, ParseMode};

use std::sync::Arc;

use crate::imdb::get_imdb_info;
use crate::jackett::{
    dispatch_from_reply, request_jackett, TelegramJackettResponse,
    TorrentLocation,
};
use crate::transmission::{
    add_torrent, delete_torrent, get_media_type_from_path, get_storage_info, get_torrents,
    stop_seeding_all, Media, Torrent,
};

const HELP: &str = "
/help - Show commands and usage
/chat_id - Show this chat ID
/torrent_tv (Magnet Link)
/torrent_movie (Magnet Link)
/search (Movie or TV Show e.g. The Matrix or Simpsons s01e01)
/imdb (Imdb link). Requires omdb token set https://www.omdbapi.com/
/status - Show the 10 most recently added torrents
/delete_torrent - List all downloads (reply with number to delete torrent)
/delete_tv - Browse TV alphabetically and permanently delete with buttons
/delete_movie - Browse movies alphabetically and permanently delete with buttons
/restructure <tv|movie> - Scan and reorganize media files
/stop_seed - Stop seeding for all downloads
/storage - Get available storage information

Search results have buttons: tap a result, then Download as Movie or TV.
Use Previous / Next for more results. Pickers expire after one hour.

Typed replies also work:
Position of the torrent
If jackett doesn't provide a category, it's possible to force with:
tv (position)
movie (position)
";

pub(crate) fn allowed_groups() -> Vec<ChatId> {
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
        return Err("Send the magnet-url after command (/torrent_tv magnet_url)".to_string());
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
        return Err("Send the magnet-url after command (/torrent_movie magnet_url)".to_string());
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
    Ok(crate::torrent_status::format_status(&get_torrents().await?))
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
    Restructure(crate::restructure::RestructurePlan),
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

pub async fn handle_message(
    api: &Api,
    message: &Message,
    text: Vec<String>,
    responses: &mut Arc<Mutex<Vec<TelegramJackettResponse>>>,
    torrent_lists: &mut Arc<Mutex<Vec<(Vec<i64>, String, MessageId)>>>,
    restructure_plans: &mut Arc<Mutex<Vec<(crate::restructure::RestructurePlan, String, MessageId)>>>,
    pickers: &crate::search_picker::PickerStore,
    deletes: &crate::delete_picker::DeleteStore,
) -> Result<(), ()> {
    let chat_id = message.chat.id();
    let mut result: Result<String, String> = Err("🤷🏻‍I didn't get it!".to_string());
    let mut pending_list: Option<PendingList> = None;

    let normalized_prefix = crate::command_menu::normalize_command(text.first().map(String::as_str).unwrap_or(""));
    let prefix = &normalized_prefix;
    let suffix = match text.last() { Some(s) => s, None => return Ok(()) };

    if prefix.as_str() == "/chat-id" {
        result = dispatch_chat_id(message.clone()).await;
    }

    if allowed_groups().is_empty() || allowed_groups().contains(&chat_id) {
        if !prefix.starts_with('/') && pickers.reply(api, message, &text).await { return Ok(()); }
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

            // Check restructure plans first — these don't need a numeric index
            let mut restructure_matched = false;
            {
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
                            restructure_matched = true;
                            break;
                        }

                        // Parse reply and execute
                        let full_reply = text.join(" ");
                        match crate::restructure::parse_restructure_reply(&full_reply, plan) {
                            Ok(operations) => {
                                drop(restructure_guard);
                                result = crate::restructure::execute_moves(&operations).await;
                                restructure_matched = true;
                            }
                            Err(e) => {
                                result = Err(e);
                                restructure_matched = true;
                            }
                        }
                        break;
                    }
                }
            }

            if !restructure_matched {
                if let Some(num) = num {
                    if let Some(reply_text) = reply.text() {
                        let mut matched = false;

                        // 2) if not matched, check TORRENT lists
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

                        // 3) If not a delete reply, try Jackett response
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
            match response {
                Ok(response) => { pickers.send(api, message, response).await.map_err(|e| { eprintln!("Picker send failed: {}", e); })?; return Ok(()); }
                Err(e) => result = Err(e),
            }
        };

        result = match prefix.as_str() {
            "/torrent-tv" => dispatch_tv(text).await,
            "/torrent-movie" => dispatch_movie(text).await,
            "/help" => Ok(HELP.to_string()),
            "/search" => {
                let response = dispatch_search(text).await;
                match response {
                    Ok(response) => { pickers.send(api, message, response).await.map_err(|e| { eprintln!("Picker send failed: {}", e); })?; return Ok(()); }
                    Err(e) => Err(e),
                }
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
            "/delete-tv" | "/delete-movie" => {
                let media = if prefix == "/delete-tv" { Media::TV } else { Media::Movie };
                match deletes.send(api, message, media).await {
                    Ok(()) => return Ok(()),
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
                            let base_path_result = crate::delete_picker::roots(&m)
                                .map(|paths| paths[0].to_string_lossy().to_string());

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
