//! Message-bound native picker; sessions are bounded, expiring and requester-owned.
use std::{sync::Arc, time::{Duration, Instant}};
use telegram_bot::{prelude::*, Api, CallbackQuery, ChatId, EditMessageText,
    InlineKeyboardButton as Button, InlineKeyboardMarkup, Message, MessageId, UserId, ToMessageId};
use tokio::sync::Mutex;
use size_format::SizeFormatterSI;
use crate::{jackett::TelegramJackettResponse, transmission::{add_torrent, Media}};

const PAGE_SIZE: usize = 5;
const TTL: Duration = Duration::from_secs(3600);
#[derive(Clone, Default)]
pub struct PickerStore(Arc<Mutex<Vec<Entry>>>);
struct Entry {
    chat: ChatId,
    message: MessageId,
    created: Instant,
    session: Arc<Mutex<Session>>,
}
struct Session {
    owner: UserId,
    response: TelegramJackettResponse,
    page: usize,
    selected: Option<usize>,
    done: bool,
    cancelled: bool,
}
#[derive(Debug, PartialEq)]
enum Action { Page(usize), Select(usize), Download(usize, Media), Cancel }
fn parse(data: &str) -> Option<Action> {
    let parts: Vec<_> = data.split(':').collect();
    match parts.as_slice() {
        ["pick", "cancel"] => Some(Action::Cancel),
        ["pick", "page", n] => Some(Action::Page(n.parse().ok()?)),
        ["pick", "select", n] => Some(Action::Select(n.parse().ok()?)),
        ["pick", "movie", n] => Some(Action::Download(n.parse().ok()?, Media::Movie)),
        ["pick", "tv", n] => Some(Action::Download(n.parse().ok()?, Media::TV)),
        _ => None,
    }
}
fn short(s: &str, count: usize) -> String {
    let mut result: String = s.chars().take(count).collect();
    if s.chars().count() > count { result.push('…'); }
    result
}
fn button(label: &str, action: &str, n: usize) -> Button {
    Button::callback(label, format!("pick:{}:{}", action, n))
}
impl Session {
    fn view(&self) -> (String, InlineKeyboardMarkup) {
        let mut keyboard = InlineKeyboardMarkup::new();
        if self.cancelled { return ("Search dismissed. Use /search to start again.".into(), keyboard); }
        if let Some(index) = self.selected {
            let torrent = &self.response.torrents[index];
            let detected = torrent.media();
            let movie = if detected == Some(Media::Movie) { "🎬 Download as Movie ✓ recommended" } else { "🎬 Download as Movie" };
            let tv = if detected == Some(Media::TV) { "📺 Download as TV ✓ recommended" } else { "📺 Download as TV" };
            keyboard.add_row(vec![button(movie, "movie", index)]);
            keyboard.add_row(vec![button(tv, "tv", index)]);
            keyboard.add_row(vec![button("‹ Back to results", "page", self.page)]);
            keyboard.add_row(vec![Button::callback("✖ Cancel", "pick:cancel")]);
            return (format!("{}\n{}B · {} seeders\n\nChoose the download type:", short(&torrent.title, 240), SizeFormatterSI::new(torrent.size), torrent.seeders), keyboard);
        }
        let total = self.response.torrents.len();
        let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut text = format!("Search results · page {}/{} · {} results\n\n", self.page + 1, pages, total);
        for (index, torrent) in self.response.torrents.iter().enumerate().skip(self.page * PAGE_SIZE).take(PAGE_SIZE) {
            text.push_str(&format!("{}. {}\n{}B · {} seeders\n\n", index + 1, short(&torrent.title, 120), SizeFormatterSI::new(torrent.size), torrent.seeders));
            keyboard.add_row(vec![button(&format!("{}. {}", index + 1, short(&torrent.title, 48)), "select", index)]);
        }
        let mut navigation = vec![];
        if self.page > 0 { navigation.push(button("‹ Previous", "page", self.page - 1)); }
        if self.page + 1 < pages { navigation.push(button("Next ›", "page", self.page + 1)); }
        if !navigation.is_empty() { keyboard.add_row(navigation); }
        keyboard.add_row(vec![Button::callback("✖ Cancel", "pick:cancel")]);
        text.push_str("Tap a result, or reply with its number (movie N / tv N to override).");
        (text, keyboard)
    }
    fn authorize(&self, user: UserId) -> Result<(), &'static str> {
        if self.owner != user { return Err("This picker belongs to the person who searched."); }
        if self.cancelled { return Err("Search dismissed. Run /search again."); }
        if self.done { return Err("Already added. Run /search again for another download."); }
        Ok(())
    }
    fn navigate(&mut self, action: Action) -> Result<(), &'static str> {
        match action {
            Action::Cancel => { self.cancelled = true; self.selected = None; Ok(()) }
            Action::Page(page) if page <= (self.response.torrents.len().saturating_sub(1)) / PAGE_SIZE => {
                self.page = page; self.selected = None; Ok(())
            }
            Action::Select(index) if index < self.response.torrents.len() && index / PAGE_SIZE == self.page => {
                self.selected = Some(index); Ok(())
            }
            _ => Err("That button is outdated. Choose a visible result."),
        }
    }
}
async fn edit(api: &Api, chat: ChatId, message: MessageId, text: String, keyboard: InlineKeyboardMarkup) -> Result<(), telegram_bot::Error> {
    api.send(EditMessageText::new(chat, message, text).reply_markup(keyboard)).await?;
    Ok(())
}
impl PickerStore {
    async fn find(&self, chat: ChatId, message: MessageId) -> Option<Arc<Mutex<Session>>> {
        let mut entries = self.0.lock().await;
        entries.retain(|e| e.created.elapsed() < TTL);
        entries.iter().find(|e| e.chat == chat && e.message == message).map(|e| e.session.clone())
    }
    pub async fn send(&self, api: &Api, message: &Message, response: TelegramJackettResponse) -> Result<(), telegram_bot::Error> {
        let session = Session { owner: message.from.id, response, page: 0, selected: None, done: false, cancelled: false };
        let (text, keyboard) = session.view();
        // Keep lookup blocked until the returned message ID is registered.
        let mut entries = self.0.lock().await;
        let sent = api.send(message.text_reply(text).reply_markup(keyboard)).await?;
        entries.retain(|e| e.created.elapsed() < TTL);
        if entries.len() >= 100 { entries.remove(0); }
        entries.push(Entry { chat: message.chat.id(), message: sent.to_message_id(), created: Instant::now(), session: Arc::new(Mutex::new(session)) });
        Ok(())
    }
    pub async fn callback(&self, api: &Api, query: CallbackQuery) {
        let action = query.data.as_deref().and_then(parse);
        let message = match &query.message {
            Some(telegram_bot::MessageOrChannelPost::Message(m)) => m,
            _ => { let _ = api.send(query.answer("Run /search again.")).await; return; }
        };
        let groups = crate::telegram::allowed_groups();
        if !groups.is_empty() && !groups.contains(&message.chat.id()) {
            let _ = api.send(query.answer("This chat is not allowed.")).await; return;
        }
        let session = match self.find(message.chat.id(), message.id).await {
            Some(s) => s,
            None => { let _ = api.send(query.answer("Search expired or bot restarted. Run /search again.")).await; return; }
        };
        let mut session = match session.try_lock() {
            Ok(s) => s,
            Err(_) => { let _ = api.send(query.answer("Still processing your previous tap.")).await; return; }
        };
        if let Err(e) = session.authorize(query.from.id) { let _ = api.send(query.answer(e)).await; return; }
        match action {
            Some(Action::Cancel) => {
                let _ = api.send(query.acknowledge()).await;
                match edit(api, message.chat.id(), message.id, "Search dismissed. Use /search to start again.".into(), InlineKeyboardMarkup::new()).await {
                    Ok(()) => { let _ = session.navigate(Action::Cancel); }
                    Err(e) => eprintln!("Picker cancel edit failed: {}", e),
                }
            }
            Some(Action::Download(index, media)) if session.selected == Some(index) => {
                let _ = api.send(query.acknowledge()).await;
                self.download(api, message.chat.id(), message.id, &mut session, index, media).await;
            }
            Some(action) => {
                if let Err(e) = session.navigate(action) { let _ = api.send(query.answer(e)).await; return; }
                let _ = api.send(query.acknowledge()).await;
                let (text, keyboard) = session.view();
                if let Err(e) = edit(api, message.chat.id(), message.id, text, keyboard).await { eprintln!("Picker edit failed: {}", e); }
            }
            None => { let _ = api.send(query.answer("Invalid button. Run /search again.")).await; }
        }
    }
    async fn download(&self, api: &Api, chat: ChatId, message: MessageId, session: &mut Session, index: usize, media: Media) {
        let torrent = &session.response.torrents[index];
        let title = short(&torrent.title, 240);
        let result = match torrent.location().await {
            Ok(location) => add_torrent(location, media.clone()).await,
            Err(e) => Err(e),
        };
        let (text, keyboard) = match result {
            Ok(_) => {
                session.done = true;
                let label = if media == Media::TV { "TV" } else { "Movie" };
                (format!("🧲 Added as {}\n{}", label, title), InlineKeyboardMarkup::new())
            }
            Err(e) => {
                session.selected = Some(index);
                let (text, keyboard) = session.view();
                (format!("❌ {}\n\n{}", short(&e, 240), text), keyboard)
            }
        };
        if let Err(e) = edit(api, chat, message, text.clone(), keyboard).await {
            eprintln!("Picker download-result edit failed: {}", e);
            let _ = api.send(telegram_bot::SendMessage::new(chat, text)).await;
        }
    }
    /// Typed replies use the message ID, never title/text matching.
    pub async fn reply(&self, api: &Api, message: &Message, text: &[String]) -> bool {
        let reply = match &message.reply_to_message { Some(r) => r, None => return false };
        let session = match self.find(message.chat.id(), reply.to_message_id()).await { Some(s) => s, None => return false };
        let mut session = session.lock().await;
        let result = session.authorize(message.from.id);
        if let Err(e) = result { let _ = api.send(message.text_reply(e)).await; return true; }
        let (number, forced) = match text {
            [n] => (n.parse::<usize>().ok(), None),
            [kind, n] if kind == "movie" => (n.parse::<usize>().ok(), Some(Media::Movie)),
            [kind, n] if kind == "tv" => (n.parse::<usize>().ok(), Some(Media::TV)),
            _ => (None, None),
        };
        let index = match number.and_then(|n| n.checked_sub(1)).filter(|&n| n < session.response.torrents.len()) {
            Some(n) => n,
            None => { let _ = api.send(message.text_reply("Reply with a valid result number, movie N or tv N.")).await; return true; }
        };
        let media = forced.or_else(|| session.response.torrents[index].media());
        match media {
            Some(media) => self.download(api, message.chat.id(), reply.to_message_id(), &mut session, index, media).await,
            None => {
                session.page = index / PAGE_SIZE;
                session.selected = Some(index);
                let (text, keyboard) = session.view();
                let _ = edit(api, message.chat.id(), reply.to_message_id(), text, keyboard).await;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(count: usize) -> Session {
        let torrents = (0..count).map(|n| crate::jackett::Torrent {
            title: format!("{}{}", n, "🦉<&>".repeat(1000)), seeders: 10, size: 1000,
            categories: vec![if n % 2 == 0 { 2000 } else { 5000 }],
            magnet_uri: Some("magnet:test".into()), torrent_url: None,
        }).collect();
        Session { owner: UserId::new(1), response: TelegramJackettResponse { torrents }, page: 0, selected: None, done: false, cancelled: false }
    }
    #[test]
    fn pagination_bounded_text_and_callback_data() {
        let mut s = session(20);
        for page in 0..4 {
            s.navigate(Action::Page(page)).unwrap();
            let (text, keyboard) = s.view();
            assert!(text.len() < 3800);
            let json = serde_json::to_value(keyboard).unwrap();
            let rows = json["inline_keyboard"].as_array().unwrap();
            assert_eq!(rows.len(), 7);
            for row in rows { for b in row.as_array().unwrap() {
                let data = b["callback_data"].as_str().unwrap();
                assert!(data.len() <= 64); assert!(parse(data).is_some());
            }}
        }
        assert!(s.navigate(Action::Page(4)).is_err());
        assert!(s.navigate(Action::Page(usize::MAX)).is_err());
    }
    #[test]
    fn recommendation_back_and_stale_selection() {
        let mut s = session(6);
        assert!(s.navigate(Action::Select(5)).is_err());
        s.navigate(Action::Select(0)).unwrap();
        let (_, keyboard) = s.view();
        let json = serde_json::to_value(keyboard).unwrap();
        assert!(json["inline_keyboard"][0][0]["text"].as_str().unwrap().contains("recommended"));
        s.navigate(Action::Page(1)).unwrap();
        assert_eq!(s.selected, None);
        s.navigate(Action::Select(5)).unwrap();
        let (_, keyboard) = s.view();
        let json = serde_json::to_value(keyboard).unwrap();
        assert!(json["inline_keyboard"][1][0]["text"].as_str().unwrap().contains("recommended"));
        assert!(s.navigate(Action::Select(6)).is_err());
    }
    #[test]
    fn ownership_completed_and_malformed_actions() {
        let mut s = session(1);
        assert!(s.authorize(UserId::new(2)).is_err());
        assert!(s.authorize(UserId::new(1)).is_ok());
        s.done = true;
        assert!(s.authorize(UserId::new(1)).is_err());
        for data in ["pick:page:-1", "pick:tv", "pick:movie:0:extra", "wrong:page:0", "pick:select:no"] { assert_eq!(parse(data), None); }
        assert_eq!(parse("pick:tv:0"), Some(Action::Download(0, Media::TV)));
    }
    #[test]
    fn cancel_on_both_screens_closes_session_and_clears_keyboard() {
        let mut s = session(6);
        assert_eq!(parse("pick:cancel"), Some(Action::Cancel));
        assert_eq!(parse("pick:cancel:0"), None);
        for selected in [None, Some(0)] {
            s.selected = selected;
            let (_, keyboard) = s.view();
            let json = serde_json::to_value(keyboard).unwrap();
            let rows = json["inline_keyboard"].as_array().unwrap();
            assert_eq!(rows.last().unwrap()[0]["callback_data"], "pick:cancel");
        }
        s.navigate(Action::Cancel).unwrap();
        assert!(s.authorize(UserId::new(1)).is_err());
        assert!(s.selected.is_none());
        let (text, keyboard) = s.view();
        assert!(text.contains("dismissed"));
        assert!(serde_json::to_value(keyboard).unwrap()["inline_keyboard"].as_array().unwrap().is_empty());
    }
    #[tokio::test]
    async fn sessions_expire_and_are_chat_bound() {
        let store = PickerStore::default();
        store.0.lock().await.push(Entry { chat: ChatId::new(1), message: MessageId::new(1), created: Instant::now(), session: Arc::new(Mutex::new(session(1))) });
        assert!(store.find(ChatId::new(2), MessageId::new(1)).await.is_none());
        assert!(store.find(ChatId::new(1), MessageId::new(1)).await.is_some());
        store.0.lock().await[0].created = Instant::now() - TTL;
        assert!(store.find(ChatId::new(1), MessageId::new(1)).await.is_none());
    }
    #[tokio::test]
    #[ignore = "requires TELEGRAM_BOT_TOKEN and PICKER_TEST_CHAT; sends and deletes a temporary UI check"]
    async fn live_telegram_keyboard_send_edit_and_cleanup() {
        let api = Api::new(std::env::var("TELEGRAM_BOT_TOKEN").unwrap());
        let chat = ChatId::new(std::env::var("PICKER_TEST_CHAT").unwrap().parse().unwrap());
        let mut s = session(6);
        for (i, torrent) in s.response.torrents.iter_mut().enumerate() {
            torrent.title = format!("Picker UI verification (no download) · result {}", i + 1);
        }
        let (text, keyboard) = s.view();
        let sent = api.send(telegram_bot::SendMessage::new(chat, text).reply_markup(keyboard)).await.expect("Telegram send failed");
        let id = sent.to_message_id();
        let result: Result<(), telegram_bot::Error> = async {
            s.navigate(Action::Page(1)).unwrap();
            let (text, keyboard) = s.view();
            edit(&api, chat, id, text, keyboard).await?;
            s.navigate(Action::Select(5)).unwrap();
            let (text, keyboard) = s.view();
            edit(&api, chat, id, text, keyboard).await?;
            s.navigate(Action::Page(1)).unwrap();
            let (text, keyboard) = s.view();
            edit(&api, chat, id, text, keyboard).await?;
            s.navigate(Action::Cancel).unwrap();
            let (text, keyboard) = s.view();
            edit(&api, chat, id, text, keyboard).await?;
            Ok(())
        }.await;
        api.send(telegram_bot::DeleteMessage::new(chat, id)).await.expect("Temporary UI cleanup failed");
        assert!(result.is_ok(), "Telegram picker edit failed");
    }

}
