# Data Exporter

A native mod for [Horizon](https://github.com/horizon-rust/horizon) that turns what happens in your game into data you own. While you play, it saves events and statistics to files on your computer, ready to analyze with DuckDB, pandas, Spark or a spreadsheet.

Nothing is sent over the network.

## What it saves

| Group | Data | Folder |
| --- | --- | --- |
| Session & player | Joining and leaving, dimension changes, a sample of position, health and food every 5 seconds | `events/session`, `events/dimension_change`, `events/player_state` |
| Blocks & combat | Blocks broken and placed with their coordinates, your deaths, the mobs you kill | `events/block_broken`, `events/block_placed`, `events/player_deaths`, `events/mobs_killed` |
| Items & progress | Crafting, items picked up, advancements | `events/crafting`, `events/pickups`, `events/advancements` |
| Statistics | Every increase of the game's statistics, at the moment it happens | `stats/stat_awards` |

By default, it only saves your own data. Other players' data, typed text and keyboard input are not captured.

## Using it

Go to **Options > Mods > Data Exporter > Config**:

- One switch per group turns it on or off.
- **Format** picks JSON Lines or CSV.

In a world, two commands:

- `/data_exporter` says how many records were saved and dropped (or why saving stopped), and where your profile is.
- `/data_exporter open` shows the profile in your file manager. Only the world's owner can use it, since it opens on the computer that runs the world.

Files go to `data-exporter/` in your game folder, one file per kind of data, world and session:

```text
data-exporter/
  events/block_broken/My World_20261109-183205.jsonl
  stats/stat_awards/My World_20261109-183205.jsonl
```

Every record has the schema version, a UTC timestamp, the event and the world:

```json
{"schema":1,"ts":"2026-11-09T18:32:05.123Z","event":"block_broken","world":"My World","player_uuid":"...","player_name":"nalyd006","dimension":"minecraft:overworld","block_x":12,"block_y":64,"block_z":-3,"block":"minecraft:stone","item":null}
```

## Your profile

Your settings live in `config/data_exporter.json`, and it is the same thing the switches change. Edit it while playing and the mod picks it up within a second. If the file has a mistake, the mod keeps your last valid settings, and `/data_exporter` tells you.

```json
{
  "profile": "survival-analysis",
  "folder": "~/Data/minecraft",
  "format": "jsonl",
  "capture": { "pickups": false },
  "outputs": {
    "player_deaths": { "folder": "~/Analysis/deaths", "format": "csv" }
  },
  "include_other_players": false,
  "player_state_seconds": 5,
  "max_file_mb": 64,
  "max_total_mb": 1024
}
```

- `capture` turns each kind of data on or off.
- `outputs` sends one kind of data to its own folder or format.
- `max_file_mb` starts a new file when one gets too big.
- `max_total_mb` stops saving when the folder reaches that size. The mod never deletes your data: free some space, or raise the limit, and saving resumes on its own.

## Build it

A Horizon native mod (`horizon-mod.toml`, mod API 0.2), pinned to Horizon `dd67b76` (Nightly 2026-11-18) and horizon-mixins `e59da90`. You need Rust and the `cargo horizon-mod` tool from that Horizon build:

```sh
cargo install --locked --git https://github.com/horizon-rust/horizon --rev dd67b76ff23e28c6d8e21859df600d31572f81a0 cargo-horizon-mod
cargo horizon-mod build            # fetches the SDK into .horizon/sdk, writes target/horizon-mod/data_exporter.hmod
cargo test                          # the runtime's tests
```

To play it, let the launcher build it against the pinned game: `horizon-launcher native-build . --instance "<instance>" --trust-native-code`. A native mod runs with your rights, outside any sandbox.

`Cargo.lock` pins the registry crates at the versions the pinned game itself uses: the launcher refuses a lock that moves one of the game's packages.

Tested on macOS (Apple silicon) with Horizon launcher 0.2.78 and Nightly 2026-11-18 (`dd67b76`): every kind of data written, the Config page and both commands.

## Credits

Version 0.1's build tooling was adapted from the [Horizon native mod template](https://github.com/horizon-rust/horizon-native-mod-template) and the [Veinminer port](https://github.com/levzzz5154/horizon-veinminer), both MIT. Since 0.2 the mod is built with Horizon's own `cargo horizon-mod`.

## License

[MIT](LICENSE)
