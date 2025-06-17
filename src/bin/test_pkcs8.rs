use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;

fn main() {
    println!("Testing PKCS8 generation...");
    
    // Generate a new key to see what ring produces
    let rng = SystemRandom::new();
    let pkcs8_bytes = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    
    println!("Generated PKCS8 length: {}", pkcs8_bytes.as_ref().len());
    println!("PKCS8 hex: {}", hex::encode(pkcs8_bytes.as_ref()));
    
    // Analyze the structure
    let bytes = pkcs8_bytes.as_ref();
    println!("\nStructure analysis:");
    println!("Header: {:02x} {:02x}", bytes[0], bytes[1]);
    println!("Version section: {:02x} {:02x} {:02x}", bytes[2], bytes[3], bytes[4]);
    println!("Algorithm ID: {:02x} {:02x}", bytes[5], bytes[6]);
    println!("OID: {:02x} {:02x} {:02x} {:02x} {:02x}", bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]);
    println!("Private key wrapper: {:02x} {:02x}", bytes[12], bytes[13]);
    println!("Inner octet string: {:02x} {:02x}", bytes[14], bytes[15]);
    
    // Try to load it back
    match Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref()) {
        Ok(_) => println!("\nSuccessfully loaded generated PKCS8!"),
        Err(e) => println!("\nFailed to load generated PKCS8: {:?}", e),
    }
}