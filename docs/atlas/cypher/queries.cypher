// Example queries
// All bins and their direct internal deps:
MATCH (a:Crate {kind:'bin'})-[:DEPENDS_ON]->(b) RETURN a.name, b.name;
// Crates with no internal dependents:
MATCH (c:Crate) WHERE NOT ()-[:DEPENDS_ON]->(c) RETURN c.name;
// Fan-in per crate:
MATCH (a)-[:DEPENDS_ON]->(c:Crate) RETURN c.name, count(a) AS dependents ORDER BY dependents DESC;
