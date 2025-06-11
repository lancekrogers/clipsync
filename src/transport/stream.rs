//! Streaming support for large clipboard payloads
//!
//! This module handles efficient streaming of large clipboard data
//! using chunked transfer with progress tracking and flow control.

use crate::transport::{
    protocol::*, Result, TransportError, Connection, PeerInfo,
};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Default chunk size for streaming (64KB)
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Maximum number of in-flight chunks
pub const MAX_IN_FLIGHT_CHUNKS: usize = 10;

/// Stream chunk data
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// Stream identifier
    pub stream_id: Uuid,
    
    /// Chunk sequence number
    pub sequence: u64,
    
    /// Chunk data
    pub data: Vec<u8>,
    
    /// Whether this is the final chunk
    pub is_final: bool,
    
    /// Chunk checksum
    pub checksum: String,
}

/// Streaming progress update
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Stream identifier
    pub stream_id: Uuid,
    
    /// Bytes transferred so far
    pub bytes_transferred: u64,
    
    /// Total bytes to transfer
    pub total_bytes: u64,
    
    /// Transfer rate in bytes/second
    pub transfer_rate: f64,
    
    /// Estimated time remaining
    pub eta_seconds: Option<f64>,
    
    /// Current chunk being processed
    pub current_chunk: u64,
    
    /// Total chunks
    pub total_chunks: u64,
}

/// Streaming transport wrapper
pub struct StreamingTransport {
    /// Underlying connection
    connection: Box<dyn Connection>,
    
    /// Active outbound streams
    outbound_streams: HashMap<Uuid, OutboundStream>,
    
    /// Active inbound streams
    inbound_streams: HashMap<Uuid, InboundStream>,
    
    /// Progress update sender
    progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
    
    /// Stream configuration
    config: StreamConfig,
    
    /// Next sequence number
    next_sequence: u64,
}

/// Stream configuration
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Chunk size for streaming
    pub chunk_size: usize,
    
    /// Maximum in-flight chunks
    pub max_in_flight: usize,
    
    /// Stream timeout
    pub timeout: std::time::Duration,
    
    /// Enable compression for streams
    pub enable_compression: bool,
    
    /// Compression method to use
    pub compression_method: CompressionMethod,
}

/// Outbound stream state
struct OutboundStream {
    /// Stream metadata
    metadata: StreamMetadata,
    
    /// Remaining data to send
    data: Vec<u8>,
    
    /// Current position in data
    position: usize,
    
    /// Chunks sent but not acknowledged
    in_flight: HashMap<u64, StreamChunk>,
    
    /// Next chunk sequence number
    next_sequence: u64,
    
    /// Completion notification
    completion_tx: Option<oneshot::Sender<Result<()>>>,
    
    /// Start time for rate calculation
    start_time: std::time::Instant,
    
    /// Bytes acknowledged
    bytes_acked: u64,
}

/// Inbound stream state
struct InboundStream {
    /// Stream metadata
    metadata: StreamMetadata,
    
    /// Received chunks
    chunks: HashMap<u64, Vec<u8>>,
    
    /// Next expected sequence number
    next_expected: u64,
    
    /// Assembled data buffer
    assembled_data: Vec<u8>,
    
    /// Completion notification
    completion_tx: Option<oneshot::Sender<Result<ClipboardData>>>,
    
    /// Start time
    start_time: std::time::Instant,
    
    /// Last progress update time
    last_progress: std::time::Instant,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_in_flight: MAX_IN_FLIGHT_CHUNKS,
            timeout: std::time::Duration::from_secs(300), // 5 minutes
            enable_compression: true,
            compression_method: CompressionMethod::Zstd,
        }
    }
}

