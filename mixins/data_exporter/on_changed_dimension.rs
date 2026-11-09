fn data_exporter_changed_dimension(game: &mut crate::game::Game<'_>, player: u128, name: &str, from: &str, to: &str) {
    crate::game::data_exporter_glue::changed_dimension(&*game, player, name, from, to);
}
