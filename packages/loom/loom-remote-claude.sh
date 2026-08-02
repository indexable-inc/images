#!@shell@
set -eu

vm=$1
cwd=$2
shift 2

exec @ix@ shell "$vm" --noninteractive -- \
  @shell@ -c 'cd "$1" && shift && exec "$@"' sh "$cwd" loom-claude "$@"
