use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

use clipsync::{
    config::Config,
    clipboard::{ClipboardProvider, ClipboardData, ClipboardError},
    discovery::PeerDiscovery,
    history::HistoryManager,
    sync::SyncEngine,
    transport::{TransportManager, TransportConfig},
};

// Benchmark clipboard provider
struct BenchClipboardProvider {
    content: tokio::sync::RwLock<String>,
}

impl BenchClipboardProvider {
    fn new() -> Self {
        Self {
            content: tokio::sync::RwLock::new(String::new()),
        }
    }
}

#[async_trait::async_trait]
impl ClipboardProvider for BenchClipboardProvider {
    async fn get_text(&self) -> Result<String, ClipboardError> {
        Ok(self.content.read().await.clone())
    }

    async fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        *self.content.write().await = text.to_string();
        Ok(())
    }

    async fn get_formats(&self) -> Result<Vec<String>, ClipboardError> {
        Ok(vec!["text/plain".to_string()])
    }

    async fn has_format(&self, format: &str) -> Result<bool, ClipboardError> {
        Ok(format == "text/plain")
    }
}

async fn create_bench_setup() -> (
    Arc<Config>,
    Arc<BenchClipboardProvider>,
    Arc<HistoryManager>,
    Arc<SyncEngine>,
) {
    let temp_dir = TempDir::new().unwrap();
    let config = Arc::new(Config::default_with_path(temp_dir.path().join("config.toml")));
    let clipboard = Arc::new(BenchClipboardProvider::new());
    let history = Arc::new(HistoryManager::new(&temp_dir.path().join("history.db")).await.unwrap());
    let discovery = Arc::new(PeerDiscovery::new(config.clone()).await.unwrap());
    let transport = Arc::new(TransportManager::new(TransportConfig::default()));

    let sync_engine = Arc::new(SyncEngine::new(
        config.clone(),
        clipboard.clone(),
        history.clone(),
        discovery,
        transport,
    ));

    (config, clipboard, history, sync_engine)
}

fn bench_clipboard_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("clipboard_get_text", |b| {
        let (_config, clipboard, _history, _sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            black_box(clipboard.get_text().await.unwrap());
        });
    });

    c.bench_function("clipboard_set_text", |b| {
        let (_config, clipboard, _history, _sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            black_box(clipboard.set_text("Benchmark text content").await.unwrap());
        });
    });
}

fn bench_history_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    // Benchmark history operations with different entry counts
    let entry_counts = vec![10, 100, 1000];
    
    for count in entry_counts {
        c.bench_with_input(
            BenchmarkId::new("history_get_recent_entries", count),
            &count,
            |b, &count| {
                let (_config, _clipboard, history, _sync_engine) = rt.block_on(create_bench_setup());
                
                // Pre-populate history
                rt.block_on(async {
                    for i in 0..count {
                        let entry = clipsync::history::ClipboardEntry {
                            id: uuid::Uuid::new_v4(),
                            content: ClipboardData::Text(format!("Entry {}", i)),
                            timestamp: chrono::Utc::now(),
                            source: uuid::Uuid::new_v4(),
                            checksum: format!("checksum_{}", i),
                        };
                        history.add_entry(&entry).await.unwrap();
                    }
                });
                
                b.to_async(&rt).iter(|| async {
                    black_box(history.get_recent_entries(count).await.unwrap());
                });
            },
        );
    }

    c.bench_function("history_add_entry", |b| {
        let (_config, _clipboard, history, _sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            let entry = clipsync::history::ClipboardEntry {
                id: uuid::Uuid::new_v4(),
                content: ClipboardData::Text("Benchmark entry".to_string()),
                timestamp: chrono::Utc::now(),
                source: uuid::Uuid::new_v4(),
                checksum: "benchmark_checksum".to_string(),
            };
            black_box(history.add_entry(&entry).await.unwrap());
        });
    });
}

