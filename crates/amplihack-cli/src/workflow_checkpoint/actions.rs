//! The checkpoint actions and the publication decisions they share.

use super::advert::AdvertMap;
use super::authority::{
    binding_str, capture_publication, file_identity, observe, private_dir, retain,
};
use super::json::{digest, parse, serial, unb64};
use super::repo::{
    env_os, env_required, env_text, get, get_arr, get_obj, get_str, lines, path_text, selectors,
    strip_newlines, worktree_setup,
};
use super::{Ctx, Fail, Lookup, Res, io_refusal, refuse};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const INSPECTION_POLICY: &str = "External filters use qualified identity normalisation only during inspection. \
Candidates can include filter-normalised files; this grants no task path permission. \
Normal authorised staging retains operator filters.";

/// What a successful action produced.
pub(crate) enum Done {
    /// Exit 0 with these stdout bytes.
    Stdout(Vec<u8>),
    /// Exit 3 with empty stdout: `advertised-ref` found the exact ref absent.
    Absent,
    /// `verify-pushed-ref`: the verifier's stdout, written on every status,
    /// then the verifier's own status (`None` is 0).
    Verifier(Vec<u8>, Option<std::num::NonZeroI32>),
}

/// A text argument, read where 918c indexed `sys.argv`: a missing one is
/// that `IndexError` (status 1). A non-UTF-8 value is refused; see
/// `super::json::SURROGATE_AUDIT`.
fn arg(args: &[OsString], i: usize) -> Res<&str> {
    super::indexed(args, i)?
        .to_str()
        .ok_or_else(|| Fail::Refusal(format!("non-UTF-8 argument {}", i + 1)))
}

fn texts(args: &[OsString]) -> Res<Vec<&str>> {
    (0..args.len()).map(|i| arg(args, i)).collect()
}

fn without(map: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    map.iter()
        .filter(|(k, _)| !keys.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn object(value: Value, reason: &str) -> Res<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(Fail::Refusal(reason.into())),
    }
}

/// Decision 17.116 withdraws rejection of unreachable local objects. Keep the
/// captured inventory for existing-identity checks, not permission.
fn candidate_digest(snapshot: &Map<String, Value>) -> String {
    digest(&serial(&Value::Object(without(
        snapshot,
        &["initial_commits"],
    ))))
}

/// The one encoder for the NUL records the shell reads with fixed
/// `read -r -d ''` assignments: exactly `N` fields, each NUL-terminated, no
/// trailing bytes. A field containing NUL is refused.
pub(crate) fn nul_record<const N: usize>(fields: [&[u8]; N]) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    for field in fields {
        refuse(!field.contains(&0), "record field contains NUL")?;
        out.extend_from_slice(field);
        out.push(0);
    }
    Ok(out)
}

fn line(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out.push(b'\n');
    out
}

impl Ctx {
    fn snapshot(
        &mut self,
        binding: &Map<String, Value>,
        captured: Option<&Map<String, Value>>,
    ) -> Res<Map<String, Value>> {
        let captured_publication = match captured {
            Some(c) => Some(get_obj(c, "publication")?.clone()),
            None => None,
        };
        let publication = self.publication(binding, captured_publication.as_ref())?;
        let history = self.outgoing(&publication, "HEAD")?;
        let mut paths = self.pending()?;
        for row in &history {
            paths.extend(row_paths(row)?);
        }
        let identities = paths
            .iter()
            .map(|p| self.identity(p))
            .collect::<Res<Vec<_>>>()?;
        let snapshot = json!({
            "paths": identities,
            "outgoing_commits": history,
            "publication": publication,
            "initial_commits": self.commits()?,
            "inspection_policy": INSPECTION_POLICY,
        });
        object(snapshot, "malformed snapshot")
    }
}

/// Run the named action. `run` has already matched the name against the
/// closed table; each action reads its arguments where 918c did.
pub(crate) fn dispatch(ctx: &mut Ctx, action: &str, args: &[OsString]) -> Res<Done> {
    let stdout = match action {
        "selectors" => selectors().map(|()| Vec::new()),
        "capture" => capture(ctx, arg(args, 0)?),
        "admit" => admit(ctx, arg(args, 0)?),
        "check" => ctx.check().map(|_| Vec::new()),
        "paths" => paths(ctx),
        "stage-strict" => super::strict::stage_strict(args),
        "before" => before(ctx, &texts(args)?),
        "sync-fetch-mode" => sync_fetch_mode(ctx, &texts(args)?),
        "after" => after(ctx, args),
        "destination-is-head" => destination_is_head(ctx),
        "verify-pushed-ref" => return verify_pushed_ref(ctx, args),
        "advertised-ref" => {
            return advertised_ref(args).map(|found| match found {
                Lookup::Present => Done::Stdout(Vec::new()),
                Lookup::Absent => Done::Absent,
            });
        }
        "sha256" => sha256_stdin(),
        "admission-refused-result" => Ok(admission_refused_result()),
        "cleanup-skip-result" => Ok(cleanup_skip_result(super::indexed(args, 0)?)),
        "worktree-path" => worktree_path(),
        _ => Err(Fail::Refusal("unknown private checkpoint operation".into())),
    }?;
    Ok(Done::Stdout(stdout))
}

