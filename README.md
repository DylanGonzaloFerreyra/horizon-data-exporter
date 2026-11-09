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

Go to **Options > Mods > Data Exporter > Settings**:

- One button per group turns it on or off.
- **Format** switches between JSONL and CSV.
- **Open profile location** shows the settings file in your file manager.
- **Copy profile path** copies its path.

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

Your settings live in `config/data_exporter.json`, and it is the same thing the buttons change. Edit it while playing and the mod picks it up within a second. If the file has a mistake, the mod keeps your last valid settings and tells you on its page.

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
- `max_total_mb` stops saving when the folder reaches that size. The mod never deletes your data.

## Build it

You need Rust 1.95, Python 3.12+, and clones of [Horizon](https://github.com/horizon-rust/horizon) and [horizon-mixins](https://github.com/horizon-rust/horizon-mixins) at the revisions in `compatibility.json`.

```sh
cargo test --locked
python3 tools/horizon.py build --horizon ../horizon
python3 tools/package.py
```

`tools/proof.py` runs the mod in a real game and checks the files it writes.

Tested on macOS with Horizon Nightly 2026-11-08 (`fdc87ae`).

## Credits

The build tooling is adapted from the [Horizon native mod template](https://github.com/horizon-rust/horizon-native-mod-template) and the [Veinminer port](https://github.com/levzzz5154/horizon-veinminer), both MIT.

## License

[MIT](LICENSE)
