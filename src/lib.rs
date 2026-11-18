//! Data Exporter: turns the game's hooks into records for `runtime`, which knows nothing of the
//! game and writes them to files. Every hook only listens: nothing here cancels or changes what
//! the game does.
mod runtime;

use mod_api::{
    ClientHost, ClientPlayerLoggingInEvent, Command, CommandSource, EntityPlaceEvent, Flow, Hand,
    Hooks, Host, ItemCraftedEvent, ItemPickedUpPartialEvent, ItemPickedUpPostEvent, ItemStack,
    PlayerAdvancementDoneEvent, PlayerChangedDimensionEvent, PlayerDestroyedBlockEvent,
    PlayerDiedEvent, PlayerLoggedInEvent, PlayerLoggedOutEvent, PlayerRef, PlayerStatAwardEvent,
    RegisterCommandsEvent, Registrar, RegistrationContext, RunModCommandEvent, ServerHost,
    ServerTickEvent, ServerTickPhase, SettingChangedEvent, SettingSpec, Text,
};
use runtime as m;
use runtime::{Field, Fields};

#[derive(Default)]
struct DataExporter;

/// The settings page's toggles, one per group of `runtime::GROUPS`, in that order.
const GROUP_KEYS: [&str; 4] = [
    "session_player",
    "blocks_combat",
    "items_progress",
    "statistics",
];
const FORMAT_KEY: &str = "format";

fn text(value: impl Into<String>) -> Field {
    Field::Str(value.into())
}

/// Starts the runtime once the game's folder is known: the profile lives in its `config/`.
fn ensure(host: &dyn Host) -> bool {
    m::ensure(|| Some(host.instance_dir().join("config")))
}

fn ready(host: &dyn Host, id: &str) -> bool {
    host.on(m::ID) && ensure(host) && m::wants(id)
}

