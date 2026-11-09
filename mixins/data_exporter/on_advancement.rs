fn data_exporter_advancement(game: &mut crate::game::Game<'_>, player: u128, name: &str, advancement: &str) {
    crate::game::data_exporter_glue::advancement(&*game, player, name, advancement);
}
