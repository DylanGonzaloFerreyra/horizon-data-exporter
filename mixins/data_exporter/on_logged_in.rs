fn data_exporter_logged_in(game: &mut crate::game::Game<'_>, player: u128, name: &str) {
    crate::game::data_exporter_glue::logged_in(&*game, player, name);
}
