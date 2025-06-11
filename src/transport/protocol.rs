//! Wire protocol definitions for ClipSync transport layer
//!
//! This module defines the message format and protocol used for
//! communication between ClipSync peers over WebSocket connections.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;
use crate::auth::PeerId;

/// Protocol version for compatibility checking
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Connection identifier type
pub type ConnectionId = Uuid;

/// Wire format message container
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Message type for routing
    pub message_type: MessageType,
    
    /// Message payload
    pub payload: MessagePayload,
    
    /// Message sequence number for ordering
    pub sequence: u64,
    
    /// Message timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// Optional message correlation ID
    pub correlation_id: Option<Uuid>,
    
    /// Protocol version
    pub version: String,
}

/// Message type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// Protocol handshake initiation
    Handshake,
    
    /// Handshake response
    HandshakeResponse,
    
    /// Authentication challenge
    AuthChallenge,
    
    /// Authentication response
    AuthResponse,
    
    /// Authentication result
    AuthResult,
    
    /// Clipboard data transfer
    ClipboardData,
    
    /// Large payload streaming start
    StreamStart,
    
    /// Stream chunk
    StreamChunk,
    
    /// Stream completion
    StreamEnd,
    
    /// Stream acknowledgment
    StreamAck,
    
    /// Connection keep-alive
    KeepAlive,
    
    /// Connection close notification
    Close,
    
    /// Error message
    Error,
    
    /// Capability negotiation
    Capabilities,
    
    /// Status update
    Status,
}

/// Message payload variants
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum MessagePayload {
    /// Handshake payload
    Handshake(HandshakePayload),
    
    /// Authentication payload
    Auth(AuthPayload),
    
    /// Clipboard data payload
    Clipboard(ClipboardData),
    
    /// Streaming payload
    Stream(StreamPayload),
    
    /// Keep-alive payload (empty)
    KeepAlive,
    
    /// Close payload
    Close(ClosePayload),
    
    /// Error payload
    Error(ErrorPayload),
    
    /// Capabilities payload
    Capabilities(CapabilitiesPayload),
    
    /// Status payload
    Status(StatusPayload),
}

/// Handshake message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandshakePayload {
    /// Client/server protocol version
    pub version: String,
    
    /// Peer identification
    pub peer_id: Uuid,
    
    /// Supported capabilities
    pub capabilities: Vec<String>,
    
    /// Optional connection parameters
    pub parameters: std::collections::HashMap<String, String>,
}

/// Authentication message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthPayload {
    /// Authentication method
    pub method: String,
    
    /// Authentication data (base64 encoded)
    pub data: String,
    
    /// Authentication step/phase
    pub step: u32,
    
    /// Authentication result
    pub result: Option<AuthResult>,
}

/// Authentication result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthResult {
    /// Authentication successful
    Success { token: String, peer_id: PeerId },
    
    /// Authentication failed
    Failed { reason: String },
    
    /// Continue authentication (multi-step)
    Continue,
}

/// Clipboard data payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipboardData {
    /// Data format/MIME type
    pub format: ClipboardFormat,
    
    /// Clipboard content data
    pub data: Vec<u8>,
    
    /// Data compression method
    pub compression: Option<CompressionMethod>,
    
    /// Data checksum for integrity
    pub checksum: String,
    
    /// Optional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// Clipboard data format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardFormat {
    /// Plain text
    Text,
    
    /// HTML content
    Html,
    
    /// Rich text format
    Rtf,
    
    /// Image data
    Image { mime_type: String },
    
    /// File list
    Files,
    
    /// Binary data
    Binary { mime_type: String },
    
    /// Custom format
    Custom { format_name: String },
}

/// Compression method for large payloads
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompressionMethod {
    /// No compression
    None,
    
    /// Zstandard compression
    Zstd,
    
    /// Gzip compression
    Gzip,
}

/// Streaming message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamPayload {
    /// Stream operation type
    pub operation: StreamOperation,
    
    /// Stream identifier
    pub stream_id: Uuid,
    
    /// Stream metadata
    pub metadata: Option<StreamMetadata>,
    
    /// Chunk data for stream chunks
    pub data: Option<Vec<u8>>,
    
    /// Chunk sequence number
    pub chunk_sequence: Option<u64>,
    
    /// Stream completion info
    pub completion: Option<StreamCompletion>,
}

