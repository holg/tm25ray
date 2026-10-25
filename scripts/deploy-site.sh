#!/usr/bin/env bash
# Deploy the tm25.eu site: site/ plus the sample files and images from docs/,
# the nginx vhost, and a symlink that points the shared viewer bundle at its
# newest build. Idempotent. Usage: scripts/deploy-site.sh [ssh-host]
#
# The viewer WASM is not built or uploaded here: iesna.eu's deploy
# (eulumdat-rs, scripts/build-wasm-split.sh) puts the hashed bundle in
# /var/www/iesna.eu/html/tm25/, and nginx aliases /tm25/ on tm25.eu onto it.

set -euo pipefail
cd "$(dirname "$0")/.."

HOST="${1:-iesna.eu}"
REMOTE_ROOT=/var/www/tm25.eu/html
BUNDLE_DIR=/var/www/iesna.eu/html/tm25
STAGE=tmp/site-stage

echo "== staging"
rm -rf "$STAGE"
mkdir -p "$STAGE/img" "$STAGE/sample"
cp -R site/. "$STAGE/"
cp docs/img/*.jpg "$STAGE/img/"
cp docs/sample/*.TM25RAY "$STAGE/sample/"
du -sh "$STAGE"

echo "== nginx vhost"
ssh "$HOST" "mkdir -p $REMOTE_ROOT"
scp -q scripts/nginx-tm25.eu.conf "$HOST:/etc/nginx/sites-available/tm25.eu"
ssh "$HOST" "ln -sfn /etc/nginx/sites-available/tm25.eu /etc/nginx/sites-enabled/tm25.eu && nginx -t && systemctl reload nginx"

echo "== files"
rsync -az --delete "$STAGE/" "$HOST:$REMOTE_ROOT/"

echo "== viewer bundle symlink"
ssh "$HOST" "cd $BUNDLE_DIR && js=\$(ls -t tm25-viewer-*.js | grep -v '\.br\$' | head -1) && ln -sfn \"\$js\" tm25-viewer.js && { [ -f \"\$js.br\" ] && ln -sfn \"\$js.br\" tm25-viewer.js.br || rm -f tm25-viewer.js.br; } && echo \"tm25-viewer.js -> \$js\" && grep -o '[A-Za-z0-9_-]*_bg\.wasm' \"\$js\" | sort -u | sed 's/^/  wasm: /'"

echo "== check"
for u in https://tm25.eu/ https://www.tm25.eu/ https://anything.tm25.eu/ https://tm25.eu/viewer/ https://tm25.eu/tm25/tm25-viewer.js https://tm25.eu/sample/sample_white_led_25k.TM25RAY "https://tm25.eu/app/?wasm=goniosim&source=tm25"; do
    printf "%-60s %s\n" "$u" "$(curl -sS -o /dev/null -w '%{http_code} %{content_type} %{size_download}B' -H 'Accept-Encoding: br' "$u")"
done
wasm=$(curl -sS https://tm25.eu/tm25/tm25-viewer.js | grep -o '[A-Za-z0-9_-]*_bg\.wasm' | head -1)
printf "%-60s %s\n" "https://tm25.eu/tm25/$wasm" "$(curl -sS -o /dev/null -w '%{http_code} %{content_type} %{size_download}B' -H 'Accept-Encoding: br' -r 0-0 "https://tm25.eu/tm25/$wasm")"
# The editor under /app/ must resolve its own glue, wasm and the hashed TM-25 loader relative to /app/.
for rel in $(curl -sS https://tm25.eu/app/ | grep -o "\./[A-Za-z0-9_-]*\.\(js\|wasm\|css\)" | sort -u | head -4); do
    printf "%-60s %s\n" "https://tm25.eu/app/${rel#./}" "$(curl -sS -o /dev/null -w '%{http_code} %{content_type}' -H 'Accept-Encoding: br' -r 0-0 "https://tm25.eu/app/${rel#./}")"
done
