# Test data

## `issue_1970_5nov.jsonl`

The usage history of host-a on 5 November 2026 from 10:52Z to 19:00Z, in the
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
  week reset in the window, login-as's at 12:00Z, and the log shows
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
- **Names.** login-as and login-ax are the two logins issue #1970 discusses.
  They keep a name of their own rather than a number, so the tests can refer
  to them. The issue gives their original names; this tree uses generic
  names in their place. The other logins are numbered `login-01` to
  `login-20` in the order of their names. `login-19` is login-az.

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
USAGE = [(ts("2026-11-05T13:23:37Z"), set()), (ts("2026-11-05T16:52:30Z"), {"login-aa", "login-af"})]
KEEP = {"login-as", "login-ax"}
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

## `issue_3258_26nov.jsonl`

A usage history of 19 `profile usage` lines, one every 20 minutes from
2026-11-25T20:28:00Z to 2026-11-26T02:28:00Z, in the `usage-history.jsonl`
line format. It reproduces the patterns that issue #3258 recorded on the
night of 25 to 26 November, under generic names. It is made up, not copied
from a host. The tests in `tests_capacity_summary.rs` take the last line as
now and the others as the history.

| Login | Pattern | Use counted by hand |
|-------|---------|---------------------|
| `login-a` | 43 rising to 46, with three single readings of 29 that the next reading reverses | 3 |
| `login-b` | 78 rising to 87, with one reading of 72 between 82 and 83 | 9 |
| `login-c` | 70 to 74, then a fall to 30 that stays, then 30 to 34 | 8 |
| `login-d` | 74, then a fall to 24 that stays, then 24 to 27 | 3 |
| `login-e` | no session window; 85 to 87, then a fall to 37 that stays, then 37 to 39 | 4 |
| `login-f`, `login-g`, `login-h` | steady use: 20 to 74, 10 to 82, 40 to 76 | 54, 72, 36 |
| `login-i`, `login-j` | idle at 0 and 75 | 0 |
| `login-r1` to `login-r5` | full at 100, renewing 4.5, 6.5 and 9.5 hours after the last line, and two at 16.5 hours | 0 |

Every fall keeps the login's `w_reset`. The use is 189 points over 6 hours,
0.315 login-weeks an hour. The weighted headroom is 4.6 login-weeks, and the
five renewals give back 5.0 login-weeks within 24 hours, so the headroom lasts
beyond 24 hours at that rate.

### How it was made

```python
import json, sys
from datetime import datetime, timezone
def ts(s): return int(datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp())
def iso(t): return datetime.fromtimestamp(t, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
NOW = ts("2026-11-26T02:28:00Z")
START = NOW - 6 * 3600
FAR = iso(ts("2026-11-29T17:00:00Z"))
def at_h(h): return iso(NOW + int(h * 3600))
def w_of(n, i):
    if n == "login-a":
        return 29 if i in (5, 9, 13) else 43 + i // 5
    if n == "login-b":
        return 72 if i == 10 else 78 + i // 2
    if n == "login-c":
        return [70, 70, 71, 71, 72, 72, 73, 74][i] if i < 8 else 30 + (i - 8) * 4 // 10
    if n == "login-d":
        return 74 if i < 6 else 24 + (i - 6) // 4
    if n == "login-e":
        return 85 + i // 4 if i < 12 else 37 + (i - 12) // 3
    if n == "login-f":
        return 20 + 3 * i
    if n == "login-g":
        return 10 + 4 * i
    if n == "login-h":
        return 40 + 2 * i
    if n == "login-i":
        return 0
    if n == "login-j":
        return 75
    return 100
RESET = {"login-r1": at_h(4.5), "login-r2": at_h(6.5), "login-r3": at_h(9.5),
         "login-r4": at_h(16.5), "login-r5": at_h(16.5)}
NAMES = ["login-a", "login-b", "login-c", "login-d", "login-e", "login-f", "login-g",
         "login-h", "login-i", "login-j", "login-r1", "login-r2", "login-r3", "login-r4", "login-r5"]
def point(n, i):
    d = {"s": None, "session_limit": False} if n == "login-e" else {"s": 10}
    d["w"] = w_of(n, i)
    d["w_reset"] = RESET.get(n, FAR)
    return d
out = []
for i in range(19):
    o = {"at": iso(START + i * 20 * 60), "by": "usage",
         "profiles": {n: point(n, i) for n in NAMES}}
    out.append(json.dumps(o, separators=(",", ":")))
open(sys.argv[1], "w").write("".join(l + "\n" for l in out))
```

Run as `python3 -I gen.py issue_3258_26nov.jsonl`.

## `usage_json_golden.json`

The `profile usage --json` output for the rows of `golden_rows` in
`tests_usage_cmd.rs`, which the output must match byte for byte.
