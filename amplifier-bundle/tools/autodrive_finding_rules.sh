#!/usr/bin/env bash
# autodrive_finding_rules.sh - the one definition of which crusty findings may
# leave a review as "minor", and of how agent text is made safe for a GitHub
# issue body. Sourced, never run; sourcing only defines variables and
# functions, so it has no side effects.
#
# Used by:
#   autodrive_light_round.sh  the auto-drive-light minor-only rule (#1802)
#   autodrive_followups.sh    the crusty-loop minor-only end and its
#                             follow-up issues (#1979)
#
# KEYWORDS        matched anywhere in the lower-cased finding id
# TEXT_KEYWORDS   matched as whole words in the lower-cased summary and
#                 evidence, so that "second" or "author" do not match
# ZW              a zero-width space, used to break mentions and links
# cell <text>     one Markdown table cell or body line: needs
#                 sanitize_cli_output (workflow_issue_tracking.sh) loaded
# AUTODRIVE_FINDING_JQ
#                 jq definitions: `clean` (any value to a one-line string of
#                 at most 200 characters) and `minor_finding($kw; $tkw)`,
#                 true only for an object whose severity is exactly "minor",
#                 whose id matches \A[A-Za-z0-9._-]{1,64}\z, and whose id,
#                 summary and evidence match no security keyword. Anything
#                 else, an unknown or missing severity included, is not minor.
#
# Contract: docs/reference/auto-drive-light.md#cell-sanitising and
# docs/reference/auto-drive-to-merge.md (minor findings, #1979).

KEYWORDS='sec|auth|secret|token|credential|inject|xss|csrf|cve|vuln|unsafe|privilege|perm'
TEXT_KEYWORDS='\b(secur[a-z]*|insecure|authenticat[a-z]*|authori[sz][a-z]*|authn|authz|secrets?|tokens?|credentials?|inject[a-z]*|xss|csrf|cve|vuln[a-z]*|unsafe|privileges?|permissions?|sanitis[a-z]*|sanitiz[a-z]*|escap(e|es|ed|ing)|traversal|ssrf|rce|password[a-z]*)\b'
ZW=$'\xe2\x80\x8b'

AUTODRIVE_FINDING_JQ='
def clean: (if type == "string" then . elif . == null then "" else tojson end)
  | gsub("[[:cntrl:]]"; " ") | .[0:200];
def minor_finding($kw; $tkw):
  type == "object"
  and (.severity | type) == "string" and .severity == "minor"
  and (.id | type) == "string" and (.id | test("\\A[A-Za-z0-9._-]{1,64}\\z"))
  and ((.id | ascii_downcase | test($kw)) | not)
  and (([.summary, .evidence] | map(clean) | join(" ") | ascii_downcase | test($tkw)) | not);
'

cell() {
  local s
  s="$(sanitize_cli_output "$1")"
  s="${s//&/&amp;}"; s="${s//</&lt;}"; s="${s//>/&gt;}"
  s="${s//@/@$ZW}"
  s="$(printf '%s' "$s" | sed -E -e "s/#([0-9])/#${ZW}\\1/g" -e "s#(github\\.com)/#\\1/${ZW}#Ig")"
  s="${s//|/&#x7C;}"; s="${s//\`/&#x60;}"
  printf '%s' "$s"
}
