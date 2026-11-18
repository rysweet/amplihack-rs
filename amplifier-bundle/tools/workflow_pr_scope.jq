# One set of predicates for selection and exclusion evidence. Each check keeps
# its expected and observed data, so diagnostics cannot drift from filtering.
def check($name; $expected; $observed; $passed):
  {check:$name, expected:$expected, observed:$observed, passed:$passed};
# Missing and null observations retain the historical predicate defaults.
# Wrong types are malformed even if a different primary-key check would fail.
# Never retain a malformed value in evidence; field names and types suffice.
def candidate_validation_exclusions:
  def invalid($field; $expected; $value):
    check("malformed candidate: " + $field; $expected; ($value | type); false);
  def optional_strings($fields):
    . as $object | $fields[] as $field |
    $object[$field] as $value |
    if $value == null or ($value | type) == "string" then empty
    else invalid($field; "string or null/missing"; $value) end;
  if type != "object" then [invalid("record"; "object"; .)]
  else [
    optional_strings(["title","body","state","createdAt","mergedAt","url",
                      "headRefName","baseRefName","headRefOid"]),
    (if .number == null or
        ((.number | type) == "number" and .number > 0 and .number == (.number | floor))
     then empty else invalid("number"; "positive integer or null/missing"; .number) end),
    (. as $pr | ["isCrossRepository","isDraft"][] as $field |
     if $pr[$field] == null or ($pr[$field] | type) == "boolean" then empty
     else invalid($field; "boolean or null/missing"; $pr[$field]) end),
    (if .headRepository == null then empty
     elif (.headRepository | type) == "object" then
       .headRepository | optional_strings(["name","nameWithOwner"])
     else invalid("headRepository"; "object or null/missing"; .headRepository) end),
    (if .headRepositoryOwner == null or (.headRepositoryOwner | type) == "string" then empty
     elif (.headRepositoryOwner | type) == "object" then
       .headRepositoryOwner | optional_strings(["login","name"])
     else invalid("headRepositoryOwner"; "object, string or null/missing"; .headRepositoryOwner) end)
  ] end;
# Retain only type-valid identity observations in protected diagnostics.
def candidate_identifiers:
  {number:(if type == "object" and (.number | type) == "number"
              and .number > 0 and .number == (.number | floor)
           then .number else null end),
   url:(if type == "object" and (.url | type) == "string" then .url else null end)};
# GraphQL supplies an owner object; REST adapters may supply its name directly.
# An absent or incomplete owner is missing identity evidence, never an object
# to concatenate into the repository name. Discovery uses these same helpers.
def owner_name:
  .headRepositoryOwner |
  (if type == "object" then (.login // .name // "")
   elif type == "string" then . else "" end) |
  if type == "string" then . else "" end;
def repo_name:
  (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // "");
def candidate_checks:
  def string_or_null: . == null or type == "string";
  def strings($fields): . as $pr | all($fields[]; $pr[.] | string_or_null);
  def number_or_null: . == null or (type == "number" and . > 0 and . == floor);
  def bool_or_null: . == null or type == "boolean";
  def valid_repository:
    (.headRepository == null or
      ((.headRepository | type) == "object" and (.headRepository | strings(["name","nameWithOwner"])))) and
    (.headRepositoryOwner == null or (.headRepositoryOwner | type) == "string" or
      ((.headRepositoryOwner | type) == "object" and (.headRepositoryOwner | strings(["login","name"]))));
  def text: ((.title // "") + " " + (.body // ""));
  def has_token($token): text | contains($token);
  # Malformed fields exclude the candidate, while independent type-safe checks
  # still explain head/base/identity mismatches. Never retain malformed values.
  candidate_validation_exclusions as $invalid |
  if type != "object" then {primary:$invalid,secondary:[]}
  else {primary:($invalid + [
    (select(strings(["headRefName"])) | check("head branch"; $headRefName; .headRefName; (.headRefName // "") == $headRefName)),
    (select(strings(["baseRefName"])) | check("base branch"; $baseRefName; .baseRefName; (.baseRefName // "") == $baseRefName)),
    (select(.isCrossRepository | bool_or_null) | check("cross repository"; false; .isCrossRepository; (.isCrossRepository // false) == false)),
    (select(valid_repository) | check("repository"; $repo; (owner_name + "/" + repo_name); (owner_name + "/" + repo_name) == $repo)),
    (if $prNumber != "" and (.number | number_or_null) then check("PR number"; $prNumber; .number; ((.number // "") | tostring) == $prNumber) else empty end),
    (if $prUrl != "" and strings(["url"]) then check("PR URL"; $prUrl; .url; (.url // "") == $prUrl) else empty end)
  ]), secondary:[
    (if $headRefOid != "" and strings(["headRefOid"]) then check("head SHA"; $headRefOid; .headRefOid; (.headRefOid // "") == $headRefOid) else empty end),
    (if $expected_pr_title_prefix != "" and strings(["title"]) then check("title prefix"; $expected_pr_title_prefix; .title; ((.title // "") | startswith($expected_pr_title_prefix))) else empty end),
    (if $created_after != "" and strings(["createdAt"]) then check("created-after"; $created_after; .createdAt; (.createdAt // "") >= $created_after) else empty end),
    (if $issueId != "" and strings(["title","body"]) and (.number | number_or_null) then check("issue token"; $issueId; {number:.number,text:text}; (((.number // "") | tostring) == $issueId) or has_token("#" + $issueId) or has_token("issue-" + $issueId)) else empty end),
    (if $workItemId != "" and strings(["title","body"]) and (.number | number_or_null) then check("work-item"; $workItemId; {number:.number,text:text}; (((.number // "") | tostring) == $workItemId) or has_token("#" + $workItemId) or has_token("AB#" + $workItemId) or has_token("work-item-" + $workItemId)) else empty end),
    (if $recipeRunId != "" and strings(["title","body"]) then check("recipe run"; $recipeRunId; text; has_token($recipeRunId)) else empty end),
    (if $treeId != "" and strings(["title","body"]) then check("tree"; $treeId; text; has_token($treeId)) else empty end),
    (if $workstreamId != "" and strings(["title","body"]) then check("workstream"; $workstreamId; text; has_token($workstreamId)) else empty end)
  ]} end;
