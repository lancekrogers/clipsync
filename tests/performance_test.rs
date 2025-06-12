use clipsync::clipboard::ClipboardManager;
use clipsync::history::HistoryDatabase;
use clipsync::sync::SyncEngine;
use criterion::{black_box, Criterion};
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[test]
fn benchmark_clipboard_operations() {
    let clipboard = ClipboardManager::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    
    // Benchmark small text
    let small_text = "Hello, World!";
    let start = Instant::now();
    for _ in 0..1000 {
        runtime.block_on(async {
            clipboard.set_text(small_text).await.unwrap();
            clipboard.get_text().await.unwrap();
        });
    }
    let duration = start.elapsed();
    println!("Small text (1000 ops): {:?}", duration);
    assert!(duration < Duration::from_secs(1), "Small text operations too slow");
    
    // Benchmark medium text (100KB)
    let medium_text = "x".repeat(100 * 1024);
    let start = Instant::now();
    for _ in 0..100 {
        runtime.block_on(async {
            clipboard.set_text(&medium_text).await.unwrap();
            clipboard.get_text().await.unwrap();
        });
    }
    let duration = start.elapsed();
    println!("Medium text (100 ops): {:?}", duration);
    assert!(duration < Duration::from_secs(5), "Medium text operations too slow");
    
    // Benchmark large text (10MB)
    let large_text = "x".repeat(10 * 1024 * 1024);
    let start = Instant::now();
    for _ in 0..10 {
        runtime.block_on(async {
            clipboard.set_text(&large_text).await.unwrap();
            clipboard.get_text().await.unwrap();
        });
    }
    let duration = start.elapsed();
    println!("Large text (10 ops): {:?}", duration);
    assert!(duration < Duration::from_secs(10), "Large text operations too slow");
}

#[test]
fn benchmark_history_database() {
    let temp_dir = TempDir::new().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    
    runtime.block_on(async {
        let db = HistoryDatabase::new(temp_dir.path()).await.unwrap();
        
        // Benchmark insertions
        let start = Instant::now();
        for i in 0..10000 {
            db.add_entry(&format!("Entry {}", i), None).await.unwrap();
        }
        let duration = start.elapsed();
        println!("10k insertions: {:?}", duration);
        assert!(duration < Duration::from_secs(5), "Database insertions too slow");
        
        // Benchmark queries
        let start = Instant::now();
        for _ in 0..1000 {
            db.get_recent_entries(100).await.unwrap();
        }
        let duration = start.elapsed();
        println!("1k queries: {:?}", duration);
        assert!(duration < Duration::from_secs(2), "Database queries too slow");
        
        // Benchmark search
        let start = Instant::now();
        for i in 0..100 {
            db.search_entries(&format!("Entry {}", i * 100)).await.unwrap();
        }
        let duration = start.elapsed();
        println!("100 searches: {:?}", duration);
        assert!(duration < Duration::from_secs(1), "Database searches too slow");
    });
}

#[test]
fn benchmark_encryption() {
    use clipsync::history::encryption::Encryptor;
    
    let encryptor = Encryptor::new(b"test-key-32-bytes-long-for-aes!!").unwrap();
    
    // Small payload
    let small_data = b"Hello, World!";
    let start = Instant::now();
    for _ in 0..10000 {
        let encrypted = encryptor.encrypt(small_data).unwrap();
        let _decrypted = encryptor.decrypt(&encrypted).unwrap();
    }
    let duration = start.elapsed();
    println!("Small encryption (10k ops): {:?}", duration);
    assert!(duration < Duration::from_secs(1), "Small encryption too slow");
    
    // Large payload (1MB)
    let large_data = vec![0u8; 1024 * 1024];
    let start = Instant::now();
    for _ in 0..100 {
        let encrypted = encryptor.encrypt(&large_data).unwrap();
        let _decrypted = encryptor.decrypt(&encrypted).unwrap();
    }
    let duration = start.elapsed();
    println!("Large encryption (100 ops): {:?}", duration);
    assert!(duration < Duration::from_secs(5), "Large encryption too slow");
}

#[test]
fn benchmark_sync_serialization() {
    use clipsync::transport::protocol::{Message, MessageType};
    
    let test_messages = vec![
        Message {
            id: "test-id".to_string(),
            msg_type: MessageType::ClipboardUpdate,
            payload: vec![0u8; 1024],
            timestamp: 0,
            sender_id: "sender".to_string(),
            signature: None,
        },
        Message {
            id: "test-id-2".to_string(),
            msg_type: MessageType::Heartbeat,
            payload: vec![],
            timestamp: 0,
            sender_id: "sender".to_string(),
            signature: None,
        },
    ];
    
    // Benchmark serialization
    let start = Instant::now();
    for _ in 0..100000 {
        for msg in &test_messages {
            let _serialized = serde_json::to_vec(msg).unwrap();
        }
    }
    let duration = start.elapsed();
    println!("Message serialization (200k ops): {:?}", duration);
    assert!(duration < Duration::from_secs(2), "Serialization too slow");
    
    // Benchmark deserialization
    let serialized_messages: Vec<_> = test_messages
        .iter()
        .map(|msg| serde_json::to_vec(msg).unwrap())
        .collect();
    
    let start = Instant::now();
    for _ in 0..100000 {
        for data in &serialized_messages {
            let _msg: Message = serde_json::from_slice(data).unwrap();
        }
    }
    let duration = start.elapsed();
    println!("Message deserialization (200k ops): {:?}", duration);
    assert!(duration < Duration::from_secs(2), "Deserialization too slow");
}

#[test]
fn benchmark_memory_usage() {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    struct MemoryTracker;
    static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
    
    unsafe impl GlobalAlloc for MemoryTracker {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATED.fetch_add(layout.size(), Ordering::SeqCst);
            System.alloc(layout)
        }
        
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            ALLOCATED.fetch_sub(layout.size(), Ordering::SeqCst);
            System.dealloc(ptr, layout)
        }
    }
    
    // Test memory usage during normal operations
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let initial_memory = ALLOCATED.load(Ordering::SeqCst);
    
    runtime.block_on(async {
        let clipboard = ClipboardManager::new();
        
        // Simulate typical usage
        for i in 0..100 {
            clipboard.set_text(&format!("Test entry {}", i)).await.unwrap();
            if i % 10 == 0 {
                clipboard.get_text().await.unwrap();
            }
        }
    });
    
    let final_memory = ALLOCATED.load(Ordering::SeqCst);
    let memory_growth = final_memory.saturating_sub(initial_memory);
    
    println!("Memory growth during 100 operations: {} bytes", memory_growth);
    assert!(memory_growth < 10 * 1024 * 1024, "Excessive memory usage");
}

#[test]
fn benchmark_startup_time() {
    use clipsync::config::Config;
    
    let temp_dir = TempDir::new().unwrap();
    
    // Cold start
    let start = Instant::now();
    let _config = Config::new(temp_dir.path().to_path_buf()).unwrap();
    let cold_start = start.elapsed();
    println!("Cold start: {:?}", cold_start);
    assert!(cold_start < Duration::from_millis(200), "Cold start too slow");
    
    // Warm start
    let start = Instant::now();
    for _ in 0..10 {
        let _config = Config::load(temp_dir.path()).unwrap();
    }
    let warm_start = start.elapsed() / 10;
    println!("Warm start (avg): {:?}", warm_start);
    assert!(warm_start < Duration::from_millis(50), "Warm start too slow");
}