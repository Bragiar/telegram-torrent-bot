//! Requester-owned, paginated permanent filesystem deletion.
use std::{path::PathBuf, sync::Arc, time::{Duration, Instant}};
use telegram_bot::{prelude::*, Api, CallbackQuery, ChatId, EditMessageText, InlineKeyboardButton as Button, InlineKeyboardMarkup, Message, MessageId, ToMessageId, UserId};
use tokio::sync::Mutex;
use crate::transmission::Media;
const PAGE: usize = 5;
const TTL: Duration = Duration::from_secs(3600);
#[derive(Clone, Default)]
pub struct DeleteStore(Arc<Mutex<Vec<Entry>>>);
struct Entry { chat: ChatId, message: MessageId, created: Instant, session: Arc<Mutex<Session>> }
#[derive(Clone)]
struct Item { path: PathBuf, root: PathBuf, name: String }
#[derive(Clone)]
struct Session { owner: UserId, items: Vec<Item>, page: usize, selected: Option<usize>, closed: bool, label: &'static str }
#[derive(Debug, PartialEq)]
enum Action { Page(usize), Select(usize), Delete(usize), Cancel }
fn parse(s: &str) -> Option<Action> {
    let p: Vec<_> = s.split(':').collect();
    match p.as_slice() {
        ["del", "cancel"] => Some(Action::Cancel),
        ["del", "page", n] => Some(Action::Page(n.parse().ok()?)),
        ["del", "select", n] => Some(Action::Select(n.parse().ok()?)),
        ["del", "delete", n] => Some(Action::Delete(n.parse().ok()?)),
        _ => None,
    }
}
fn short(s: &str, n: usize) -> String { let mut t: String = s.chars().take(n).collect(); if s.chars().count()>n {t.push('…');} t }
fn button(s: &str, a: &str, n: usize) -> Button { Button::callback(s, format!("del:{}:{}",a,n)) }
pub(crate) fn roots(media: &Media) -> Result<Vec<PathBuf>, String> {
    let (actual, transmission, key) = match media { Media::TV => ("ACTUAL_TV_PATH", "TRANSMISSION_TV_PATH", "tv"), Media::Movie => ("ACTUAL_MOVIE_PATH", "TRANSMISSION_MOVIE_PATH", "movies") };
    let config = match std::fs::read_to_string("media-roots.json") {
        Ok(s) => Some(serde_json::from_str::<serde_json::Value>(&s).map_err(|e| format!("Invalid media-roots.json: {}",e))?),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let mut paths = vec![];
    if let Some(p) = std::env::var(actual).ok().filter(|p| !p.is_empty()).or_else(||config.as_ref()?.get(key)?.as_str().map(str::to_owned)) { paths.push(PathBuf::from(p)); }
    if let Ok(p) = std::env::var(transmission) { if !p.is_empty() { paths.push(PathBuf::from(p)); } }
    if paths.is_empty() { return Err(format!("{} is not configured",transmission)); }
    Ok(paths)
}
fn scan(roots: &[PathBuf]) -> Result<Vec<Item>, String> {
    let mut items = vec![];
    let mut seen = vec![];
    for root in roots {
        let root = root.canonicalize().map_err(|e|format!("Cannot access {}: {}",root.display(),e))?;
        if seen.contains(&root) {continue;} seen.push(root.clone());
        for entry in std::fs::read_dir(&root).map_err(|e|e.to_string())? {
            let entry = entry.map_err(|e|e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            let kind = entry.file_type().map_err(|e|e.to_string())?;
            if !name.starts_with('.') && (kind.is_dir() || kind.is_file()) { items.push(Item {path: entry.path(), root: root.clone(), name}); }
        }
    }
    items.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.name.cmp(&b.name)).then(a.path.cmp(&b.path)));
    Ok(items)
}
fn remove(item: &Item) -> Result<(), String> {
    // Refuse roots, moved entries, and symlinks (including replaced parent directories).
    if item.path.parent()!=Some(item.root.as_path()) || item.root.canonicalize().map_err(|e|e.to_string())? != item.root {return Err("Library path changed; reopen the delete command.".into());}
    let meta = std::fs::symlink_metadata(&item.path).map_err(|e|e.to_string())?;
    if meta.file_type().is_symlink() {return Err("Refusing to delete a symbolic link.".into());}
    if meta.is_dir() {std::fs::remove_dir_all(&item.path)} else {std::fs::remove_file(&item.path)}.map_err(|e|e.to_string())
}
impl Session {
    fn authorize(&self, user: UserId) -> Result<(), &'static str> {if user!=self.owner {Err("This list belongs to its requester.")} else if self.closed {Err("List closed. Run the delete command again.")} else {Ok(())}}
    fn navigate(&mut self, action: Action) -> Result<(), &'static str> {
        match action {
            Action::Page(p) if p <= self.items.len().saturating_sub(1)/PAGE => {self.page=p;self.selected=None;Ok(())},
            Action::Select(i) if i<self.items.len() && i/PAGE==self.page => {self.selected=Some(i);Ok(())},
            _ => Err("Outdated button. Choose a visible item."),
        }
    }
    fn view(&self) -> (String, InlineKeyboardMarkup) {
        let mut k = InlineKeyboardMarkup::new();
        if let Some(i)=self.selected {
            let item=&self.items[i];
            k.add_row(vec![button("🗑 Delete permanently", "delete",i)]);
            k.add_row(vec![button("‹ Back", "page",self.page)]);
            k.add_row(vec![Button::callback("✖ Cancel", "del:cancel")]);
            return (format!("Delete {} permanently?\n\n{}\n\n{}\n\nFolders include all files and episodes inside. This cannot be undone.",self.label,short(&item.name,240),short(&item.path.display().to_string(),300)),k);
        }
        let pages=(self.items.len()+PAGE-1)/PAGE;
        let mut text=format!("Delete {} · A–Z · page {}/{} · {} items\n\n",self.label,self.page+1,pages.max(1),self.items.len());
        for (i,item) in self.items.iter().enumerate().skip(self.page*PAGE).take(PAGE) {
            text.push_str(&format!("{}. {}\n",i+1,short(&item.name,120)));
            k.add_row(vec![button(&format!("{}. {}",i+1,short(&item.name,48)),"select",i)]);
        }
        if self.items.is_empty() {text.push_str("No files found in the configured library folders.");}
        let mut nav=vec![];
        if self.page>0 {nav.push(button("‹ Previous","page",self.page-1));}
        if self.page+1<pages {nav.push(button("Next ›","page",self.page+1));}
        if !nav.is_empty(){k.add_row(nav);}
        k.add_row(vec![Button::callback("✖ Cancel","del:cancel")]);
        (text,k)
    }
}
async fn edit(api: &Api, chat: ChatId, msg: MessageId, text: String, k: InlineKeyboardMarkup) -> Result<(),telegram_bot::Error> {api.send(EditMessageText::new(chat,msg,text).reply_markup(k)).await?; Ok(())}
impl DeleteStore {
    pub async fn send(&self, api: &Api, message: &Message, media: Media) -> Result<(),String> {
        let paths=roots(&media)?;
        let items=tokio::task::spawn_blocking(move||scan(&paths)).await.map_err(|e|e.to_string())??;
        let s=Session{owner:message.from.id,items,page:0,selected:None,closed:false,label:if media==Media::TV {"TV"} else {"movies"}};
        let (text,k)=s.view();
        let mut entries=self.0.lock().await;
        let sent=api.send(message.text_reply(text).reply_markup(k)).await.map_err(|e|e.to_string())?;
        entries.retain(|e|e.created.elapsed()<TTL);
        if entries.len()>=100 {entries.remove(0);}
        entries.push(Entry{chat:message.chat.id(),message:sent.to_message_id(),created:Instant::now(),session:Arc::new(Mutex::new(s))}); Ok(())
    }
    pub async fn callback(&self, api: &Api, query: CallbackQuery) {
        let message=match &query.message {Some(telegram_bot::MessageOrChannelPost::Message(m))=>m,_=>{let _=api.send(query.answer("Reopen the delete command.")).await;return;}};
        let groups=crate::telegram::allowed_groups();
        if !groups.is_empty() && !groups.contains(&message.chat.id()){let _=api.send(query.answer("This chat is not allowed.")).await;return;}
        let found={let mut entries=self.0.lock().await;entries.retain(|e|e.created.elapsed()<TTL);entries.iter().find(|e|e.chat==message.chat.id() && e.message==message.id).map(|e|e.session.clone())};
        let Some(s)=found else {let _=api.send(query.answer("List expired or bot restarted. Reopen the delete command.")).await;return;};
        let Ok(mut s)=s.try_lock() else {let _=api.send(query.answer("Still processing your previous tap.")).await;return;};
        if let Err(e)=s.authorize(query.from.id){let _=api.send(query.answer(e)).await;return;}
        let action=query.data.as_deref().and_then(parse);
        let _=api.send(query.acknowledge()).await;
        match action {
            Some(Action::Cancel)=>{if edit(api,message.chat.id(),message.id,"Deletion cancelled.".into(),InlineKeyboardMarkup::new()).await.is_ok(){s.closed=true;}},
            Some(Action::Delete(i)) if s.selected==Some(i) && i<s.items.len()=>{
                let item=s.items[i].clone();let work=item.clone();
                let result=tokio::task::spawn_blocking(move||remove(&work)).await.map_err(|e|e.to_string()).and_then(|r|r);
                let (text,k)=match result {
                    Ok(())=>{s.items.remove(i);s.selected=None;s.page=s.page.min(s.items.len().saturating_sub(1)/PAGE);let (text,k)=s.view();(format!("🗑 Permanently deleted: {}\n\n{}",short(&item.name,120),text),k)},
                    Err(e)=>{let (text,k)=s.view();(format!("Could not delete: {}\n\n{}",short(&e,240),text),k)},
                };
                if edit(api,message.chat.id(),message.id,text.clone(),k).await.is_err(){s.closed=true;let _=api.send(telegram_bot::SendMessage::new(message.chat.id(),text)).await;}
            },
            Some(action)=>{let mut next=s.clone();if next.navigate(action).is_ok(){let (text,k)=next.view();if edit(api,message.chat.id(),message.id,text,k).await.is_ok(){*s=next;}}},
            None=>{},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(n: usize) -> Session {
        Session {owner:UserId::new(1),items:(0..n).map(|i|Item{path:PathBuf::from(format!("/tmp/library/{}",i)),root:PathBuf::from("/tmp/library"),name:"🦉<&>".repeat(1000)}).collect(),page:0,selected:None,closed:false,label:"movies"}
    }
    #[test]
    fn pagination_selection_ownership_and_bounds() {
        let mut s=session(11);
        assert!(s.authorize(UserId::new(2)).is_err());
        assert!(s.navigate(Action::Select(5)).is_err());
        for p in 0..3 {
            s.navigate(Action::Page(p)).unwrap();
            let (text,k)=s.view(); assert!(text.len()<3800);
            let value=serde_json::to_value(k).unwrap();
            for row in value["inline_keyboard"].as_array().unwrap(){for b in row.as_array().unwrap(){let data=b["callback_data"].as_str().unwrap();assert!(data.len()<=64);assert!(parse(data).is_some());}}
        }
        assert!(s.navigate(Action::Page(usize::MAX)).is_err());
        s.navigate(Action::Select(10)).unwrap();
        let (_,k)=s.view(); assert_eq!(serde_json::to_value(k).unwrap()["inline_keyboard"][0][0]["callback_data"],"del:delete:10");
        s.navigate(Action::Page(0)).unwrap(); assert!(s.selected.is_none());
        s.closed=true;assert!(s.authorize(UserId::new(1)).is_err());
        assert!(parse("del:delete:-1").is_none());assert!(parse("del:delete:0:extra").is_none());
    }
    #[test]
    fn filesystem_sort_nested_delete_and_symlink_refusal() {
        let root=std::env::temp_dir().join(format!("delete-picker-test-{}",std::process::id()));
        std::fs::create_dir_all(root.join("beta/Season 01")).unwrap();
        std::fs::write(root.join("beta/Season 01/episode.mkv"),b"test").unwrap();
        std::fs::write(root.join("Alpha.mkv"),b"test").unwrap();
        std::fs::write(root.join(".hidden"),b"test").unwrap();
        #[cfg(unix)] std::os::unix::fs::symlink(root.join("Alpha.mkv"),root.join("link")).unwrap();
        let items=scan(&[root.clone(),root.clone()]).unwrap();
        assert_eq!(items.iter().map(|i|i.name.as_str()).collect::<Vec<_>>(),vec!["Alpha.mkv","beta"]);
        let mut outside=items[0].clone();outside.path=root.clone();assert!(remove(&outside).is_err());
        #[cfg(unix)] {let mut link=items[0].clone();link.path=root.join("link");assert!(remove(&link).is_err());}
        remove(&items[1]).unwrap();assert!(!root.join("beta").exists());assert!(root.join("Alpha.mkv").exists());
        assert!(scan(&[root.join("missing")]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
