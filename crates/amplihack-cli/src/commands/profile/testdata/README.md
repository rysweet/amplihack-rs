# Test data

## `issue_1970_5nov.jsonl`

The usage history of candide on 5 November 2026 from 10:52Z to 19:00Z, in the
`usage-history.jsonl` line format. Issue #1970 reported a rate of 0.483
login-weeks an hour and a runway of 7.5 h at 16:52Z that day. The tests in
`tests_capacity_summary.rs` replay these lines under the rate rule of release
1.2.97 and under the current rule.

### Source

The history file no longer held 5 November when the regression test was
written on 8 November: its oldest line was from 6 November at 08:29Z. The
writers keep the file under 3 MiB after a rewrite, and at this host's rate of
writing that held about two and a half days, not the eight days of
retention.

The lines are therefore rebuilt from the rotator log of the same host
(`<root>/.auto-switch.d/rotate/rotate.log.1`). Each `cycle` line of the log
gives the session and weekly percentage of every login the cycle read, in the
form `name:s=N%,w=N%`. Each such line becomes one history line written by the
rotator (`"by":"rotate"`), holding those logins and no others, as a rotator
line does. Logins the cycle skipped or could not read are left out.

### What is inferred

- **Weekly reset times.** The log does not give them. Each login's reset
  time is taken from its oldest line in the surviving history and moved back
  by whole weeks to the first reset after 10:52Z on 5 November. A reading
  taken at or after that time gets the next week's reset. Only one login's
  week reset in the window, recovery-ryansweet's at 12:00Z, and the log shows
  its fall from 100% to 0% at 12:00:57Z.
- **No session limit.** Taken from the same surviving lines.
- **The two `profile usage` lines.** The issue's deputy found a `profile
  usage` line at 13:23:37Z, and the reported reading was taken at 16:52Z. The
  log does not hold them. Each is rebuilt with every login's newest reading in
  the log at that time. At 16:52Z two logins are listed as unknown, the ones
  the rotator could not read most of that day (no login; a session running).
  The 20 logins left give the 360 points of headroom the issue reports. The
  log does not give the second of the reading. 16:52:30Z is used; any time
  from 16:52:11Z to 16:52:43Z prints the same rate and runway.
- **Names.** recovery-ryansweet and ryan-mistt-personal keep their names,
  because the issue cites them. The other logins are numbered `login-01` to
  `login-20` in the order of their names. `login-19` is ryanc2-mistt-org.

A history line also holds passive readings and is checked against them in
ways the log does not show, so the rebuilt lines can differ from the lost ones
by a point here and there. They still give the reported rate and runway
exactly under the rule of release 1.2.97.

### How it was made

```python
import json, re, sys
from datetime import datetime, timezone

log, hist, out = sys.argv[1:4]
def ts(s): return int(datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp())
def iso(t): return datetime.fromtimestamp(t, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
FROM, TO = ts("2026-11-05T10:52:00Z"), ts("2026-11-05T19:00:00Z")
USAGE = [(ts("2026-11-05T13:23:37Z"), set()), (ts("2026-11-05T16:52:30Z"), {"aanderson", "ffarmer"})]
KEEP = {"recovery-ryansweet", "ryan-mistt-personal"}
WEEK = 7 * 86400
cycles = []
for line in open(log):
    if not line.startswith("2026-11-05T") or " cycle " not in line: continue
    pts = {m.group(1): (int(m.group(2)), int(m.group(3)))
           for m in re.finditer(r" ([a-z0-9-]+):s=(\d+)%,w=(\d+)%", line)}
    cycles.append((ts(line[:20]), pts))
first_reset, no_limit = {}, {}
for line in open(hist):
    e = json.loads(line)
    for n, p in e["profiles"].items():
        if n in first_reset or not p.get("w_reset"): continue
        r = ts(p["w_reset"])
        while r - WEEK > FROM: r -= WEEK
        first_reset[n] = r; no_limit[n] = p.get("session_limit") is False
names = sorted({n for _, p in cycles for n in p})
others = [n for n in names if n not in KEEP]
alias = {n: (n if n in KEEP else f"login-{others.index(n) + 1:02d}") for n in names}
def point(n, at, s, w):
    r = first_reset.get(n)
    if r is not None and at >= r: r += WEEK
    d = {"s": None if no_limit.get(n) else s}
    if no_limit.get(n): d["session_limit"] = False
    d["w"] = w; d["w_reset"] = None if r is None else iso(r)
    return d
def line(at, by, pts, unknown):
    o = {"at": iso(at), "by": by,
         "profiles": dict(sorted((alias[n], point(n, at, *v)) for n, v in pts.items()))}
    if unknown: o["unknown"] = sorted(alias[n] for n in unknown)
    return json.dumps(o, separators=(",", ":"))
def newest(at):
    o = {}
    for t, p in cycles:
        if t <= at: o.update(p)
    return o
rows = [(t, 1, line(t, "rotate", p, [])) for t, p in cycles if FROM <= t < TO]
for t, left_out in USAGE:
    pts = {n: v for n, v in newest(t).items() if n not in left_out}
    rows.append((t, 0, line(t, "usage", pts, [n for n in names if n not in pts])))
rows.sort()
open(out, "w").write("".join(r + "\n" for _, _, r in rows))
```

Run as `python3 -I gen.py rotate.log.1 usage-history.jsonl issue_1970_5nov.jsonl`.

## `usage_json_golden.json`

The `profile usage --json` output for the rows of `golden_rows` in
`tests_usage_cmd.rs`, which the output must match byte for byte.
