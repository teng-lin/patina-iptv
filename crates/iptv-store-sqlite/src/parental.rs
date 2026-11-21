//! Parental credentials are encrypted with the catalog. Only one-way
//! verifiers are stored, and credential Debug output is always redacted.
use crate::{Result, Store};
use iptv_core::parental::{ContentKind, Target};
use rusqlite::{OptionalExtension, params};

pub struct ParentalCredential {
    pub salt: [u8; 16],
    pub verifier: [u8; 32],
    pub recovery_verifier: [u8; 32],
    pub failures: u32,
    pub retry_after: i64,
}
impl std::fmt::Debug for ParentalCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ParentalCredential(***)")
    }
}

pub fn kind_slug(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Live => "live",
        ContentKind::Movie => "movie",
        ContentKind::Series => "series",
        ContentKind::Episode => "episode",
        ContentKind::Group => "group",
        ContentKind::VodCategory => "vod_category",
        ContentKind::Settings => "settings",
    }
}
fn kind(value: &str) -> Option<ContentKind> {
    Some(match value {
        "live" => ContentKind::Live,
        "movie" => ContentKind::Movie,
        "series" => ContentKind::Series,
        "episode" => ContentKind::Episode,
        "group" => ContentKind::Group,
        "vod_category" => ContentKind::VodCategory,
        _ => return None,
    })
}
impl Store {
    pub fn parental_credential(&self) -> Result<Option<ParentalCredential>> {
        self.conn
            .query_row(
                "SELECT salt,verifier,recovery_verifier,failures,retry_after \
             FROM parental_security WHERE singleton=1",
                [],
                |r| {
                    Ok(ParentalCredential {
                        salt: r.get(0)?,
                        verifier: r.get(1)?,
                        recovery_verifier: r.get(2)?,
                        failures: r.get(3)?,
                        retry_after: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn set_parental_credential(&mut self, credential: &ParentalCredential) -> Result<()> {
        const SQL: &str = "INSERT INTO parental_security(singleton,salt,verifier,recovery_verifier,failures,retry_after) \
            VALUES(1,?1,?2,?3,?4,?5) ON CONFLICT(singleton) DO UPDATE SET \
            salt=excluded.salt,verifier=excluded.verifier,recovery_verifier=excluded.recovery_verifier, \
            failures=excluded.failures,retry_after=excluded.retry_after";
        self.conn.execute(
            SQL,
            params![
                credential.salt.as_slice(),
                credential.verifier.as_slice(),
                credential.recovery_verifier.as_slice(),
                credential.failures,
                credential.retry_after
            ],
        )?;
        Ok(())
    }
    pub fn set_parental_blocked(&mut self, target: &Target, blocked: bool) -> Result<()> {
        if target.kind == ContentKind::Settings || target.id.is_empty() {
            return Err(crate::StoreError::Unavailable("invalid parental target"));
        }
        // Explicit allow overrides also let a parent unmark a provider's adult
        // category; refreshes do not silently reinstate it.
        const SQL: &str = "INSERT INTO parental_blocks(source_id,kind,content_id,blocked) \
            VALUES(?1,?2,?3,?4) ON CONFLICT(source_id,kind,content_id) DO UPDATE SET blocked=excluded.blocked";
        self.conn.execute(
            SQL,
            params![target.source, kind_slug(target.kind), target.id, blocked],
        )?;
        if !blocked && target.kind == ContentKind::Live {
            self.set_blocked(
                crate::SourceId(target.source),
                &iptv_providers::ContentId::from_stored(target.id.clone()),
                false,
            )?;
        }
        Ok(())
    }
    /// Explicit blocks and provider-labeled adult categories. Parent membership
    /// is resolved from the current catalog, never from a caller's claim.
    pub fn parental_blocks(&self) -> Result<Vec<Target>> {
        let mut q=self.conn.prepare("SELECT source_id,kind,content_id FROM parental_blocks WHERE blocked=1 UNION SELECT source_id,'live',channel_id \
             FROM channel_overlays WHERE blocked=1 UNION SELECT source_id,CASE WHEN kind IN ('live','radio') THEN 'group' \
             ELSE 'vod_category' END,id FROM groups g WHERE adult=1 AND NOT EXISTS(SELECT 1 FROM parental_blocks b WHERE \
             b.source_id=g.source_id AND b.content_id=g.id AND b.kind=CASE WHEN g.kind IN ('live','radio') THEN 'group' \
             ELSE 'vod_category' END AND b.blocked=0)")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut targets = Vec::new();
        for row in rows {
            let (source, k, id) = row?;
            if let Some(kind) = kind(&k) {
                targets.push(Target { source, kind, id });
            }
        }
        Ok(targets)
    }
    /// Resolve only this identity's explicit, legacy and provider-labeled block.
    /// Each branch uses its source/content key; unrelated catalog memberships
    /// are never materialized for a playback authorization.
    pub fn parental_target_blocked(&self, target: &Target) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM parental_blocks WHERE source_id=?1 \
             AND kind=?2 AND content_id=?3 AND blocked=1) \
             OR (?2='live' AND EXISTS(SELECT 1 FROM channel_overlays \
             WHERE source_id=?1 AND channel_id=?3 AND blocked=1)) \
             OR (?2 IN ('group','vod_category') AND EXISTS(SELECT 1 FROM groups \
             WHERE source_id=?1 AND id=?3 AND adult=1 \
             AND CASE WHEN kind IN ('live','radio') THEN 'group' ELSE 'vod_category' END=?2) \
             AND NOT EXISTS(SELECT 1 FROM parental_blocks WHERE source_id=?1 \
             AND kind=?2 AND content_id=?3 AND blocked=0))",
                params![target.source, kind_slug(target.kind), target.id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
    pub fn parental_custom_group_blocked(&self, group: crate::CustomGroupId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM parental_blocks b \
            JOIN custom_groups g ON b.source_id=g.source_id AND b.content_id='custom:'||g.id \
            WHERE g.id=?1 AND b.kind='group' AND b.blocked=1)",
            [group.0],
            |r| r.get(0),
        )?)
    }
    pub fn parental_name(&self, target: &Target) -> Result<Option<String>> {
        let table = match target.kind {
            ContentKind::Live => "channels",
            ContentKind::Movie => "vod",
            ContentKind::Series => "series",
            ContentKind::Group | ContentKind::VodCategory => "groups",
            _ => return Ok(None),
        };
        let sql = format!("SELECT name FROM {table} WHERE source_id=?1 AND id=?2");
        let sql = if target.kind == ContentKind::Group && target.id.starts_with("custom:") {
            "SELECT name FROM custom_groups WHERE source_id=?1 AND 'custom:'||id=?2".into()
        } else if target.kind == ContentKind::Series {
            format!("{sql} UNION SELECT name FROM vod WHERE source_id=?1 AND id=?2 AND is_series=1")
        } else {
            sql
        };
        let name = self
            .conn
            .query_row(&sql, params![target.source, target.id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        let source = self
            .conn
            .query_row(
                "SELECT name FROM sources WHERE id=?1",
                [target.source],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(name
            .map(|name| source.map_or_else(|| name.clone(), |source| format!("{name} · {source}"))))
    }
    pub fn parental_memberships(&self) -> Result<std::collections::BTreeMap<Target, Vec<Target>>> {
        let mut result = std::collections::BTreeMap::<Target, Vec<Target>>::new();
        for (table, column, child, parent) in [
            (
                "channel_groups",
                "channel_id",
                ContentKind::Live,
                ContentKind::Group,
            ),
            (
                "vod_groups",
                "vod_id",
                ContentKind::Movie,
                ContentKind::VodCategory,
            ),
            (
                "series_groups",
                "series_id",
                ContentKind::Series,
                ContentKind::VodCategory,
            ),
        ] {
            let sql = format!("SELECT source_id,{column},group_id FROM {table}");
            let sql = if child == ContentKind::Live {
                format!(
                    "{sql} UNION SELECT g.source_id,m.channel_id,'custom:'||g.id \
                FROM custom_groups g JOIN custom_group_members m ON m.group_id=g.id"
                )
            } else {
                sql
            };
            let mut query = self.conn.prepare(&sql)?;
            let rows = query.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (source, id, group) = row?;
                result
                    .entry(Target {
                        source,
                        kind: child,
                        id,
                    })
                    .or_default()
                    .push(Target {
                        source,
                        kind: parent,
                        id: group,
                    });
            }
        }
        let mut query = self.conn.prepare(
            "SELECT vg.source_id,vg.vod_id,vg.group_id FROM vod_groups vg \
             JOIN vod v ON v.source_id=vg.source_id AND v.id=vg.vod_id WHERE v.is_series=1",
        )?;
        for row in query.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (source, id, group) = row?;
            result
                .entry(Target {
                    source,
                    kind: ContentKind::Series,
                    id,
                })
                .or_default()
                .push(Target {
                    source,
                    kind: ContentKind::VodCategory,
                    id: group,
                });
        }
        Ok(result)
    }
    pub fn parental_ancestors(&self, target: &Target) -> Result<Vec<Target>> {
        let table = match target.kind {
            ContentKind::Live => "channel_groups",
            ContentKind::Movie => "vod_groups",
            ContentKind::Series => "series_groups",
            _ => return Ok(Vec::new()),
        };
        let column = match target.kind {
            ContentKind::Live => "channel_id",
            ContentKind::Movie => "vod_id",
            _ => "series_id",
        };
        let parent_kind = if target.kind == ContentKind::Live {
            ContentKind::Group
        } else {
            ContentKind::VodCategory
        };
        let sql = format!("SELECT group_id FROM {table} WHERE source_id=?1 AND {column}=?2");
        let sql = if target.kind == ContentKind::Live {
            format!(
                "{sql} UNION SELECT 'custom:'||g.id \
            FROM custom_groups g JOIN custom_group_members m ON m.group_id=g.id \
            WHERE g.source_id=?1 AND m.channel_id=?2"
            )
        } else {
            sql
        };
        let sql = if target.kind == ContentKind::Series {
            format!(
                "{sql} UNION SELECT vg.group_id \
            FROM vod_groups vg JOIN vod v ON v.source_id=vg.source_id AND v.id=vg.vod_id \
            WHERE vg.source_id=?1 AND vg.vod_id=?2 AND v.is_series=1"
            )
        } else {
            sql
        };
        let mut query = self.conn.prepare(&sql)?;
        let result = query
            .query_map(params![target.source, target.id], |r| r.get::<_, String>(0))?
            .map(|row| {
                row.map(|id| Target {
                    source: target.source,
                    kind: parent_kind,
                    id,
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(result)
    }
}