impl StreamingTransport {
    /// Create a new streaming transport wrapper
    pub fn new(
        connection: Box<dyn Connection>,
        config: StreamConfig,
    ) -> (Self, mpsc::UnboundedReceiver<ProgressUpdate>) {
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        
        let transport = Self {
            connection,
            outbound_streams: HashMap::new(),
            inbound_streams: HashMap::new(),
            progress_tx,
            config,
            next_sequence: 1,
        };
        
        (transport, progress_rx)
    }
    
    /// Send large clipboard data using streaming
    pub async fn send_clipboard_stream(
        &mut self,
        data: ClipboardData,
    ) -> Result<oneshot::Receiver<Result<()>>> {
        if data.data.len() <= self.config.chunk_size {
            // Small payload, send directly
            let message = Message::new(
                MessageType::ClipboardData,
                MessagePayload::Clipboard(data),
            );
            self.connection.send(message).await?;
            
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Ok(()));
            return Ok(rx);
        }
        
        info!("Starting clipboard stream of {} bytes", data.data.len());
        
        let stream_id = Uuid::new_v4();
        let total_chunks = (data.data.len() + self.config.chunk_size - 1) / self.config.chunk_size;
        
        // Compress data if enabled
        let compressed_data = if self.config.enable_compression {
            self.compress_data(&data.data)?
        } else {
            data.data.clone()
        };
        
        // Create stream metadata
        let metadata = StreamMetadata {
            total_size: compressed_data.len() as u64,
            total_chunks: total_chunks as u64,
            chunk_size: self.config.chunk_size,
            content_type: data.format.clone(),
            compression: if self.config.enable_compression {
                self.config.compression_method.clone()
            } else {
                CompressionMethod::None
            },
            checksum: self.calculate_checksum(&compressed_data),
        };
        
        // Send stream start message
        let stream_payload = StreamPayload {
            operation: StreamOperation::Start,
            stream_id,
            metadata: Some(metadata.clone()),
            data: None,
            chunk_sequence: None,
            completion: None,
        };
        
        let start_message = Message::new(
            MessageType::StreamStart,
            MessagePayload::Stream(stream_payload),
        );
        
        self.connection.send(start_message).await?;
        
        // Create completion channel
        let (completion_tx, completion_rx) = oneshot::channel();
        
        // Create outbound stream
        let outbound_stream = OutboundStream {
            metadata,
            data: compressed_data,
            position: 0,
            in_flight: HashMap::new(),
            next_sequence: 1,
            completion_tx: Some(completion_tx),
            start_time: std::time::Instant::now(),
            bytes_acked: 0,
        };
        
        self.outbound_streams.insert(stream_id, outbound_stream);
        
        // Start sending chunks
        self.send_next_chunks(stream_id).await?;
        
