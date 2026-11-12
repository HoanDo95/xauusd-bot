#!/bin/sh
# Optional HTTP Basic auth for the dashboard/API.
#
# Enabled only when both DASHBOARD_USER and DASHBOARD_PASSWORD are set.
# Webhooks (/api/webhook*, /api/candles/webhook) and health endpoints are
# explicitly exempt in nginx.conf because TradingView cannot send Basic auth.
set -eu

AUTH_CONF="/etc/nginx/auth.conf"

if [ -n "${DASHBOARD_USER:-}" ] && [ -n "${DASHBOARD_PASSWORD:-}" ]; then
  htpasswd -bc /etc/nginx/.htpasswd "$DASHBOARD_USER" "$DASHBOARD_PASSWORD"
  cat > "$AUTH_CONF" <<'EOF'
auth_basic "Gold Signal Bot";
auth_basic_user_file /etc/nginx/.htpasswd;
EOF
  echo "[auth] Dashboard/API Basic auth enabled for user ${DASHBOARD_USER}"
else
  : > "$AUTH_CONF"
  echo "[auth] Dashboard/API Basic auth disabled (set DASHBOARD_USER and DASHBOARD_PASSWORD to enable)"
fi
