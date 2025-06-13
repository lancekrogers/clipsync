# Task 09: Service Discovery Module

## Objective
Implement mDNS/DNS-SD for automatic peer discovery on the local network.

## Steps

1. **Create src/discovery/mod.rs**
   - mDNS service registration
   - Service browsing
   - Peer management

2. **Implement discovery service**
   ```rust
   pub struct DiscoveryService {
       service_name: String,
       port: u16,
       txt_records: HashMap<String, String>,
   }
   
   impl DiscoveryService {
       pub fn new(name: &str, port: u16) -> Self;
       pub async fn start(&self) -> Result<()>;
       pub async fn browse(&self) -> Result<Vec<ServiceInfo>>;
       pub async fn resolve(&self, name: &str) -> Result<SocketAddr>;
   }
   ```

3. **Add service registration**
   - Register as "_clipsync._tcp"
   - Include version in TXT records
   - Add public key fingerprint
   - Set TTL appropriately

4. **Implement peer discovery**
   - Browse for other ClipSync instances
   - Filter by version compatibility
   - Verify service authenticity
   - Handle multiple NICs

5. **Create peer manager**
   - Track discovered peers
   - Monitor peer availability
   - Handle peer departure
   - Automatic connection attempts

6. **Add fallback mechanisms**
   - Manual peer addition by IP
   - Static peer configuration
   - DNS-SD server support

## Success Criteria
- Services discoverable on LAN
- Multiple instances coexist
- Discovery works on both platforms
- Graceful handling of network changes