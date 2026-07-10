// amplihack-rs atlas graph schema (Kuzu / OpenCypher)
CREATE NODE TABLE IF NOT EXISTS Crate(name STRING PRIMARY KEY, kind STRING, dir STRING);
CREATE REL TABLE IF NOT EXISTS DEPENDS_ON(FROM Crate TO Crate);
