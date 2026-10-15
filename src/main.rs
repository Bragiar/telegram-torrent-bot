use std::env;
use std::process::exit;

use futures::lock::Mutex;
use futures::StreamExt;

mod command_menu;
mod delete_picker;
mod search_picker;
mod imdb;
mod jackett;
mod restructure;
mod telegram;
mod transmission;
mod torrent_status;

use telegram::handle_message;
use telegram_bot::MessageId;

use std::error::Error;
use std::sync::Arc;
use telegram_bot::types::{MessageKind, UpdateKind};
use telegram_bot::{AllowedUpdate, Api, UpdatesStream};

use crate::jackett::TelegramJackettResponse;


const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        let command = &args[1];
        if command == "--version" {
            println!("{}", VERSION);
            exit(0);
        }
        if command == "--check-download-paths" {
            for media in [transmission::Media::Movie, transmission::Media::TV] {
                println!("{:?}: {}", media, transmission::download_path(&media)?);
            }
            return Ok(());
        }
    }

    let tracing = env::var("ENABLE_TRACING").is_ok();

    if tracing {
        tracing::subscriber::set_global_default(
            tracing_subscriber::FmtSubscriber::builder()
                .with_env_filter("telegram_bot=trace")
                .finish(),
        )
        .unwrap();
    }

    let pickers = search_picker::PickerStore::default();
    let deletes = delete_picker::DeleteStore::default();
    let responses: Arc<Mutex<Vec<TelegramJackettResponse>>> = Arc::new(Mutex::new(Vec::new()));
    let torrent_lists: Arc<Mutex<Vec<(Vec<i64>, String, MessageId)>>> = Arc::new(Mutex::new(Vec::new()));
    let restructure_plans: Arc<Mutex<Vec<(restructure::RestructurePlan, String, MessageId)>>> = Arc::new(Mutex::new(Vec::new()));

    let telegram_token = env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN not set");

    let api = Api::new(telegram_token);
    command_menu::register(&api).await?;
    let mut stream = UpdatesStream::new(&api);
    stream.allowed_updates(&[AllowedUpdate::Message, AllowedUpdate::CallbackQuery]);

    while let Some(update) = stream.next().await {
        let Ok(update) = update else {
            continue;
        };

        let kind = match update.kind {
            UpdateKind::CallbackQuery(query) => {
                let api = api.clone();
                let pickers = pickers.clone();
                let deletes = deletes.clone();
                tokio::spawn(async move {
                    if query.data.as_deref().map_or(false, |s| s.starts_with("del:")) { deletes.callback(&api, query).await; }
                    else { pickers.callback(&api, query).await; }
                });
                continue;
            }
            kind => kind,
        };
        let UpdateKind::Message(message) = kind else {
            continue;
        };

        let MessageKind::Text { ref data, .. } = message.kind else {
            continue;
        };

        let text = data.split_whitespace().map(|s| s.to_string()).collect();
        let cloned_api = api.clone();
        let pickers = pickers.clone();
        let deletes = deletes.clone();
        let mut shared_responses = Arc::clone(&responses);
        let mut shared_torrent_lists = Arc::clone(&torrent_lists);
        let mut shared_restructure_plans = Arc::clone(&restructure_plans);
        let data_cloned = data.clone();

        tokio::spawn(async move {
            let handle = handle_message(&cloned_api, &message, text, &mut shared_responses, &mut shared_torrent_lists, &mut shared_restructure_plans, &pickers, &deletes);
            if (handle.await).is_err() {
                let error_msg = format!(
                    "Errors should be handled in handle_message {:?}",
                    data_cloned.clone()
                );
                println!("{}", error_msg);
            };
        });
    }

    Ok(())
}
