#!/usr/bin/env bash
set -euo pipefail

DB_NAME="${1:-chitta_astrobench}"
DB_USER="${PGUSER:-josh}"
MIGRATIONS_DIR="$(cd "$(dirname "$0")/../../migrations" && pwd)"

echo "=== astrobench DB provisioning ==="
echo "  database: $DB_NAME"
echo "  migrations: $MIGRATIONS_DIR"

if psql -lqt | cut -d\| -f1 | grep -qw "$DB_NAME"; then
    echo "  database already exists — skipping createdb"
else
    createdb "$DB_NAME"
    echo "  created database $DB_NAME"
fi

for f in "$MIGRATIONS_DIR"/0*.sql; do
    fname="$(basename "$f")"
    echo "  applying $fname ..."
    psql -q "$DB_NAME" < "$f"
done

echo "=== done — $DB_NAME is ready ==="
