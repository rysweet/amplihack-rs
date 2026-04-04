//! # amplihack-blarify
//!
//! Code-graph analysis engine ported from the Python blarify project.
//! Builds a navigable graph of code entities (files, folders, functions, classes)
//! and their relationships (contains, defines, calls, imports, etc.).

pub mod agents;
pub mod code_refs;
pub mod db;
pub mod documentation;
pub mod graph;
pub mod languages;
pub mod mcp;
pub mod project;
pub mod tools;
pub mod vcs;