fn capture(ctx: &mut Ctx, phase: &str) -> Res<Vec<u8>> {
    let mut binding = ctx.context(phase)?;
    ctx.clean_index()?;
    let snapshot = ctx.snapshot(&binding, None)?;
    binding.insert(
        "invocation".into(),
        json!(uuid::Uuid::new_v4().simple().to_string()),
    );
    binding.insert(
        "candidate_digest".into(),
        json!(candidate_digest(&snapshot)),
    );
    let index = std::fs::read(get_str(&binding, "index_path")?).map_err(io_refusal("index"))?;
    let mut value = snapshot;
    value.insert("binding".into(), Value::Object(binding));
    value.insert("skipped".into(), Value::Bool(false));
    value.insert("index_sha256".into(), json!(digest(&index)));
    let directory = private_dir(ctx)?;
    let path = directory.join("capture.json");
    let sha = retain(&path, &Value::Object(value.clone()))?;
    // The reviewer needs the finite candidates and outgoing history. Keep the
    // existing-object inventory private rather than exporting it through a
    // recipe environment variable subject to the platform argv limit.
    let mut public = without(&value, &["initial_commits"]);
    public.insert("capture_file".into(), json!(path_text(&path)?));
    public.insert("capture_hash".into(), json!(sha));
    Ok(line(&serial(&Value::Object(public))))
}

fn admit(ctx: &mut Ctx, phase: &str) -> Res<Vec<u8>> {
    let binding = ctx.context(phase)?;
    ctx.clean_index()?;
    let cap = object(
        parse(env_required("RECIPE_VAR_checkpoint_scope_capture")?.as_bytes())?,
        "missing deterministic capture",
    )?;
    let path = PathBuf::from(get_str(&cap, "capture_file")?);
    refuse(
        !ctx.within_root(&super::repo::canonical(&path)),
        "capture lies inside worktree",
    )?;
    let raw = std::fs::read(&path).map_err(io_refusal("capture"))?;
    refuse(
        Some(digest(&raw).as_str()) == cap.get("capture_hash").and_then(Value::as_str),
        "capture hash mismatch",
    )?;
    let saved = object(parse(&raw)?, "malformed capture")?;
    refuse(
        without(&saved, &["initial_commits"]) == without(&cap, &["capture_file", "capture_hash"]),
        "capture output mismatch",
    )?;
    let saved_binding = get_obj(&saved, "binding")?;
    for (key, value) in &binding {
        refuse(
            saved_binding.get(key) == Some(value),
            &format!("stale capture binding: {key}"),
        )?;
    }
    let index = std::fs::read(get_str(&binding, "index_path")?).map_err(io_refusal("index"))?;
    refuse(
        saved.get("index_sha256").and_then(Value::as_str) == Some(digest(&index).as_str()),
        "initial index changed",
    )?;
    let current = ctx.snapshot(&binding, Some(&saved))?;
    refuse(
        saved_binding
            .get("candidate_digest")
            .and_then(Value::as_str)
            == Some(candidate_digest(&current).as_str()),
        "initial candidates changed",
    )?;
    let verdict = parse(env_required("RECIPE_VAR_checkpoint_scope_review")?.as_bytes())?;
    let verdict = match verdict {
        Value::Object(map)
            if map.len() == 4
                && ["approved", "binding", "approved_paths", "reason"]
                    .iter()
                    .all(|k| map.contains_key(*k)) =>
        {
            map
        }
        _ => return Err(Fail::Refusal("malformed scope verdict".into())),
    };
    refuse(
        verdict.get("approved") == Some(&Value::Bool(true))
            && verdict.get("binding") == Some(&Value::Object(saved_binding.clone())),
        "scope review refused or mismatched",
    )?;
    refuse(
        verdict
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|r| !r.trim().is_empty()),
        "scope reason is required",
    )?;
    let approved = verdict
        .get("approved_paths")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::Refusal("approved paths must be finite".into()))?;
    let decoded = approved.iter().map(unb64).collect::<Res<Vec<_>>>()?;
    let unique: BTreeSet<Vec<u8>> = decoded.iter().cloned().collect();
    refuse(decoded.len() == unique.len(), "duplicate approved identity")?;
    let captured = get_arr(&saved, "paths")?
        .iter()
        .map(|p| unb64(p.get("path_b64").unwrap_or(&Value::Null)))
        .collect::<Res<BTreeSet<_>>>()?;
    refuse(
        unique == captured,
        "approval must cover all pending and outgoing candidates",
    )?;
    let record = json!({
        "binding": saved_binding,
        "approved_paths": approved,
        "capture": saved,
        "verdict": verdict,
    });
    let directory = private_dir(ctx)?;
    let path = directory.join("authority.json");
    let sha = retain(&path, &record)?;
    // The shell retains these values independently. No sidecar checksum is
    // read on continuation; observations are outside this hashed record.
    let publication = get_obj(&saved, "publication")?;
    nul_record([
        path.as_os_str().as_bytes(),
        file_identity(&path)?.as_bytes(),
        sha.as_bytes(),
        get_str(saved_binding, "invocation")?.as_bytes(),
        get_str(&binding, "initial_head")?.as_bytes(),
        get_str(publication, "upstream")?.as_bytes(),
        get_str(publication, "upstream_ref")?.as_bytes(),
        get_str(publication, "base_ref")?.as_bytes(),
    ])
}

fn destination_is_head(ctx: &mut Ctx) -> Res<Vec<u8>> {
    let record = ctx.check()?;
    refuse(
        binding_str(&record, "phase")? == "publish",
        "invalid destination decision phase",
    )?;
    let captured = capture_publication(&record)?.clone();
    let publication = ctx.publication(get_obj(&record, "binding")?, Some(&captured))?;
    refuse(
        ["ref", "remote", "url_sha256"]
            .iter()
            .all(|k| publication.get(*k) == captured.get(*k)),
        "publication destination binding changed",
    )?;
    let head = ctx.textgit(&["rev-parse", "HEAD"])?;
    let same = publication.get("destination_tip").and_then(Value::as_str) == Some(head.as_str());
    Ok(line(if same { b"true" } else { b"false" }))
}

