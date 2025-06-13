# Task 06: History Database Module

## Objective
Implement encrypted SQLite database for storing clipboard history with automatic cleanup.

## Steps

1. **Create src/history/database.rs**
   - SQLite connection management
   - Schema creation and migrations
   - CRUD operations for clipboard entries

2. **Implement database operations**
   ```rust
   pub struct HistoryDatabase {
       conn: Connection,
       encryption_key: [u8; 32],
   }
   
   impl HistoryDatabase {
       pub fn new(path: &Path, key: &[u8; 32]) -> Result<Self>;
       pub fn insert(&self, content: &ClipboardContent) -> Result<()>;
       pub fn get_recent(&self, count: usize) -> Result<Vec<HistoryEntry>>;
       pub fn get_by_id(&self, id: Uuid) -> Result<HistoryEntry>;
       pub fn search(&self, query: &str) -> Result<Vec<HistoryEntry>>;
       pub fn cleanup_old(&self) -> Result<usize>;
   }
   ```

3. **Create src/history/encryption.rs**
   - AES-256-GCM encryption/decryption
   - Key management with system keyring
   - Secure memory handling with zeroize

4. **Implement key management**
   - Try system keyring first
   - Fall back to SSH-encrypted file
   - Last resort: derive from password
   - Generate new key on first run

5. **Add history operations**
   - Ring buffer behavior (keep last 20)
   - Automatic encryption before storage
   - Checksum verification
   - Compression for large entries

6. **Create migration system**
   - Version tracking
   - Forward migrations only
   - Backup before migration

## Success Criteria
- Database creates and opens correctly
- Encryption/decryption works transparently
- 20-item limit enforced automatically
- Key storage is secure and reliable