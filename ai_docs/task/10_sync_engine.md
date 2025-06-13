# Task 10: Synchronization Engine

## Objective
Implement the core synchronization logic that coordinates clipboard monitoring, history, and peer communication.

## Steps

1. **Create src/sync/mod.rs**
   - Main sync loop
   - Event handling
   - State management

2. **Implement sync engine**
   ```rust
   pub struct SyncEngine {
       clipboard: Arc<dyn ClipboardProvider>,
       transport: Arc<Mutex<dyn Transport>>,
       history: Arc<HistoryDatabase>,
       config: Arc<Config>,
       state: Arc<Mutex<SyncState>>,
   }
   
   impl SyncEngine {
       pub async fn start(&self) -> Result<()>;
       pub async fn stop(&self) -> Result<()>;
       pub async fn handle_local_change(&self, content: ClipboardContent) -> Result<()>;
       pub async fn handle_remote_change(&self, msg: Message) -> Result<()>;
   }
   ```

3. **Add clipboard monitoring**
   - Watch for local clipboard changes
   - Debounce rapid changes (100ms)
   - Filter self-triggered updates
   - Handle large content efficiently

4. **Implement sync protocol**
   - Send updates to all connected peers
   - Handle incoming updates
   - Resolve conflicts (latest wins)
   - Update local clipboard

5. **Add history integration**
   - Store all clipboard changes
   - Handle history requests
   - Implement history cycling
   - Sync history between peers

6. **Create control flow**
   - Pause/resume synchronization
   - Selective sync (text only, etc.)
   - Bandwidth throttling
   - Error recovery

## Success Criteria
- Bidirectional sync works reliably
- History maintained correctly
- Conflicts resolved consistently
- Performance acceptable for 5MB payloads