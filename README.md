# telegram-bot-torrents

Telegram Bot to search by torrents in [Jackett](https://github.com/Jackett/Jackett) indexers and forward it to [Transmission](https://transmissionbt.com/).


[![Docker release](https://img.shields.io/docker/v/gjhenrique/telegram-bot-torrents?color=blue&label=Docker%20Hub&sort=semver)](https://hub.docker.com/repository/docker/gjhenrique/telegram-bot-torrents)


## Features

It's possible to communicate with the bot in a group or privately.
By default, only movies and TV shows destination directories are supported.

### Available Commands

- `/torrent-tv <magnet link>` - Add a torrent/magnet link for TV shows
- `/torrent-movie <magnet link>` - Add a torrent/magnet link for movies
- `/search <query>` - Search for movies or TV shows (e.g., "The Matrix" or "Simpsons s01e01")
- `/imdb <imdb link>` - Search using an IMDB link (requires OMDB token)
- `/status` - Get status of all active downloads
- `/delete-torrent` - List and delete torrents from Transmission
- `/delete-tv` - List and delete TV show files from disk
- `/delete-movie` - List and delete movie files from disk
- `/stop-seed` - Stop seeding for all downloads
- `/storage` - Get storage information for all disks
- `/help` - Show help message

### Add Movies
The format is `{Index}. {Name} - {Size} - {Seeds}` and the list is sorted by seeds.
Reply to the original message with the index of your prefered torrent.

![movie](./doc/movie-search.png)

### Add TV Shows
Jackett indexers split some torrents into [categories](https://github.com/Jackett/Jackett/wiki/Jackett-Categories).
But sometimes, a torrent might not have a TV or Movie category.
Specify the `tv` or `movie` command with the index when that happens (e.g., reply with `tv 1` or `movie 1`).

![tv](./doc/tv-search.png)

### Add an IMDB page
Search the movie or TV show of the IMDB link. For example, `Matrix (1999)` is sent to Jackett.
An [OMDB key](http://www.omdbapi.com/apikey.aspx) is required.
![imdb](./doc/movie-imdb.png)

### Send a direct torrent link

You can bypass Jackett, and add a torrent or a magnet link directly to the bot.

![imdb](./doc/manual-torrent.png)

### Check Download Status

Use `/status` to see all active downloads with their current status (downloading, seeding, stopped, etc.), progress percentage, file sizes, and download/upload statistics.

### Manage Torrents

- `/delete-torrent` - Lists all torrents in Transmission. Reply with a number to remove the torrent from Transmission (keeps files on disk).
- `/delete-tv` - Lists all files and folders in the TV directory. Reply with a number to delete the file/folder from disk.
- `/delete-movie` - Lists all files and folders in the Movie directory. Reply with a number to delete the file/folder from disk.

### Stop Seeding

Use `/stop-seed` to stop seeding for all active downloads in Transmission.

### Check Storage

Use `/storage` to get detailed storage information for all mounted disks, including total space, used space, available space, and usage percentages.

## Configuration
The bot is configured through some environment variables.

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
```


## Security
The bot is open by default. This means any person can add any torrent to your bot (as long as they can find it in the first place).
To make it secure, add the chat id of the group or your own id to the `TELEGRAM_ALLOWED_GROUPS` environment variable.
Issue the command `/chat-id`, and the bot will reply with your id.
After changing the variable `TELEGRAM_ALLOWED_GROUPS`, restart the server, and only the private chat or groups are allowed to talk with the bot.

**⚠️ Warning:** The `/delete-tv` and `/delete-movie` commands permanently delete files from your disk. Use with caution!

## Running

### Building
Either run `cargo build` or with [cross](https://github.com/rust-embedded/cross) to build for other architectures.
Cross supports the compilation of [lots of different architectures](https://github.com/rust-embedded/cross#supported-targets)

``` bash
cargo build
./target/debug/telegram-bot-torrents

cargo build --release
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
