//! Semantic revisions are publication fences, not blanket edit preconditions.
use iptv_epg::FeedId;
use rusqlite::OptionalExtension;

use crate::{Result, SourceId, Store, StoreError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuideRevisions {
    pub mapping: u64,
    pub configuration: u64,
}

pub(crate) fn revision(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    value.try_into().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

impl Store {
    /// Stable across ordinary owner restart; changes if a source row is reused.
    pub fn source_incarnation(&self, source: SourceId) -> Result<String> {
        self.conn
            .query_row(
                "SELECT incarnation FROM guide_revisions WHERE source_id = ?1",
                [source.0],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::SourceNotFound(source))
    }

    /// A reused numeric feed ID never revives an obsolete candidate reference.
    pub fn feed_incarnation(&self, source: SourceId, feed: FeedId) -> Result<Option<String>> {
        crate::sources::ensure_source(&self.conn, source)?;
        Ok(self
            .conn
            .query_row(
                "SELECT incarnation FROM guide_feed_incarnations
             WHERE source_id = ?1 AND feed_id = ?2",
                rusqlite::params![source.0, feed.0],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn guide_revisions(&self, source: SourceId) -> Result<GuideRevisions> {
        self.conn
            .query_row(
                "SELECT mapping_revision, configuration_revision FROM guide_revisions
             WHERE source_id = ?1",
                [source.0],
                |row| {
                    Ok(GuideRevisions {
                        mapping: revision(row, 0)?,
                        configuration: revision(row, 1)?,
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::SourceNotFound(source))
    }

    /// A restored catalogue cannot carry authority to undo pre-restore edits.
    /// Call on the accepted restore before any runtime owner can open it.
    pub fn invalidate_guide_repair_authority(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM guide_repair_commits", [])?;
        tx.execute(
            "UPDATE guide_revisions SET incarnation = lower(hex(randomblob(16)))",
            [],
        )?;
        tx.execute(
            "UPDATE guide_feed_incarnations
                    SET incarnation = lower(hex(randomblob(16)))",
            [],
        )?;
        // Restored data and configuration were validated together. Rebind cached
        // provenance to the new restore incarnation without reviving old handles.
        tx.execute(
            "UPDATE store_meta SET value=(SELECT i.incarnation FROM guide_feed_incarnations i
             JOIN guide_sources g ON g.source_id=i.source_id AND g.feed_id=i.feed_id
             JOIN epg_feeds f ON f.source_id=g.source_id AND f.feed_id=g.feed_id
             WHERE store_meta.key='inspection.feed.' || i.source_id || '.' || i.feed_id
             AND f.url=g.url AND f.shift_minutes=g.shift_minutes)
             WHERE key LIKE 'inspection.feed.%' AND EXISTS(SELECT 1 FROM guide_feed_incarnations i
             JOIN guide_sources g ON g.source_id=i.source_id AND g.feed_id=i.feed_id
             JOIN epg_feeds f ON f.source_id=g.source_id AND f.feed_id=g.feed_id
             WHERE store_meta.key='inspection.feed.' || i.source_id || '.' || i.feed_id
             AND f.url=g.url AND f.shift_minutes=g.shift_minutes)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EpgOverride, GuideSource};
    use iptv_providers::{
        ContentId, SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };

    fn source(store: &mut Store) -> SourceId {
        store
            .add_source(&Source {
                name: "Synthetic revisions".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://synthetic.invalid/list.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap()
    }
    fn feed() -> GuideSource {
        GuideSource {
            feed: FeedId(1),
            provider_index: None,
            url: SecretUrl::new("https://synthetic.invalid/guide.xml"),
            name: None,
            enabled: true,
            shift_minutes: 0,
        }
    }

    #[test]
    fn no_op_config_and_non_guide_edits_leave_semantic_revisions_alone() {
        let mut store = Store::open_in_memory().unwrap();
        let source = source(&mut store);
        let config = feed();
        store
            .set_guide_sources(source, std::slice::from_ref(&config))
            .unwrap();
        let incarnation = store.feed_incarnation(source, config.feed).unwrap();
        let revisions = store.guide_revisions(source).unwrap();
        store.set_guide_sources(source, &[config]).unwrap();
        let channel = ContentId::from_stored("synthetic-channel");
        store.set_hidden(source, &channel, true).unwrap();
        store.set_favourite(source, &channel, true).unwrap();
        assert_eq!(store.guide_revisions(source).unwrap(), revisions);
        assert_eq!(
            store.feed_incarnation(source, FeedId(1)).unwrap(),
            incarnation
        );
        store
            .set_epg_override(source, &channel, Some(&EpgOverride::Disabled))
            .unwrap();
        let changed = store.guide_revisions(source).unwrap();
        assert_eq!(changed.mapping, revisions.mapping + 1);
        store
            .set_epg_override(source, &channel, Some(&EpgOverride::Disabled))
            .unwrap();
        assert_eq!(store.guide_revisions(source).unwrap(), changed);
        store.set_epg_override(source, &channel, None).unwrap();
        assert_eq!(
            store.guide_revisions(source).unwrap().mapping,
            changed.mapping + 1
        );
    }

    #[test]
    fn a_replacement_can_exchange_two_provider_indices() {
        let mut store = Store::open_in_memory().unwrap();
        let source = source(&mut store);
        let mut a = feed();
        a.provider_index = Some(0);
        let mut b = feed();
        b.feed = FeedId(2);
        b.provider_index = Some(1);
        store
            .set_guide_sources(source, &[a.clone(), b.clone()])
            .unwrap();
        a.provider_index = Some(1);
        b.provider_index = Some(0);
        store
            .set_guide_sources(source, &[a.clone(), b.clone()])
            .unwrap();
        let stored = store.guide_sources(source).unwrap();
        assert_eq!(stored[0].provider_index, a.provider_index);
        assert_eq!(stored[1].provider_index, b.provider_index);
    }

    #[test]
    fn feed_and_source_reuse_never_revive_old_identity() {
        let mut store = Store::open_in_memory().unwrap();
        let id = source(&mut store);
        let owner = store.source_incarnation(id).unwrap();
        store.set_guide_sources(id, &[feed()]).unwrap();
        let original = store.feed_incarnation(id, FeedId(1)).unwrap();
        let mut changed = feed();
        changed.shift_minutes = 60;
        store.set_guide_sources(id, &[changed]).unwrap();
        assert_ne!(store.feed_incarnation(id, FeedId(1)).unwrap(), original);
        store.set_guide_sources(id, &[]).unwrap();
        assert_eq!(store.feed_incarnation(id, FeedId(1)).unwrap(), None);
        store.set_guide_sources(id, &[feed()]).unwrap();
        assert_ne!(store.feed_incarnation(id, FeedId(1)).unwrap(), original);
        store
            .conn
            .execute("DELETE FROM sources WHERE id = ?1", [id.0])
            .unwrap();
        assert_eq!(source(&mut store), id);
        assert_ne!(store.source_incarnation(id).unwrap(), owner);
    }
}
