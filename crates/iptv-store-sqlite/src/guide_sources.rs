//! The guide (XMLTV) sources of a playlist source as the user arranged
//! them (schema v6): the provider's own guides and any the user added, each
//! with a switch, a place in the priority order and a time shift.
//!
//! The list is replaced as a whole ([`Store::set_guide_sources`]); the app
//! decides what changes (adding, removing, moving, the provider's guides
//! after a playlist import) and turns the list into the configs of a guide
//! download.

use iptv_providers::SecretUrl;
use rusqlite::{TransactionBehavior, params};

use crate::error::Result;
use crate::sources::ensure_source;
use crate::{SourceId, Store};

pub use iptv_library::GuideSource;

pub(crate) fn set_guide_sources_in(
    conn: &rusqlite::Connection,
    source: SourceId,
    sources: &[GuideSource],
    now: i64,
) -> Result<()> {
    ensure_source(conn, source)?;
    iptv_library::guides::validate_sources(sources).map_err(feed_error)?;
    let highest = crate::guide_repair::feed_high_water_in(conn, source, now)?
        .max(sources.iter().map(|guide| guide.feed.0).max().unwrap_or(0));
    crate::guide_repair::remember_feed_high_water(conn, source, highest)?;
    // Preserve equal configured rows and their incarnations on a physical
    // reimport. Delete only removed feeds; triggers record actual changes.
    let retained: std::collections::HashSet<_> = sources.iter().map(|g| g.feed.0).collect();
    if retained.len() != sources.len() {
        return Err(crate::StoreError::InvalidData(
            "duplicate guide feed ID".into(),
        ));
    }
    let existing = {
        let mut statement =
            conn.prepare_cached("SELECT feed_id FROM guide_sources WHERE source_id = ?1")?;
        statement
            .query_map([source.0], |row| row.get::<_, u32>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for feed in existing {
        if !retained.contains(&feed) {
            conn.execute(
                "DELETE FROM guide_sources WHERE source_id = ?1 AND feed_id = ?2",
                rusqlite::params![source.0, feed],
            )?;
        }
    }
    // Release changed provider slots before upserting: two retained feed IDs
    // may swap provider indices in one otherwise valid replacement.
    for feed in sources {
        conn.execute(
            "UPDATE guide_sources SET provider_index = NULL
             WHERE source_id = ?1 AND feed_id = ?2 AND provider_index IS NOT ?3",
            rusqlite::params![source.0, feed.feed.0, feed.provider_index],
        )?;
    }
    {
        let mut insert = conn.prepare_cached(
            "INSERT INTO guide_sources (source_id, feed_id, provider_index, url, name,
                     enabled, position, shift_minutes, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (source_id, feed_id) DO UPDATE SET
                     provider_index = excluded.provider_index, url = excluded.url,
                     name = excluded.name, enabled = excluded.enabled,
                     position = excluded.position, shift_minutes = excluded.shift_minutes,
                     updated_at = excluded.updated_at",
        )?;
        for (position, g) in sources.iter().enumerate() {
            insert.execute(params![
                source.0,
                g.feed.0,
                g.provider_index,
                g.url.expose(),
                g.name,
                g.enabled,
                position as i64,
                g.shift_minutes,
                now
            ])?;
        }
    }
    Ok(())
}

impl Store {
    /// `source`'s guide sources in the user's order (first wins). Empty for
    /// a source whose guides were never arranged (a new one).
    pub fn guide_sources(&self, source: SourceId) -> Result<Vec<GuideSource>> {
        guide_sources_in(&self.conn, source)
    }

    /// Plan provider guides in one snapshot, including durable repair reservations.
    pub fn plan_provider_guides(
        &self,
        source: SourceId,
        urls: &[SecretUrl],
    ) -> Result<Vec<GuideSource>> {
        self.read_snapshot(|store| {
            plan_provider_guides_in(&store.conn, source, urls, None, store.now())
        })
    }

    /// Apply the portable edit and reserve new identities in the same writer transaction.
    pub fn edit_guide_sources(
        &mut self,
        source: SourceId,
        edit: &iptv_library::guides::Edit,
    ) -> Result<bool> {
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = guide_sources_in(&tx, source)?;
        let highest = crate::guide_repair::feed_high_water_in(&tx, source, now)?;
        let Some(updated) = iptv_library::guides::apply_with_high_water(&current, edit, highest)
            .map_err(feed_error)?
        else {
            return Ok(false);
        };
        set_guide_sources_in(&tx, source, &updated, now)?;
        tx.commit()?;
        Ok(true)
    }

    /// Replaces `source`'s guide sources with `sources`, in that order.
    pub fn set_guide_sources(&mut self, source: SourceId, sources: &[GuideSource]) -> Result<()> {
        let now = self.now();
        // Immediate: it reads before it writes (see `update_source`).
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        set_guide_sources_in(&tx, source, sources, now)?;
        tx.commit()?;
        Ok(())
    }
}

fn feed_error(error: iptv_library::guides::GuideFeedError) -> crate::StoreError {
    crate::StoreError::InvalidData(error.to_string())
}

fn guide_sources_in(conn: &rusqlite::Connection, source: SourceId) -> Result<Vec<GuideSource>> {
    ensure_source(conn, source)?;
    let mut stmt = conn.prepare_cached(
        "SELECT feed_id, provider_index, url, name, enabled, shift_minutes
         FROM guide_sources WHERE source_id = ?1 ORDER BY position, feed_id",
    )?;
    let rows = stmt.query_map([source.0], |row| {
        Ok(GuideSource {
            feed: iptv_epg::FeedId(row.get(0)?),
            provider_index: row.get(1)?,
            url: SecretUrl::new(row.get::<_, String>(2)?),
            name: row.get(3)?,
            enabled: row.get(4)?,
            shift_minutes: row.get(5)?,
        })
    })?;
    let sources = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    iptv_library::guides::validate_sources(&sources).map_err(feed_error)?;
    Ok(sources)
}

pub(crate) fn plan_provider_guides_in(
    conn: &rusqlite::Connection,
    source: SourceId,
    urls: &[SecretUrl],
    initial: Option<&[GuideSource]>,
    now: i64,
) -> Result<Vec<GuideSource>> {
    let highest = crate::guide_repair::feed_high_water_in(conn, source, now)?;
    let current = guide_sources_in(conn, source)?;
    // Source creation precedes publication. Seed only untouched guide state;
    // even an empty configured list may retain identities from earlier edits.
    let current = if current.is_empty() && highest == 0 {
        initial.unwrap_or(&current)
    } else {
        &current
    };
    iptv_library::guides::sync_provider_with_high_water(current, urls, highest).map_err(feed_error)
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    use crate::{CatalogStage, EpgOverride};
    use iptv_epg::FeedId;
    use iptv_import::{CatalogRow, CatalogSink};
    use iptv_library::guides::{Edit, user_source};
    use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};

    fn url(name: &str) -> SecretUrl {
        SecretUrl::new(format!("https://guide.example.test/{name}.xml"))
    }

    fn fixture() -> (Store, SourceId, iptv_providers::ContentId) {
        let mut store = Store::open_in_memory().unwrap();
        store.set_clock(|| 100);
        let source = store
            .add_source(&Source {
                name: "Generated retained source".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://playlist.example.test/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated retained channel\nhttps://media.example.test/retained.ts\n",
        );
        let key = parsed.channels.items[0].id.clone();
        store
            .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        (store, source, key)
    }

    fn incoming_stage(store: &Store, directory: &std::path::Path) -> CatalogStage {
        let parsed = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated incoming channel\nhttps://media.example.test/incoming.ts\n",
        );
        let mut stage =
            CatalogStage::create_in_with(directory, |path| store.open_staging_connection(path))
                .unwrap();
        stage.begin_step().unwrap();
        stage
            .push(CatalogRow::Channel(parsed.channels.items[0].clone()))
            .unwrap();
        stage.end_step(true).unwrap();
        stage.finish().unwrap();
        stage
    }

    #[test]
    fn ordinary_edits_and_provider_planning_share_retained_repair_reservations() {
        let (mut store, source, channel) = fixture();
        let original = vec![
            user_source(FeedId(1), url("first"), None),
            user_source(FeedId(1001), url("second"), None),
        ];
        store.set_guide_sources(source, &original).unwrap();
        let mapping = EpgOverride::for_feed(FeedId(2), "synthetic.retained");
        store
            .set_epg_override(source, &channel, Some(&mapping))
            .unwrap();
        // Emulate a historical receipt predating the cached allocation counter.
        store
            .conn
            .execute(
                "DELETE FROM store_meta WHERE key LIKE 'guide.repair.feed_high_water.%'",
                [],
            )
            .unwrap();
        let incarnation = store.source_incarnation(source).unwrap();
        let payload =
            serde_json::json!({"feeds":[],"rows":[],"added":[5000],"order":null}).to_string();
        store
            .conn
            .execute(
                "INSERT INTO guide_repair_commits
            (id,source_id,source_incarnation,created_at,expires_at,payload)
            VALUES('generated-legacy',?1,?2,100,1000,?3)",
                params![source.0, incarnation, payload],
            )
            .unwrap();
        let planned = store
            .plan_provider_guides(source, &[url("provider")])
            .unwrap();
        assert_eq!(&planned[..2], original.as_slice());
        assert_eq!(planned[2].feed, FeedId(5001));
        assert!(
            store
                .edit_guide_sources(
                    source,
                    &Edit::Add {
                        url: url("user"),
                        name: None
                    }
                )
                .unwrap()
        );
        assert_eq!(
            store.guide_sources(source).unwrap().last().unwrap().feed,
            FeedId(5001)
        );
        assert!(
            store
                .edit_guide_sources(source, &Edit::Remove(FeedId(5001)))
                .unwrap()
        );
        // Removing a row cannot release a retained numeric identity for another origin.
        assert_eq!(store.guide_repair_feed_high_water(source).unwrap(), 5001);
        assert_eq!(
            store
                .plan_provider_guides(source, &[url("provider")])
                .unwrap()[2]
                .feed,
            FeedId(5002)
        );
        assert_eq!(
            store.repair_mapping(source, &channel).unwrap(),
            Some(Some(mapping))
        );
    }

    #[test]
    fn final_catalog_publication_replans_after_an_intervening_guide_edit() {
        let (mut store, source, _) = fixture();
        let original = vec![user_source(FeedId(1), url("first"), None)];
        store.set_guide_sources(source, &original).unwrap();
        let urls = [url("provider")];
        let initial = store.plan_provider_guides(source, &urls).unwrap();
        assert_eq!(initial[1].feed, FeedId(2));
        // This consumes a fresh durable identity after the import's preflight plan.
        assert!(
            store
                .edit_guide_sources(
                    source,
                    &Edit::Add {
                        url: url("intervening"),
                        name: None
                    }
                )
                .unwrap()
        );
        let intervening = store.guide_sources(source).unwrap();
        assert_eq!(intervening[1].feed, FeedId(1001));
        let directory = tempfile::tempdir().unwrap();
        let (counts, published) = store
            .publish_catalog_stage_with_provider_guides(
                source,
                incoming_stage(&store, directory.path()),
                None,
                &urls,
                Some(&initial),
            )
            .unwrap();
        assert_eq!(counts.channels, 1);
        assert_eq!(&published[..2], intervening.as_slice());
        assert_eq!(published[2].feed, FeedId(1002));
        assert_eq!(store.guide_sources(source).unwrap(), published);
        assert_eq!(
            store.load_channels(source, &Default::default()).unwrap()[0]
                .channel
                .name,
            "Generated incoming channel"
        );
    }

    #[test]
    fn initial_guides_cannot_restore_cleared_rows_or_reuse_retained_identities() {
        let (mut store, source, _) = fixture();
        let initial = vec![user_source(FeedId(1), url("stale-user"), None)];
        store
            .set_guide_sources(
                source,
                &[user_source(FeedId(5000), url("removed-user"), None)],
            )
            .unwrap();
        store.set_guide_sources(source, &[]).unwrap();
        assert!(store.guide_sources(source).unwrap().is_empty());
        assert_eq!(store.guide_repair_feed_high_water(source).unwrap(), 5000);
        let directory = tempfile::tempdir().unwrap();
        let urls = [url("provider")];
        let (_, published) = store
            .publish_catalog_stage_with_provider_guides(
                source,
                incoming_stage(&store, directory.path()),
                None,
                &urls,
                Some(&initial),
            )
            .unwrap();
        assert_eq!(
            published.len(),
            1,
            "a stale seed cannot restore a cleared user guide"
        );
        assert_eq!(published[0].provider_index, Some(0));
        assert_eq!(published[0].feed, FeedId(5001));
        assert_eq!(store.guide_sources(source).unwrap(), published);
        assert_eq!(store.guide_repair_feed_high_water(source).unwrap(), 5001);
    }

    #[test]
    fn initial_guides_seed_only_uninitialized_guide_state() {
        let (mut store, source, _) = fixture();
        assert!(store.guide_sources(source).unwrap().is_empty());
        assert_eq!(store.guide_repair_feed_high_water(source).unwrap(), 0);
        let urls = [url("provider")];
        let mut initial = iptv_library::guides::sync_provider(
            &[user_source(
                FeedId(1),
                url("initial-user"),
                Some("Generated initial guide".into()),
            )],
            &urls,
        )
        .unwrap();
        initial[1].enabled = false;
        initial[1].shift_minutes = 30;
        let directory = tempfile::tempdir().unwrap();
        let (_, published) = store
            .publish_catalog_stage_with_provider_guides(
                source,
                incoming_stage(&store, directory.path()),
                None,
                &urls,
                Some(&initial),
            )
            .unwrap();
        assert_eq!(
            published, initial,
            "new-source seeding preserves prepared IDs and settings"
        );
        assert_eq!(store.guide_sources(source).unwrap(), initial);
        assert_eq!(store.guide_repair_feed_high_water(source).unwrap(), 2);
    }

    #[test]
    fn exhausted_allocation_rolls_back_guides_details_and_staged_catalog() {
        let (mut store, source, _) = fixture();
        let guides = vec![user_source(FeedId(u32::MAX), url("retained"), None)];
        store.set_guide_sources(source, &guides).unwrap();
        let original = store.source(source).unwrap().unwrap();
        assert!(matches!(
            store.edit_guide_sources(
                source,
                &Edit::Add {
                    url: url("new"),
                    name: None
                }
            ),
            Err(crate::StoreError::InvalidData(_))
        ));
        assert!(matches!(
            store.plan_provider_guides(source, &[url("new")]),
            Err(crate::StoreError::InvalidData(_))
        ));
        let mut details = original.source.clone();
        details.name = "Generated attempted edit".into();
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            store.publish_catalog_stage_with_provider_guides(
                source,
                incoming_stage(&store, directory.path()),
                Some(&details),
                &[url("new")],
                None,
            ),
            Err(crate::StoreError::InvalidData(_))
        ));
        assert_eq!(
            store.source(source).unwrap().unwrap().source,
            original.source
        );
        assert_eq!(store.guide_sources(source).unwrap(), guides);
        assert_eq!(
            store.load_channels(source, &Default::default()).unwrap()[0]
                .channel
                .name,
            "Generated retained channel"
        );
        assert_eq!(
            store.guide_repair_feed_high_water(source).unwrap(),
            u32::MAX
        );
        // The browser's stage is a memory file, gone with its connection.
        #[cfg(not(target_family = "wasm"))]
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            0,
            "failed stage must be removed"
        );
    }
}
