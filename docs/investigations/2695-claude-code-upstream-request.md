# Continue the same interactive conversation after a stored login change

We observed interactive Claude Code sessions remain on the usage-limit menu after a login rotator switched to a login with available capacity. A hook wake can run while the menu still holds the session. The session owner currently has to dismiss the menu by hand and confirm that the original conversation continues.

Please provide a documented action that validates the changed login through Claude Code’s own credential store, releases the usage-limit menu when that login has room, and continues the same conversation. A documented `StopFailure` (`rate_limit`) outcome that ends the held turn and lets the launcher resume that exact conversation would also address the problem.

The action should avoid terminal input and extra-usage choices. It should leave the held session alone when the changed login cannot be validated or has no capacity.
