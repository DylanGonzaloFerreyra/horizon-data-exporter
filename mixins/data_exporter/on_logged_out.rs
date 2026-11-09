fn data_exporter_logged_out(game: &mut crate::game::Game<'_>, player: u128, name: &str) {
    crate::game::data_exporter_glue::logged_out(&*game, player, name);
}
