# Task 08: WebSocket Transport Layer

## Objective
Implement WebSocket transport with SSH authentication, streaming support, and automatic reconnection.

## Steps

1. **Create src/transport/websocket.rs**
   - WebSocket server and client
   - Message framing and streaming
   - Connection management

2. **Implement transport traits**
   ```rust
   #[async_trait]
   pub trait Transport: Send + Sync {
       async fn connect(&mut self, addr: &str) -> Result<()>;
       async fn accept(&mut self) -> Result<Box<dyn Transport>>;
       async fn send(&mut self, msg: &Message) -> Result<()>;
       async fn recv(&mut self) -> Result<Message>;
       async fn close(&mut self) -> Result<()>;
   }
   
   pub struct WebSocketTransport {
       socket: Option<WebSocketStream<TcpStream>>,
       authenticator: Arc<SshAuthenticator>,
       peer_id: Option<Uuid>,
   }
   ```

3. **Add streaming support**
   - Chunk large payloads (64KB chunks)
   - Reassembly with timeout (1 second)
   - Progress tracking
   - Backpressure handling

4. **Implement authentication flow**
   - Exchange public keys
   - Challenge-response auth
   - Establish encrypted channel
   - Perfect forward secrecy

5. **Add connection management**
   - Automatic reconnection with backoff
   - Connection pooling
   - Health checks/heartbeats
   - Graceful shutdown

6. **Create message protocol**
   - Binary format with bincode
   - Message types (clipboard, history, control)
   - Compression for large payloads
   - Checksums for integrity

## Success Criteria
- Connections establish reliably
- Authentication prevents unauthorized access
- Large payloads stream efficiently
- Reconnection works transparently