        Ok(completion_rx)
    }
    
    /// Handle incoming stream messages
    pub async fn handle_stream_message(&mut self, message: Message) -> Result<Option<ClipboardData>> {
        if let MessagePayload::Stream(payload) = message.payload {
            match payload.operation {
                StreamOperation::Start => {
                    self.handle_stream_start(payload).await
                }
                StreamOperation::Chunk => {
                    self.handle_stream_chunk(payload).await
                }
                StreamOperation::End => {
                    self.handle_stream_end(payload).await
                }
                StreamOperation::Ack => {
                    self.handle_stream_ack(payload).await?;
                    Ok(None)
                }
                StreamOperation::Cancel => {
                    self.handle_stream_cancel(payload).await?;
                    Ok(None)
                }
            }
        } else {
            Err(TransportError::Streaming(
                "Invalid message type for stream handler".to_string()
            ))
        }
    }
    
    /// Send next available chunks for a stream
    async fn send_next_chunks(&mut self, stream_id: Uuid) -> Result<()> {
        let stream = self.outbound_streams.get_mut(&stream_id)
            .ok_or_else(|| TransportError::Streaming("Stream not found".to_string()))?;
        
        while stream.in_flight.len() < self.config.max_in_flight 
            && stream.position < stream.data.len() {
            
            let chunk_size = std::cmp::min(
                self.config.chunk_size,
                stream.data.len() - stream.position
            );
            
            let chunk_data = stream.data[stream.position..stream.position + chunk_size].to_vec();
            let is_final = stream.position + chunk_size >= stream.data.len();
            let next_sequence = stream.next_sequence;
            
            // Calculate checksum before borrowing stream again
            let checksum = {
                use sha2::{Sha256, Digest};
                let mut hasher = Sha256::new();
                hasher.update(&chunk_data);
                hex::encode(hasher.finalize())
            };
            
            let chunk = StreamChunk {
                stream_id,
                sequence: next_sequence,
                data: chunk_data.clone(),
                is_final,
                checksum,
            };
            
            // Send chunk message
            let stream_payload = StreamPayload {
                operation: StreamOperation::Chunk,
                stream_id,
                metadata: None,
                data: Some(chunk_data),
                chunk_sequence: Some(stream.next_sequence),
                completion: None,
            };
            
            let chunk_message = Message::new(
                MessageType::StreamChunk,
                MessagePayload::Stream(stream_payload),
            );
            
            self.connection.send(chunk_message).await?;
            
            // Track in-flight chunk
            stream.in_flight.insert(stream.next_sequence, chunk);
            stream.position += chunk_size;
            stream.next_sequence += 1;
            
            debug!("Sent chunk {} for stream {}", stream.next_sequence - 1, stream_id);
        }
        
        // Send stream end if all data sent
        if stream.position >= stream.data.len() && stream.in_flight.is_empty() {
            self.send_stream_end(stream_id).await?;
        }
        
        Ok(())
    }
    
    /// Handle stream start message
    async fn handle_stream_start(&mut self, payload: StreamPayload) -> Result<Option<ClipboardData>> {
        let metadata = payload.metadata.ok_or_else(|| {
            TransportError::Streaming("Stream start missing metadata".to_string())
        })?;
        
        info!("Receiving stream {} of {} bytes", payload.stream_id, metadata.total_size);
        
        let inbound_stream = InboundStream {
            metadata,
            chunks: HashMap::new(),
            next_expected: 1,
            assembled_data: Vec::new(),
            completion_tx: None,
            start_time: std::time::Instant::now(),
            last_progress: std::time::Instant::now(),
        };
        
        self.inbound_streams.insert(payload.stream_id, inbound_stream);
        
        Ok(None)
    }
    
    /// Handle stream chunk message
    async fn handle_stream_chunk(&mut self, payload: StreamPayload) -> Result<Option<ClipboardData>> {
        let stream_id = payload.stream_id;
        let chunk_data = payload.data.ok_or_else(|| {
            TransportError::Streaming("Stream chunk missing data".to_string())
        })?;
        let sequence = payload.chunk_sequence.ok_or_else(|| {
            TransportError::Streaming("Stream chunk missing sequence".to_string())
        })?;
        
        let stream = self.inbound_streams.get_mut(&stream_id)
            .ok_or_else(|| TransportError::Streaming("Stream not found".to_string()))?;
        
        // Store chunk
        stream.chunks.insert(sequence, chunk_data);
        
        // Assemble sequential chunks
        while let Some(chunk_data) = stream.chunks.remove(&stream.next_expected) {
            stream.assembled_data.extend_from_slice(&chunk_data);
            stream.next_expected += 1;
        }
        
        // Send progress update (clone needed to avoid borrowing issues)
        let next_expected = stream.next_expected;
        let assembled_len = stream.assembled_data.len();
        let metadata = stream.metadata.clone();
        let start_time = stream.start_time;
        drop(stream); // Release mutable borrow
        
        // Send acknowledgment
        self.send_stream_ack(stream_id, sequence).await?;
        
        // Send progress update using cloned data
        self.send_progress_update_with_data(stream_id, assembled_len, &metadata, start_time, next_expected)?;
        
        debug!("Received chunk {} for stream {}", sequence, stream_id);
        
        Ok(None)
    }
    
    /// Handle stream end message
    async fn handle_stream_end(&mut self, payload: StreamPayload) -> Result<Option<ClipboardData>> {
        let stream_id = payload.stream_id;
        
        let stream = self.inbound_streams.remove(&stream_id)
            .ok_or_else(|| TransportError::Streaming("Stream not found".to_string()))?;
        
        info!("Stream {} completed, received {} bytes", stream_id, stream.assembled_data.len());
        
        // Clone data to avoid move issues
        let assembled_data = stream.assembled_data.clone();
        let compression_method = stream.metadata.compression.clone();
        let expected_checksum = stream.metadata.checksum.clone();
        let content_type = stream.metadata.content_type.clone();
        
        // Decompress data if needed
        let final_data = if compression_method != CompressionMethod::None {
            self.decompress_data(&assembled_data, &compression_method)?
        } else {
            assembled_data.clone()
        };
        
        // Verify checksum
        let calculated_checksum = self.calculate_checksum(&assembled_data);
        if calculated_checksum != expected_checksum {
            return Err(TransportError::Streaming(
                "Stream checksum verification failed".to_string()
            ));
        }
        
        // Create clipboard data
        let clipboard_data = ClipboardData {
            format: content_type,
            data: final_data,
            compression: Some(compression_method),
            checksum: expected_checksum,
            metadata: std::collections::HashMap::new(),
        };
        
        Ok(Some(clipboard_data))
    }
    
    /// Handle stream acknowledgment
    async fn handle_stream_ack(&mut self, payload: StreamPayload) -> Result<()> {
        let stream_id = payload.stream_id;
        let sequence = payload.chunk_sequence.ok_or_else(|| {
            TransportError::Streaming("Stream ack missing sequence".to_string())
        })?;
        
        if let Some(stream) = self.outbound_streams.get_mut(&stream_id) {
            // Remove acknowledged chunk
            if let Some(chunk) = stream.in_flight.remove(&sequence) {
                stream.bytes_acked += chunk.data.len() as u64;
                debug!("Chunk {} acknowledged for stream {}", sequence, stream_id);
                
                // Send progress update (clone data to avoid borrowing issues)
                let bytes_acked = stream.bytes_acked;
                let total_size = stream.metadata.total_size;
                let total_chunks = stream.metadata.total_chunks;
                let next_sequence = stream.next_sequence;
                let start_time = stream.start_time;
                drop(stream); // Release mutable borrow
                
                self.send_outbound_progress_update_with_data(
                    stream_id, bytes_acked, total_size, total_chunks, 
                    next_sequence, start_time
                )?;
                
                // Send more chunks if available
                self.send_next_chunks(stream_id).await?;
            }
        }
        
        Ok(())
    }
    
    /// Handle stream cancellation
    async fn handle_stream_cancel(&mut self, payload: StreamPayload) -> Result<()> {
        let stream_id = payload.stream_id;
        
        // Remove streams
        self.outbound_streams.remove(&stream_id);
        self.inbound_streams.remove(&stream_id);
        
        warn!("Stream {} was cancelled", stream_id);
        
        Ok(())
    }
    
    /// Send stream end message
    async fn send_stream_end(&mut self, stream_id: Uuid) -> Result<()> {
        let completion = StreamCompletion {
            success: true,
            chunks_received: 0, // Will be filled by receiver
            bytes_received: 0,  // Will be filled by receiver
            error: None,
        };
        
        let stream_payload = StreamPayload {
            operation: StreamOperation::End,
            stream_id,
            metadata: None,
            data: None,
            chunk_sequence: None,
            completion: Some(completion),
        };
        
        let end_message = Message::new(
            MessageType::StreamEnd,
            MessagePayload::Stream(stream_payload),
        );
        
        self.connection.send(end_message).await?;
        
        // Complete the stream
        if let Some(stream) = self.outbound_streams.remove(&stream_id) {
            if let Some(tx) = stream.completion_tx {
                let _ = tx.send(Ok(()));
            }
        }
        
        Ok(())
    }
    
    /// Send stream acknowledgment
    async fn send_stream_ack(&mut self, stream_id: Uuid, sequence: u64) -> Result<()> {
        let stream_payload = StreamPayload {
            operation: StreamOperation::Ack,
            stream_id,
            metadata: None,
            data: None,
            chunk_sequence: Some(sequence),
            completion: None,
        };
        
        let ack_message = Message::new(
            MessageType::StreamAck,
            MessagePayload::Stream(stream_payload),
        );
        
        self.connection.send(ack_message).await
    }
    
    /// Send progress update for inbound stream with data
    fn send_progress_update_with_data(
        &self, 
        stream_id: Uuid, 
        assembled_len: usize,
        metadata: &StreamMetadata,
        start_time: std::time::Instant,
        next_expected: u64
    ) -> Result<()> {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(start_time).as_secs_f64();
        
        let bytes_transferred = assembled_len as u64;
        let total_bytes = metadata.total_size;
        let transfer_rate = if elapsed > 0.0 { bytes_transferred as f64 / elapsed } else { 0.0 };
        
        let eta_seconds = if transfer_rate > 0.0 && bytes_transferred < total_bytes {
            Some((total_bytes - bytes_transferred) as f64 / transfer_rate)
        } else {
            None
        };
        
        let progress = ProgressUpdate {
            stream_id,
            bytes_transferred,
            total_bytes,
            transfer_rate,
            eta_seconds,
            current_chunk: next_expected.saturating_sub(1),
            total_chunks: metadata.total_chunks,
        };
        
        let _ = self.progress_tx.send(progress);
        Ok(())
    }

    /// Send progress update for inbound stream
    fn send_progress_update(&self, stream_id: Uuid, stream: &InboundStream) -> Result<()> {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(stream.start_time).as_secs_f64();
        
        let bytes_transferred = stream.assembled_data.len() as u64;
        let total_bytes = stream.metadata.total_size;
        let transfer_rate = if elapsed > 0.0 { bytes_transferred as f64 / elapsed } else { 0.0 };
        
        let eta_seconds = if transfer_rate > 0.0 && bytes_transferred < total_bytes {
            Some((total_bytes - bytes_transferred) as f64 / transfer_rate)
        } else {
            None
        };
        
        let progress = ProgressUpdate {
            stream_id,
            bytes_transferred,
            total_bytes,
            transfer_rate,
            eta_seconds,
            current_chunk: stream.next_expected.saturating_sub(1),
            total_chunks: stream.metadata.total_chunks,
        };
        
        let _ = self.progress_tx.send(progress);
        Ok(())
    }
    
    /// Send progress update for outbound stream with data
    fn send_outbound_progress_update_with_data(
        &self,
        stream_id: Uuid,
        bytes_acked: u64,
        total_size: u64,
        total_chunks: u64,
        next_sequence: u64,
        start_time: std::time::Instant
    ) -> Result<()> {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(start_time).as_secs_f64();
        
        let bytes_transferred = bytes_acked;
        let total_bytes = total_size;
        let transfer_rate = if elapsed > 0.0 { bytes_transferred as f64 / elapsed } else { 0.0 };
        
        let eta_seconds = if transfer_rate > 0.0 && bytes_transferred < total_bytes {
            Some((total_bytes - bytes_transferred) as f64 / transfer_rate)
        } else {
            None
        };
        
        let progress = ProgressUpdate {
            stream_id,
            bytes_transferred,
            total_bytes,
            transfer_rate,
            eta_seconds,
            current_chunk: next_sequence.saturating_sub(1),
            total_chunks,
        };
        
        let _ = self.progress_tx.send(progress);
        Ok(())
    }

    /// Send progress update for outbound stream
    fn send_outbound_progress_update(&self, stream_id: Uuid, stream: &OutboundStream) -> Result<()> {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(stream.start_time).as_secs_f64();
        
        let bytes_transferred = stream.bytes_acked;
        let total_bytes = stream.metadata.total_size;
        let transfer_rate = if elapsed > 0.0 { bytes_transferred as f64 / elapsed } else { 0.0 };
        
        let eta_seconds = if transfer_rate > 0.0 && bytes_transferred < total_bytes {
            Some((total_bytes - bytes_transferred) as f64 / transfer_rate)
        } else {
            None
        };
        
        let progress = ProgressUpdate {
            stream_id,
            bytes_transferred,
            total_bytes,
            transfer_rate,
            eta_seconds,
            current_chunk: stream.next_sequence.saturating_sub(1),
            total_chunks: stream.metadata.total_chunks,
        };
        
        let _ = self.progress_tx.send(progress);
        Ok(())
    }
    
    /// Compress data using configured method
    fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self.config.compression_method {
            CompressionMethod::None => Ok(data.to_vec()),
            CompressionMethod::Zstd => {
                zstd::bulk::compress(data, 3)
                    .map_err(|e| TransportError::Streaming(format!("Compression failed: {}", e)))
            }
            CompressionMethod::Gzip => {
                // Would implement gzip compression here
                Ok(data.to_vec()) // Placeholder
            }
        }
    }
    
    /// Decompress data using specified method
    fn decompress_data(&self, data: &[u8], method: &CompressionMethod) -> Result<Vec<u8>> {
        match method {
            CompressionMethod::None => Ok(data.to_vec()),
            CompressionMethod::Zstd => {
                zstd::bulk::decompress(data, crate::MAX_PAYLOAD_SIZE)
                    .map_err(|e| TransportError::Streaming(format!("Decompression failed: {}", e)))
            }
            CompressionMethod::Gzip => {
                // Would implement gzip decompression here
                Ok(data.to_vec()) // Placeholder
            }
        }
    }
    
    /// Calculate data checksum
    fn calculate_checksum(&self, data: &[u8]) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }
}

