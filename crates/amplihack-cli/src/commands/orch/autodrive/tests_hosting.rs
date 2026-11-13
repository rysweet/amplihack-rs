//! Controlled hosting dependencies for #2555, with no network or real profiles.
//!
//! Only hosting responses are simulated. Policy selection, readiness acceptance,
//! comment reconciliation and durable completion must run production code.
//! Production policy and handoff actions consume these controlled responses.

use crate::commands::orch::autodrive::{
    hosting::{BaseConfig, Comment, CommentPage, HandoffHosting, PolicyHosting, PrObservation},
    records::{Binding, PlatformState},
};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

pub const STEWARD: &str = "This pull request is ready for its steward.";
pub const LOGIN: &str = "drive-bot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Observe,
    BaseTip {
        repository: String,
        branch: String,
    },
    Config {
        repository: String,
        commit: String,
        path: String,
    },
    Ready,
    Login,
    Comments(Option<String>),
    Comment(u64),
    Create(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Observe,
    BaseTip,
    Config,
    Ready,
    ReadyNotApplied,
    Login,
    Comments,
    Comment,
    Create,
    LostCreateResponse,
}

pub struct Hosting {
    pub pr: PrObservation,
    pub config: BaseConfig,
    pub base_tip: String,
    pub calls: Vec<Call>,
    pub fault: Option<Fault>,
    pub pages: BTreeMap<Option<String>, CommentPage>,
    pub unreadable_cursor: Option<String>,
    pub comments: BTreeMap<u64, Comment>,
    /// Each observation consumes one queued response. This gives deterministic
    /// changes between reads without another thread or a wall-clock window.
    pub observations: VecDeque<PrObservation>,
    pub change_pr_after_create: Option<PrObservation>,
    pub remove_state_on_comment_read: Option<PathBuf>,
    next_id: u64,
}

impl Hosting {
    pub fn new(binding: Binding) -> Self {
        Self {
            base_tip: binding.base_sha.clone(),
            pr: PrObservation {
                binding,
                state: PlatformState::Open,
                draft: false,
                auto_merge_enabled: false,
            },
            config: BaseConfig::Missing,
            calls: Vec::new(),
            fault: None,
            pages: BTreeMap::new(),
            unreadable_cursor: None,
            comments: BTreeMap::new(),
            observations: VecDeque::new(),
            change_pr_after_create: None,
            remove_state_on_comment_read: None,
            next_id: 100,
        }
    }

    fn fail(&self, operation: Fault) -> Result<()> {
        if self.fault == Some(operation) {
            bail!("fixture-{operation:?}: authenticated hosting operation failed");
        }
        Ok(())
    }

    pub fn matching_comment(&self, id: u64) -> Comment {
        Comment {
            id,
            pr_url: self.pr.binding.pr_url.clone(),
            author: LOGIN.into(),
            body: STEWARD.into(),
        }
    }

    pub fn seed_comment(&mut self, comment: Comment) {
        self.comments.insert(comment.id, comment);
    }

    pub fn creations(&self) -> usize {
        self.calls
            .iter()
            .filter(|c| matches!(c, Call::Create(_)))
            .count()
    }

    pub fn clear_calls(&mut self) {
        self.calls.clear();
    }
}

// Deliberately no merge or scheduling capability in either interface. A
// successful action must additionally report zero such requests in its outcome.
impl PolicyHosting for Hosting {
    fn observe_pr(&mut self, _binding: &Binding) -> Result<PrObservation> {
        self.calls.push(Call::Observe);
        self.fail(Fault::Observe)?;
        if let Some(pr) = self.observations.pop_front() {
            self.pr = pr;
        }
        Ok(self.pr.clone())
    }

    fn base_tip(&mut self, binding: &Binding) -> Result<String> {
        self.calls.push(Call::BaseTip {
            repository: binding.repository.clone(),
            branch: binding.base_branch.clone(),
        });
        self.fail(Fault::BaseTip)?;
        Ok(self.base_tip.clone())
    }

    fn read_base_config(
        &mut self,
        binding: &Binding,
        commit: &str,
        path: &str,
    ) -> Result<BaseConfig> {
        self.calls.push(Call::Config {
            repository: binding.repository.clone(),
            commit: commit.into(),
            path: path.into(),
        });
        self.fail(Fault::Config)?;
        Ok(self.config.clone())
    }
}

impl HandoffHosting for Hosting {
    fn mark_ready(&mut self, _binding: &Binding) -> Result<()> {
        self.calls.push(Call::Ready);
        self.fail(Fault::Ready)?;
        if self.fault != Some(Fault::ReadyNotApplied) {
            self.pr.draft = false;
        }
        Ok(())
    }

    fn authenticated_login(&mut self) -> Result<String> {
        self.calls.push(Call::Login);
        self.fail(Fault::Login)?;
        Ok(LOGIN.into())
    }

    fn list_comments(&mut self, _binding: &Binding, cursor: Option<&str>) -> Result<CommentPage> {
        let cursor = cursor.map(str::to_owned);
        self.calls.push(Call::Comments(cursor.clone()));
        self.fail(Fault::Comments)?;
        if cursor.is_some() && cursor == self.unreadable_cursor {
            bail!("fixture-comment-page-unreadable");
        }
        Ok(self
            .pages
            .get(&cursor)
            .cloned()
            .unwrap_or_else(|| CommentPage {
                comments: self.comments.values().cloned().collect(),
                next: None,
            }))
    }

    fn read_comment(&mut self, _binding: &Binding, id: u64) -> Result<Comment> {
        self.calls.push(Call::Comment(id));
        self.fail(Fault::Comment)?;
        let comment = self
            .comments
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("fixture-comment-missing"))?;
        if let Some(state) = self.remove_state_on_comment_read.take() {
            // Removing the directory, rather than changing permissions, also
            // causes openat publication to fail when CI runs as root.
            std::fs::remove_dir_all(state)?;
        }
        Ok(comment)
    }

    fn create_comment(&mut self, _binding: &Binding, body: &str) -> Result<Comment> {
        self.calls.push(Call::Create(body.into()));
        self.fail(Fault::Create)?;
        let mut comment = self.matching_comment(self.next_id);
        comment.body = body.into();
        self.next_id += 1;
        self.seed_comment(comment.clone());
        if let Some(pr) = self.change_pr_after_create.take() {
            self.pr = pr;
        }
        self.fail(Fault::LostCreateResponse)?;
        Ok(comment)
    }
}
