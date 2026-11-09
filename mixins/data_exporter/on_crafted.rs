fn data_exporter_crafted(game: &mut crate::game::Game<'_>, player: u128, recipe: &str, stack: &horizon_client::items::ItemStack) {
    crate::game::data_exporter_glue::crafted(&*game, player, recipe, stack);
}
