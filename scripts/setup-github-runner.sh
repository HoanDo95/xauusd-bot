#!/usr/bin/env bash
# Register a GitHub Actions self-hosted runner for this repo on the xauusd-bot box.
# Run ON the server as user `jason` (NO sudo needed):
#
#   REPO_URL="https://github.com/<owner>/<repo>" \
#   RUNNER_TOKEN="<token from Settings > Actions > Runners > New runner>" \
#   bash scripts/setup-github-runner.sh
#
# The runner is installed under ~/actions-runner and supervised by a
# systemd --user unit (linger is already enabled on this box).

set -euo pipefail

: "${REPO_URL:?Set REPO_URL, e.g. https://github.com/hoando95/xauusd-bot}"
: "${RUNNER_TOKEN:?Set RUNNER_TOKEN from GitHub repo settings}"

RUNNER_DIR="${RUNNER_DIR:-$HOME/actions-runner}"
RUNNER_LABELS="${RUNNER_LABELS:-xauusd-bot}"

mkdir -p "$RUNNER_DIR"
cd "$RUNNER_DIR"

if [ ! -f ./config.sh ]; then
  RUNNER_VERSION="$(
    curl -fsSL https://api.github.com/repos/actions/runner/releases/latest \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["tag_name"].lstrip("v"))'
  )"
  echo "Installing actions runner v${RUNNER_VERSION}"
  curl -fL -o runner.tar.gz \
    "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz"
  tar xzf runner.tar.gz
  rm -f runner.tar.gz
fi

./config.sh \
  --url "$REPO_URL" \
  --token "$RUNNER_TOKEN" \
  --name "xauusd-bot-runner" \
  --labels "$RUNNER_LABELS" \
  --unattended \
  --replace

mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/github-runner.service" <<UNIT
[Unit]
Description=GitHub Actions self-hosted runner (xauusd-bot)
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=0

[Service]
Type=simple
WorkingDirectory=$RUNNER_DIR
ExecStart=$RUNNER_DIR/run.sh
Restart=always
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=30

[Install]
WantedBy=default.target
UNIT

systemctl --user daemon-reload
systemctl --user enable --now github-runner.service
systemctl --user --no-pager --full status github-runner.service | head -n 20