fn verify_pushed_ref(ctx: &mut Ctx, args: &[OsString]) -> Res<Done> {
    let record = ctx.check()?;
    refuse(args.len() == 3, "invalid publication verification input")?;
    let helper = Path::new(&args[0]);
    let (branch, expected) = (arg(args, 1)?, arg(args, 2)?);
    refuse(
        binding_str(&record, "phase")? == "publish"
            && branch == binding_str(&record, "publication_branch")?
            && expected == ctx.textgit(&["rev-parse", "HEAD"])?,
        "publication verification binding differs",
    )?;
    refuse(
        helper
            .file_name()
            .is_some_and(|n| n == "verify_pushed_ref.sh")
            && helper.is_file(),
        "publication verification helper is unavailable",
    )?;
    let captured = capture_publication(&record)?.clone();
    let publication = ctx.publication(get_obj(&record, "binding")?, Some(&captured))?;
    let (env, alias) = ctx.publication_transport(&publication)?;
    // Run the unchanged verifier against the same privately resolved push
    // destination used for ancestry inspection. Its argv has only an alias.
    let result = super::repo::run_cmd(
        "bash",
        &[
            helper.as_os_str(),
            super::repo::os(&alias),
            super::repo::os(branch),
            super::repo::os(expected),
        ],
        &env,
    )?;
    // The alias is private transport preparation, not a user-facing remote
    // name. Keep the original verifier's origin/branch messages. The alias is
    // a fresh random token, replaced byte for byte; other bytes are kept.
    let stdout = replace_token(&result.stdout, alias.as_bytes(), b"origin");
    let stderr = replace_token(&result.stderr, alias.as_bytes(), b"origin");
    use std::io::Write;
    let _ = std::io::stderr().lock().write_all(&stderr);
    // 918c: `sys.exit(result.returncode)`. A verifier killed by signal N has
    // returncode -N, which exits with (-N) & 0xff.
    use std::os::unix::process::ExitStatusExt;
    let code = match (result.status.code(), result.status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => -signal,
        (None, None) => -1,
    };
    Ok(Done::Verifier(
        stdout,
        std::num::NonZeroI32::new(code.rem_euclid(256)),
    ))
}