/// Stream operation types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamOperation {
    /// Start a new stream
    Start,
    
    /// Send a chunk of data
    Chunk,
    
    /// End the stream
    End,
    
    /// Acknowledge chunk receipt
    Ack,
    
    /// Cancel the stream
    Cancel,
}

/// Stream metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamMetadata {
    /// Total size of the stream
    pub total_size: u64,
    
    /// Expected number of chunks
    pub total_chunks: u64,
    
    /// Chunk size
    pub chunk_size: usize,
    
    /// Content type
    pub content_type: ClipboardFormat,
    
    /// Compression method
    pub compression: CompressionMethod,
    
    /// Stream checksum
    pub checksum: String,
}

/// Stream completion information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamCompletion {
    /// Whether stream completed successfully
    pub success: bool,
    
    /// Number of chunks received
    pub chunks_received: u64,
    
    /// Total bytes received
    pub bytes_received: u64,
    
    /// Error message if failed
    pub error: Option<String>,
}

/// Close message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClosePayload {
    /// Close reason code
    pub code: CloseCode,
    
    /// Human-readable close reason
    pub reason: String,
}

/// Connection close codes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CloseCode {
    /// Normal close
    Normal,
    
    /// Authentication failed
    AuthFailed,
    
    /// Protocol error
    ProtocolError,
    
    /// Version mismatch
    VersionMismatch,
    
    /// Server shutdown
    ServerShutdown,
    
    /// Client disconnect
    ClientDisconnect,
    
    /// Connection timeout
    Timeout,
    
    /// Unknown error
    Unknown,
}

/// Error message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorPayload {
    /// Error code
    pub code: ErrorCode,
    
    /// Error message
    pub message: String,
    
    /// Optional error details
    pub details: Option<std::collections::HashMap<String, String>>,
}

/// Error codes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    /// Authentication error
    AuthError,
    
    /// Protocol violation
    ProtocolError,
    
    /// Message too large
    MessageTooLarge,
    
    /// Unsupported operation
    UnsupportedOperation,
    
    /// Rate limit exceeded
    RateLimitExceeded,
    
    /// Internal server error
    InternalError,
    
    /// Bad request format
    BadRequest,
    
    /// Resource not found
    NotFound,
    
    /// Service unavailable
    ServiceUnavailable,
}

/// Capabilities negotiation payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilitiesPayload {
    /// Supported clipboard formats
    pub formats: Vec<ClipboardFormat>,
    
    /// Supported compression methods
    pub compression: Vec<CompressionMethod>,
    
    /// Maximum message size
    pub max_message_size: usize,
    
    /// Streaming support
    pub streaming_support: bool,
    
    /// Keep-alive interval preference
    pub keepalive_interval: u64,
    
    /// Additional capabilities
    pub extensions: std::collections::HashMap<String, String>,
}

/// Status message payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusPayload {
    /// Connection status
    pub status: ConnectionStatus,
    
    /// Optional status message
    pub message: Option<String>,
    
    /// Status-specific data
    pub data: Option<std::collections::HashMap<String, String>>,
}

/// Connection status enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Connection is healthy
    Healthy,
    
    /// Connection experiencing issues
    Degraded,
    
    /// Connection is busy
    Busy,
    
    /// Connection is idle
    Idle,
    
    /// Connection is closing
    Closing,
}

impl Message {
    /// Create a new message with the current timestamp
    pub fn new(message_type: MessageType, payload: MessagePayload) -> Self {
        Self {
            message_type,
            payload,
            sequence: 0, // Will be set by transport layer
            timestamp: chrono::Utc::now(),
            correlation_id: None,
            version: PROTOCOL_VERSION.to_string(),
        }
    }
    
    /// Create a message with correlation ID
    pub fn with_correlation_id(
        message_type: MessageType, 
        payload: MessagePayload, 
        correlation_id: Uuid
    ) -> Self {
        let mut msg = Self::new(message_type, payload);
        msg.correlation_id = Some(correlation_id);
        msg
    }
    
