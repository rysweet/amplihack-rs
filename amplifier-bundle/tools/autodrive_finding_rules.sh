#!/usr/bin/env bash
# autodrive_finding_rules.sh - the one definition of which crusty findings may
# leave a review as "minor", and of how agent text is made safe for a GitHub
# issue body. Sourced, never run; sourcing only defines variables and
# functions, so it has no side effects.
#
# Used by:
#   autodrive_followups.sh    the crusty-loop minor-only end and its
#                             follow-up issues (#1979), for auto-drive-to-merge
#                             and auto-drive-light alike (#1982)
#
# ID_KEYWORDS     the id pattern of the crusty loop (#1979): matched as whole
#                 words in the lower-cased id with "-", "_" and "." read as
#                 spaces, so "pr-body-testing-section" and
#                 "head-commit-wrong-author-identity" stay minor while
#                 "sec-3", "auth-bypass" and "token-leak" do not. It is
#                 TEXT_KEYWORDS plus the short forms used in ids (sec, auth,
#                 perm, cred, priv, xsrf, privesc)
# TEXT_KEYWORDS   matched as whole words in the lower-cased summary and
#                 evidence, so that "second" or "author" do not match
# ZW              a zero-width space, used to break mentions and links
# break_refs <t>  <t> with a zero-width break after every @, in #N, GH-N and
#                 github.com/, so GitHub links and notifies nothing; used
#                 alone for issue titles, which are not HTML-escaped
# cell <text>     one Markdown table cell or body line (break_refs after
#                 HTML escaping): needs
#                 sanitize_cli_output (workflow_issue_tracking.sh) loaded
# AUTODRIVE_FINDING_JQ
#                 jq definitions: `clean` (any value to a one-line string of
#                 at most 200 characters) and `minor_finding($kw; $tkw)`,
#                 true only for an object whose severity is exactly "minor",
#                 whose id matches \A[A-Za-z0-9._-]{1,64}\z, whose id (lower-
#                 cased, "-", "_" and "." read as spaces) does not match $kw,
#                 and whose summary and evidence do not match $tkw. The caller
#                 passes ID_KEYWORDS and TEXT_KEYWORDS. Anything else, an
#                 unknown or missing severity included, is not minor.
#
# Contract: docs/reference/auto-drive-to-merge.md#cell-sanitising and
# docs/reference/auto-drive-to-merge.md (minor findings, #1979).

TEXT_KEYWORDS='\b(secur[a-z]*|insecure|authenticat[a-z]*|authori[sz][a-z]*|authn|authz|secrets?|tokens?|credentials?|inject[a-z]*|xss|csrf|cve|vuln[a-z]*|unsafe|privileges?|permissions?|sanitis[a-z]*|sanitiz[a-z]*|escap(e|es|ed|ing)|traversal|ssrf|rce|password[a-z]*)\b'
ID_KEYWORDS='\b(sec|secur[a-z]*|insecure|auth|authn|authz|authenticat[a-z]*|authori[sz][a-z]*|secrets?|tokens?|creds?|credentials?|inject[a-z]*|xss|csrf|xsrf|cve|vuln[a-z]*|unsafe|privs?|privilege[a-z]*|privesc|perms?|permissions?|sanitis[a-z]*|sanitiz[a-z]*|escap(e|es|ed|ing)|traversal|ssrf|rce|password[a-z]*)\b'
ZW=$'\xe2\x80\x8b'

AUTODRIVE_FINDING_JQ='
def clean: (if type == "string" then . elif . == null then "" else tojson end)
  | gsub("[[:cntrl:]]"; " ") | .[0:200];
def minor_finding($kw; $tkw):
  type == "object"
  and (.severity | type) == "string" and .severity == "minor"
  and (.id | type) == "string" and (.id | test("\\A[A-Za-z0-9._-]{1,64}\\z"))
  and ((.id | ascii_downcase | gsub("[-_.]"; " ") | test($kw)) | not)
  and (([.summary, .evidence] | map(clean) | join(" ") | ascii_downcase | test($tkw)) | not);
'

# The sed expressions use nothing beyond POSIX extended syntax, so GNU and BSD
# (macOS) sed give the same text (#2931): letter case is spelled out in
# brackets where GNU has the `I` flag, and "GH starts a word" is "the line
# starts here, or the character before is not a letter, a digit or an
# underscore" where GNU has `\b`. BSD sed reads `\b` as a backspace and
# rejects `I`. LC_ALL=C makes sed work on bytes: BSD sed refuses input that is
# not valid UTF-8 otherwise, and a byte of a non-ASCII character then counts as
# "not a word character", so a GH-N beside one is broken too, which is the safe
# side. The output is printed only when sed succeeded: text that was not
# broken must not leave this function.
break_refs() {
  local s="${1//@/@$ZW}" out
  out="$(printf '%s' "$s" | LC_ALL=C sed -E -e "s/#([0-9])/#${ZW}\\1/g" \
    -e "s#([Gg][Ii][Tt][Hh][Uu][Bb]\\.[Cc][Oo][Mm])/#\\1/${ZW}#g" \
    -e "s/(^|[^A-Za-z0-9_])([Gg][Hh])-([0-9])/\\1\\2-${ZW}\\3/g")" || return 1
  printf '%s' "$out"
}

cell() {
  local s
  s="$(sanitize_cli_output "$1")"
  s="${s//&/&amp;}"; s="${s//</&lt;}"; s="${s//>/&gt;}"
  s="$(break_refs "$s")" || return 1
  s="${s//|/&#x7C;}"; s="${s//\`/&#x60;}"
  printf '%s' "$s"
}
