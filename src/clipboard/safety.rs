/// Safety checks to prevent clipboard sync from interfering with sensitive operations
use regex::Regex;
use once_cell::sync::Lazy;

/// Common patterns that might indicate sensitive data
static SENSITIVE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Password manager patterns
        Regex::new(r"^[A-Za-z0-9+/]{20,}={0,2}$").unwrap(), // Base64 encoded passwords
        Regex::new(r"^[A-Za-z\d@$!%*?&]{12,}$").unwrap(), // Long passwords (simplified pattern)
        
        // SSH keys
        Regex::new(r"^ssh-(rsa|ed25519|ecdsa|dss)\s+[A-Za-z0-9+/]+=*(\s+.+)?$").unwrap(),
        Regex::new(r"-----BEGIN (RSA|DSA|EC|OPENSSH) PRIVATE KEY-----").unwrap(),
        
        // API tokens and secrets
        Regex::new(r"^(ghp|gho|ghs|ghr)_[A-Za-z0-9]{36}$").unwrap(), // GitHub tokens
        Regex::new(r"^sk-[A-Za-z0-9]{48}$").unwrap(), // OpenAI API keys
        Regex::new(r"^AKIA[0-9A-Z]{16}$").unwrap(), // AWS access keys
        
        // Credit card patterns
        Regex::new(r"^\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}$").unwrap(),
        
        // Common secret patterns
        Regex::new(r"^(password|passwd|pwd|secret|token|apikey|api_key)[:=]\s*.+", ).unwrap(),
    ]
});

/// Check if clipboard content might be sensitive
pub fn is_potentially_sensitive(content: &str) -> bool {
    // Skip empty or very short content
    if content.len() < 8 {
        return false;
    }
    
    // Check against sensitive patterns
    for pattern in SENSITIVE_PATTERNS.iter() {
        if pattern.is_match(content) {
            return true;
        }
    }
    
    // Check for high entropy (might be encrypted/encoded data)
    if calculate_entropy(content) > 4.5 {
        return true;
    }
    
    false
}

/// Calculate Shannon entropy of a string
fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    
    let mut char_counts = std::collections::HashMap::new();
    for c in s.chars() {
        *char_counts.entry(c).or_insert(0) += 1;
    }
    
    let len = s.len() as f64;
    let mut entropy = 0.0;
    
    for count in char_counts.values() {
        let p = *count as f64 / len;
        entropy -= p * p.log2();
    }
    
    entropy
}

/// Check if we're in a sensitive context (e.g., terminal with sudo)
pub fn is_sensitive_context() -> bool {
    // Check if we're in a sudo session
    if std::env::var("SUDO_USER").is_ok() {
        return true;
    }
    
    // Check if terminal has a password prompt active
    // This is a heuristic - we can't reliably detect all password prompts
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("password") {
            return true;
        }
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sensitive_patterns() {
        // Should detect long passwords
        assert!(is_potentially_sensitive("MyP@ssw0rd123!xyz"));
        
        // Should detect SSH keys
        assert!(is_potentially_sensitive("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQ user@host"));
        
        // Should detect API tokens
        assert!(is_potentially_sensitive("ghp_1234567890abcdef1234567890abcdef1234"));
        
        // Should not flag normal text
        assert!(!is_potentially_sensitive("Hello world"));
        assert!(!is_potentially_sensitive("This is a normal clipboard content"));
    }
    
    #[test]
    fn test_entropy() {
        // High entropy (random)
        assert!(calculate_entropy("aB3$mK9@pL5#nR7!") > 3.5);
        
        // Low entropy (repetitive)
        assert!(calculate_entropy("aaaaaaaaaa") < 1.0);
        
        // Medium entropy (normal text)
        let entropy = calculate_entropy("Hello world");
        assert!(entropy > 2.0 && entropy < 4.0);
    }
}