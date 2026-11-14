#!/bin/sh

INSTALL_DIR="${CADMUS_INSTALL_DIR:?CADMUS_INSTALL_DIR must be set}"

cadmus_pid() {
  for pid in $(pidof cadmus 2>/dev/null); do
    if [ "$(readlink "/proc/$pid/cwd" 2>/dev/null)" = "$INSTALL_DIR" ]; then
      echo "$pid"
      return 0
    fi
  done
  return 1
}

BEFORE=$(cadmus_pid) || BEFORE=
if [ -z "$BEFORE" ]; then
  echo "cadmus is not running"
  exit 1
fi

env kill -USR1 "$BEFORE"

i=0
while [ "$i" -lt 30 ]; do
  sleep 1
  AFTER=$(cadmus_pid) || AFTER=
  if [ -n "$AFTER" ] && [ "$AFTER" != "$BEFORE" ]; then
    echo "cadmus pid: $BEFORE -> $AFTER"
    echo "RESULT: restarted"
    exit 0
  fi
  i=$((i + 1))
done

AFTER=$(cadmus_pid) || AFTER=
echo "cadmus pid: $BEFORE -> ${AFTER:-none}"
echo "RESULT: cadmus did not restart"
exit 2
