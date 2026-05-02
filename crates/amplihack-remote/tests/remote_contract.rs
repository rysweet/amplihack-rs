//! crates/amplihack-remote/tests/remote_contract.rs
//!
//! Issue #536 tests-first contract for the native Rust port of
//! amplifier-bundle/tools/amplihack/remote/.

use std::sync::Mutex;

use amplihack_remote::{
    AzureCredentials, ErrorContext, ExecutionResult, Executor, RemoteError, SessionManager,
    SessionStatus, VM, VMOptions, VMSize,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn remove(key: &'static str) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, old }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn session_defaults_preserve_detached_node_options_memory_contract() {
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("remote-state.json");
    let mut sessions = SessionManager::new(Some(state_file.clone())).unwrap();

    let session = sessions
        .create_session("amplihack-vm", "ship the remote port", None, None, None)
        .unwrap();

    assert_eq!(session.command, "auto");
    assert_eq!(session.max_turns, 10);
    assert_eq!(session.status, SessionStatus::Pending);
    assert_eq!(
        session.memory_mb, 32768,
        "detached remote sessions must persist the NODE_OPTIONS memory contract"
    );
    assert_eq!(
        SessionManager::DEFAULT_MEMORY_MB,
        32768,
        "default memory must match --max-old-space-size=32768"
    );

    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_file).unwrap()).unwrap();
    let persisted_memory = state["sessions"][session.session_id.as_str()]["memory_mb"]
        .as_u64()
        .unwrap();
    assert_eq!(persisted_memory, 32768);
}

#[test]
fn session_lifecycle_and_validation_contracts_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("remote-state.json");
    let mut sessions = SessionManager::new(Some(state_file)).unwrap();

    assert!(
        sessions
            .create_session("", "prompt", None, None, None)
            .is_err()
    );
    assert!(sessions.create_session("vm", "", None, None, None).is_err());
    assert!(
        sessions
            .create_session("vm", "prompt", None, Some(0), None)
            .is_err()
    );
    assert!(
        sessions
            .create_session("vm", "prompt", None, None, Some(0))
            .is_err()
    );

    let session = sessions
        .create_session("vm", "prompt", Some("fix"), Some(12), Some(32768))
        .unwrap();
    assert_eq!(session.command, "fix");
    assert_eq!(session.max_turns, 12);
    assert_eq!(session.memory_mb, 32768);

    let started = sessions.start_session(&session.session_id).unwrap();
    assert_eq!(started.status, SessionStatus::Running);
    assert!(started.started_at.is_some());
    assert!(sessions.start_session(&session.session_id).is_err());

    assert!(sessions.kill_session(&session.session_id));
    let killed = sessions.get_session(&session.session_id).unwrap();
    assert_eq!(killed.status, SessionStatus::Killed);
    assert!(killed.completed_at.is_some());
    assert!(!sessions.kill_session("sess-20990101-000000-dead"));
}

#[test]
fn vm_pool_size_and_provisioning_defaults_match_python_contract() {
    assert_eq!(VMSize::S.capacity(), 1);
    assert_eq!(VMSize::M.capacity(), 2);
    assert_eq!(VMSize::L.capacity(), 4);
    assert_eq!(VMSize::XL.capacity(), 8);

    assert_eq!(VMSize::S.azure_size(), "Standard_D8s_v3");
    assert_eq!(VMSize::M.azure_size(), "Standard_E8s_v5");
    assert_eq!(VMSize::L.azure_size(), "Standard_E16s_v5");
    assert_eq!(VMSize::XL.azure_size(), "Standard_E32s_v5");

    let options = VMOptions::default();
    assert_eq!(options.size, "Standard_D2s_v3");
    assert_eq!(options.region, None);
    assert_eq!(options.vm_name, None);
    assert!(!options.no_reuse);
    assert!(!options.keep_vm);
    assert_eq!(options.azlin_extra_args, None);
    assert_eq!(options.tunnel_port, None);
}

#[test]
fn azure_credentials_require_all_fields_and_never_serialize_secret() {
    let missing = AzureCredentials {
        tenant_id: String::new(),
        client_id: "client".into(),
        client_secret: "secret".into(),
        subscription_id: "subscription".into(),
        resource_group: None,
    };
    let err = missing.validate().unwrap_err();
    assert!(err.contains("tenant_id"));

    let credentials = AzureCredentials {
        tenant_id: "tenant".into(),
        client_id: "client".into(),
        client_secret: "super-secret".into(),
        subscription_id: "subscription".into(),
        resource_group: Some("rg".into()),
    };
    credentials.validate().unwrap();
    let json = serde_json::to_value(&credentials).unwrap();
    assert!(json.get("client_secret").is_none());
    assert_eq!(json["tenant_id"], "tenant");
}

#[test]
fn remote_errors_preserve_phase_and_context() {
    let err = RemoteError::provisioning_ctx(
        "quota exceeded",
        ErrorContext::new()
            .insert("vm_name", "amplihack-test")
            .insert("region", "eastus"),
    );
    let rendered = err.to_string();

    assert!(matches!(err, RemoteError::ProvisioningError { .. }));
    assert!(rendered.contains("Provisioning error"));
    assert!(rendered.contains("quota exceeded"));
    assert!(rendered.contains("vm_name=amplihack-test"));
    assert!(rendered.contains("region=eastus"));
}

#[test]
fn execution_result_is_stable_json_contract() {
    let result = ExecutionResult {
        exit_code: 7,
        stdout: "out".into(),
        stderr: "err".into(),
        duration_seconds: 1.25,
        timed_out: false,
    };

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["exit_code"], 7);
    assert_eq!(json["stdout"], "out");
    assert_eq!(json["stderr"], "err");
    assert_eq!(json["duration_seconds"], 1.25);
    assert_eq!(json["timed_out"], false);
}

#[test]
fn executor_validates_anthropic_api_key_before_remote_execution() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _api_key = EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let vm = VM {
        name: "fake-vm".into(),
        size: "Standard_D2s_v3".into(),
        region: "eastus".into(),
        created_at: None,
        tags: None,
    };
    let executor = Executor::new(vm, 1, None);

    let err = runtime
        .block_on(executor.execute_remote("auto", "prompt", 10))
        .unwrap_err();

    assert!(matches!(err, RemoteError::ExecutionError { .. }));
    assert!(
        err.to_string().contains("ANTHROPIC_API_KEY"),
        "missing credential must be surfaced before any remote command is attempted: {err}"
    );
}
