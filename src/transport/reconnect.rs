//! Automatic reconnection logic for transport connections
//!
//! This module provides robust reconnection capabilities with exponential
//! backoff, connection health monitoring, and graceful degradation.

use crate::transport::{
    Connection, Result, TransportError, PeerInfo, 
    TransportEvent,
};
use crate::auth::Authenticator;
use std::time::{Duration, Instant};
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, info, warn, error};
use uuid::Uuid;

/// Reconnection configuration
#[derive(Debug, Clone)]
pub struct ReconnectionConfig {
    /// Maximum number of reconnection attempts (0 = infinite)
    pub max_attempts: u32,
    
    /// Initial delay between reconnection attempts
    pub initial_delay: Duration,
    
    /// Maximum delay between attempts
    pub max_delay: Duration,
    
    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
    
    /// Jitter factor to randomize delays (0.0 - 1.0)
    pub jitter_factor: f64,
    
    /// Health check interval
    pub health_check_interval: Duration,
    
    /// Connection timeout for each attempt
    pub connection_timeout: Duration,
    
    /// Enable automatic reconnection
    pub enabled: bool,
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter_factor: 0.1,
            health_check_interval: Duration::from_secs(30),
            connection_timeout: Duration::from_secs(10),
            enabled: true,
        }
    }
}

/// Connection health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Connection is healthy
    Healthy,
    
    /// Connection is degraded but functional
    Degraded,
    
    /// Connection has failed
    Failed,
    
    /// Connection status unknown
    Unknown,
}

/// Reconnection manager for handling connection failures
pub struct ReconnectionManager {
    /// Target peer information
    peer_info: PeerInfo,
    
    /// Authenticator for connections
    authenticator: Box<dyn Authenticator>,
    
    /// Current connection (if any)
    connection: Option<Box<dyn Connection>>,
    
    /// Reconnection configuration
    config: ReconnectionConfig,
    
    /// Current reconnection attempt count
    attempt_count: u32,
    
    /// Last connection attempt time
    last_attempt: Option<Instant>,
    
    /// Connection health status
    health_status: HealthStatus,
    
    /// Health check statistics
    health_stats: HealthStats,
    
    /// Event sender for notifications
    event_tx: mpsc::UnboundedSender<TransportEvent>,
    
    /// Shutdown signal
    shutdown_rx: Option<tokio::sync::oneshot::Receiver<()>>,
}

/// Health check statistics
#[derive(Debug, Clone, Default)]
struct HealthStats {
    /// Number of successful health checks
    successful_checks: u64,
    
    /// Number of failed health checks
    failed_checks: u64,
    
    /// Last health check time
    last_check: Option<Instant>,
    
    /// Average response time
    avg_response_time: Duration,
    
    /// Connection uptime since last connect
    uptime_start: Option<Instant>,
}

impl ReconnectionManager {
    /// Create a new reconnection manager
    pub fn new(
        peer_info: PeerInfo,
        authenticator: Box<dyn Authenticator>,
        config: ReconnectionConfig,
    ) -> (Self, mpsc::UnboundedReceiver<TransportEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        let manager = Self {
            peer_info,
            authenticator,
            connection: None,
            config,
            attempt_count: 0,
            last_attempt: None,
            health_status: HealthStatus::Unknown,
            health_stats: HealthStats::default(),
            event_tx,
            shutdown_rx: None,
        };
        
        (manager, event_rx)
    }
    
