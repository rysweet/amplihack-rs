// Code Atlas — Example Queries

// 1. List all atlas layers ordered by layer_id
MATCH (l:AtlasLayer) RETURN l.layer_id, l.slug, l.name, l.node_count, l.edge_count ORDER BY l.layer_id;

// 2. Find all services and their component counts
MATCH (s:AtlasService) RETURN s.name, s.component_count ORDER BY s.component_count DESC;

// 3. Find crates with no internal dependencies (leaf crates)
MATCH (s:AtlasService) WHERE NOT EXISTS { MATCH (s)-[:DEPENDS_ON]->() } RETURN s.name;

// 4. Find the most depended-upon crates
MATCH (s:AtlasService)<-[:DEPENDS_ON]-(d) RETURN s.name, COUNT(d) AS dependents ORDER BY dependents DESC;

// 5. Find dependency chains (transitive deps, 2 hops)
MATCH (a:AtlasService)-[:DEPENDS_ON]->(b:AtlasService)-[:DEPENDS_ON]->(c:AtlasService)
RETURN a.name AS source, b.name AS via, c.name AS transitive_dep;

// 6. Find layers with the most edges (most complex diagrams)
MATCH (l:AtlasLayer) RETURN l.slug, l.edge_count ORDER BY l.edge_count DESC LIMIT 3;

// 7. Total node and edge counts across all layers
MATCH (l:AtlasLayer) RETURN SUM(l.node_count) AS total_nodes, SUM(l.edge_count) AS total_edges;
