#!/usr/bin/env bash
# Build and restart the practice onboarding session for this checkout.
set -euo pipefail
cd "$(dirname "$0")"

skip_build=false
with_connection=false
for arg in "$@"; do
  case "$arg" in
    --no-build) skip_build=true ;;
    --with-connection) with_connection=true ;;
    *) echo "Usage: ./rehearse-onboarding.sh [--no-build] [--with-connection]" >&2; exit 2 ;;
  esac
done
if [[ "$skip_build" == false ]]; then ./build.sh preview; fi

app="build/Passband.app"
if [[ ! -x "$app/Contents/MacOS/Passband" ]]; then
  echo "Build missing. Run ./rehearse-onboarding.sh without --no-build first." >&2
  exit 1
fi

# Only stop processes launched as rehearsals from THIS checkout. Match the
# full executable path and the standalone argument, never the app name: the
# regular inbox and previews from other worktrees may be running beside it.
python3 - "$PWD/$app/Contents/MacOS/Passband" <<'PYTHON'
import os
import signal
import subprocess
import sys
import time

executable = sys.argv[1]

def rehearsals():
    output = subprocess.check_output(
        ["ps", "-ww", "-axo", "pid=,args="], text=True
    )
    matches = set()
    for line in output.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) != 2:
            continue
        pid, command = fields
        prefix = executable + " "
        if command.startswith(prefix) and "--onboarding-rehearsal" in command[len(prefix):].split():
            matches.add(int(pid))
    return matches

previous = rehearsals()
for pid in previous:
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass

# Give the app time to exit before forcing a stuck rehearsal closed. Recheck
# identity before each signal so an exited/reused PID never targets other apps.
deadline = time.monotonic() + 5
remaining = previous & rehearsals()
while remaining and time.monotonic() < deadline:
    time.sleep(0.1)
    remaining = previous & rehearsals()
for pid in remaining:
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
if previous:
    print(f"Closed {len(previous)} previous rehearsal instance(s).")
PYTHON

# A fresh process resets the fixture session, even with a normal inbox open.
launch_args=(--onboarding-rehearsal)
if [[ "$with_connection" == true ]]; then launch_args+=(--rehearse-connection); fi
open -n "$app" --args "${launch_args[@]}"
