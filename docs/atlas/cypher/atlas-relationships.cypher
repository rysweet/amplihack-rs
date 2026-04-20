// Code Atlas — Relationships

// Compile dependency edges (crate -> crate it depends on)
MATCH (a:AtlasService {service_id: 'amplihack-state'}), (b:AtlasService {service_id: 'amplihack-types'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-agent-core'}), (b:AtlasService {service_id: 'amplihack-memory'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-domain-agents'}), (b:AtlasService {service_id: 'amplihack-agent-core'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-domain-agents'}), (b:AtlasService {service_id: 'amplihack-memory'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-domain-agents'}), (b:AtlasService {service_id: 'amplihack-workflows'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hive'}), (b:AtlasService {service_id: 'amplihack-agent-core'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hive'}), (b:AtlasService {service_id: 'amplihack-memory'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-cli'}), (b:AtlasService {service_id: 'amplihack-types'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-cli'}), (b:AtlasService {service_id: 'amplihack-state'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-cli'}), (b:AtlasService {service_id: 'amplihack-hive'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hooks'}), (b:AtlasService {service_id: 'amplihack-types'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hooks'}), (b:AtlasService {service_id: 'amplihack-state'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hooks'}), (b:AtlasService {service_id: 'amplihack-cli'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hooks'}), (b:AtlasService {service_id: 'amplihack-security'}) CREATE (a)-[:DEPENDS_ON]->(b);
MATCH (a:AtlasService {service_id: 'amplihack-hooks'}), (b:AtlasService {service_id: 'amplihack-workflows'}) CREATE (a)-[:DEPENDS_ON]->(b);
