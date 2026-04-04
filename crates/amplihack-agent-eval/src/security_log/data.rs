//! Security log MDE event data generation.
//!
//! Companion to `security_log` — provides device pools, user lists,
//! C2 domains, and campaign generation helpers.

use super::AttackCampaign;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Enterprise device/user pools
// ---------------------------------------------------------------------------

/// Generate workstation device names.
pub fn workstation_devices() -> Vec<String> {
    let depts = ["FIN", "ENG", "MKT", "HR", "EXEC", "IT", "LEGAL"];
    let mut devices = Vec::new();
    for dept in &depts {
        for i in 1..=15 {
            devices.push(format!("WS-{dept}-{i:03}"));
        }
    }
    devices
}

/// Generate server device names.
pub fn server_devices() -> Vec<String> {
    let roles = ["DC", "SQL", "WEB", "APP", "FILE", "EXCH", "SCCM", "WSUS"];
    let mut devices = Vec::new();
    for role in &roles {
        for i in 1..=5 {
            devices.push(format!("SRV-{role}-{i:02}"));
        }
    }
    devices
}

/// Standard enterprise users (username, full name, department).
pub fn enterprise_users() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("jsmith", "John Smith", "Finance"),
        ("agarcia", "Ana Garcia", "Engineering"),
        ("mwong", "Michael Wong", "Marketing"),
        ("ljohnson", "Lisa Johnson", "HR"),
        ("rbrown", "Robert Brown", "IT"),
        ("kwilliams", "Karen Williams", "Legal"),
        ("dlee", "David Lee", "Engineering"),
        ("spatel", "Sanjay Patel", "IT"),
        ("jchen", "Jennifer Chen", "Finance"),
        ("tmartin", "Tom Martin", "Executive"),
        ("nkim", "Nancy Kim", "Engineering"),
        ("crodriguez", "Carlos Rodriguez", "IT"),
        ("svc_backup", "Service Account", "IT"),
        ("svc_deploy", "Service Account", "IT"),
        ("admin_spatel", "Sanjay Patel (Admin)", "IT"),
    ]
}

/// Known C2 domains.
pub fn c2_domains() -> Vec<&'static str> {
    vec![
        "cdn-static-assets.com",
        "api-telemetry-service.net",
        "cloud-sync-update.com",
        "global-content-delivery.net",
        "secure-update-check.com",
        "analytics-reporting.io",
    ]
}

/// Techniques grouped by attack objective.
pub fn techniques_for_objective(objective: &str) -> Vec<&'static str> {
    match objective {
        "data_exfiltration" => vec![
            "T1566.001",
            "T1059.001",
            "T1003.001",
            "T1021.002",
            "T1083",
            "T1560.001",
            "T1048.003",
        ],
        "ransomware" => vec![
            "T1566.001",
            "T1059.003",
            "T1053.005",
            "T1021.001",
            "T1562.001",
            "T1490",
            "T1486",
        ],
        "espionage" => vec![
            "T1566.001",
            "T1059.001",
            "T1055.001",
            "T1003.001",
            "T1087.002",
            "T1018",
            "T1071.001",
        ],
        "cryptomining" => vec!["T1059.001", "T1053.005", "T1543.003", "T1105"],
        "supply_chain" => vec![
            "T1059.001",
            "T1036.005",
            "T1547.001",
            "T1027",
            "T1140",
            "T1218.011",
        ],
        _ => vec![],
    }
}

/// All supported attack objectives.
pub fn objectives() -> &'static [&'static str] {
    &[
        "data_exfiltration",
        "ransomware",
        "espionage",
        "cryptomining",
        "supply_chain",
    ]
}

/// Threat actor entries (name, description).
pub fn threat_actors() -> Vec<(&'static str, &'static str)> {
    vec![
        ("APT-BEAR", "Nation-state: Eastern European"),
        ("APT-DRAGON", "Nation-state: East Asian"),
        ("CARBON-SPIDER", "eCrime: Ransomware group"),
        ("SCATTERED-SPIDER", "eCrime: Social engineering"),
        ("VELVET-TYPHOON", "Nation-state: Espionage"),
        ("SANDSTORM-7", "Hacktivist collective"),
    ]
}

/// Operation name fragments for campaign naming.
pub fn operation_adjectives() -> &'static [&'static str] {
    &[
        "Midnight", "Shadow", "Storm", "Glacier", "Phoenix", "Cobalt", "Iron", "Crimson", "Azure",
        "Onyx", "Jade", "Ruby",
    ]
}

pub fn operation_animals() -> &'static [&'static str] {
    &[
        "Wolf", "Bear", "Eagle", "Fox", "Lion", "Hawk", "Viper", "Falcon",
    ]
}

// ---------------------------------------------------------------------------
// Deterministic campaign generation
// ---------------------------------------------------------------------------

/// Simple deterministic RNG (splitmix64-style) for reproducible generation.
#[derive(Debug, Clone)]
pub struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo)
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = self.next_u64() as usize % items.len();
        &items[idx]
    }

    pub fn sample<T: Clone>(&mut self, items: &[T], n: usize) -> Vec<T> {
        let n = n.min(items.len());
        let mut pool: Vec<T> = items.to_vec();
        for i in 0..n {
            let j = i + (self.next_u64() as usize % (pool.len() - i));
            pool.swap(i, j);
        }
        pool.truncate(n);
        pool
    }
}

