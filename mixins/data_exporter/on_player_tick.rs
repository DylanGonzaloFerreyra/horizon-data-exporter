fn data_exporter_player_tick(game: &mut crate::game::Game<'_>) {
    crate::game::data_exporter_glue::player_tick(&*game);
}
