//! Simple test to verify the project builds correctly

#[test]
fn test_project_builds() {
    // This test simply ensures the project compiles
    assert!(true, "Project builds successfully");
}

#[test]
fn test_library_exports() {
    // Verify main types are exported
    let _ = clipsync::MAX_PAYLOAD_SIZE;
    assert_eq!(clipsync::MAX_PAYLOAD_SIZE, 5 * 1024 * 1024);
}
