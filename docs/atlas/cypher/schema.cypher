// Code Atlas — OpenCypher Schema
// Compatible with Kuzu, Neo4j, Memgraph, and other OpenCypher databases.

// Node tables
CREATE NODE TABLE IF NOT EXISTS AtlasLayer (
    layer_id INT64,
    slug STRING,
    name STRING,
    diagram_path_mmd STRING,
    diagram_path_dot STRING,
    node_count INT64,
    edge_count INT64,
    built_at STRING,
    PRIMARY KEY (layer_id)
);

CREATE NODE TABLE IF NOT EXISTS AtlasService (
    service_id STRING,
    name STRING,
    component_count INT64,
    PRIMARY KEY (service_id)
);

CREATE NODE TABLE IF NOT EXISTS CodeFile (
    file_path STRING,
    crate_name STRING,
    line_count INT64,
    PRIMARY KEY (file_path)
);

// Relationship tables
CREATE REL TABLE IF NOT EXISTS ATLAS_MAPS (
    FROM AtlasLayer TO CodeFile
);

CREATE REL TABLE IF NOT EXISTS SERVICE_CONTAINS (
    FROM AtlasService TO CodeFile
);
