# Continue the same interactive conversation after a stored login change

amplihack answers a held usage-limit menu itself by sending keys to its own session's tmux pane (see [Resume sessions stopped by a usage limit](../howto/claude-profile-rotation.md#resume-sessions-stopped-by-a-usage-limit)). Its run against a live usage limit is not yet confirmed. That route needs a terminal that tmux controls and a screen the hook can recognise from a short list of fixed phrases, which is a match and not a parser. This request stands for a route that needs neither.

We observed interactive Claude Code sessions remain on the usage-limit menu after a login rotator switched to a login with available capacity. A hook wake can run while the menu still holds the session. Without tmux, the session owner currently has to dismiss the menu by hand and confirm that the original conversation continues.

Please provide a documented action that validates the changed login through Claude Code’s own credential store, releases the usage-limit menu when that login has room, and continues the same conversation. A documented `StopFailure` (`rate_limit`) outcome that ends the held turn and lets the launcher resume that exact conversation would also address the problem.

The action should avoid terminal input and extra-usage choices. It should leave the held session alone when the changed login cannot be validated or has no capacity.