fn bench_sync_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("sync_force_sync", |b| {
        let (_config, clipboard, _history, sync_engine) = rt.block_on(create_bench_setup());
        
        // Set some content to sync
        rt.block_on(async {
            clipboard.set_text("Content to sync").await.unwrap();
        });
        
        b.to_async(&rt).iter(|| async {
            black_box(sync_engine.force_sync().await.unwrap());
        });
    });

    c.bench_function("sync_event_creation", |b| {
        let (_config, _clipboard, _history, sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            let _receiver = black_box(sync_engine.subscribe());
        });
    });
}

fn bench_content_sizes(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    // Test different content sizes
    let sizes = vec![1024, 10_240, 102_400, 1_024_000]; // 1KB, 10KB, 100KB, 1MB
    
    for size in sizes {
        c.bench_with_input(
            BenchmarkId::new("sync_large_content", size),
            &size,
            |b, &size| {
                let (_config, clipboard, _history, sync_engine) = rt.block_on(create_bench_setup());
                let content = "A".repeat(size);
                
                b.to_async(&rt).iter(|| async {
                    clipboard.set_text(&content).await.unwrap();
                    black_box(sync_engine.force_sync().await.unwrap());
                });
            },
        );
    }
}

fn bench_concurrent_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("concurrent_clipboard_access", |b| {
        let (_config, clipboard, _history, _sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            let mut handles = Vec::new();
            
            // Spawn multiple concurrent clipboard operations
            for i in 0..10 {
                let clipboard = clipboard.clone();
                let handle = tokio::spawn(async move {
                    let content = format!("Concurrent content {}", i);
                    clipboard.set_text(&content).await.unwrap();
                    clipboard.get_text().await.unwrap()
                });
                handles.push(handle);
            }
            
            // Wait for all operations to complete
            for handle in handles {
                black_box(handle.await.unwrap());
            }
        });
    });

    c.bench_function("concurrent_history_access", |b| {
        let (_config, _clipboard, history, _sync_engine) = rt.block_on(create_bench_setup());
        
        // Pre-populate some history
        rt.block_on(async {
            for i in 0..100 {
                let entry = clipsync::history::ClipboardEntry {
                    id: uuid::Uuid::new_v4(),
                    content: ClipboardData::Text(format!("Entry {}", i)),
                    timestamp: chrono::Utc::now(),
                    source: uuid::Uuid::new_v4(),
                    checksum: format!("checksum_{}", i),
                };
                history.add_entry(&entry).await.unwrap();
            }
        });
        
        b.to_async(&rt).iter(|| async {
            let mut handles = Vec::new();
            
            // Spawn multiple concurrent history operations
            for _ in 0..10 {
                let history = history.clone();
                let handle = tokio::spawn(async move {
                    history.get_recent_entries(10).await.unwrap()
                });
                handles.push(handle);
            }
            
            // Wait for all operations to complete
            for handle in handles {
                black_box(handle.await.unwrap());
            }
        });
    });
}

fn bench_memory_usage(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("memory_large_history", |b| {
        let (_config, _clipboard, history, _sync_engine) = rt.block_on(create_bench_setup());
        
        b.to_async(&rt).iter(|| async {
            // Create a large number of history entries
            for i in 0..1000 {
                let entry = clipsync::history::ClipboardEntry {
                    id: uuid::Uuid::new_v4(),
                    content: ClipboardData::Text(format!("Large history entry {} with more content to increase memory usage", i)),
                    timestamp: chrono::Utc::now(),
                    source: uuid::Uuid::new_v4(),
                    checksum: format!("checksum_{}", i),
                };
                history.add_entry(&entry).await.unwrap();
            }
            
            // Read back all entries
            black_box(history.get_recent_entries(1000).await.unwrap());
        });
    });
}

criterion_group!(
    benches,
    bench_clipboard_operations,
    bench_history_operations,
    bench_sync_operations,
    bench_content_sizes,
    bench_concurrent_operations,
    bench_memory_usage
);

criterion_main!(benches);