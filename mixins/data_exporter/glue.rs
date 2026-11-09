// Turns the game's hooks into records for `data_exporter`, which knows nothing of the game.
// Every hook only listens: nothing here cancels or changes what the game does.

use super::Game;
use super::data_exporter as m;
use horizon_client::items::ItemStack;
use horizon_client::mod_api::NativeServerPlayer;
use horizon_world::registry::BlockStateId;
use m::{Field, Fields};

fn text(value: impl Into<String>) -> Field {
    Field::Text(value.into())
}

fn ready(game: &Game<'_>, id: &str) -> bool {
    game.native_on(m::ID) && m::ensure(|| Game::native_config_dir().ok()) && m::wants(id)
}

fn world(game: &Game<'_>) -> String {
    game.native_world_dir()
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// By default only the world owner's own data is saved.
fn allowed(game: &Game<'_>, player: u128) -> bool {
    if let Some(owner) = game.lan.as_ref().and_then(|host| host.owner.as_ref()) {
        m::remember_owner(owner.uuid);
    }
    m::others_allowed() || m::is_owner(player)
}

fn find(game: &Game<'_>, player: u128) -> Option<NativeServerPlayer> {
    game.native_acting_player()
        .filter(|p| p.uuid == player)
        .or_else(|| {
            game.native_server_players()
                .into_iter()
                .find(|p| p.uuid == player)
        })
}

/// Who and where: the same columns in every record, empty when the game can't say.
fn who(game: &Game<'_>, player: u128, name: Option<&str>) -> Fields {
    let found = find(game, player);
    let at = |i: usize| found.as_ref().map_or(Field::Null, |p| Field::Float(p.position[i]));
    let name = name
        .map(str::to_owned)
        .or_else(|| found.as_ref().map(|p| p.name.clone()));
    vec![
        ("player_uuid", text(m::uuid_text(player))),
        ("player_name", name.map_or(Field::Null, text)),
        (
            "dimension",
            found
                .as_ref()
                .map_or(Field::Null, |p| text(p.dimension.clone())),
        ),
        ("x", at(0)),
        ("y", at(1)),
        ("z", at(2)),
    ]
}

fn record(game: &Game<'_>, id: &'static str, player: u128, name: Option<&str>, extra: Fields) {
    if !ready(game, id) || !allowed(game, player) {
        return;
    }
    let mut fields = who(game, player, name);
    fields.extend(extra);
    m::emit(&world(game), id, fields);
}

/// A block's event: the block's own position, for the player acting.
fn block(game: &Game<'_>, id: &'static str, pos: [i32; 3], name: String, held: Field) {
    if !ready(game, id) {
        return;
    }
    let Some(player) = game.native_acting_player() else {
        return;
    };
    if !allowed(game, player.uuid) {
        return;
    }
    let fields = vec![
        ("player_uuid", text(m::uuid_text(player.uuid))),
        ("player_name", text(player.name.clone())),
        ("dimension", text(player.dimension.clone())),
        ("block_x", Field::Int(pos[0].into())),
        ("block_y", Field::Int(pos[1].into())),
        ("block_z", Field::Int(pos[2].into())),
        ("block", text(name)),
        ("item", held),
    ];
    m::emit(&world(game), id, fields);
}

pub(in crate::game) fn logged_in(game: &Game<'_>, player: u128, name: &str) {
    record(game, "session", player, Some(name), vec![("action", text("join"))]);
}

pub(in crate::game) fn logged_out(game: &Game<'_>, player: u128, name: &str) {
    record(game, "session", player, Some(name), vec![("action", text("leave"))]);
    m::flush();
}

pub(in crate::game) fn changed_dimension(
    game: &Game<'_>,
    player: u128,
    name: &str,
    from: &str,
    to: &str,
) {
    record(
        game,
        "dimension_change",
        player,
        Some(name),
        vec![("from", text(from)), ("to", text(to))],
    );
}

pub(in crate::game) fn player_tick(game: &Game<'_>) {
    if !game.native_on(m::ID) || !m::ensure(|| Game::native_config_dir().ok()) {
        return;
    }
    if let Some(owner) = game.lan.as_ref().and_then(|host| host.owner.as_ref()) {
        m::remember_owner(owner.uuid);
    }
    m::poll_profile();
    if !m::wants("player_state") {
        return;
    }
    let Some(player) = game.native_acting_player() else {
        return;
    };
    if !allowed(game, player.uuid) || !m::sample_due(player.uuid) {
        return;
    }
    let food = game.native_player_food(player.uuid);
    let fields = vec![
        ("player_uuid", text(m::uuid_text(player.uuid))),
        ("player_name", text(player.name.clone())),
        ("dimension", text(player.dimension.clone())),
        ("x", Field::Float(player.position[0])),
        ("y", Field::Float(player.position[1])),
        ("z", Field::Float(player.position[2])),
        ("yaw", Field::Float(player.yaw.into())),
        ("pitch", Field::Float(player.pitch.into())),
        ("health", Field::Float(player.health.into())),
        ("food", food.map_or(Field::Null, |f| Field::Int(f.0.into()))),
        ("saturation", food.map_or(Field::Null, |f| Field::Float(f.1.into()))),
        ("game_mode", Field::Int(player.game_mode.into())),
    ];
    m::emit(&world(game), "player_state", fields);
}

pub(in crate::game) fn block_broken(game: &Game<'_>, pos: [i32; 3], state: BlockStateId) {
    block(game, "block_broken", pos, game.state_string(state), Field::Null);
}

pub(in crate::game) fn block_placed(
    game: &Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    state: BlockStateId,
) {
    let held = text(stack.item.to_string());
    block(game, "block_placed", pos, game.state_string(state), held);
}

pub(in crate::game) fn died(game: &Game<'_>, player: u128, cause: Option<&str>) {
    let cause = cause.map_or(Field::Null, text);
    record(game, "player_deaths", player, None, vec![("cause", cause)]);
}

pub(in crate::game) fn crafted(game: &Game<'_>, player: u128, recipe: &str, stack: &ItemStack) {
    record(
        game,
        "crafting",
        player,
        None,
        vec![
            ("recipe", text(recipe)),
            ("item", text(stack.item.to_string())),
            ("count", Field::Int(stack.count.into())),
        ],
    );
}

pub(in crate::game) fn picked_up(game: &Game<'_>, player: u128, item: &str, count: u32) {
    record(
        game,
        "pickups",
        player,
        None,
        vec![("item", text(item)), ("count", Field::Int(count.into()))],
    );
}

pub(in crate::game) fn advancement(game: &Game<'_>, player: u128, name: &str, id: &str) {
    record(
        game,
        "advancements",
        player,
        Some(name),
        vec![("advancement", text(id))],
    );
}

pub(in crate::game) fn stat_award(game: &Game<'_>, stat_type: &str, key: &str, amount: i32) {
    if m::is_tick_counter(stat_type, key) {
        return;
    }
    let (Some(player), Some(name)) = (game.lan_acting_uuid, game.lan_acting_name.clone()) else {
        return;
    };
    // The `killed` statistic: unlike the `mob_killed` hook point, it hears every kill of a player's.
    if stat_type == "killed" {
        record(
            game,
            "mobs_killed",
            player,
            Some(&name),
            vec![("mob", text(m::namespaced(key)))],
        );
    }
    record(
        game,
        "stat_awards",
        player,
        Some(&name),
        vec![
            ("stat_type", text(m::namespaced(stat_type))),
            ("stat", text(m::namespaced(key))),
            ("amount", Field::Int(amount.into())),
        ],
    );
}

pub(in crate::game) fn listed(mods: &mut Vec<crate::game::native_hooks::NativeMod>) {
    mods.push(crate::game::native_hooks::NativeMod {
        id: m::ID.into(),
        title: format!("{} {}", m::NAME, m::VERSION),
        description: m::DESCRIPTION.into(),
        keys: Vec::new(),
    });
}

pub(in crate::game) fn settings(mod_id: &str, buttons: &mut Vec<String>) {
    if mod_id == m::ID && m::ensure(|| Game::native_config_dir().ok()) {
        buttons.extend(m::labels());
    }
}

pub(in crate::game) fn settings_click(game: &mut Game<'_>, mod_id: &str, button: usize) {
    use horizon_client::clipboard::Clipboard;
    if mod_id != m::ID || !m::ensure(|| Game::native_config_dir().ok()) {
        return;
    }
    match m::click(button) {
        m::Click::Nothing => {}
        m::Click::Open(path) => {
            if let Err(error) = m::reveal(&path) {
                game.native_chat(&format!("Data Exporter: can't open the folder: {error}"));
            }
        }
        m::Click::Copy(path) => {
            game.clipboard.set(&path);
            game.native_chat(&format!("Data Exporter: copied {path}"));
        }
        m::Click::Failed(message) => game.native_chat(&format!("Data Exporter: {message}")),
    }
}
