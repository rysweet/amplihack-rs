# One set of predicates for selection and exclusion evidence. Each check keeps
# its expected and observed data, so diagnostics cannot drift from filtering.
def check($name; $expected; $observed; $passed):
  {check:$name, expected:$expected, observed:$observed, passed:$passed};
def candidate_checks:
  def text: ((.title // "") + " " + (.body // ""));
  def has_token($token): text | contains($token);
  def owner_name: (.headRepositoryOwner.login // .headRepositoryOwner.name // .headRepositoryOwner // "");
  def repo_name: (.headRepository.name // ((.headRepository.nameWithOwner // "") | split("/") | .[-1]) // "");
  {primary:[
    check("head branch"; $headRefName; .headRefName; (.headRefName // "") == $headRefName),
    check("base branch"; $baseRefName; .baseRefName; (.baseRefName // "") == $baseRefName),
    check("cross repository"; false; .isCrossRepository; (.isCrossRepository // false) == false),
    check("repository"; $repo; (owner_name + "/" + repo_name); (owner_name + "/" + repo_name) == $repo),
    (if $prNumber != "" then check("PR number"; $prNumber; .number; ((.number // "") | tostring) == $prNumber) else empty end),
    (if $prUrl != "" then check("PR URL"; $prUrl; .url; (.url // "") == $prUrl) else empty end)
  ], secondary:[
    (if $headRefOid != "" then check("head SHA"; $headRefOid; .headRefOid; (.headRefOid // "") == $headRefOid) else empty end),
    (if $expected_pr_title_prefix != "" then check("title prefix"; $expected_pr_title_prefix; .title; ((.title // "") | startswith($expected_pr_title_prefix))) else empty end),
    (if $created_after != "" then check("created-after"; $created_after; .createdAt; (.createdAt // "") >= $created_after) else empty end),
    (if $issueId != "" then check("issue token"; $issueId; {number:.number,text:text}; (((.number // "") | tostring) == $issueId) or has_token("#" + $issueId) or has_token("issue-" + $issueId)) else empty end),
    (if $workItemId != "" then check("work-item"; $workItemId; {number:.number,text:text}; (((.number // "") | tostring) == $workItemId) or has_token("#" + $workItemId) or has_token("AB#" + $workItemId) or has_token("work-item-" + $workItemId)) else empty end),
    (if $recipeRunId != "" then check("recipe run"; $recipeRunId; text; has_token($recipeRunId)) else empty end),
    (if $treeId != "" then check("tree"; $treeId; text; has_token($treeId)) else empty end),
    (if $workstreamId != "" then check("workstream"; $workstreamId; text; has_token($workstreamId)) else empty end)
  ]};
