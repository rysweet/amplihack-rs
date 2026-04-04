use std::collections::HashMap;

use tracing::{debug, info, warn};

use super::reference::Reference;
use super::scip::ScipReferenceResolver;
use crate::project::config::ProjectDetector;

// ---------------------------------------------------------------------------
// ResolverMode
// ---------------------------------------------------------------------------

/// Strategy for resolving code references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverMode {
    ScipOnly,
    LspOnly,
    ScipWithLspFallback,
    Auto,
}

// ---------------------------------------------------------------------------
// HybridReferenceResolver
// ---------------------------------------------------------------------------

/// Resolves code references using SCIP indexes with optional LSP fallback.
///
/// In `Auto` mode, selects SCIP for Python/TypeScript projects and falls
/// back to LSP for others.
#[derive(Debug)]
pub struct HybridReferenceResolver {
    pub root_uri: String,
    pub mode: ResolverMode,
    scip_resolver: Option<ScipReferenceResolver>,
    use_scip: bool,
}

impl HybridReferenceResolver {
    pub fn new(root_uri: &str, mode: ResolverMode) -> Self {
        let root_path = root_uri.strip_prefix("file://").unwrap_or(root_uri);
        let mut resolver = Self {
            root_uri: root_uri.to_string(),
            mode,
            scip_resolver: None,
            use_scip: false,
        };
        resolver.setup_resolvers(root_path);
        resolver
    }

    fn setup_resolvers(&mut self, root_path: &str) {
        match self.mode {
            ResolverMode::ScipOnly | ResolverMode::ScipWithLspFallback | ResolverMode::Auto => {
                if let Some(lang) = ProjectDetector::get_primary_language(root_path) {
                    if lang == "python" || lang == "typescript" {
                        info!(language = %lang, "setting up SCIP resolver");
                        self.scip_resolver = Some(ScipReferenceResolver::new(root_path, None));
                        self.use_scip = true;
                    } else {
                        debug!(language = %lang, "SCIP not available for this language");
                    }
                }
            }
            ResolverMode::LspOnly => {
                debug!("LSP-only mode, skipping SCIP setup");
            }
        }
    }

    /// Try to set up the SCIP resolver (generate index if needed).
    pub fn try_setup_scip(&mut self) -> bool {
        if let Some(ref mut scip) = self.scip_resolver {
            match scip.generate_index_if_needed("blarify") {
                Ok(generated) => {
                    if generated {
                        info!("SCIP index generated successfully");
                    }
                    let loaded = scip.ensure_loaded();
                    if loaded {
                        let stats = scip.get_statistics();
                        info!(
                            documents = stats.get("documents").unwrap_or(&0),
                            symbols = stats.get("symbols").unwrap_or(&0),
                            "SCIP index loaded"
                        );
                    }
                    loaded
                }
                Err(e) => {
                    warn!(error = %e, "failed to set up SCIP resolver");
                    false
                }
            }
        } else {
            false
        }
    }

    /// Get references for a symbol.
    pub fn get_references(&self, symbol: &str) -> Vec<Reference> {
        if self.use_scip
            && let Some(ref scip) = self.scip_resolver
        {
            let refs = scip.get_references_for_symbol(symbol);
            if !refs.is_empty() {
                return refs;
            }
            debug!(symbol = %symbol, "no SCIP references found, would fall back to LSP");
        }
        // LSP fallback would go here in a full implementation
        Vec::new()
    }

    /// Get resolver info for diagnostics.
    pub fn get_resolver_info(&self) -> HashMap<String, String> {
        let mut info = HashMap::new();
        info.insert("root_uri".into(), self.root_uri.clone());
        info.insert("mode".into(), format!("{:?}", self.mode));
        info.insert("use_scip".into(), self.use_scip.to_string());
        info.insert(
            "scip_available".into(),
            self.scip_resolver.is_some().to_string(),
        );
        info
    }

    /// Shut down resolvers.
    pub fn shutdown(&mut self) {
        self.scip_resolver = None;
        self.use_scip = false;
        info!("hybrid resolver shut down");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_mode_auto_default() {
        let r = HybridReferenceResolver::new("/nonexistent/path", ResolverMode::Auto);
        assert!(!r.use_scip);
    }

    #[test]
    fn resolver_info() {
        let r = HybridReferenceResolver::new("/repo", ResolverMode::ScipOnly);
        let info = r.get_resolver_info();
        assert_eq!(info["root_uri"], "/repo");
        assert!(info.contains_key("mode"));
    }

    #[test]
    fn resolver_get_references_empty() {
        let r = HybridReferenceResolver::new("/repo", ResolverMode::Auto);
        let refs = r.get_references("some.symbol");
        assert!(refs.is_empty());
    }

    #[test]
    fn resolver_shutdown() {
        let mut r = HybridReferenceResolver::new("/repo", ResolverMode::Auto);
        r.shutdown();
        assert!(!r.use_scip);
        assert!(r.scip_resolver.is_none());
    }

    #[test]
    fn resolver_lsp_only_mode() {
        let r = HybridReferenceResolver::new("/repo", ResolverMode::LspOnly);
        assert!(!r.use_scip);
        assert!(r.scip_resolver.is_none());
    }
}
