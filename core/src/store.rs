//! Persistent state in one SQLite database: group chain and device records, outgoing transfers
//! (send queue and outbox), the replay seen-cache with its receipts, received items, and the
//! last address each peer answered on. Every state change is committed before the network call
//! that depends on it (design §3).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::envelope::ContentKind;
use crate::group::GroupState;
use crate::signed::SignedObject;
use crate::{DeviceId, Error, Result, TransferId};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS group_versions (version INTEGER PRIMARY KEY, signed TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS records (device_id BLOB PRIMARY KEY, signed TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS seen (
    sender BLOB NOT NULL, transfer_id BLOB NOT NULL, at INTEGER NOT NULL, receipt TEXT,
    PRIMARY KEY (sender, transfer_id));
CREATE TABLE IF NOT EXISTS transfers (
    transfer_id BLOB PRIMARY KEY, target BLOB NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL,
    mime TEXT NOT NULL, size INTEGER NOT NULL, envelope TEXT NOT NULL, envelope_len INTEGER NOT NULL,
    state TEXT NOT NULL, reason TEXT, via_outbox INTEGER NOT NULL, attempts INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL, receipt TEXT);
CREATE INDEX IF NOT EXISTS transfers_target ON transfers (target, state);
CREATE TABLE IF NOT EXISTS received (
    sender BLOB NOT NULL, transfer_id BLOB NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL,
    size INTEGER NOT NULL, location TEXT, at INTEGER NOT NULL, PRIMARY KEY (sender, transfer_id));
CREATE TABLE IF NOT EXISTS peers (device_id BLOB PRIMARY KEY, address TEXT NOT NULL, last_seen INTEGER NOT NULL);
";

/// Persistent state of a send (design §3, protocol.md "Send states").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Sending,
    Delivered,
    /// Target offline and a retry is scheduled, or parked in the outbox for an iPhone.
    Waiting,
    Failed,
}

impl TransferState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Delivered => "delivered",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "queued" => Self::Queued,
            "sending" => Self::Sending,
            "delivered" => Self::Delivered,
            "waiting" => Self::Waiting,
            "failed" => Self::Failed,
            _ => return Err(Error::Db(format!("bad transfer state {s}"))),
        })
    }

    pub fn is_final(self) -> bool {
        matches!(self, Self::Delivered | Self::Failed)
    }
}

pub(crate) fn kind_str(k: ContentKind) -> &'static str {
    match k {
        ContentKind::File => "file",
        ContentKind::Text => "text",
        ContentKind::Link => "link",
    }
}

fn parse_kind(s: &str) -> Result<ContentKind> {
    Ok(match s {
        "file" => ContentKind::File,
        "text" => ContentKind::Text,
        "link" => ContentKind::Link,
        _ => return Err(Error::Db(format!("bad kind {s}"))),
    })
}

/// One outgoing send to one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub transfer_id: TransferId,
    pub target: DeviceId,
    pub kind: ContentKind,
    /// File name; empty for text and links.
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Sealed envelope file in the spool directory; reused unchanged by every retry.
    pub envelope: PathBuf,
    pub envelope_len: u64,
    pub state: TransferState,
    /// Why the transfer is `failed` or `waiting` (error code), if known.
    pub reason: Option<String>,
    /// Parked in this device's outbox for the target to pull (iPhone targets).
    pub via_outbox: bool,
    pub attempts: u32,
    pub next_attempt_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub expires_at: u64,
}

/// One item this device received and stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    pub sender: DeviceId,
    pub transfer_id: TransferId,
    pub kind: ContentKind,
    pub name: String,
    pub size: u64,
    /// Final file path for files; `None` for text and links (handed to the host).
    pub location: Option<PathBuf>,
    pub at: u64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn id16(row: &Row, i: usize) -> rusqlite::Result<[u8; 16]> {
    let v: Vec<u8> = row.get(i)?;
    v.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Blob, "id".into())
    })
}

fn signed_from(json: &str) -> Result<SignedObject> {
    serde_json::from_str(json).map_err(|e| Error::Db(format!("stored object: {e}")))
}

fn to_json(o: &SignedObject) -> String {
    serde_json::to_string(o).expect("signed object serializes")
}

const TRANSFER_COLS: &str = "transfer_id, target, kind, name, mime, size, envelope, envelope_len, \
    state, reason, via_outbox, attempts, next_attempt_at, created_at, updated_at, expires_at";

