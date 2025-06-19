//! SQLite database implementation for clipboard history

use crate::history::{
    encryption::{EncryptedData, Encryptor},
    ClipboardContent, HistoryEntry,
};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use tokio::sync::Mutex;
use uuid::Uuid;

const SCHEMA_VERSION: u32 = 1;
const HISTORY_LIMIT: usize = 20;

/// SQLite database wrapper for clipboard history storage
pub struct HistoryDatabase {
    conn: Mutex<Connection>,
}

impl HistoryDatabase {
    /// Create new database instance with encryption key
    pub async fn new(path: &Path, _key: &[u8; 32]) -> Result<Self> {
        // Create directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )?;

        let db = Self {
            conn: Mutex::new(conn),
        };

        db.initialize().await?;
        Ok(db)
    }

    async fn initialize(&self) -> Result<()> {
        let conn = self.conn.lock().await;

        // Check if we need to create or migrate the schema
        let version = self.get_schema_version(&conn)?;

        if version == 0 {
            self.create_schema(&conn)?;
        } else if version < SCHEMA_VERSION {
            self.migrate_schema(&conn, version)?;
        }

        Ok(())
    }

    fn get_schema_version(&self, conn: &Connection) -> Result<u32> {
        // First check if the schema_version table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
                [],
                |row| row.get(0),
            )?;

        if !table_exists {
            return Ok(0);
        }

