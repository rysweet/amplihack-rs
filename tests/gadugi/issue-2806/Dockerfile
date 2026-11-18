# Downloads finish before scenarios run. The context contains selected assets,
# tested executables and the pinned toolchain, never a host home or checkout.
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    bash ca-certificates coreutils curl diffutils findutils git jq python3 python3-venv \
    procps util-linux tmux ripgrep build-essential libssl3t64 libcurl4t64 \
    && python3 -m venv /opt/precommit \
    && /opt/precommit/bin/pip install --no-cache-dir pre-commit==4.3.0
COPY tools/ /opt/fixture/
COPY toolchain/ /opt/toolchain/
COPY assets/ /assets/
RUN chmod -R a+rX /opt/fixture /opt/toolchain /assets
USER 65532:65532
WORKDIR /state
ENTRYPOINT ["/usr/bin/env", "-i", "HOME=/state/home", "TMPDIR=/state/tmp", "PATH=/usr/bin:/bin", "/opt/fixture/issue-2806-build-drive", "run-all"]