    /// Set the sequence number
    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
        self
    }
    
    /// Check if this is a response to another message
    pub fn is_response_to(&self, other: &Message) -> bool {
        self.correlation_id.is_some() && 
        self.correlation_id == Some(other.correlation_id.unwrap_or(Uuid::new_v4()))
    }
    
    /// Get message size in bytes (approximate)
    pub fn size(&self) -> usize {
        // Rough estimate - in practice would use actual serialized size
        std::mem::size_of_val(self) + 
        match &self.payload {
            MessagePayload::Clipboard(data) => data.data.len(),
            MessagePayload::Stream(stream) => stream.data.as_ref().map_or(0, |d| d.len()),
            _ => 0,
        }
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageType::Handshake => write!(f, "HANDSHAKE"),
            MessageType::HandshakeResponse => write!(f, "HANDSHAKE_RESPONSE"),
            MessageType::AuthChallenge => write!(f, "AUTH_CHALLENGE"),
            MessageType::AuthResponse => write!(f, "AUTH_RESPONSE"),
            MessageType::AuthResult => write!(f, "AUTH_RESULT"),
            MessageType::ClipboardData => write!(f, "CLIPBOARD_DATA"),
            MessageType::StreamStart => write!(f, "STREAM_START"),
            MessageType::StreamChunk => write!(f, "STREAM_CHUNK"),
            MessageType::StreamEnd => write!(f, "STREAM_END"),
            MessageType::StreamAck => write!(f, "STREAM_ACK"),
            MessageType::KeepAlive => write!(f, "KEEP_ALIVE"),
            MessageType::Close => write!(f, "CLOSE"),
            MessageType::Error => write!(f, "ERROR"),
            MessageType::Capabilities => write!(f, "CAPABILITIES"),
            MessageType::Status => write!(f, "STATUS"),
        }
    }
}

impl Default for CompressionMethod {
    fn default() -> Self {
        CompressionMethod::Zstd
    }
}

impl ClipboardFormat {
    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &str {
        match self {
            ClipboardFormat::Text => "text/plain",
            ClipboardFormat::Html => "text/html", 
            ClipboardFormat::Rtf => "text/rtf",
            ClipboardFormat::Image { mime_type } => mime_type,
            ClipboardFormat::Files => "application/x-file-list",
            ClipboardFormat::Binary { mime_type } => mime_type,
            ClipboardFormat::Custom { format_name } => format_name,
        }
    }
    
    /// Check if this format supports large payloads
    pub fn supports_streaming(&self) -> bool {
        matches!(self, 
            ClipboardFormat::Image { .. } | 
            ClipboardFormat::Files |
            ClipboardFormat::Binary { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_message_creation() {
        let payload = MessagePayload::KeepAlive;
        let msg = Message::new(MessageType::KeepAlive, payload);
        
        assert_eq!(msg.message_type, MessageType::KeepAlive);
        assert_eq!(msg.version, PROTOCOL_VERSION);
        assert!(msg.correlation_id.is_none());
    }
    
    #[test]
    fn test_message_with_correlation() {
        let correlation_id = Uuid::new_v4();
        let payload = MessagePayload::KeepAlive;
        let msg = Message::with_correlation_id(
            MessageType::KeepAlive, 
            payload, 
            correlation_id
        );
        
        assert_eq!(msg.correlation_id, Some(correlation_id));
    }
    
    #[test]
    fn test_clipboard_format_mime_types() {
        assert_eq!(ClipboardFormat::Text.mime_type(), "text/plain");
        assert_eq!(ClipboardFormat::Html.mime_type(), "text/html");
        
        let image = ClipboardFormat::Image { 
            mime_type: "image/png".to_string() 
        };
        assert_eq!(image.mime_type(), "image/png");
    }
    
    #[test]
    fn test_format_streaming_support() {
        assert!(!ClipboardFormat::Text.supports_streaming());
        assert!(ClipboardFormat::Files.supports_streaming());
        assert!(ClipboardFormat::Image { 
            mime_type: "image/png".to_string() 
        }.supports_streaming());
    }
    
    #[test]
    fn test_message_type_display() {
        assert_eq!(MessageType::Handshake.to_string(), "HANDSHAKE");
        assert_eq!(MessageType::ClipboardData.to_string(), "CLIPBOARD_DATA");
        assert_eq!(MessageType::StreamStart.to_string(), "STREAM_START");
    }
    
    #[test]
    fn test_message_serialization() {
        let data = ClipboardData {
            format: ClipboardFormat::Text,
            data: b"Hello, world!".to_vec(),
            compression: None,
            checksum: "abc123".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        
        let payload = MessagePayload::Clipboard(data);
        let msg = Message::new(MessageType::ClipboardData, payload);
        
        // Test serialization round-trip
        let serialized = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&serialized).unwrap();
        
        assert_eq!(msg, deserialized);
    }
}