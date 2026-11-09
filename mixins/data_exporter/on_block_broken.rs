fn data_exporter_block_broken(game: &mut crate::game::Game<'_>, pos: [i32;3], state: horizon_world::registry::BlockStateId) {
    crate::game::data_exporter_glue::block_broken(&*game, pos, state);
}