/// `raw` with each occurrence of `token` replaced by `with`.
fn replace_token(raw: &[u8], token: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if !token.is_empty() && raw[i..].starts_with(token) {
            out.extend_from_slice(with);
            i += token.len();
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

fn paths(ctx: &mut Ctx) -> Res<Vec<u8>> {
    ctx.check()?;
    // History-only deleted identities have nothing to stage. Pending files
    // remain a subset of the fixed authority, never an expansion.
    let selected = ctx.pending()?;
    for path in &selected {
        ctx.identity(path)?;
    }
    let mut out = Vec::new();
    for path in &selected {
        out.extend_from_slice(path);
        out.push(0);
    }
    Ok(out)
}

fn before(ctx: &mut Ctx, args: &[&str]) -> Res<Vec<u8>> {
    let record = ctx.check()?;
    refuse(
        !args.is_empty() && matches!(args[0], "commit" | "push" | "pull" | "fetch" | "sync"),
        "missing explicit checkpoint operation",
    )?;
    let operation = args[0];
    let phase = binding_str(&record, "phase")?.to_owned();
    let publication_branch = binding_str(&record, "publication_branch")?.to_owned();
    let mut cleanup_sync = false;
    if operation == "push" {
        push_selection(&record, &args[1..])?;
        ctx.verify_publication_destination(&record)?;
    } else {
        cleanup_sync = operation == "sync"
            && phase == "cleanup"
            && args.len() == 3
            && args[1] == "workflow_checkpoint_sync_body";
        refuse(
            args.len() == 1 || cleanup_sync,
            "unexpected checkpoint operation arguments",
        )?;
        if cleanup_sync {
            let checked_ref = ctx.checked_ref()?;
            refuse(
                format!("refs/heads/{}", args[2]) == checked_ref
                    && checked_ref == format!("refs/heads/{publication_branch}"),
                "sync callback reference differs from publication",
            )?;
        }
    }
    let checked = ctx
        .checked_publication
        .clone()
        .ok_or_else(|| Fail::Refusal("missing checked publication".into()))?;
    let mut token = json!({
        "head": ctx.textgit(&["rev-parse", "HEAD"])?,
        "commits": ctx.commits()?,
        "upstream": get(&checked, "upstream")?,
        "invocation": get(get_obj(&record, "binding")?, "invocation")?,
    });
    if cleanup_sync {
        // Reuse check()'s fresh advertisement. Configuration-only URL
        // resolution honours Git's insteadOf/pushInsteadOf handling.
        let fetch_urls = lines(&ctx.git(&["remote", "get-url", "--all", "origin"])?);
        let fetch_hash = if fetch_urls.len() == 1 {
            Some(digest(&fetch_urls[0]))
        } else {
            None
        };
        let branch = binding_str(&record, "branch")?;
        let no_upstream = get_str(&checked, "upstream_ref")?.is_empty()
            && ctx
                .textgit_opt(&["config", "--get", &format!("branch.{branch}.remote")])?
                .is_empty()
            && ctx
                .textgit_opt(&["config", "--get", &format!("branch.{branch}.merge")])?
                .is_empty();
        let initial_absent = capture_publication(&record)?
            .get("destination_tip")
            .is_none_or(Value::is_null);
        let current_absent = checked.get("destination_tip").is_none_or(Value::is_null);
        let push_hash = get_str(&checked, "url_sha256")?.to_owned();
        let same_destination = fetch_hash.as_deref() == Some(push_hash.as_str());
        let mode = if initial_absent && current_absent && no_upstream && same_destination {
            "skip-absent"
        } else {
            "fetch"
        };
        token["cleanup_sync"] = json!({
            "operation": "sync",
            "callback": "workflow_checkpoint_sync_body",
            "ref": get(&checked, "ref")?,
            "fetch_url_sha256": fetch_hash,
            "push_url_sha256": push_hash,
            "initial_absent": initial_absent,
            "current_absent": current_absent,
            "no_upstream": no_upstream,
            "same_destination": same_destination,
            "mode": mode,
        });
        observe(
            ctx,
            json!({
                "boundary": "sync-fetch-policy", "mode": mode, "ref": get(&checked, "ref")?,
                "same_destination": same_destination, "current_ref_absent": current_absent,
            }),
        )?;
    }
    observe(
        ctx,
        json!({"boundary": "before-operation", "head": token["head"], "upstream": token["upstream"]}),
    )?;
    let path =
        record_directory()?.join(format!("operation-{}.json", uuid::Uuid::new_v4().simple()));
    let sha = retain(&path, &token)?;
    // The shell holds this compact descriptor across the operation. The
    // potentially large object inventory never passes through argv.
    let descriptor =
        json!({"path": path_text(&path)?, "sha256": sha, "identity": file_identity(&path)?});
    Ok(line(&serial(&descriptor)))
}

impl Ctx {
    fn checked_ref(&self) -> Res<String> {
        let checked = self
            .checked_publication
            .as_ref()
            .ok_or_else(|| Fail::Refusal("missing checked publication".into()))?;
        Ok(get_str(checked, "ref")?.to_owned())
    }
}

/// The private directory of the loaded authority record.
fn record_directory() -> Res<PathBuf> {
    let record = env_os("WF_CHECKPOINT_RECORD")
        .map(PathBuf::from)
        .ok_or_else(|| Fail::Refusal("missing WF_CHECKPOINT_RECORD".into()))?;
    record
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Fail::Refusal("authority record has no private directory".into()))
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn sync_fetch_mode(ctx: &mut Ctx, args: &[&str]) -> Res<Vec<u8>> {
    refuse(args.len() == 1, "invalid sync operation descriptor")?;
    let descriptor = object(
        parse(args[0].as_bytes())?,
        "invalid sync operation descriptor",
    )?;
    let (record, actual) = ctx.load_authority()?;
    refuse(
        binding_str(&record, "phase")? == "cleanup",
        "sync mode is only for cleanup",
    )?;
    let path = PathBuf::from(get_str(&descriptor, "path")?);
    let directory = record_directory()?;
    let name = match path.file_name().map(|n| n.to_str()) {
        Some(Some(name)) => name,
        _ => "",
    };
    refuse(
        path.parent() == Some(directory.as_path())
            && name.is_ascii()
            && name.len() == "operation-.json".len() + 32
            && name.starts_with("operation-")
            && name.ends_with(".json")
            && is_hex(&name[10..42], 32),
        "sync operation is not in its original private directory",
    )?;
    refuse(
        Some(file_identity(&path)?.as_str()) == descriptor.get("identity").and_then(Value::as_str),
        "sync operation identity changed",
    )?;
    let raw = std::fs::read(&path).map_err(io_refusal("sync operation"))?;
    refuse(
        Some(digest(&raw).as_str()) == descriptor.get("sha256").and_then(Value::as_str),
        "sync operation hash changed",
    )?;
    let token = object(parse(&raw)?, "malformed sync operation")?;
    let head = token.get("head").and_then(Value::as_str);
    refuse(
        token.get("invocation") == get_obj(&record, "binding")?.get("invocation")
            && head.is_some()
            && head == actual.get("initial_head").and_then(Value::as_str)
            && head.map(str::to_owned) == env_text("WF_CHECKPOINT_HEAD")?,
        "stale sync operation descriptor",
    )?;
    const KEYS: [&str; 10] = [
        "operation",
        "callback",
        "ref",
        "fetch_url_sha256",
        "push_url_sha256",
        "initial_absent",
        "current_absent",
        "no_upstream",
        "same_destination",
        "mode",
    ];
    let facts = match token.get("cleanup_sync") {
        Some(Value::Object(f))
            if f.len() == KEYS.len() && KEYS.iter().all(|k| f.contains_key(*k)) =>
        {
            f.clone()
        }
        _ => return Err(Fail::Refusal("malformed sync fetch policy".into())),
    };
    let publication_branch = binding_str(&record, "publication_branch")?;
    refuse(
        facts["operation"] == "sync"
            && facts["callback"] == "workflow_checkpoint_sync_body"
            && facts["ref"] == json!(format!("refs/heads/{publication_branch}")),
        "wrong sync operation, callback or reference",
    )?;
    let flags = [
        "initial_absent",
        "current_absent",
        "no_upstream",
        "same_destination",
    ];
    refuse(
        flags.iter().all(|k| facts[*k].is_boolean()),
        "malformed sync policy flags",
    )?;
    let fetch_hash = &facts["fetch_url_sha256"];
    refuse(
        facts.get("push_url_sha256") == capture_publication(&record)?.get("url_sha256")
            && (fetch_hash.is_null() || fetch_hash.as_str().is_some_and(|h| is_hex(h, 64))),
        "malformed sync destination binding",
    )?;
    let expected = if flags.iter().all(|k| facts[*k] == Value::Bool(true)) {
        "skip-absent"
    } else {
        "fetch"
    };
    refuse(facts["mode"] == expected, "inconsistent sync fetch policy")?;
    if expected == "skip-absent" {
        let fetch_urls = lines(&ctx.git(&["remote", "get-url", "--all", "origin"])?);
        let push_urls = lines(&ctx.git(&["remote", "get-url", "--push", "--all", "origin"])?);
        refuse(
            fetch_urls.len() == 1
                && push_urls.len() == 1
                && fetch_hash.as_str() == Some(digest(&fetch_urls[0]).as_str())
                && digest(&fetch_urls[0]) == digest(&push_urls[0])
                && facts["push_url_sha256"].as_str() == Some(digest(&push_urls[0]).as_str()),
            "sync destination changed",
        )?;
        let branch = binding_str(&record, "branch")?;
        refuse(
            ctx.textgit_opt(&["rev-parse", "--symbolic-full-name", "@{u}"])?
                .is_empty()
                && ctx
                    .textgit_opt(&["config", "--get", &format!("branch.{branch}.remote")])?
                    .is_empty()
                && ctx
                    .textgit_opt(&["config", "--get", &format!("branch.{branch}.merge")])?
                    .is_empty(),
            "sync upstream changed",
        )?;
    }
    Ok(line(expected.as_bytes()))
}

fn after(ctx: &mut Ctx, args: &[OsString]) -> Res<Vec<u8>> {
    let descriptor = object(
        parse(arg(args, 0)?.as_bytes())?,
        "malformed operation descriptor",
    )?;
    let token_path = PathBuf::from(get_str(&descriptor, "path")?);
    refuse(
        Some(file_identity(&token_path)?.as_str())
            == descriptor.get("identity").and_then(Value::as_str),
        "operation observation identity changed",
    )?;
    let token_bytes = std::fs::read(&token_path).map_err(io_refusal("operation observation"))?;
    refuse(
        Some(digest(&token_bytes).as_str()) == descriptor.get("sha256").and_then(Value::as_str),
        "operation observation hash changed",
    )?;
    let token = object(parse(&token_bytes)?, "malformed operation observation")?;
    // 918c :946 reads `sys.argv[3]` and `int(sys.argv[4])` only here.
    let op = arg(args, 1)?.to_owned();
    let rc = py_int(arg(args, 2)?)?;
    let (record, actual) = ctx.load_authority()?;
    let token_head = get_str(&token, "head")?.to_owned();
    observe(
        ctx,
        json!({"boundary": "operation-returned", "operation": op, "command_exit": rc, "starting_head": token_head}),
    )?;
    refuse(
        token.get("invocation") == get_obj(&record, "binding")?.get("invocation"),
        "cross-invocation operation observation",
    )?;
    let phase = binding_str(&record, "phase")?.to_owned();
    let head = get_str(&actual, "initial_head")?.to_owned();
    let skip_mode = token
        .get("cleanup_sync")
        .and_then(|s| s.get("mode"))
        .and_then(Value::as_str);
    if op == "sync" && skip_mode == Some("skip-absent") {
        refuse(
            phase == "cleanup" && rc == 0 && head == token_head,
            "skipped sync changed state",
        )?;
        observe(
            ctx,
            json!({
                "boundary": "operation-skipped", "operation": "sync-fetch",
                "reason": "first-publication ref absent on the same destination",
            }),
        )?;
    }
    let captured = capture_publication(&record)?.clone();
    let publication = ctx.publication(get_obj(&record, "binding")?, Some(&captured))?;
    let upstream = get_str(&publication, "upstream")?.to_owned();
    let mut tracking = (
        env_required("WF_CHECKPOINT_UPSTREAM_REF")?,
        env_required("WF_CHECKPOINT_BASE_REF")?,
    );
    let new_upstream_ref = get_str(&publication, "upstream_ref")?.to_owned();
    if matches!(op.as_str(), "sync" | "push")
        && tracking.0.is_empty()
        && !new_upstream_ref.is_empty()
    {
        let target = get_str(&captured, "ref")?;
        let expected_ref = format!(
            "refs/remotes/origin/{}",
            target.strip_prefix("refs/heads/").unwrap_or(target)
        );
        refuse(
            new_upstream_ref == expected_ref,
            "new tracking reference differs from intended publication",
        )?;
        tracking = (expected_ref.clone(), expected_ref);
    }
    let token_upstream = get_str(&token, "upstream")?.to_owned();
    if op == "commit" {
        refuse(
            upstream == token_upstream,
            "commit changed the upstream identity",
        )?;
    }
    if op == "push" {
        refuse(
            upstream == token_upstream || upstream == head,
            "push changed the upstream to an unrelated identity",
        )?;
    }
    let before: BTreeSet<String> = get_arr(&token, "commits")?
        .iter()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    let created: BTreeSet<String> = ctx.commits()?.difference(&before).cloned().collect();
    if matches!(op.as_str(), "pull" | "sync") {
        refuse(
            matches!(phase.as_str(), "review" | "cleanup"),
            "publish may not rebase or synchronise HEAD",
        )?;
    }
    if op == "pull" && rc == 0 {
        let fetched = ctx.textgit(&["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?;
        refuse(
            upstream == fetched,
            "rebase upstream differs from fetched identity",
        )?;
        refuse(
            ctx.textgit(&["merge-base", &head, &upstream])? == upstream,
            "rebase lacks fetched upstream ancestry",
        )?;
        let base = ctx.textgit(&["merge-base", &token_head, &upstream])?;
        ctx.verify_replay(&base, &token_head, &upstream, &head)?;
    }
    if op == "sync" && head != token_head {
        refuse(
            head == upstream && ctx.textgit(&["merge-base", &token_head, &upstream])? == token_head,
            "head sync is not a fast-forward to the fetched publication reference",
        )?;
    }
    if op == "commit" && rc == 0 {
        let row = ctx.commit_info(&head)?;
        refuse(
            row["parents"] == json!([token_head]) && created.contains(&head),
            "unexpected commit transition",
        )?;
    } else if !matches!(op.as_str(), "pull" | "sync") || rc != 0 {
        refuse(head == token_head, "unexpected operation HEAD transition")?;
    }
    ctx.scope_check(
        &record,
        Some(&upstream),
        Some(tracking.clone()),
        Some(publication),
    )?;
    observe(
        ctx,
        json!({
            "boundary": "operation-checked", "operation": op, "command_exit": rc, "head": head,
            "upstream": upstream, "created_commits": created,
        }),
    )?;
    nul_record([
        head.as_bytes(),
        upstream.as_bytes(),
        tracking.0.as_bytes(),
        tracking.1.as_bytes(),
    ])
}

/// SHA-256 of stdin, for the strict stager's identity check.
/// 918c :1010 `hashlib.sha256(sys.stdin.buffer.read()).hexdigest()`. The
/// snippet had no handler, so a read error is a crash (status 1).
fn sha256_stdin() -> Res<Vec<u8>> {
    let mut raw = Vec::new();
    std::io::stdin()
        .read_to_end(&mut raw)
        .map_err(|e| Fail::Crash(format!("OSError: {e}")))?;
    Ok(line(digest(&raw).as_bytes()))
}

/// workflow-publish.yaml:137,179 and workflow-pr-review.yaml:204,246:
/// `s=json.loads(os.environ["RECIPE_VAR_worktree_setup"]);
/// print(s["worktree_path"])`. Every failure was uncaught (status 1), and
/// `print` writes `str(value)` whatever its JSON type.
fn worktree_path() -> Res<Vec<u8>> {
    use super::json::{PyValue, py_loads, py_print, surrogateescape};
    let crash = |what: &str| Fail::Crash(what.to_owned());
    let raw = env_os("RECIPE_VAR_worktree_setup")
        .ok_or_else(|| crash("KeyError: 'RECIPE_VAR_worktree_setup'"))?;
    let setup = py_loads(&surrogateescape(raw.as_bytes()))
        .ok_or_else(|| crash("json.decoder.JSONDecodeError"))?;
    let value = match &setup {
        PyValue::Dict(_) => setup
            .get("worktree_path")
            .ok_or_else(|| crash("KeyError: 'worktree_path'"))?,
        _ => return Err(crash("TypeError: worktree setup is not a dict")),
    };
    let printed = py_print(value).ok_or_else(|| crash("UnicodeEncodeError"))?;
    Ok(line(&printed))
}

/// 918c :1155 and :1184 ADMISSION_RESULT_PY: `branch_name` from the step-04
/// JSON, any JSON type, or `''` when the JSON is invalid or not an object.
fn admission_refused_result() -> Vec<u8> {
    use super::json::{PyValue, py_dumps, py_loads, surrogateescape};
    let raw = env_os("RECIPE_VAR_worktree_setup").unwrap_or_else(|| OsString::from("{}"));
    let empty = PyValue::Str(Vec::new());
    let setup = py_loads(&surrogateescape(raw.as_bytes()));
    let branch = match &setup {
        Some(dict @ PyValue::Dict(_)) => dict.get("branch_name").unwrap_or(&empty),
        _ => &empty,
    };
    line(
        format!(
            "{{\"pushed\": \"false\", \"sha\": \"\", \"branch\": {}, \"reason\": \"scope-admission-refused\"}}",
            py_dumps(branch)
        )
        .as_bytes(),
    )
}

/// workflow_cleanup_checkpoint.sh:17 CLEANUP_SKIP_JSON:
/// `json.dumps(dict(skipped=True, admitted=False, reason=sys.argv[1]))`.
/// The reason is argv bytes, written as Python writes a
/// `surrogateescape`-decoded string.
fn cleanup_skip_result(reason: &OsString) -> Vec<u8> {
    let reason = super::json::py_dumps_os(reason.as_bytes());
    line(format!("{{\"skipped\": true, \"admitted\": false, \"reason\": {reason}}}").as_bytes())
}

/// workflow_pr_head_sync.sh:83-92 ORIGIN_REF_PY, with its arguments: the
/// advertisement file, the target ref and the transport status. The
/// snippet had no handler, so an unreadable file or a non-integer status is
/// a crash (status 1). A malformed advertisement is the table's 78.
fn advertised_ref(args: &[OsString]) -> Res<Lookup> {
    let file = super::indexed(args, 0)?;
    let raw = std::fs::read(file).map_err(|e| Fail::Crash(format!("OSError: {e}")))?;
    let target = super::indexed(args, 1)?.as_bytes().to_vec();
    let rc = super::indexed(args, 2)?
        .to_str()
        .ok_or_else(|| Fail::Crash("ValueError: non-UTF-8 status".into()))
        .and_then(|text| py_int(text).map_err(|_| Fail::Crash(format!("ValueError: {text}"))))?;
    let advertisement = AdvertMap::parse(
        &raw,
        rc.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
    )
    .map_err(Fail::Advert)?;
    Ok(match advertisement.tip(&target) {
        Some(_) => Lookup::Present,
        None => Lookup::Absent,
    })
}

// ---------------------------------------------------- publication decisions

fn valid_ref_input(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-')
}

impl Ctx {
    pub(crate) fn publication(
        &mut self,
        binding: &Map<String, Value>,
        captured: Option<&Map<String, Value>>,
    ) -> Res<Map<String, Value>> {
        let branch = get_str(binding, "branch")?.to_owned();
        let publication_branch = get_str(binding, "publication_branch")?.to_owned();
        let upstream_ref = self.textgit_opt(&["rev-parse", "--symbolic-full-name", "@{u}"])?;
        let setup = worktree_setup()?;
        let owner_ref = format!("refs/remotes/origin/{publication_branch}");
        let known_owner =
            self.textgit_opt(&["rev-parse", "--verify", &format!("{owner_ref}^{{commit}}")])?;
        let base = if !upstream_ref.is_empty() {
            Value::String(upstream_ref.clone())
        } else if !known_owner.is_empty() {
            Value::String(owner_ref.clone())
        } else {
            setup
                .get("base_ref")
                .cloned()
                .unwrap_or(Value::String(String::new()))
        };
        let base = match base.as_str() {
            Some(b) if valid_ref_input(b) => b.to_owned(),
            _ => return Err(Fail::Refusal("missing publication ancestry".into())),
        };
        let upstream = self.textgit(&["rev-parse", "--verify", &format!("{base}^{{commit}}")])?;
        let remote = self.textgit_opt(&["config", "--get", &format!("branch.{branch}.remote")])?;
        let merge = self.textgit_opt(&["config", "--get", &format!("branch.{branch}.merge")])?;
        refuse(
            matches!(remote.as_str(), "" | "origin"),
            "unsupported publication remote",
        )?;
        let target = format!("refs/heads/{publication_branch}");
        if remote == "origin" {
            refuse(
                merge == target && (upstream_ref.is_empty() || upstream_ref == owner_ref),
                "upstream differs from the owner publication branch",
            )?;
        } else {
            refuse(
                merge.is_empty() && upstream_ref.is_empty(),
                "incomplete publication tracking configuration",
            )?;
        }
        let default = self.textgit_opt(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])?;
        let default = default
            .strip_prefix("origin/")
            .unwrap_or(&default)
            .to_owned();
        refuse(
            !["main", "master", "develop", "trunk", default.as_str()]
                .contains(&publication_branch.as_str()),
            "protected publication reference",
        )?;
        refuse(
            matches!(
                self.textgit_opt(&["config", "--get", "push.default"])?
                    .as_str(),
                "" | "simple" | "current" | "upstream"
            ),
            "unsupported implicit push selection",
        )?;
        for key in [
            "remote.pushDefault".to_owned(),
            format!("branch.{branch}.pushRemote"),
            "remote.origin.push".to_owned(),
        ] {
            refuse(
                self.textgit_opt(&["config", "--get-all", &key])?.is_empty(),
                &format!("unsupported push routing: {key}"),
            )?;
        }
        for key in ["remote.origin.mirror", "push.followTags"] {
            refuse(
                self.textgit_opt(&["config", "--bool", key])? != "true",
                &format!("unsupported publication expansion: {key}"),
            )?;
        }
        refuse(
            matches!(
                self.textgit_opt(&["config", "--get", "push.recurseSubmodules"])?
                    .as_str(),
                "" | "no" | "false" | "check"
            ),
            "unsupported recursive publication expansion",
        )?;
        let url = strip_newlines(self.git(&["remote", "get-url", "--push", "--all", "origin"])?);
        refuse(
            !url.is_empty() && !url.contains(&b'\n') && !url.contains(&b'\r'),
            "multiple or ambiguous publication destinations",
        )?;
        // Bind the exact destination bytes without including userinfo in
        // capture, authority, observations or any model input.
        let mut publication = object(
            json!({
            "ref": target, "remote": "origin", "url_sha256": digest(&url),
            "upstream_ref": upstream_ref, "base_ref": base, "upstream": upstream,
            }),
            "malformed publication",
        )?;
        let tip = self.destination_tip(&publication)?;
        let prepared_base = setup
            .get("base_ref")
            .cloned()
            .unwrap_or(Value::String(base.clone()));
        let prepared_base = match prepared_base.as_str() {
            Some(b) if valid_ref_input(b) => b.to_owned(),
            _ => return Err(Fail::Refusal("missing prepared publication base".into())),
        };
        // Resolve this input during capture even if the destination exists. A
        // later fetch may move the tracking ref but cannot renew this authority.
        let prepared_oid = match captured {
            None => {
                let oid = self.textgit_opt(&[
                    "rev-parse",
                    "--verify",
                    &format!("{prepared_base}^{{commit}}"),
                ])?;
                if oid.is_empty() {
                    Value::Null
                } else {
                    Value::String(oid)
                }
            }
            Some(c) => {
                refuse(
                    c.get("prepared_base_ref").and_then(Value::as_str)
                        == Some(prepared_base.as_str()),
                    "prepared publication base reference changed",
                )?;
                c.get("prepared_base_oid").cloned().unwrap_or(Value::Null)
            }
        };
        // Fetch tracking remains the input to the existing rebase checks. The
        // absent-destination fallback uses only the immutably captured commit.
        let publication_base = match (&tip, &prepared_oid) {
            (Some(t), _) if !t.is_empty() => Value::String(t.clone()),
            (_, Value::String(p)) if !p.is_empty() => Value::String(p.clone()),
            _ => {
                return Err(Fail::Refusal(
                    "captured prepared publication base is unavailable".into(),
                ));
            }
        };
        publication.insert(
            "destination_tip".into(),
            tip.clone().map_or(Value::Null, Value::String),
        );
        publication.insert("publication_base".into(), publication_base);
        publication.insert("prepared_base_ref".into(), Value::String(prepared_base));
        publication.insert("prepared_base_oid".into(), prepared_oid.clone());
        let history = match captured {
            None => {
                let seeds = [
                    Some(get_str(binding, "initial_head")?.to_owned()),
                    Some(upstream),
                    prepared_oid.as_str().map(str::to_owned),
                ];
                self.prepare_history(&publication, &seeds)?
            }
            Some(c) => {
                let history = get(c, "history")?.clone();
                let history_obj = history
                    .as_object()
                    .ok_or_else(|| Fail::Refusal("malformed private history descriptor".into()))?;
                refuse(
                    history_obj.get("captured_tip") == c.get("destination_tip"),
                    "private captured destination identity changed",
                )?;
                publication.insert("history".into(), history.clone());
                self.validate_history(history_obj, &publication)?;
                history
            }
        };
        publication.insert("history".into(), history);
        Ok(publication)
    }
}

impl Ctx {
    pub(crate) fn verify_publication_destination(
        &mut self,
        record: &Map<String, Value>,
    ) -> Res<()> {
        let publication = self
            .checked_publication
            .clone()
            .ok_or_else(|| Fail::Refusal("missing checked publication".into()))?;
        let capture = get_obj(record, "capture")?;
        let original = get_obj(capture, "publication")?;
        refuse(
            ["ref", "remote", "url_sha256"]
                .iter()
                .all(|k| publication.get(*k) == original.get(*k)),
            "publication destination binding changed",
        )?;
        let allowed = approved(record)?;
        let captured: BTreeSet<&str> = get_arr(capture, "outgoing_commits")?
            .iter()
            .filter_map(|item| item.get("commit").and_then(Value::as_str))
            .collect();
        let initial: BTreeSet<&str> = get_arr(capture, "initial_commits")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for row in self.outgoing(&publication, "HEAD")? {
            refuse(
                row_paths(&row)?.is_subset(&allowed),
                "actual publication destination exposes unreviewed outgoing history",
            )?;
            let commit = row
                .get("commit")
                .and_then(Value::as_str)
                .ok_or_else(|| Fail::Refusal("malformed outgoing commit".into()))?;
            refuse(
                captured.contains(commit) || !initial.contains(commit),
                "actual publication destination exposes an uncaptured existing commit",
            )?;
        }
        Ok(())
    }
}

pub(crate) fn approved(record: &Map<String, Value>) -> Res<BTreeSet<Vec<u8>>> {
    get_arr(record, "approved_paths")?
        .iter()
        .map(unb64)
        .collect()
}

pub(crate) fn row_paths(row: &Value) -> Res<BTreeSet<Vec<u8>>> {
    row.get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| Fail::Refusal("malformed outgoing commit".into()))?
        .iter()
        .map(unb64)
        .collect()
}

/// A task push may name only the bound remote and the task reference.
pub(crate) fn push_selection(record: &Map<String, Value>, argv: &[&str]) -> Res<()> {
    refuse(
        argv.len() >= 2 && argv[0] == "command" && argv[1] == "git",
        "unsupported push executable",
    )?;
    let mut args: Vec<&str> = argv[2..].to_vec();
    if args.first() == Some(&"--no-pager") {
        args.remove(0);
    }
    refuse(
        args.first() == Some(&"push"),
        "missing explicit push operation",
    )?;
    args.remove(0);
    while args.first().is_some_and(|a| a.starts_with('-')) {
        let option = args.remove(0);
        refuse(
            matches!(
                option,
                "-u" | "--set-upstream" | "--quiet" | "--verbose" | "--"
            ),
            "unsupported task push option",
        )?;
        if option == "--" {
            break;
        }
    }
    refuse(
        args.len() <= 2 && (args.is_empty() || args[0] == "origin"),
        "push must use only the bound task remote and ref",
    )?;
    if args.len() == 2 {
        let refspec = args[1];
        refuse(
            refspec.matches(':').count() <= 1,
            "unsupported task push refspec",
        )?;
        let (source, target) = match refspec.split_once(':') {
            Some((s, t)) => (s, t.to_owned()),
            None => (
                refspec,
                refspec
                    .strip_prefix("refs/heads/")
                    .unwrap_or(refspec)
                    .to_owned(),
            ),
        };
        let binding = get_obj(record, "binding")?;
        let local = get_str(binding, "branch")?;
        let expected = get_str(binding, "publication_branch")?;
        refuse(
            source == "HEAD" || source == local || source == format!("refs/heads/{local}"),
            "push source differs from the task branch",
        )?;
        refuse(
            target == expected || target == format!("refs/heads/{expected}"),
            "push destination differs from the task reference",
        )?;
    }
    Ok(())
}

/// Python's `int(text)` for a base-10 string: surrounding whitespace, one
/// sign, and single underscores between digits. Anything else is the
/// `ValueError` 918c caught as a refusal.
pub(crate) fn py_int(text: &str) -> Res<i64> {
    let invalid = || Fail::Refusal(format!("invalid literal for int() with base 10: '{text}'"));
    let body = text.trim_matches(|c: char| c.is_whitespace());
    let (negative, digits) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    let bytes = digits.as_bytes();
    let well_formed = !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_digit)
        && bytes.last().is_some_and(u8::is_ascii_digit)
        && bytes.iter().all(|b| b.is_ascii_digit() || *b == b'_')
        && !digits.contains("__");
    if !well_formed {
        return Err(invalid());
    }
    let clean: String = digits.chars().filter(char::is_ascii_digit).collect();
    let magnitude: i64 = clean.parse().map_err(|_| invalid())?;
    Ok(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn record() -> Map<String, Value> {
        json!({"binding": {"branch": "feat/x--wt-2", "publication_branch": "feat/x"}})
            .as_object()
            .cloned()
            .unwrap()
    }

    fn argv<'a>(items: &[&'a str]) -> Vec<&'a str> {
        items.to_vec()
    }

    #[test]
    fn push_selection_accepts_only_the_task_reference() {
        let r = record();
        assert!(push_selection(&r, &argv(&["command", "git", "push"])).is_ok());
        assert!(
            push_selection(
                &r,
                &argv(&["command", "git", "push", "-u", "origin", "HEAD:feat/x"])
            )
            .is_ok()
        );
        assert!(
            push_selection(
                &r,
                &argv(&[
                    "command",
                    "git",
                    "--no-pager",
                    "push",
                    "origin",
                    "feat/x--wt-2:refs/heads/feat/x"
                ])
            )
            .is_ok()
        );
        assert!(push_selection(&r, &argv(&["git", "push"])).is_err());
        assert!(push_selection(&r, &argv(&["command", "git", "push", "--force"])).is_err());
        assert!(
            push_selection(
                &r,
                &argv(&["command", "git", "push", "upstream", "HEAD:feat/x"])
            )
            .is_err()
        );
        assert!(
            push_selection(
                &r,
                &argv(&["command", "git", "push", "origin", "HEAD:main"])
            )
            .is_err()
        );
        assert!(
            push_selection(
                &r,
                &argv(&["command", "git", "push", "origin", "other:feat/x"])
            )
            .is_err()
        );
        assert!(push_selection(&r, &argv(&["command", "git", "push", "origin", "HEAD"])).is_err());
        assert!(push_selection(&r, &argv(&["command", "git", "push", "origin", "a:b:c"])).is_err());
    }
}