fn transfer_from(row: &Row) -> rusqlite::Result<(Transfer, String, String)> {
    let kind: String = row.get(2)?;
    let state: String = row.get(8)?;
    let envelope: String = row.get(6)?;
    Ok((
        Transfer {
            transfer_id: id16(row, 0)?,
            target: id16(row, 1)?,
            kind: ContentKind::File,
            name: row.get(3)?,
            mime: row.get(4)?,
            size: row.get::<_, i64>(5)? as u64,
            envelope: PathBuf::from(envelope),
            envelope_len: row.get::<_, i64>(7)? as u64,
            state: TransferState::Queued,
            reason: row.get(9)?,
            via_outbox: row.get::<_, i64>(10)? != 0,
            attempts: row.get::<_, i64>(11)? as u32,
            next_attempt_at: row.get::<_, i64>(12)? as u64,
            created_at: row.get::<_, i64>(13)? as u64,
            updated_at: row.get::<_, i64>(14)? as u64,
            expires_at: row.get::<_, i64>(15)? as u64,
        },
        kind,
        state,
    ))
}

fn finish_transfer(raw: (Transfer, String, String)) -> Result<Transfer> {
    let (mut t, kind, state) = raw;
    t.kind = parse_kind(&kind)?;
    t.state = TransferState::parse(&state)?;
    Ok(t)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(v) => return Err(Error::Db(format!("unknown schema version {v}"))),
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- group ----

    pub fn load_group(&self) -> Result<Option<GroupState>> {
        let conn = self.conn();
        let mut st = conn.prepare("SELECT signed FROM group_versions ORDER BY version")?;
        let versions = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if versions.is_empty() {
            return Ok(None);
        }
        let mut st = conn.prepare("SELECT signed FROM records")?;
        let records = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let versions = versions
            .iter()
            .map(|s| signed_from(s))
            .collect::<Result<_>>()?;
        let records = records
            .iter()
            .map(|s| signed_from(s))
            .collect::<Result<_>>()?;
        GroupState::from_parts(versions, records).map(Some)
    }

    /// Replaces the stored group state (versions are few; records are one per member).
    pub fn save_group(&self, group: &GroupState) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM group_versions", [])?;
        tx.execute("DELETE FROM records", [])?;
        for v in group.chain() {
            tx.execute(
                "INSERT INTO group_versions (version, signed) VALUES (?1, ?2)",
                params![v.list.version as i64, to_json(&v.signed)],
            )?;
        }
        for r in group.records_signed() {
            tx.execute(
                "INSERT INTO records (device_id, signed) VALUES (?1, ?2)",
                params![r.signer.to_vec(), to_json(&r)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes everything tied to the group (used when this device was removed from it).
    pub fn reset_group(&self) -> Result<()> {
        let conn = self.conn();
        conn.execute_batch("DELETE FROM group_versions; DELETE FROM records; DELETE FROM peers;")?;
        Ok(())
    }

    // ---- seen-cache (replay protection) and receipts ----

    /// `Some(receipt)` if `(sender, transfer_id)` was already processed.
    pub fn seen(
        &self,
        sender: &DeviceId,
        tid: &TransferId,
    ) -> Result<Option<Option<SignedObject>>> {
        let conn = self.conn();
        let row: Option<Option<String>> = conn
            .query_row(
                "SELECT receipt FROM seen WHERE sender = ?1 AND transfer_id = ?2",
                params![sender.to_vec(), tid.to_vec()],
                |r| r.get(0),
            )
            .optional()?;
        row.map(|r| r.map(|s| signed_from(&s)).transpose())
            .transpose()
    }

    /// Records a processed envelope together with the receipt we signed for it, and the
    /// received-item log entry, in one transaction.
    pub fn mark_received(&self, item: &Received, receipt: &SignedObject) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO seen (sender, transfer_id, at, receipt) VALUES (?1, ?2, ?3, ?4)",
            params![
                item.sender.to_vec(),
                item.transfer_id.to_vec(),
                item.at as i64,
                to_json(receipt)
            ],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO received (sender, transfer_id, kind, name, size, location, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.sender.to_vec(),
                item.transfer_id.to_vec(),
                kind_str(item.kind),
                item.name,
                item.size as i64,
                item.location
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                item.at as i64
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn purge_seen(&self, before: u64) -> Result<usize> {
        Ok(self
            .conn()
            .execute("DELETE FROM seen WHERE at < ?1", params![before as i64])?)
    }

    pub fn received(&self, limit: u32) -> Result<Vec<Received>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT sender, transfer_id, kind, name, size, location, at FROM received
             ORDER BY at DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map(params![limit], |r| {
                Ok((
                    id16(r, 0)?,
                    id16(r, 1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(sender, transfer_id, kind, name, size, loc, at)| {
                Ok(Received {
                    sender,
                    transfer_id,
                    kind: parse_kind(&kind)?,
                    name,
                    size: size as u64,
                    location: loc.map(PathBuf::from),
                    at: at as u64,
                })
            })
            .collect()
    }

    // ---- outgoing transfers ----

    pub fn insert_transfer(&self, t: &Transfer) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO transfers ({TRANSFER_COLS}) VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
            ),
            params![
                t.transfer_id.to_vec(),
                t.target.to_vec(),
                kind_str(t.kind),
                t.name,
                t.mime,
                t.size as i64,
                t.envelope.to_string_lossy(),
                t.envelope_len as i64,
                t.state.as_str(),
                t.reason,
                t.via_outbox as i64,
                t.attempts as i64,
                t.next_attempt_at as i64,
                t.created_at as i64,
                t.updated_at as i64,
                t.expires_at as i64
            ],
        )?;
        Ok(())
    }

    /// Persists state, reason, attempt counter and schedule of a transfer.
    ///
    /// A final state (`delivered`, `failed`) is never overwritten: concurrent paths (an attempt
    /// finishing, a cancel, an expiry) may hold stale copies. Returns whether the row changed.
    pub fn update_transfer(&self, t: &Transfer) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE transfers SET state = ?2, reason = ?3, attempts = ?4, next_attempt_at = ?5,
             updated_at = ?6, envelope_len = ?7, via_outbox = ?8
             WHERE transfer_id = ?1 AND state NOT IN ('delivered', 'failed')",
            params![
                t.transfer_id.to_vec(),
                t.state.as_str(),
                t.reason,
                t.attempts as i64,
                t.next_attempt_at as i64,
                t.updated_at as i64,
                t.envelope_len as i64,
                t.via_outbox as i64
            ],
        )?;
        Ok(n == 1)
    }

    /// Makes waiting direct sends to `target` due now (the target was just seen online).
    pub fn kick_target(&self, target: &DeviceId, now: u64) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE transfers SET next_attempt_at = ?2
             WHERE target = ?1 AND state = 'waiting' AND via_outbox = 0",
            params![target.to_vec(), now as i64],
        )?)
    }

    pub fn set_receipt(&self, tid: &TransferId, receipt: &SignedObject) -> Result<()> {
        self.conn().execute(
            "UPDATE transfers SET receipt = ?2 WHERE transfer_id = ?1",
            params![tid.to_vec(), to_json(receipt)],
        )?;
        Ok(())
    }

    pub fn transfer(&self, tid: &TransferId) -> Result<Option<Transfer>> {
        let conn = self.conn();
        let raw = conn
            .query_row(
                &format!("SELECT {TRANSFER_COLS} FROM transfers WHERE transfer_id = ?1"),
                params![tid.to_vec()],
                transfer_from,
            )
            .optional()?;
        raw.map(finish_transfer).transpose()
    }

    fn query_transfers(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Transfer>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {TRANSFER_COLS} FROM transfers {filter}"))?;
        let rows = st
            .query_map(args, transfer_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(finish_transfer).collect()
    }

    /// Newest first.
    pub fn transfers(&self, limit: u32) -> Result<Vec<Transfer>> {
        self.query_transfers("ORDER BY created_at DESC, rowid DESC LIMIT ?1", &[&limit])
    }

    /// Transfers that are not final (queued, sending, waiting).
    pub fn open_transfers(&self) -> Result<Vec<Transfer>> {
        self.query_transfers(
            "WHERE state NOT IN ('delivered', 'failed') ORDER BY created_at",
            &[],
        )
    }

    /// Outbox items parked for `target` (protocol.md, "Outbox").
    pub fn outbox_for(&self, target: &DeviceId) -> Result<Vec<Transfer>> {
        self.query_transfers(
            "WHERE target = ?1 AND via_outbox = 1 AND state = 'waiting' ORDER BY created_at",
            &[&target.to_vec()],
        )
    }

    pub fn open_transfers_to(&self, target: &DeviceId) -> Result<Vec<Transfer>> {
        self.query_transfers(
            "WHERE target = ?1 AND state NOT IN ('delivered', 'failed')",
            &[&target.to_vec()],
        )
    }

    // ---- peers ----

    pub fn set_peer_address(&self, id: &DeviceId, address: &str, now: u64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO peers (device_id, address, last_seen) VALUES (?1, ?2, ?3)",
            params![id.to_vec(), address, now as i64],
        )?;
        Ok(())
    }

    pub fn peer_address(&self, id: &DeviceId) -> Result<Option<(String, u64)>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT address, last_seen FROM peers WHERE device_id = ?1",
                params![id.to_vec()],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)),
            )
            .optional()?)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }
}
