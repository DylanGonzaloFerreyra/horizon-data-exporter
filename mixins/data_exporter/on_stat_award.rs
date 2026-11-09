fn data_exporter_stat_award(game: &mut crate::game::Game<'_>, stat_type: &str, key: &str, amount: i32) {
    crate::game::data_exporter_glue::stat_award(&*game, stat_type, key, amount);
}
