//! Playback provenance and the explicit synthetic-development override.
pub(crate) fn policy(public_only: bool) -> iptv_net::playback::PlaybackPolicy {
    iptv_net::playback::PlaybackPolicy {
        public_only,
        // Play through the same extra trust anchors the importer uses. This
        // runs on player and JNI threads, which hold no application handle:
        // the anchors come from the configuration no setting changes.
        extra_roots_der: crate::runtime::adapter_config(
            &crate::runtime::settings::base_network_config(),
        )
        .extra_roots_der,
        // Release packages cannot enable private content destinations, even if
        // a fixture URL was accidentally supplied in the build environment.
        #[cfg(debug_assertions)]
        allow_private_fixture: cfg!(test)
            || cfg!(feature = "test-support")
            || crate::runtime::build_config::DEV_PLAYLIST.is_some(),
    }
}