#[async_trait]
impl Connection for StreamingTransport {
    async fn send(&mut self, message: Message) -> Result<()> {
        self.connection.send(message).await
    }
    
    async fn receive(&mut self) -> Result<Message> {
        self.connection.receive().await
    }
    
    fn peer_info(&self) -> &PeerInfo {
        self.connection.peer_info()
    }
    
    fn connection_info(&self) -> crate::transport::ConnectionInfo {
        self.connection.connection_info()
    }
    
    fn is_connected(&self) -> bool {
        self.connection.is_connected()
    }
    
    async fn close(&mut self) -> Result<()> {
        self.connection.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();
        assert_eq!(config.chunk_size, DEFAULT_CHUNK_SIZE);
        assert_eq!(config.max_in_flight, MAX_IN_FLIGHT_CHUNKS);
        assert!(config.enable_compression);
        assert_eq!(config.compression_method, CompressionMethod::Zstd);
    }
    
    #[test]
    fn test_progress_calculation() {
        let stream_id = Uuid::new_v4();
        let progress = ProgressUpdate {
            stream_id,
            bytes_transferred: 1024,
            total_bytes: 2048,
            transfer_rate: 512.0,
            eta_seconds: Some(2.0),
            current_chunk: 1,
            total_chunks: 2,
        };
        
        assert_eq!(progress.bytes_transferred * 2, progress.total_bytes);
        assert_eq!(progress.current_chunk * 2, progress.total_chunks);
    }
    
    #[test]
    fn test_stream_chunk_creation() {
        let stream_id = Uuid::new_v4();
        let data = b"test data".to_vec();
        
        let chunk = StreamChunk {
            stream_id,
            sequence: 1,
            data: data.clone(),
            is_final: true,
            checksum: "test".to_string(),
        };
        
        assert_eq!(chunk.data, data);
        assert_eq!(chunk.sequence, 1);
        assert!(chunk.is_final);
    }
}