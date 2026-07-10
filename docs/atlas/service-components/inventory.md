# service-components inventory (top-level modules per crate)


## `amplihack` (bins/amplihack)

`bin/`

## `amplihack-agent-core` (crates/amplihack-agent-core)

`action_executor`, `agent`, `agentic_loop/`, `answer_synth/`, `code_synthesis`, `cognitive_adapter/`, `continuous_eval`, `error`, `flat_retriever_adapter`, `graph_rag_retriever`, `hierarchical_memory_local`, `hierarchical_memory_types`, `input_events`, `input_source`, `intent`, `json_utils`, `knowledge_utils`, `learning_agent`, `learning_ingestion`, `lifecycle`, `memory_export`, `memory_retrieval`, `models`, `partition_routing`, `prompt_utils`, `runtime_factory`, `safe_calc`, `sdk_adapters/`, `session`, `similarity`, `sub_agents/`, `task_queue`, `temporal_reasoning/`

## `amplihack-agent-eval` (crates/amplihack-agent-eval)

`agent_adapter`, `agent_subprocess`, `compat`, `distributed_adapter`, `domain_eval`, `error`, `five_agent_experiment`, `general_capability/`, `grader`, `gym`, `harness`, `levels`, `llm_grader`, `long_horizon`, `long_horizon_eval`, `long_horizon_multi_seed`, `matrix_eval`, `meta_eval_experiment`, `metacognition_grader`, `models`, `multi_source_collector`, `progressive`, `progressive_levels`, `quiz_generator`, `run_domain_evals`, `sdk_eval_loop`, `security_log/`, `self_improve`, `self_improve_helpers`, `teaching_eval`, `teaching_session`, `teaching_subprocess`, `tests/`, `tla_prompt_experiment`, `trace_to_test`

## `amplihack-agent-generator` (crates/amplihack-agent-generator)

`analyzer`, `assembler`, `distributor`, `documentation_generator`, `error`, `models`, `models_tests`, `packager`, `planner`, `repackage_generator`, `repository_creator`, `synthesizer`, `update_manager`

## `amplihack-asset-resolver-bin` (bins/amplihack-asset-resolver)

_(no top-level modules)_

## `amplihack-blarify` (crates/amplihack-blarify)

`agents/`, `code_refs/`, `db/`, `documentation/`, `graph/`, `languages/`, `mcp/`, `project/`, `tools/`, `vcs/`

## `amplihack-builders` (crates/amplihack-builders)

`claude/`, `codex/`, `export_on_compact`

## `amplihack-cli` (crates/amplihack-cli)

`auto_mode_append`, `auto_mode_completion_signals`, `auto_mode_completion_verifier`, `auto_mode_state`, `auto_mode_ui/`, `auto_mode_work_summary`, `auto_mode_work_summary_generator`, `auto_stager`, `auto_update`, `binary_finder`, `bootstrap`, `ci_resource_discipline_tests`, `claude_plugin`, `cli_commands`, `cli_extensions`, `cli_subcommands`, `cli_tests`, `command_error`, `commands/`, `copilot_setup/`, `docker/`, `env_builder/`, `fleet_local/`, `freshness`, `health_check`, `install_output_contract`, `launcher`, `launcher_context`, `memory_config`, `nesting/`, `path_conflicts/`, `path_conflicts`, `pr_recovery_readiness`, `remote_cli_tests`, `resolve_bundle_asset/`, `runtime_assets`, `rust_trial`, `self_heal`, `session_tracker`, `settings_manager`, `signals`, `test_support`, `tool_update_check/`, `uninstall`, `update/`, `util`

## `amplihack-context` (crates/amplihack-context)

`launcher_detector`, `lsp_detector`, `migration`, `mode_detector`, `path_resolver`, `strategies`

## `amplihack-delegation` (crates/amplihack-delegation)

`error`, `evidence_collector`, `models`, `persona`, `platform_cli/`, `scenario`, `state_machine`, `success_evaluator`, `tests/`

## `amplihack-domain-agents` (crates/amplihack-domain-agents)

`base`, `code_review/`, `code_synthesis`, `error`, `learning`, `meeting_synthesizer/`, `models`, `router`, `security`, `skill_catalog`, `skill_injector`, `teaching`

## `amplihack-fleet` (crates/amplihack-fleet)

`auth`, `health`, `task_queue`, `transcript`, `vm_state`

## `amplihack-hive` (crates/amplihack-hive)

`bloom`, `controller`, `crdt/`, `dht/`, `distributed/`, `embeddings`, `error`, `event_bus`, `fact_lifecycle`, `feed`, `gossip`, `graph/`, `hive_eval`, `hive_events`, `models`, `orchestrator`, `quality`, `query_expansion`, `reranker`, `tests/`, `workload`