        let version: Option<u32> = conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        Ok(version.unwrap_or(0))
    }

    fn create_schema(&self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER DEFAULT (strftime('%s', 'now'))
            );

            CREATE TABLE IF NOT EXISTS clipboard_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                content BLOB NOT NULL,
                content_type TEXT NOT NULL,
                content_size INTEGER NOT NULL,
                checksum TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                origin_node TEXT NOT NULL,
                iv BLOB NOT NULL,
                compressed INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER DEFAULT (strftime('%s', 'now'))
            );

            CREATE INDEX idx_timestamp ON clipboard_history(timestamp DESC);
            CREATE INDEX idx_content_type ON clipboard_history(content_type);

            -- Trigger to maintain history limit
            CREATE TRIGGER limit_history_size
            AFTER INSERT ON clipboard_history
            BEGIN
                DELETE FROM clipboard_history
                WHERE id IN (
                    SELECT id FROM clipboard_history
                    ORDER BY timestamp DESC
                    LIMIT -1 OFFSET 20
                );
            END;
            ",
        )?;

        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?)",
            params![SCHEMA_VERSION],
        )?;

        Ok(())
    }

    fn migrate_schema(&self, _conn: &Connection, _from_version: u32) -> Result<()> {
        // Future migrations would go here
        Ok(())
    }

    /// Insert new clipboard content into history
    pub async fn insert(&self, content: &ClipboardContent, encryptor: &Encryptor) -> Result<()> {
        let checksum = Encryptor::compute_checksum(&content.content);
        let encrypted = encryptor.encrypt(&content.content)?;

        let conn = self.conn.lock().await;

        conn.execute(
            "INSERT INTO clipboard_history
             (uuid, content, content_type, content_size, checksum, timestamp, origin_node, iv, compressed)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                content.id.to_string(),
                &encrypted.ciphertext,
                &content.content_type,
                content.content.len() as i64,
                &checksum,
                content.timestamp,
                content.origin_node.to_string(),
                &encrypted.nonce,
                encrypted.compressed as i32,
            ],
        )?;

        Ok(())
    }

    /// Get recent entries from history
    pub async fn get_recent(
        &self,
        count: usize,
        encryptor: &Encryptor,
    ) -> Result<Vec<HistoryEntry>> {
        let conn = self.conn.lock().await;

        // Ensure we don't request more than the limit
        let count = count.min(HISTORY_LIMIT);

        let mut stmt = conn.prepare(
            "SELECT uuid, content, content_type, content_size, checksum, timestamp, origin_node, iv, compressed
             FROM clipboard_history
             ORDER BY timestamp DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map(params![count], |row| Ok(self.row_to_entry(row, encryptor)))?
            .collect::<Result<Vec<_>, _>>()?;

        entries.into_iter().collect()
    }

    /// Get entry by index position
    pub async fn get_by_index(&self, index: u8, encryptor: &Encryptor) -> Result<HistoryEntry> {
        let conn = self.conn.lock().await;

        let entry = conn
            .query_row(
                "SELECT uuid, content, content_type, content_size, checksum, timestamp, origin_node, iv, compressed
                 FROM clipboard_history
                 ORDER BY timestamp DESC
                 LIMIT 1 OFFSET ?",
                params![index as i64],
                |row| Ok(self.row_to_entry(row, encryptor)),
            )?;

        entry
    }

    /// Search for entries containing query text
    pub async fn search(&self, query: &str, encryptor: &Encryptor) -> Result<Vec<HistoryEntry>> {
        let conn = self.conn.lock().await;

        // For security, we can't search encrypted content directly
        // Instead, we'll decrypt and search in memory
        let mut stmt = conn.prepare(
            "SELECT uuid, content, content_type, content_size, checksum, timestamp, origin_node, iv, compressed
             FROM clipboard_history
             WHERE content_type LIKE 'text/%'
             ORDER BY timestamp DESC",
        )?;

        let entries: Vec<Result<HistoryEntry>> = stmt
            .query_map([], |row| Ok(self.row_to_entry(row, encryptor)))?
            .collect::<Result<Vec<_>, _>>()?;

        // Filter entries that match the query
        let mut results = Vec::new();
        for entry_result in entries {
            match entry_result {
                Ok(entry) => {
                    if let Ok(text) = String::from_utf8(entry.content.clone()) {
                        if text.to_lowercase().contains(&query.to_lowercase()) {
                            results.push(entry);
                        }
                    }
                }
                Err(_) => {}
            }
        }

        Ok(results)
    }

    /// Clear all entries from history
    pub async fn clear(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM clipboard_history", [])?;
        Ok(())
    }

    fn row_to_entry(&self, row: &Row, encryptor: &Encryptor) -> Result<HistoryEntry> {
        let uuid: String = row.get(0)?;
        let ciphertext: Vec<u8> = row.get(1)?;
        let content_type: String = row.get(2)?;
        let content_size: i64 = row.get(3)?;
        let checksum: String = row.get(4)?;
        let timestamp: i64 = row.get(5)?;
        let origin_node: String = row.get(6)?;
        let iv: Vec<u8> = row.get(7)?;
        let compressed: i32 = row.get(8)?;

        let encrypted = EncryptedData {
            ciphertext,
            nonce: iv,
            compressed: compressed != 0,
        };

        let content = encryptor.decrypt(&encrypted)?;

        // Verify checksum
        let computed_checksum = Encryptor::compute_checksum(&content);
        if computed_checksum != checksum {
            return Err(anyhow!("Checksum mismatch for entry {}", uuid));
        }

        Ok(HistoryEntry {
            id: Uuid::parse_str(&uuid)?,
            content,
            content_type,
            content_size: content_size as u64,
            timestamp,
            origin_node: Uuid::parse_str(&origin_node)?,
            checksum,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn setup_test_db() -> Result<(HistoryDatabase, Encryptor, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // Create a test encryptor with a fixed key
        use aes_gcm::aead::{OsRng, rand_core::RngCore};
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);

        let encryptor = Encryptor::new_for_tests(key)?;

        let db = HistoryDatabase::new(&db_path, encryptor.get_key()).await?;
        Ok((db, encryptor, temp_dir))
    }

    #[tokio::test]
    async fn test_insert_and_retrieve() {
        let (db, encryptor, _temp_dir) = setup_test_db().await.unwrap();

        let content = ClipboardContent {
            id: Uuid::new_v4(),
            content: b"Test content".to_vec(),
            content_type: "text/plain".to_string(),
            timestamp: Utc::now().timestamp(),
            origin_node: Uuid::new_v4(),
        };

        db.insert(&content, &encryptor).await.unwrap();

        let entries = db.get_recent(1, &encryptor).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, content.content);
    }

    #[tokio::test]
    async fn test_history_limit() {
        let (db, encryptor, _temp_dir) = setup_test_db().await.unwrap();

        // Insert 25 items
        for i in 0..25 {
            let content = ClipboardContent {
                id: Uuid::new_v4(),
                content: format!("Item {}", i).into_bytes(),
                content_type: "text/plain".to_string(),
                timestamp: Utc::now().timestamp() + i,
                origin_node: Uuid::new_v4(),
            };
            db.insert(&content, &encryptor).await.unwrap();
        }

        // Should only have 20 items
        let entries = db.get_recent(30, &encryptor).await.unwrap();
        assert_eq!(entries.len(), 20);

        // Should have the most recent items
        assert_eq!(
            String::from_utf8(entries[0].content.clone()).unwrap(),
            "Item 24"
        );
    }

    #[tokio::test]
    async fn test_search() {
        let (db, encryptor, _temp_dir) = setup_test_db().await.unwrap();

        let contents = vec![
            ("Hello world", "text/plain"),
            ("Goodbye world", "text/plain"),
            ("Random image data", "image/png"),
        ];

        for (text, mime) in contents {
            let content = ClipboardContent {
                id: Uuid::new_v4(),
                content: text.as_bytes().to_vec(),
                content_type: mime.to_string(),
                timestamp: Utc::now().timestamp(),
                origin_node: Uuid::new_v4(),
            };
            db.insert(&content, &encryptor).await.unwrap();
        }

        let results = db.search("world", &encryptor).await.unwrap();
        assert_eq!(results.len(), 2);
    }
}
