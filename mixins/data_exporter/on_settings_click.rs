fn data_exporter_settings_click(game: &mut crate::game::Game<'_>, mod_id: &str, button: usize) {
    crate::game::data_exporter_glue::settings_click(game, mod_id, button);
}