/// Compute a simple hash for deterministic IOC generation.
pub fn simple_hash(input: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}{:016x}", h.wrapping_mul(0x517c_c1b7_2722_0a95))
}

/// Generate deterministic attack campaigns.
pub fn generate_campaigns(seed: u64, num_campaigns: usize) -> Vec<AttackCampaign> {
    let mut rng = SimpleRng::new(seed);
    let actors = threat_actors();
    let objs = objectives();
    let all_devices: Vec<String> = workstation_devices()
        .into_iter()
        .chain(server_devices())
        .collect();
    let users = enterprise_users();
    let c2 = c2_domains();
    let adj = operation_adjectives();
    let animals = operation_animals();

    let mut campaigns = Vec::with_capacity(num_campaigns);

    for i in 0..num_campaigns {
        let (actor_name, actor_desc) = actors[i % actors.len()];
        let objective = objs[i % objs.len()];
        let techniques: Vec<String> = techniques_for_objective(objective)
            .into_iter()
            .map(String::from)
            .collect();

        let num_devices = rng.next_range(3, 12) as usize;
        let target_devices = rng.sample(&all_devices, num_devices);
        let num_users = rng.next_range(1, 4) as usize;
        let target_users: Vec<String> = rng
            .sample(&users, num_users)
            .into_iter()
            .map(|(u, _, _)| u.to_string())
            .collect();
        let campaign_c2: Vec<String> = {
            let n = rng.next_range(1, 3) as usize;
            rng.sample(&c2, n)
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        };

        let campaign_hash = simple_hash(&format!("campaign-{i}-{actor_name}"));
        let malware_hashes = vec![campaign_hash[..32].to_string()];
        let ip_count = rng.next_range(2, 5) as usize;
        let ips: Vec<String> = (0..ip_count)
            .map(|j| {
                format!(
                    "185.{}.{}.{}",
                    rng.next_range(100, 255),
                    rng.next_range(1, 254),
                    rng.next_range(1, 254) + j as u64,
                )
            })
            .collect();

        let lat_len = rng.next_range(2, 5.min(target_devices.len() as u64)) as usize;
        let lateral_path = target_devices[..lat_len].to_vec();

        let mut iocs = HashMap::new();
        iocs.insert("ip".to_string(), ips);
        iocs.insert("domain".to_string(), campaign_c2.clone());
        iocs.insert("hash".to_string(), malware_hashes.clone());

        let op_name = format!(
            "Operation {} {}",
            adj[rng.next_u64() as usize % adj.len()],
            animals[rng.next_u64() as usize % animals.len()],
        );

        campaigns.push(AttackCampaign {
            campaign_id: format!("CAMP-{}-{:03}", 2024 + i / 6, i + 1),
            name: op_name,
            threat_actor: format!("{actor_name} ({actor_desc})"),
            start_day: i as u32 * 5 + rng.next_range(0, 3) as u32,
            duration_days: rng.next_range(2, 14) as u32,
            initial_access: "T1566.001".into(),
            techniques,
            target_devices,
            target_users,
            c2_domains: campaign_c2,
            malware_hashes,
            objective: objective.to_string(),
            iocs,
            lateral_movement_path: lateral_path,
            data_exfil_gb: if objective == "data_exfiltration" {
                (rng.next_f64() * 49.9 + 0.1) * 100.0 / 100.0
            } else {
                0.0
            },
            detected: rng.next_f64() > 0.15,
            detection_delay_hours: rng.next_range(1, 72) as u32,
        });
    }

    campaigns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_pool_sizes() {
        assert_eq!(workstation_devices().len(), 7 * 15);
        assert_eq!(server_devices().len(), 8 * 5);
    }

    #[test]
    fn campaign_generation_deterministic() {
        let c1 = generate_campaigns(42, 3);
        let c2 = generate_campaigns(42, 3);
        assert_eq!(c1.len(), c2.len());
        for (a, b) in c1.iter().zip(c2.iter()) {
            assert_eq!(a.campaign_id, b.campaign_id);
            assert_eq!(a.name, b.name);
            assert_eq!(a.threat_actor, b.threat_actor);
        }
    }

    #[test]
    fn campaign_count() {
        let campaigns = generate_campaigns(42, 12);
        assert_eq!(campaigns.len(), 12);
        let ids: std::collections::HashSet<_> = campaigns.iter().map(|c| &c.campaign_id).collect();
        assert_eq!(ids.len(), 12);
    }

    #[test]
    fn simple_rng_range() {
        let mut rng = SimpleRng::new(1);
        for _ in 0..100 {
            let v = rng.next_range(5, 10);
            assert!((5..10).contains(&v));
        }
    }

    #[test]
    fn techniques_for_known_objectives() {
        assert!(!techniques_for_objective("ransomware").is_empty());
        assert!(!techniques_for_objective("espionage").is_empty());
        assert!(techniques_for_objective("unknown").is_empty());
    }
}
