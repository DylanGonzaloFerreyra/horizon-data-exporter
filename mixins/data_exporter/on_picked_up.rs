fn data_exporter_picked_up(game: &mut crate::game::Game<'_>, player: u128, item: &str, count: u32) {
    crate::game::data_exporter_glue::picked_up(&*game, player, item, count);
}
