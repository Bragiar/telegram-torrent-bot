# telegram-bot-torrents

> Documentation checked against local source on 2026-10-11.

Telegram Bot to search by torrents in [Jackett](https://github.com/Jackett/Jackett) indexers and forward it to [Transmission](https://transmissionbt.com/).


[![Docker release](https://img.shields.io/docker/v/gjhenrique/telegram-bot-torrents?color=blue&label=Docker%20Hub&sort=semver)](https://hub.docker.com/repository/docker/gjhenrique/telegram-bot-torrents)


## Features

It's possible to communicate with the bot in a group or privately.
By default, only movies and TV shows destination directories are supported.

### Available Commands

The bot registers Telegram’s native command menu and Menu button on every startup.
Tap **Menu** beside the message box or type `/` to see the commands.
Menu commands use underscores (for example `/torrent_tv` and `/delete_movie`);
the legacy hyphen spellings below remain supported. Group commands with an
`@bot_username` suffix are supported too. `/restructure tv` or `/restructure movie`
previews media reorganization, and `/chat_id` shows your chat ID.

- `/torrent-tv <magnet link>` - Add a torrent/magnet link for TV shows
- `/torrent-movie <magnet link>` - Add a torrent/magnet link for movies
- `/search <query>` - Search for movies or TV shows (e.g., "The Matrix" or "Simpsons s01e01")
- `/imdb <imdb link>` - Search using an IMDB link (requires OMDB token)
- `/status` - Show the 10 most recently added torrents (with shown/total count)
- `/delete-torrent` - List and delete torrents from Transmission
- `/delete-tv` - List and delete TV show files from disk
- `/delete-movie` - List and delete movie files from disk
- `/stop-seed` - Stop all torrents (including active downloads)
- `/storage` - Get storage information for all disks
- `/restructure <tv|movie>` - Preview media reorganization; reply to apply selected moves
- `/chat_id` - Show this chat ID
- `/help` - Show help message

### Search picker
`/search <query>` shows five results per page, ordered by seed count (up to 20 total).
Each result includes its release title (including quality when supplied by the indexer), size and seed count.
Tap a result to see **Download as Movie** and **Download as TV**, with the detected category marked recommended.
Use **Back**, **Previous** and **Next** to navigate without sending new messages.
**Cancel** is available on both screens to dismiss the search and disable further selection.
A successful download removes the buttons; concurrent/repeated taps cannot add it twice.

Typed replies still work: reply to the search message with its result number,
`movie N` or `tv N`. Unknown categories open the type picker instead of failing.
Search sessions are restricted to the original requester and chat, expire after one hour,
and are cleared on restart. Run `/search` again if a picker is expired.
IMDb searches use the same picker. The picker adds no dependencies; `/restructure` separately requires the GuessIt CLI.

### Add an IMDB page
Search the movie or TV show of the IMDB link. For example, `Matrix (1999)` is sent to Jackett.
An [OMDB key](http://www.omdbapi.com/apikey.aspx) is required.
![imdb](./doc/movie-imdb.png)

### Send a direct torrent link

You can bypass Jackett, and add a torrent or a magnet link directly to the bot.

![imdb](./doc/manual-torrent.png)

### Check Download Status

Use `/status` to see up to 10 torrents, ordered by Transmission’s `addedDate` newest first, with a shown/total count. Each includes its current status (downloading, seeding, stopped, etc.), progress percentage, file sizes, and download/upload statistics. Long names are safely truncated and HTML-escaped so the reply stays under Telegram’s message limit.

### Manage Torrents

- `/delete-torrent` - Lists all torrents in Transmission. Reply with a number to remove the torrent from Transmission (keeps files on disk).
- `/delete-tv` - Browse TV files/folders alphabetically, five per page. Select a title, then **Delete permanently**; folders include every episode inside.
- `/delete-movie` - Browse movies alphabetically with **Previous / Next**, select, then **Delete permanently**. **Back / Cancel** leave files intact.

Delete lists include the existing library and Transmission download folders, deduplicating identical roots. Set `ACTUAL_TV_PATH` / `ACTUAL_MOVIE_PATH` for existing libraries, or configure `tv` / `movies` in `media-roots.json` in the bot working directory. Missing/inaccessible roots produce an error, not a misleading empty list. Symlinks and hidden entries are excluded. Pickers are requester/chat/message-bound, expire after one hour, and disappear on restart.

### Stop Seeding

Despite its name, `/stop_seed` sends `torrent-stop` for **all torrents** in Transmission, including active downloads.

### Check Storage

Use `/storage` to get detailed storage information for all mounted disks, including total space, used space, available space, and usage percentages.

### Reorganize media

`/restructure tv` or `/restructure movie` recursively scans the configured media root
and uses `guessit -j` to preview video and matching subtitle moves. GuessIt must be
installed on PATH, in `.guessit-venv`, or selected with `GUESSIT_PATH`; each lookup has a five-second timeout.
Reply to the preview with `apply all`, `apply 1 2 5`, or `cancel`.
TV targets follow `Title/Season 01/Original.Release.Name.ext`; movies follow
`Title (Year)/Original.Release.Name.ext`. Release names and subtitle language suffixes
are preserved. Root selection uses `ACTUAL_*_PATH`, then `media-roots.json`, then
`TRANSMISSION_*_PATH`. Conflicts are skipped, never overwritten. Symlinks are excluded.
Moves use Linux atomic no-overwrite rename; cross-filesystem moves are refused.
Applying a plan moves files on disk; it does **not** update Transmission's download
locations, so moving managed files can interrupt seeding. Pending previews are lost on restart.
Existing `Show/Season NN`, `Movie (Year)`, and recognized extras folders are left untouched;
their curated show identity takes precedence over re-parsing a release filename.

## Configuration
The bot is configured through process environment variables. It does not load a `.env` file itself; use a service `EnvironmentFile` or export variables before starting it.

New downloads use the Plex library root: `ACTUAL_MOVIE_PATH` / `ACTUAL_TV_PATH`,
then `movies` / `tv` in `media-roots.json`, then the legacy `TRANSMISSION_*_PATH`
fallback. The chosen folder must already exist; an unavailable library produces
an error rather than silently downloading elsewhere. Transmission must have write
access to it. Existing torrents keep their current paths. Run
`telegram-bot-torrents --check-download-paths` from the service working directory
to inspect routing without starting a Telegram poller.

``` bash
# Token provided after creating a new bot https://core.telegram.org/bots#creating-a-new-bot
TELEGRAM_BOT_TOKEN=token
# Find the token in the field API Key
JACKETT_TOKEN=xyz
# Another option is to get the token directly passing the configuration folder
JACKETT_DATA_DIR=/home/user/.config/jackett
# Defaults to http://localhost:9117
JACKETT_URL=http://192.168.1.10:9117
# Only needed if /imdb command is issued
OMDB_TOKEN=xyz
# Directory where TV torrents are stored
TRANSMISSION_TV_PATH=/home/user/torrent/tv
# Directory where Movie torrents are stored
TRANSMISSION_MOVIE_PATH=/home/user/torrent/movies
# If transmission requires
TRANSMISSION_CREDENTIALS=admin:admin
# Defaults to http://localhost:9091
TRANSMISSION_URL=http://192.168.1.10:9091
# Allowed ids to talk with the bot
TELEGRAM_ALLOWED_GROUPS=1,2,3
# Optional: presence enables Telegram-client tracing (even if value is "0")
ENABLE_TRACING=1
# Optional: presence enables torrent-add debugging; use only for diagnosis
DEBUG_TORRENT=1
```


## Security
The bot is open by default. This means any person can add any torrent to your bot (as long as they can find it in the first place).
To make it secure, add the chat id of the group or your own id to the `TELEGRAM_ALLOWED_GROUPS` environment variable.
Issue the command `/chat-id`, and the bot will reply with your id.
After changing the variable `TELEGRAM_ALLOWED_GROUPS`, restart the server, and only the private chat or groups are allowed to talk with the bot.

**⚠️ Warning:** The `/delete-tv` and `/delete-movie` commands permanently delete files from your disk. Use with caution!

## Running

### Building

Use a current Rust toolchain. The legacy `Dockerfile` pins Rust 1.49 and is not
an up-to-date build recipe for the current source. The published upstream Docker
image is not guaranteed to include this checkout's local changes.

Offline/unit verification: `cargo test --locked`. The ignored live Telegram picker
test requires credentials and sends temporary messages; it is not run by default.

Either run `cargo build` or with [cross](https://github.com/rust-embedded/cross) to build for other architectures.
Cross supports the compilation of [lots of different architectures](https://github.com/rust-embedded/cross#supported-targets)

``` bash
cargo build --locked
./target/debug/telegram-bot-torrents

cargo build --release --locked
./target/release/telegram-bot-torrents

cross build --release --target=armv7-unknown-linux-gnueabihf
./target/armv7-unknown-linux-gnueabihf/release/telegram-bot-torrents telegram-bot-torrents.linux.armv7
```

### Running in Background

#### Using nohup
``` bash
nohup ./target/release/telegram-bot-torrents > bot.log 2>&1 &
```

#### Using systemd (Linux)
Create a service file at `/etc/systemd/system/telegram-bot-torrents.service`:

``` ini
[Unit]
Description=Telegram Torrent Bot
After=network.target

[Service]
Type=simple
User=your-user
WorkingDirectory=/path/to/telegram-bot-torrents
EnvironmentFile=/path/to/.env
ExecStart=/path/to/target/release/telegram-bot-torrents
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Then enable and start the service:
``` bash
sudo systemctl daemon-reload
sudo systemctl enable telegram-bot-torrents
sudo systemctl start telegram-bot-torrents
sudo systemctl status telegram-bot-torrents
```

#### Using screen
``` bash
screen -S telegram-bot
./target/release/telegram-bot-torrents
# Press Ctrl+A then D to detach
# Reattach with: screen -r telegram-bot
```

#### Using tmux
``` bash
tmux new-session -d -s telegram-bot './target/release/telegram-bot-torrents'
# Attach with: tmux attach -t telegram-bot
# Detach with: Ctrl+B then D
```
