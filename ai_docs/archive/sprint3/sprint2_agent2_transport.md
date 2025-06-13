# Sprint 2 - Agent 2: WebSocket Transport Layer

## Context
You are Agent 2 for Sprint 2 of the ClipSync project. Your focus is implementing the WebSocket transport layer that will handle all network communication between peers.

## Prerequisites
- Sprint 1 has been completed
- Agent 1 is developing the SSH authentication module
- You will integrate with Agent 1's authentication API

## Your Tasks (Task 08)

### 1. WebSocket Transport Implementation
Create the core transport in `src/transport/websocket.rs`:
- WebSocket server and client implementation
- TLS support for secure connections
- Message framing and protocol handling
- Connection state management

### 2. Streaming Support
Implement streaming in `src/transport/stream.rs`:
- Handle large clipboard payloads efficiently
- Chunk large messages for streaming
- Progress tracking for transfers
- Backpressure handling

### 3. Reconnection Logic
Create robust reconnection in `src/transport/reconnect.rs`:
- Automatic reconnection on disconnect
- Exponential backoff strategy
- Connection health monitoring
- Graceful degradation

### 4. Transport API
Define the public transport API:
```rust
pub trait Transport {
    async fn connect(peer: &PeerInfo, auth: &dyn Authenticator) -> Result<Connection>;
    async fn listen(addr: SocketAddr, auth: &dyn Authenticator) -> Result<Listener>;
    async fn send(&mut self, msg: Message) -> Result<()>;
    async fn receive(&mut self) -> Result<Message>;
}
```

## Key Design Decisions
- Use WebSockets for firewall traversal
- Support both TCP and Unix sockets
- Implement proper flow control
- Handle network interruptions gracefully

## Integration Points
- Use Agent 1's Authenticator trait for auth
- Integrate with service discovery (Agent 3)
- Use config module for transport settings
- Log all connection events

## Testing Requirements
- Unit tests for message framing
- Integration tests with mock WebSocket server
- Network failure simulation tests
- Performance tests for streaming

## Success Criteria
- Establish authenticated WebSocket connections
- Stream large payloads efficiently
- Handle reconnections automatically
- Clean API for sync engine integration
- All tests passing

## Files to Create/Modify
- `src/transport/mod.rs` - Module declaration
- `src/transport/websocket.rs` - WebSocket implementation
- `src/transport/stream.rs` - Streaming support
- `src/transport/reconnect.rs` - Reconnection logic
- `src/transport/protocol.rs` - Wire protocol
- `src/lib.rs` - Export transport module
- Tests in `src/transport/` subdirectories

## Dependencies
- `tokio-tungstenite` for WebSocket support
- `tokio` for async runtime
- `bytes` for efficient buffer handling
- `tracing` for logging

## Coordination Notes
- Wait for Agent 1's Authenticator trait definition
- Coordinate with Agent 3 on peer addressing
- Document wire protocol for future reference

Remember: You depend on Agent 1's authentication API, so coordinate early on the interface.