## `amplihack-hooks` (crates/amplihack-hooks)

`agent_memory`, `copilot_stop_handler`, `hook_verification`, `issue_dedup`, `known_agents`, `known_skills`, `original_request`, `post_tool_use/`, `pre_compact/`, `pre_tool_use/`, `precommit_prefs`, `prompt_input`, `protocol`, `session_start/`, `session_stop/`, `stop/`, `strategies/`, `test_support`, `user_prompt/`, `workflow_classification`

## `amplihack-hooks-bin` (bins/amplihack-hooks)

_(no top-level modules)_

## `amplihack-launcher` (crates/amplihack-launcher)

`agent_memory`, `amplifier`, `append_handler`, `auto_mode`, `auto_mode_coordinator`, `auto_mode_exec`, `auto_mode_state`, `auto_mode_ui`, `auto_stager`, `claude_binary_manager`, `codex`, `completion_signals`, `completion_verifier`, `completion_verifier_tests`, `copilot_auto_install`, `copilot_launcher`, `copilot_mcp`, `copilot_staging`, `flag_matrix`, `fork_manager`, `json_logger`, `launcher_core`, `memory_config`, `nesting_detector`, `platform_check`, `prompt_delivery`, `repo_checkout`, `session_capture`, `session_tracker`, `settings_manager`, `staging_cleanup`, `staging_safety`, `work_summary`, `work_summary_tests`

## `amplihack-memory` (crates/amplihack-memory)

`agent_memory`, `auto_backend`, `auto_install`, `backend`, `bloom`, `config`, `context_preservation`, `coordinator`, `database`, `database_helpers`, `discoveries`, `distributed_store`, `evaluation/`, `facade`, `graph_db`, `graph_store`, `hash_ring`, `maintenance`, `manager`, `memory_store`, `models`, `network_store`, `network_store_types`, `pyo3_bindings`, `quality`, `retrieval/`, `retrieval_pipeline`, `sqlite_backend`, `storage_pipeline`, `tests/`

## `amplihack-multilspy` (crates/amplihack-multilspy)

`config`, `error`, `language_server`, `lsp_client`, `servers/`, `types`

## `amplihack-orchestration` (crates/amplihack-orchestration)

`claude_process`, `claude_process_builder`, `execution`, `patterns/`, `result_sink`, `session`, `text_utils`, `time_utils`

## `amplihack-recovery` (crates/amplihack-recovery)

`coordinator`, `models`, `results`, `stage1`, `stage2`, `stage3`, `stage4`

## `amplihack-reflection` (crates/amplihack-reflection)

`display`, `error_analysis/`, `lightweight_analyzer`, `reflection`, `security`, `semantic_duplicate_detector`, `semaphore`, `state_machine`

## `amplihack-remote` (crates/amplihack-remote)

`auth`, `azlin_parse`, `cli`, `commands`, `error`, `executor`, `integrator`, `orchestrator`, `packager`, `session`, `state_lock`, `vm_pool`

## `amplihack-safety` (crates/amplihack-safety)

`conflict_detector`, `copy_strategy`, `prompt_transformer`

## `amplihack-security` (crates/amplihack-security)

`config`, `defender`, `health`, `patterns`, `risk`, `tests/`

## `amplihack-session` (crates/amplihack-session)

`batch`, `config`, `file_utils`, `logger`, `manager`, `session`, `toolkit`

## `amplihack-state` (crates/amplihack-state)

`atomic_json`, `counter`, `env_config`, `file_lock`, `semaphore`

## `amplihack-types` (crates/amplihack-types)

`hook_io`, `paths/`, `settings`, `workflow`

## `amplihack-utils` (crates/amplihack-utils)

`agent_binary`, `artifact_guard`, `bundle_generator`, `claude_cli`, `claude_md`, `cleanup`, `defensive`, `docker_detector`, `docker_manager`, `hook_merge`, `kb_types`, `knowledge_builder`, `litellm_callbacks`, `llm_client`, `observability`, `plugin_cli`, `plugin_manager`, `plugin_manager_paths`, `plugin_manifest`, `plugin_verifier`, `power_steering`, `prerequisites`, `process`, `project_init`, `project_init_detect`, `prompt_delivery`, `secure_files`, `send_input_allowlist`, `settings_generator`, `settings_helpers`, `simple_tui`, `simple_tui_runner`, `simple_tui_types`, `slugify`, `terminal_launcher`, `tests/`, `trace_logger`, `uvx_manager`, `worktree`

## `amplihack-workflows` (crates/amplihack-workflows)

`agent_contract`, `cascade`, `classifier`, `gh_aw_compiler`, `orchestrator`, `provenance`, `remote_repository`, `session`, `simulation`, `stale_cleanup`, `workflow_contract`