    /// Start the reconnection manager
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            info!("Reconnection disabled for peer {}", self.peer_info.id);
            return Ok(());
        }
        
        info!("Starting reconnection manager for peer {}", self.peer_info.id);
        
        // Initial connection attempt
        self.attempt_connection().await?;
        
        // Start health monitoring and reconnection loop
        self.run_connection_loop().await
    }
    
    /// Get the current connection if available and healthy
    pub fn get_connection(&mut self) -> Option<&mut Box<dyn Connection>> {
        if self.is_connection_healthy() {
            self.connection.as_mut()
        } else {
            None
        }
    }
    
    /// Check if we have a healthy connection
    pub fn is_connection_healthy(&self) -> bool {
        self.connection.is_some() && 
        matches!(self.health_status, HealthStatus::Healthy | HealthStatus::Degraded)
    }
    
    /// Force a reconnection attempt
    pub async fn force_reconnect(&mut self) -> Result<()> {
        info!("Forcing reconnection to peer {}", self.peer_info.id);
        
        // Close existing connection
        if let Some(mut conn) = self.connection.take() {
            let _ = conn.close().await;
        }
        
        // Reset attempt count
        self.attempt_count = 0;
        self.health_status = HealthStatus::Unknown;
        
        // Attempt new connection
        self.attempt_connection().await
    }
    
    /// Main connection management loop
    async fn run_connection_loop(&mut self) -> Result<()> {
        let mut health_check_interval = tokio::time::interval(self.config.health_check_interval);
        
        loop {
            tokio::select! {
                // Periodic health checks
                _ = health_check_interval.tick() => {
                    self.perform_health_check().await;
                    
                    // Attempt reconnection if connection is failed
                    if self.health_status == HealthStatus::Failed && self.config.enabled {
                        if let Err(e) = self.attempt_reconnection().await {
                            warn!("Reconnection attempt failed: {}", e);
                        }
                    }
                }
                
                // Shutdown signal
                _ = async {
                    if let Some(ref mut rx) = self.shutdown_rx {
                        rx.await.unwrap_or(())
                    } else {
                        std::future::pending().await
                    }
                } => {
                    info!("Shutting down reconnection manager");
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    /// Attempt to establish a connection
    async fn attempt_connection(&mut self) -> Result<()> {
        self.attempt_count += 1;
        self.last_attempt = Some(Instant::now());
        
        info!("Attempting connection to peer {} (attempt {})", 
              self.peer_info.id, self.attempt_count);
        
        // Send reconnection attempt event
        let _ = self.event_tx.send(TransportEvent::ReconnectionAttempt(
            self.peer_info.id,
            self.attempt_count,
        ));
        
        match tokio::time::timeout(
            self.config.connection_timeout,
            self.connect_to_peer()
        ).await {
            Ok(Ok(connection)) => {
                info!("Successfully connected to peer {}", self.peer_info.id);
                
                self.connection = Some(connection);
                self.health_status = HealthStatus::Healthy;
                self.health_stats.uptime_start = Some(Instant::now());
                self.attempt_count = 0; // Reset on successful connection
                
                // Send connection established event
                if let Some(conn) = &self.connection {
                    let _ = self.event_tx.send(TransportEvent::ConnectionEstablished(
                        conn.connection_info()
                    ));
                }
                
                Ok(())
            }
            Ok(Err(e)) => {
                warn!("Connection attempt {} failed: {}", self.attempt_count, e);
                self.health_status = HealthStatus::Failed;
                
                let _ = self.event_tx.send(TransportEvent::ConnectionFailed(
                    self.peer_info.id,
                    e.to_string(),
                ));
                
                Err(e)
            }
            Err(_) => {
                let error_msg = "Connection attempt timed out";
                warn!("{}", error_msg);
                self.health_status = HealthStatus::Failed;
                
                let _ = self.event_tx.send(TransportEvent::ConnectionFailed(
                    self.peer_info.id,
                    error_msg.to_string(),
                ));
                
                Err(TransportError::Timeout)
            }
        }
    }
    
    /// Attempt reconnection with exponential backoff
    async fn attempt_reconnection(&mut self) -> Result<()> {
        if self.config.max_attempts > 0 && self.attempt_count >= self.config.max_attempts {
            error!("Max reconnection attempts ({}) reached for peer {}", 
                   self.config.max_attempts, self.peer_info.id);
            return Err(TransportError::Reconnection(
                "Maximum reconnection attempts exceeded".to_string()
            ));
        }
        
        // Calculate backoff delay
        let delay = self.calculate_backoff_delay();
        
        debug!("Waiting {} seconds before reconnection attempt", delay.as_secs());
        sleep(delay).await;
        
        // Close existing failed connection
        if let Some(mut conn) = self.connection.take() {
            let _ = conn.close().await;
        }
        
        self.attempt_connection().await
    }
    
    /// Perform health check on current connection
    async fn perform_health_check(&mut self) {
        if let Some(connection) = &mut self.connection {
            let start_time = Instant::now();
            
            // Simple health check - verify connection is still active
            if connection.is_connected() {
                // Could send a ping/keepalive message here
                let response_time = start_time.elapsed();
                
                self.health_stats.successful_checks += 1;
                self.health_stats.last_check = Some(Instant::now());
                self.update_avg_response_time(response_time);
                
                // Determine health status based on response time
                if response_time > Duration::from_secs(5) {
                    self.health_status = HealthStatus::Degraded;
                } else {
                    self.health_status = HealthStatus::Healthy;
                }
                
                debug!("Health check passed in {:?}", response_time);
            } else {
                self.health_stats.failed_checks += 1;
                self.health_status = HealthStatus::Failed;
                
                warn!("Health check failed - connection is not active");
                
                let _ = self.event_tx.send(TransportEvent::ConnectionFailed(
                    self.peer_info.id,
                    "Health check failed".to_string(),
                ));
            }
        } else {
            self.health_status = HealthStatus::Failed;
        }
    }
    
    /// Calculate exponential backoff delay with jitter
    fn calculate_backoff_delay(&self) -> Duration {
        let base_delay = self.config.initial_delay.as_secs_f64();
        let backoff_delay = base_delay * self.config.backoff_multiplier.powi(
            (self.attempt_count - 1) as i32
        );
        
        // Apply maximum delay limit
        let clamped_delay = backoff_delay.min(self.config.max_delay.as_secs_f64());
        
        // Add jitter to prevent thundering herd
        let jitter_range = clamped_delay * self.config.jitter_factor;
        let jitter = (rand::random::<f64>() - 0.5) * 2.0 * jitter_range;
        let final_delay = (clamped_delay + jitter).max(0.0);
        
        Duration::from_secs_f64(final_delay)
    }
    
    /// Update average response time
    fn update_avg_response_time(&mut self, new_time: Duration) {
        const ALPHA: f64 = 0.1; // Exponential moving average factor
        
        let new_time_secs = new_time.as_secs_f64();
        let current_avg = self.health_stats.avg_response_time.as_secs_f64();
        
        let updated_avg = if self.health_stats.successful_checks == 1 {
            new_time_secs // First measurement
        } else {
            current_avg * (1.0 - ALPHA) + new_time_secs * ALPHA
        };
        
        self.health_stats.avg_response_time = Duration::from_secs_f64(updated_avg);
    }
    
    /// Connect to the peer (placeholder - would use actual transport)
    async fn connect_to_peer(&self) -> Result<Box<dyn Connection>> {
        // This would be implemented by the WebSocket transport
        // For now, return an error as placeholder
        Err(TransportError::Connection(
            "WebSocket transport not yet implemented".to_string()
        ))
    }
    
    /// Get connection statistics
    pub fn get_stats(&self) -> ConnectionStats {
        let uptime = self.health_stats.uptime_start
            .map(|start| Instant::now().duration_since(start))
            .unwrap_or_default();
        
        ConnectionStats {
            peer_id: self.peer_info.id,
            health_status: self.health_status,
            attempt_count: self.attempt_count,
            successful_checks: self.health_stats.successful_checks,
            failed_checks: self.health_stats.failed_checks,
            avg_response_time: self.health_stats.avg_response_time,
            uptime,
            last_check: self.health_stats.last_check,
        }
    }
}

/// Connection statistics for monitoring
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    /// Peer identifier
    pub peer_id: Uuid,
    
    /// Current health status
    pub health_status: HealthStatus,
    
    /// Total connection attempts
    pub attempt_count: u32,
    
    /// Successful health checks
    pub successful_checks: u64,
    
    /// Failed health checks
    pub failed_checks: u64,
    
    /// Average response time
    pub avg_response_time: Duration,
    
    /// Connection uptime
    pub uptime: Duration,
    
    /// Last health check time
    pub last_check: Option<Instant>,
}

impl ConnectionStats {
    /// Calculate health check success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.successful_checks + self.failed_checks;
        if total > 0 {
            self.successful_checks as f64 / total as f64
        } else {
            0.0
        }
    }
    
    /// Check if connection is considered stable
    pub fn is_stable(&self) -> bool {
        self.health_status == HealthStatus::Healthy &&
        self.success_rate() > 0.9 &&
        self.uptime > Duration::from_secs(60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_reconnection_config_default() {
        let config = ReconnectionConfig::default();
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.backoff_multiplier, 2.0);
        assert!(config.enabled);
    }
    
    #[test]
    fn test_health_status() {
        assert_ne!(HealthStatus::Healthy, HealthStatus::Failed);
        assert_ne!(HealthStatus::Degraded, HealthStatus::Unknown);
    }
    
    #[test]
    fn test_connection_stats() {
        let stats = ConnectionStats {
            peer_id: Uuid::new_v4(),
            health_status: HealthStatus::Healthy,
            attempt_count: 1,
            successful_checks: 9,
            failed_checks: 1,
            avg_response_time: Duration::from_millis(100),
            uptime: Duration::from_secs(120),
            last_check: Some(Instant::now()),
        };
        
        assert_eq!(stats.success_rate(), 0.9);
        assert!(stats.is_stable());
    }
    
    #[test]
    fn test_backoff_calculation() {
        let config = ReconnectionConfig::default();
        let mut manager = ReconnectionManager {
            peer_info: PeerInfo {
                id: Uuid::new_v4(),
                name: "test".to_string(),
                addresses: vec![],
                port: 9090,
                version: "1.0.0".to_string(),
                platform: "test".to_string(),
                metadata: Default::default(),
                last_seen: 0,
            },
            authenticator: Box::new(DummyAuth),
            connection: None,
            config,
            attempt_count: 3,
            last_attempt: None,
            health_status: HealthStatus::Unknown,
            health_stats: HealthStats::default(),
            event_tx: mpsc::unbounded_channel().0,
            shutdown_rx: None,
        };
        
        let delay = manager.calculate_backoff_delay();
        // Should be around 4 seconds (1 * 2^2) with some jitter
        assert!(delay.as_secs() >= 3 && delay.as_secs() <= 5);
    }
    
    // Dummy authenticator for testing
    struct DummyAuth;
    
    #[async_trait::async_trait]
    impl Authenticator for DummyAuth {
        async fn authenticate_peer(&self, _key: &crate::auth::PublicKey) -> std::result::Result<crate::auth::AuthToken, crate::auth::AuthError> {
            Err(crate::auth::AuthError::AuthenticationFailed("dummy".to_string()))
        }
        
        async fn verify_token(&self, _token: &crate::auth::AuthToken) -> std::result::Result<crate::auth::PeerId, crate::auth::AuthError> {
            Err(crate::auth::AuthError::AuthenticationFailed("dummy".to_string()))
        }
        
        async fn get_public_key(&self) -> std::result::Result<crate::auth::PublicKey, crate::auth::AuthError> {
            Err(crate::auth::AuthError::AuthenticationFailed("dummy".to_string()))
        }
        
        async fn is_authorized(&self, _key: &crate::auth::PublicKey) -> std::result::Result<bool, crate::auth::AuthError> {
            Ok(false)
        }
    }
}