fn world(host: &dyn ServerHost) -> String {
    host.world_dir()
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// By default only the world owner's own data is saved.
fn allowed(player: u128) -> bool {
    m::others_allowed() || m::is_owner(player)
}

fn dimension(host: &dyn ServerHost, player: PlayerRef) -> Option<String> {
    player
        .dimension(host)
        .map(|id| format!("{}:{}", id.namespace(), id.path()))
}

/// Who and where: the same columns in every record, empty when the game can't say.
fn who(host: &dyn ServerHost, player: u128, name: Option<&str>) -> Fields {
    let found = host.player(player);
    let position = found.and_then(|p| p.entity().position(host));
    let at = |i: usize| position.map_or(Field::Null, |p| Field::Float(p[i]));
    let name = name
        .map(str::to_owned)
        .or_else(|| found.and_then(|p| p.name(host)));
    vec![
        ("player_uuid", text(m::uuid_text(player))),
        ("player_name", name.map_or(Field::Null, text)),
        (
            "dimension",
            found
                .and_then(|p| dimension(host, p))
                .map_or(Field::Null, text),
        ),
        ("x", at(0)),
        ("y", at(1)),
        ("z", at(2)),
    ]
}

fn record(
    host: &dyn ServerHost,
    id: &'static str,
    player: u128,
    name: Option<&str>,
    extra: Fields,
) {
    if !ready(host, id) || !wanted(player) {
        return;
    }
    let mut fields = who(host, player, name);
    fields.extend(extra);
    save(host, id, player, fields);
}

/// Whether `player`'s records may be saved: theirs are, or the owner isn't known yet.
fn wanted(player: u128) -> bool {
    allowed(player) || !m::owner_known()
}

/// Saves a record of `player`'s. Before the window names its player, any record may be the
/// owner's (the server tells of the owner's join first): it waits for the owner.
fn save(host: &dyn ServerHost, id: &'static str, player: u128, fields: Fields) {
    if allowed(player) {
        m::emit(&world(host), id, fields);
    } else {
        m::hold(player, &world(host), id, fields);
    }
}

/// A block's event: the block's own position, for the player acting.
fn block(host: &dyn ServerHost, id: &'static str, pos: [i32; 3], name: String, held: Field) {
    if !ready(host, id) {
        return;
    }
    let Some(player) = host.players().acting_player() else {
        return;
    };
    if !wanted(player.uuid()) {
        return;
    }
    let fields = vec![
        ("player_uuid", text(m::uuid_text(player.uuid()))),
        ("player_name", player.name(host).map_or(Field::Null, text)),
        (
            "dimension",
            dimension(host, player).map_or(Field::Null, text),
        ),
        ("block_x", Field::Int(pos[0].into())),
        ("block_y", Field::Int(pos[1].into())),
        ("block_z", Field::Int(pos[2].into())),
        ("block", text(name)),
        ("item", held),
    ];
    save(host, id, player.uuid(), fields);
}

fn item_id(host: &dyn Host, stack: &ItemStack) -> String {
    stack.item().id(host)
}

/// The stack a player places a block from: the hand holding that block's item, else the main
/// hand (the hook doesn't say which hand placed it).
fn placing_stack(host: &dyn ServerHost, player: PlayerRef, block: &str) -> Option<String> {
    let living = host.living(player.entity())?;
    let held = |hand| {
        living
            .hand_item(host, hand)
            .filter(|s| s.count() > 0)
            .map(|s| item_id(host, &s))
    };
    let (main, off) = (held(Hand::Main), held(Hand::Off));
    match (&main, &off) {
        (Some(m), _) if m == block => main,
        (_, Some(o)) if o == block => off,
        _ => main,
    }
}

/// One `player_state` row per player every `player_state_seconds`.
fn sample_players(host: &dyn ServerHost) {
    if !m::wants("player_state") {
        return;
    }
    let world = world(host);
    for player in host.players().server_players() {
        if !allowed(player.uuid()) || !m::sample_due(player.uuid()) {
            continue;
        }
        let position = player.entity().position(host);
        let rotation = player.entity().rotation(host);
        let health = host.living(player.entity()).and_then(|l| l.health(host));
        let food = player.food(host);
        let float = |v: Option<f64>| v.map_or(Field::Null, Field::Float);
        let fields = vec![
            ("player_uuid", text(m::uuid_text(player.uuid()))),
            ("player_name", player.name(host).map_or(Field::Null, text)),
            (
                "dimension",
                dimension(host, player).map_or(Field::Null, text),
            ),
            ("x", float(position.map(|p| p[0]))),
            ("y", float(position.map(|p| p[1]))),
            ("z", float(position.map(|p| p[2]))),
            ("yaw", float(rotation.map(|r| r[0].into()))),
            ("pitch", float(rotation.map(|r| r[1].into()))),
            ("health", float(health.map(Into::into))),
            (
                "food",
                food.map_or(Field::Null, |f| Field::Int(f.level.into())),
            ),
            (
                "saturation",
                food.map_or(Field::Null, |f| Field::Float(f.saturation.into())),
            ),
            (
                "game_mode",
                player
                    .game_mode(host)
                    .map_or(Field::Null, |g| Field::Int(g.id().into())),
            ),
        ];
        m::emit(&world, "player_state", fields);
    }
}

/// Copies the profile's groups and format to the settings page, so the page shows what a
/// hand-edited profile says.
fn show_profile_in_settings(host: &dyn ClientHost) {
    for (group, key) in GROUP_KEYS.iter().enumerate() {
        if let Some(on) = m::group_on(group) {
            host.set_setting(m::ID, key, if on { "true" } else { "false" });
        }
    }
    host.set_setting(m::ID, FORMAT_KEY, if m::csv() { "1" } else { "0" });
}

fn tell(host: &mut dyn ServerHost, player: PlayerRef, line: String) {
    let _ = player.tell(host, Text::literal(line));
}

#[mod_api::horizon_mod]
impl Hooks for DataExporter {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        for key in GROUP_KEYS {
            let label = format!("data_exporter.setting.{key}");
            let tooltip = format!("{label}.tooltip");
            let _ = r.register_setting(SettingSpec::toggle(key, label, true).tooltip(tooltip));
        }
        let options = [
            "data_exporter.setting.format.jsonl",
            "data_exporter.setting.format.csv",
        ];
        let format = SettingSpec::choice(FORMAT_KEY, "data_exporter.setting.format", options, 0)
            .tooltip("data_exporter.setting.format.tooltip");
        let _ = r.register_setting(format);
    }

    /// The window's own player joining a world: in single player and on a LAN host, the world's
    /// owner, whose data is saved by default.
    fn client_player_logging_in(
        &self,
        host: &mut dyn ClientHost,
        event: &mut ClientPlayerLoggingInEvent,
    ) {
        m::remember_owner(event.uuid);
        if ensure(&*host) {
            show_profile_in_settings(&*host);
        }
    }

    fn setting_changed(&self, host: &mut dyn ClientHost, event: &mut SettingChangedEvent<'_>) {
        if !ensure(&*host) {
            return;
        }
        let click = if let Some(group) = GROUP_KEYS.iter().position(|k| *k == event.key) {
            match host.setting_bool(event.key) {
                Some(on) => m::set_group(group, on),
                None => m::Click::Nothing,
            }
        } else if event.key == FORMAT_KEY {
            match host.setting_choice(FORMAT_KEY) {
                Some(index) => m::set_csv(index == 1),
                None => m::Click::Nothing,
            }
        } else {
            m::Click::Nothing
        };
        if let m::Click::Failed(message) = click {
            eprintln!("[data_exporter] {message}");
        }
    }

    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        let end = matches!(event.phase, ServerTickPhase::ServerTickEnd { .. });
        if !end || !host.on(m::ID) || !ensure(&*host) {
            return;
        }
        m::poll_profile();
        sample_players(&*host);
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        let extra = vec![("action", text("join"))];
        record(&*host, "session", event.player, Some(event.name), extra);
    }

    fn player_logged_out(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        let extra = vec![("action", text("leave"))];
        record(&*host, "session", event.player, Some(event.name), extra);
        m::flush();
    }

    fn player_changed_dimension(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerChangedDimensionEvent<'_>,
    ) {
        let extra = vec![("from", text(event.from)), ("to", text(event.to))];
        record(
            &*host,
            "dimension_change",
            event.player,
            Some(event.name),
            extra,
        );
    }

    fn player_destroyed_block(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerDestroyedBlockEvent,
    ) {
        let name = host.block_state_string(event.state).unwrap_or_default();
        block(&*host, "block_broken", event.pos, name, Field::Null);
    }

    /// Heard before the block is written: a placement another mod cancels after this listener
    /// still has its row.
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        if event.cancelled || !ready(&*host, "block_placed") {
            return Flow::Continue;
        }
        let Some(player) = host.players().acting_player() else {
            return Flow::Continue;
        };
        let block_id = event.state.block().id(&*host);
        let held = placing_stack(&*host, player, &block_id).map_or(Field::Null, text);
        let name = host.block_state_string(event.state).unwrap_or(block_id);
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        block(&*host, "block_placed", pos, name, held);
        Flow::Continue
    }

    fn player_died(&self, host: &mut dyn ServerHost, event: &mut PlayerDiedEvent<'_>) {
        let cause = event.cause.map_or(Field::Null, text);
        record(
            &*host,
            "player_deaths",
            event.player,
            None,
            vec![("cause", cause)],
        );
    }

    fn item_crafted(&self, host: &mut dyn ServerHost, event: &mut ItemCraftedEvent<'_>) {
        let extra = vec![
            ("recipe", text(event.recipe)),
            ("item", text(item_id(&*host, event.stack))),
            ("count", Field::Int(event.stack.count().into())),
        ];
        record(&*host, "crafting", event.player, None, extra);
    }

    fn item_picked_up_post(
        &self,
        host: &mut dyn ServerHost,
        event: &mut ItemPickedUpPostEvent<'_>,
    ) {
        let left = event.remaining.as_ref().map_or(0, ItemStack::count);
        picked_up(&*host, event.player, event.original, left);
    }

    fn item_picked_up_partial(
        &self,
        host: &mut dyn ServerHost,
        event: &mut ItemPickedUpPartialEvent<'_>,
    ) {
        let left = event.remaining.as_ref().map_or(0, ItemStack::count);
        picked_up(&*host, event.player, event.original, left);
    }

    fn player_advancement_done(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerAdvancementDoneEvent<'_>,
    ) {
        let extra = vec![("advancement", text(event.advancement))];
        record(
            &*host,
            "advancements",
            event.player,
            Some(event.name),
            extra,
        );
    }

    fn player_stat_award(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerStatAwardEvent<'_>,
    ) -> Flow {
        if event.cancelled {
            return Flow::Continue;
        }
        // The hook's ids are namespaced (`minecraft:custom`); the runtime's checks take the bare type.
        let stat_type = event
            .stat_type
            .strip_prefix("minecraft:")
            .unwrap_or(event.stat_type);
        if m::is_tick_counter(stat_type, event.stat) {
            return Flow::Continue;
        }
        // The `killed` statistic hears every kill of a player's.
        if stat_type == "killed" {
            let extra = vec![("mob", text(m::namespaced(event.stat)))];
            record(&*host, "mobs_killed", event.player, Some(event.name), extra);
        }
        let extra = vec![
            ("stat_type", text(m::namespaced(event.stat_type))),
            ("stat", text(m::namespaced(event.stat))),
            ("amount", Field::Int(event.value.into())),
        ];
        record(&*host, "stat_awards", event.player, Some(event.name), extra);
        Flow::Continue
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        event.commands.extend([
            Command::new("data_exporter"),
            Command::new("data_exporter open"),
        ]);
    }

    /// `/data_exporter` says what was saved and where the profile is; `/data_exporter open` shows
    /// the profile in the file manager, for the world's owner (it opens on the server's computer).
    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let literals = &event.command.literals;
        if literals.first().map(String::as_str) != Some("data_exporter") {
            return Flow::Continue;
        }
        let CommandSource::Player { player, .. } = event.source else {
            return Flow::Cancel;
        };
        if !ensure(&*host) {
            return Flow::Cancel;
        }
        let path = m::profile_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if literals.get(1).map(String::as_str) == Some("open") {
            let line = if !m::is_owner(player.uuid()) {
                "Data Exporter: only the world's owner can open its folder".to_owned()
            } else {
                match m::profile_path().map(|p| m::reveal(&p)) {
                    Some(Ok(())) => format!("Data Exporter: opened {path}"),
                    Some(Err(error)) => format!("Data Exporter: can't open the folder: {error}"),
                    None => "Data Exporter: no profile yet".to_owned(),
                }
            };
            tell(host, *player, line);
        } else {
            let status = m::status_line().unwrap_or_default();
            tell(
                host,
                *player,
                format!("Data Exporter {}: {status}", m::VERSION),
            );
            tell(host, *player, format!("Profile: {path}"));
        }
        Flow::Cancel
    }
}

fn picked_up(host: &dyn ServerHost, player: u128, original: &ItemStack, left: u32) {
    let count = original.count().saturating_sub(left);
    if count == 0 {
        return;
    }
    let extra = vec![
        ("item", text(item_id(host, original))),
        ("count", Field::Int(count.into())),
    ];
    record(host, "pickups", player, None, extra);
}
