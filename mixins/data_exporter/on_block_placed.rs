fn data_exporter_block_placed(game: &mut crate::game::Game<'_>, stack: &horizon_client::items::ItemStack, pos: [i32;3], state: horizon_world::registry::BlockStateId) {
    crate::game::data_exporter_glue::block_placed(&*game, stack, pos, state);
}
