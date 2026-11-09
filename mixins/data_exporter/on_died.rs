fn data_exporter_died(game: &mut crate::game::Game<'_>, player: u128, cause: Option<&str>) {
    crate::game::data_exporter_glue::died(&*game, player, cause);
}
